use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Cooperative I/O admission. Running OS reads are never interrupted.
pub struct ForegroundGate {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    stopped: bool,
    active: usize,
    quiet_after: Instant,
}

impl Default for ForegroundGate {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                stopped: false,
                active: 0,
                quiet_after: Instant::now(),
            }),
            changed: Condvar::new(),
        }
    }
}

impl ForegroundGate {
    pub fn enter(&self) -> ForegroundGuard<'_> {
        self.state.lock().active += 1;
        ForegroundGuard(self)
    }

    pub fn is_busy(&self) -> bool {
        let state = self.state.lock();
        state.active > 0 || Instant::now() < state.quiet_after
    }

    pub fn stop(&self) {
        self.state.lock().stopped = true;
        self.changed.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        self.state.lock().stopped
    }

    pub fn wait_for_background(&self) {
        let mut state = self.state.lock();
        loop {
            if state.stopped {
                return;
            }
            if state.active > 0 {
                self.changed.wait(&mut state);
            } else if let Some(remaining) = state.quiet_after.checked_duration_since(Instant::now())
            {
                self.changed.wait_for(&mut state, remaining);
            } else {
                return;
            }
        }
    }
}

pub struct ForegroundGuard<'a>(&'a ForegroundGate);

impl Drop for ForegroundGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock();
        state.active -= 1;
        state.quiet_after = Instant::now() + Duration::from_millis(300);
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_waits_for_all_foreground_consumers() {
        let gate = ForegroundGate::default();
        let first = gate.enter();
        let second = gate.enter();
        drop(first);
        assert!(gate.is_busy());
        drop(second);
        gate.wait_for_background();
        assert!(!gate.is_busy());
    }

    #[test]
    fn stop_wakes_background_even_while_a_foreground_guard_survives() {
        let gate = std::sync::Arc::new(ForegroundGate::default());
        let _foreground = gate.enter();
        let background = std::sync::Arc::clone(&gate);
        let (finished, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            background.wait_for_background();
            finished.send(()).unwrap();
        });
        gate.stop();
        receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }
}
