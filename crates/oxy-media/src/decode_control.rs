//! Media-specific decode admission and source-work coalescing.
//! This is not the runtime request queue and does not preempt active decoders.

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, LazyLock, Mutex},
};

// Full-resolution RAW development (potentially tens of seconds) stays on its
// own lane so it never blocks the unified thumbnail/loupe gate. The gate below
// covers the progressive stages (512 / 4096) for every format.
static RAW_FULL_DECODE_LOCK: Mutex<()> = Mutex::new(());
static DECODE_GATE: DecodeGate = DecodeGate::new();
// A stale selection may finish writing its rebuildable JPEG without blocking
// the foreground decode gate needed by the newly selected HEIF.
static HEIF_SESSION_CACHE_WRITE_LOCK: Mutex<()> = Mutex::new(());

// Per-file locks coalesce duplicate cache work after the global decode gate has
// selected the next source. Shared by every format.
static DECODE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Priority for the unified decode gate. Higher priorities jump ahead of
/// lower-priority waiters but never preempt a running decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecodePriority {
    /// Overscan/off-screen thumbnails — served only when nothing else wants the gate.
    Background,
    /// On-screen thumbnails and filmstrip — served before background work.
    Visible,
    /// The currently-selected loupe image — served first.
    Foreground,
}

impl From<oxy_domain::PreviewPriority> for DecodePriority {
    fn from(priority: oxy_domain::PreviewPriority) -> Self {
        use oxy_domain::PreviewPriority;
        match priority {
            PreviewPriority::Preload | PreviewPriority::Nearby => Self::Background,
            PreviewPriority::Visible => Self::Visible,
            PreviewPriority::Loupe => Self::Foreground,
        }
    }
}

struct DecodeGate {
    state: Mutex<DecodeGateState>,
    ready: Condvar,
}

#[derive(Default)]
struct DecodeGateState {
    active: bool,
    foreground_waiters: usize,
    visible_waiters: usize,
}

struct DecodePermit<'a> {
    gate: &'a DecodeGate,
}

impl DecodeGate {
    const fn new() -> Self {
        Self {
            state: Mutex::new(DecodeGateState {
                active: false,
                foreground_waiters: 0,
                visible_waiters: 0,
            }),
            ready: Condvar::new(),
        }
    }

    fn acquire(&self, priority: DecodePriority) -> DecodePermit<'_> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if priority == DecodePriority::Foreground {
            state.foreground_waiters += 1;
        } else if priority == DecodePriority::Visible {
            state.visible_waiters += 1;
        }
        while state.active
            || match priority {
                DecodePriority::Foreground => false,
                DecodePriority::Visible => state.foreground_waiters > 0,
                DecodePriority::Background => {
                    state.foreground_waiters > 0 || state.visible_waiters > 0
                }
            }
        {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if priority == DecodePriority::Foreground {
            state.foreground_waiters -= 1;
        } else if priority == DecodePriority::Visible {
            state.visible_waiters -= 1;
        }
        state.active = true;
        DecodePermit { gate: self }
    }
}

impl Drop for DecodePermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = false;
        self.gate.ready.notify_all();
    }
}

/// Acquire the unified decode gate. Higher-priority waiters are served before
/// lower-priority ones; a running decode is never preempted (caller opted into
/// the "order pending only" scheduling policy).
pub(crate) fn acquire_decode(priority: DecodePriority) -> impl Drop {
    DECODE_GATE.acquire(priority)
}

pub(crate) fn try_acquire_heif_session_cache_write() -> Option<impl Drop> {
    match HEIF_SESSION_CACHE_WRITE_LOCK.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::WouldBlock) => None,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
    }
}

