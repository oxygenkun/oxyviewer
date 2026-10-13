//! Owned worker lifetime and nonblocking event publication for an application.
use crate::events::CoreEvent;
use std::{
    io,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SendError, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct CoreRuntime {
    inner: Arc<RuntimeState>,
}

#[derive(Default)]
struct RuntimeState {
    stopping: AtomicBool,
    state: Mutex<Workers>,
    changed: Condvar,
    events: Option<Sender<CoreEvent>>,
}

#[derive(Default)]
struct Workers {
    active: usize,
    starting: usize,
    stopping_callbacks: bool,
    threads: Vec<JoinHandle<()>>,
    wakeups: Vec<Box<dyn Fn() + Send + Sync>>,
}

struct RunningWorker(Arc<RuntimeState>);

impl Drop for RunningWorker {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        self.0.changed.notify_all();
    }
}

impl CoreRuntime {
    pub fn channel() -> (Self, Receiver<CoreEvent>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                inner: Arc::new(RuntimeState {
                    events: Some(sender),
                    ..RuntimeState::default()
                }),
            },
            receiver,
        )
    }

    pub fn is_stopping(&self) -> bool {
        self.inner.stopping.load(Ordering::Acquire)
    }

    pub fn publish(&self, event: CoreEvent) -> Result<(), Box<SendError<CoreEvent>>> {
        let _state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_stopping() {
            return Err(Box::new(SendError(event)));
        }
        self.inner
            .events
            .as_ref()
            .map_or(Ok(()), |sender| sender.send(event).map_err(Box::new))
    }

    /// Wakeups synchronize with each queue's mutex, preventing a stop signal
    /// from being lost between its empty check and its condition-variable wait.
    pub fn on_stop(&self, wakeup: impl Fn() + Send + Sync + 'static) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_stopping() {
            drop(state);
            wakeup();
        } else {
            state.wakeups.push(Box::new(wakeup));
        }
    }

    pub fn spawn_named(
        &self,
        name: String,
        work: impl FnOnce() + Send + 'static,
    ) -> io::Result<()> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_stopping() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "application is shutting down",
            ));
        }
        state.active += 1;
        state.starting += 1;
        state.threads.retain(|thread| !thread.is_finished());
        let worker = RunningWorker(Arc::clone(&self.inner));
        // Do not hold the state lock when a failed spawn drops its worker guard.
        drop(state);
        let thread = std::thread::Builder::new().name(name).spawn(move || {
            let _worker = worker;
            work();
        });
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.starting -= 1;
        self.inner.changed.notify_all();
        match thread {
            Ok(thread) => {
                state.threads.push(thread);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn spawn_blocking(&self, work: impl FnOnce() + Send + 'static) -> io::Result<()> {
        self.spawn_named("oxy-application-work".into(), work)
    }

    pub fn stop(&self) {
        let wakeups = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.inner.stopping.swap(true, Ordering::AcqRel) {
                return;
            }
            state.stopping_callbacks = true;
            self.inner.changed.notify_all();
            std::mem::take(&mut state.wakeups)
        };
        for wakeup in wakeups {
            wakeup();
        }
        if let Some(sender) = &self.inner.events {
            let _ = sender.send(CoreEvent::Shutdown);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopping_callbacks = false;
        self.inner.changed.notify_all();
    }

    pub fn wait_for_stop(&self, duration: Duration) -> bool {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _state = self
            .inner
            .changed
            .wait_timeout_while(state, duration, |state| {
                !self.is_stopping() || state.stopping_callbacks
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.is_stopping()
    }

    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.stop();
        let deadline = Instant::now() + timeout;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.active != 0 || state.starting != 0 || state.stopping_callbacks {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        let threads = std::mem::take(&mut state.threads);
        drop(state);
        for thread in threads {
            let _ = thread.join();
        }
        true
    }

    pub fn active_workers(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_wakes_idle_owned_workers_and_rejects_late_admission() {
        let (runtime, events) = CoreRuntime::channel();
        let worker = runtime.clone();
        runtime
            .spawn_named("shutdown-test".into(), move || {
                assert!(worker.wait_for_stop(Duration::from_secs(30)));
            })
            .unwrap();
        assert!(runtime.shutdown(Duration::from_secs(2)));
        assert_eq!(runtime.active_workers(), 0);
        assert!(runtime.spawn_blocking(|| {}).is_err());
        assert!(matches!(events.recv().unwrap(), CoreEvent::Shutdown));
    }

    #[test]
    fn timed_out_shutdown_can_be_completed_after_native_work_unwinds() {
        let runtime = CoreRuntime::default();
        let (release, released) = mpsc::channel();
        runtime
            .spawn_named("uninterruptible-test".into(), move || {
                released.recv().unwrap();
            })
            .unwrap();
        assert!(!runtime.shutdown(Duration::from_millis(5)));
        release.send(()).unwrap();
        assert!(runtime.shutdown(Duration::from_secs(2)));
    }

    #[test]
    fn stopping_fences_event_publication() {
        let (runtime, events) = CoreRuntime::channel();
        runtime.stop();
        assert!(runtime.publish(CoreEvent::Shutdown).is_err());
        assert!(matches!(events.recv().unwrap(), CoreEvent::Shutdown));
        assert!(events.try_recv().is_err());
    }
}
