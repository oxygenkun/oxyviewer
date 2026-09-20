use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Cooperative I/O admission. Running OS reads are never interrupted.
pub struct ForegroundGate {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    active: usize,
    quiet_after: Instant,
}

impl Default for ForegroundGate {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                active: 0,
                quiet_after: Instant::now(),
            }),
            changed: Condvar::new(),
        }
    }
}

impl ForegroundGate {
    /// Bounded waits let cancelled background jobs exit while foreground stays busy.
    pub fn wait_for_background_cancellable(&self, token: &crate::CancellationToken) -> bool {
        let mut state = self.state.lock();
        loop {
            if token.is_cancelled() {
                return false;
            }
            if state.active == 0 && Instant::now() >= state.quiet_after {
                return true;
            }
            self.changed.wait_for(&mut state, Duration::from_millis(25));
        }
    }

    pub fn enter(&self) -> ForegroundGuard<'_> {
        self.state.lock().active += 1;
        ForegroundGuard(self)
    }

    pub fn is_busy(&self) -> bool {
        let state = self.state.lock();
        state.active > 0 || Instant::now() < state.quiet_after
    }

    pub fn wait_for_background(&self) {
        let mut state = self.state.lock();
        loop {
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
    fn cancellation_releases_background_while_foreground_remains_active() {
        let gate = ForegroundGate::default();
        let _foreground = gate.enter();
        let token = crate::CancellationToken::default();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| gate.wait_for_background_cancellable(&token));
            token.cancel();
            assert!(!waiter.join().unwrap());
        });
        assert!(gate.is_busy());
    }

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
}