/// Look up or create a per-file `Mutex`, clone the `Arc`, then lock it.
/// Returns `(Arc<Mutex<()>>, MutexGuard)` — caller must keep the `Arc` alive
/// alongside the guard (it is dropped last due to reverse-order drop).
pub(crate) fn acquire_file_lock(
    cache_key: &str,
) -> (Arc<Mutex<()>>, std::sync::MutexGuard<'static, ()>) {
    let arc: Arc<Mutex<()>> = DECODE_LOCKS
        .lock()
        .unwrap()
        .entry(cache_key.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    // Access the Mutex via raw pointer to decouple the guard's lifetime from
    // the local `arc` binding. This lets us return both the Arc and the guard.
    // Safety: the Mutex lives inside the static DECODE_LOCKS HashMap behind an
    // Arc that is never removed; the returned Arc keeps it alive.
    let mutex: &'static Mutex<()> = unsafe { &*Arc::as_ptr(&arc) };
    let guard = mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (arc, guard)
}

/// Keep expensive RAW development independent of progressive source decoding.
pub(crate) fn acquire_raw_full_decode() -> impl Drop {
    RAW_FULL_DECODE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn maps_domain_priority_to_private_gate_priority() {
        use oxy_domain::PreviewPriority;

        assert_eq!(
            DecodePriority::from(PreviewPriority::Preload),
            DecodePriority::Background
        );
        assert_eq!(
            DecodePriority::from(PreviewPriority::Nearby),
            DecodePriority::Background
        );
        assert_eq!(
            DecodePriority::from(PreviewPriority::Visible),
            DecodePriority::Visible
        );
        assert_eq!(
            DecodePriority::from(PreviewPriority::Loupe),
            DecodePriority::Foreground
        );
    }

    #[test]
    fn decode_permit_releases_gate_during_unwind() {
        let gate = DecodeGate::new();
        let result = std::panic::catch_unwind(|| {
            let _permit = gate.acquire(DecodePriority::Foreground);
            panic!("simulate a failed decode");
        });
        assert!(result.is_err());
        assert!(!gate.state.lock().unwrap().active);
        let permit = gate.acquire(DecodePriority::Background);
        assert!(gate.state.lock().unwrap().active);
        drop(permit);
        assert!(!gate.state.lock().unwrap().active);
    }

    #[test]
    fn file_lock_reuses_identity_and_does_not_block_different_sources() {
        let (first, first_guard) = acquire_file_lock("test-file-lock-identity-a");
        assert!(matches!(
            first.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        // Acquiring another source while the first is held must not retain
        // the registry mutex or accidentally serialize all source work.
        let (other, other_guard) = acquire_file_lock("test-file-lock-identity-b");
        assert!(!Arc::ptr_eq(&first, &other));
        drop(other_guard);
        drop(first_guard);

        let (again, again_guard) = acquire_file_lock("test-file-lock-identity-a");
        assert!(Arc::ptr_eq(&first, &again));
        assert!(matches!(
            first.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        drop(again_guard);
        assert!(first.try_lock().is_ok());
    }

    #[test]
    fn file_lock_recovers_after_a_source_decode_panics() {
        let result = std::panic::catch_unwind(|| {
            let (_source, _guard) = acquire_file_lock("test-file-lock-poison");
            panic!("simulate a failed source decode");
        });
        assert!(result.is_err());
        let (source, guard) = acquire_file_lock("test-file-lock-poison");
        assert!(source.is_poisoned());
        assert!(matches!(
            source.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        drop(guard);
    }

    #[test]
    fn decode_gate_prefers_foreground_then_visible_then_background() {
        // The gate is now format-agnostic; the priority contract (loupe first,
        // visible second, overscan last) is preserved unchanged.
        let gate = Arc::new(DecodeGate::new());
        let active = gate.acquire(DecodePriority::Background);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let spawn_waiter = |priority| {
            let gate = gate.clone();
            let acquired_tx = acquired_tx.clone();
            thread::spawn(move || {
                let _permit = gate.acquire(priority);
                acquired_tx.send(priority).unwrap();
            })
        };
        let nearby = spawn_waiter(DecodePriority::Background);
        let visible = spawn_waiter(DecodePriority::Visible);
        let loupe = spawn_waiter(DecodePriority::Foreground);

        let started = Instant::now();
        loop {
            let state = gate
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.foreground_waiters == 1 && state.visible_waiters == 1 {
                // Queued higher priorities cannot preempt the active permit.
                assert!(state.active);
                assert!(matches!(
                    acquired_rx.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
                break;
            }
            drop(state);
            assert!(started.elapsed() < Duration::from_secs(1));
            thread::yield_now();
        }
        drop(active);

        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Foreground
        );
        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Visible
        );
        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Background
        );
        nearby.join().unwrap();
        visible.join().unwrap();
        loupe.join().unwrap();
    }

    #[test]
    fn decode_gate_orders_visible_thumbnail_before_nearby() {
        // Regression for the unified gate: a visible thumbnail that arrives
        // *after* a nearby one must still be served first. This is the core
        // "scroll into view jumps the queue" guarantee for every format.
        let gate = Arc::new(DecodeGate::new());
        let active = gate.acquire(DecodePriority::Foreground);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let spawn_waiter = |priority| {
            let gate = gate.clone();
            let acquired_tx = acquired_tx.clone();
            thread::spawn(move || {
                let _permit = gate.acquire(priority);
                acquired_tx.send(priority).unwrap();
            })
        };
        // Nearby arrives first, visible second.
        let nearby = spawn_waiter(DecodePriority::Background);
        let visible = spawn_waiter(DecodePriority::Visible);

        let started = Instant::now();
        loop {
            let state = gate
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.visible_waiters == 1 {
                break;
            }
            drop(state);
            assert!(started.elapsed() < Duration::from_secs(1));
            thread::yield_now();
        }
        drop(active);

        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Visible
        );
        assert_eq!(
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            DecodePriority::Background
        );
        visible.join().unwrap();
        nearby.join().unwrap();
    }
}
