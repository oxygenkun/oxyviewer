//! Media-specific decode admission and source-work coalescing.
//! This is not the runtime request queue and does not preempt active decoders.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, LazyLock, Mutex, MutexGuard, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

// Full-resolution RAW development (potentially tens of seconds) stays on its
// own lane so it never blocks the unified thumbnail/loupe gate. The gate below
// covers the progressive stages (512 / 4096) for every format.
static RAW_FULL_DECODE_LOCK: Mutex<()> = Mutex::new(());
static DECODE_GATE: LazyLock<DecodeGate> =
    LazyLock::new(|| DecodeGate::with_capacity(oxy_runtime::image_worker_count()));
// A stale selection may finish writing its rebuildable JPEG without blocking
// the foreground decode gate needed by the newly selected HEIF.
static HEIF_SESSION_CACHE_WRITE_LOCK: Mutex<()> = Mutex::new(());

// Per-file locks coalesce duplicate cache work after the global decode gate has
// selected the next source. Shared by every format.
static DECODE_LOCKS: LazyLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DECODE_LOCK_ACQUISITIONS: AtomicUsize = AtomicUsize::new(0);

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
    capacity: usize,
}

struct DecodeGateState {
    active: usize,
    next_ticket: u64,
    waiters: VecDeque<DecodeWaiter>,
}

impl DecodeGateState {
    const fn new() -> Self {
        Self {
            active: 0,
            next_ticket: 0,
            waiters: VecDeque::new(),
        }
    }
}

struct DecodeWaiter {
    ticket: u64,
    priority: DecodePriority,
    queued_at: Instant,
}

pub(crate) struct DecodePermit<'a> {
    gate: &'a DecodeGate,
}

impl DecodeGate {
    #[cfg(test)]
    const fn new() -> Self {
        Self::with_capacity(1)
    }

    const fn with_capacity(capacity: usize) -> Self {
        Self {
            state: Mutex::new(DecodeGateState::new()),
            ready: Condvar::new(),
            capacity,
        }
    }

    fn acquire(
        &self,
        priority: DecodePriority,
        cancelled: &impl Fn() -> bool,
    ) -> Result<DecodePermit<'_>, crate::MediaError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.wrapping_add(1);
        state.waiters.push_back(DecodeWaiter {
            ticket,
            priority,
            queued_at: Instant::now(),
        });
        loop {
            if cancelled() {
                state.waiters.retain(|waiter| waiter.ticket != ticket);
                self.ready.notify_all();
                return Err(crate::MediaError::Cancelled);
            }
            let selected = selected_ticket(&state.waiters);
            // Keep one CPU slot available for the latest selected image even
            // when older visible/nearby native calls cannot be interrupted.
            let limit = if priority == DecodePriority::Foreground {
                self.capacity
            } else {
                self.capacity.saturating_sub(1).max(1)
            };
            if state.active < limit && selected == Some(ticket) {
                state.waiters.retain(|waiter| waiter.ticket != ticket);
                state.active += 1;
                return Ok(DecodePermit { gate: self });
            }
            let (next, _) = self
                .ready
                .wait_timeout(state, Duration::from_millis(25))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
    }
}

fn effective_priority(waiter: &DecodeWaiter) -> u8 {
    let base = match waiter.priority {
        DecodePriority::Background => 0,
        DecodePriority::Visible => 1,
        DecodePriority::Foreground => 2,
    };
    let aging = waiter.queued_at.elapsed().as_secs().min(2) as u8;
    if waiter.priority == DecodePriority::Foreground {
        2
    } else {
        (base + aging).min(1)
    }
}

fn selected_ticket(waiters: &VecDeque<DecodeWaiter>) -> Option<u64> {
    waiters
        .iter()
        .max_by_key(|waiter| (effective_priority(waiter), u64::MAX - waiter.ticket))
        .map(|waiter| waiter.ticket)
}

impl Drop for DecodePermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        self.gate.ready.notify_all();
    }
}

/// Acquire the unified decode gate. Higher-priority waiters are served before
/// lower-priority ones; a running decode is never preempted (caller opted into
/// the "order pending only" scheduling policy).
pub(crate) fn acquire_decode<F: Fn() -> bool>(
    priority: DecodePriority,
    cancelled: &F,
) -> Result<DecodePermit<'static>, crate::MediaError> {
    DECODE_GATE.acquire(priority, cancelled)
}

pub(crate) fn try_acquire_heif_session_cache_write() -> Option<impl Drop> {
    match HEIF_SESSION_CACHE_WRITE_LOCK.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::WouldBlock) => None,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
    }
}

/// Returns the shared lock for one source. The registry stores only weak
/// references, so browsing new files cannot retain one mutex per source for
/// the lifetime of the process. Callers own the `Arc` while locking it.
pub(crate) fn file_lock(cache_key: &str) -> Arc<Mutex<()>> {
    let mut locks = DECODE_LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if DECODE_LOCK_ACQUISITIONS.fetch_add(1, Ordering::Relaxed) % 64 == 0 {
        reclaim_file_locks(&mut locks);
    }
    if let Some(lock) = locks.get(cache_key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(cache_key.to_owned(), Arc::downgrade(&lock));
    lock
}

fn reclaim_file_locks(locks: &mut HashMap<String, Weak<Mutex<()>>>) {
    locks.retain(|_, lock| lock.strong_count() > 0);
}

pub(crate) fn acquire_file_lock<'a>(
    lock: &'a Mutex<()>,
    cancelled: &impl Fn() -> bool,
) -> Result<MutexGuard<'a, ()>, crate::MediaError> {
    loop {
        if cancelled() {
            return Err(crate::MediaError::Cancelled);
        }
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                return Ok(poisoned.into_inner());
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Keep expensive RAW development independent of progressive source decoding.
pub(crate) fn acquire_raw_full_decode(
    cancelled: &impl Fn() -> bool,
) -> Result<MutexGuard<'static, ()>, crate::MediaError> {
    acquire_file_lock(&RAW_FULL_DECODE_LOCK, cancelled)
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
    fn equal_priority_waiters_are_fifo() {
        let now = Instant::now();
        let waiters = VecDeque::from([
            DecodeWaiter {
                ticket: 10,
                priority: DecodePriority::Visible,
                queued_at: now,
            },
            DecodeWaiter {
                ticket: 11,
                priority: DecodePriority::Visible,
                queued_at: now,
            },
        ]);
        assert_eq!(selected_ticket(&waiters), Some(10));
    }

    #[test]
    fn aging_prevents_background_starvation() {
        let waiter = DecodeWaiter {
            ticket: 0,
            priority: DecodePriority::Background,
            queued_at: Instant::now() - Duration::from_secs(2),
        };
        assert_eq!(effective_priority(&waiter), 1);
    }

    #[test]
    fn decode_permit_releases_gate_during_unwind() {
        let gate = DecodeGate::new();
        let result = std::panic::catch_unwind(|| {
            let _permit = gate.acquire(DecodePriority::Foreground, &|| false).unwrap();
            panic!("simulate a failed decode");
        });
        assert!(result.is_err());
        assert_eq!(gate.state.lock().unwrap().active, 0);
        let permit = gate.acquire(DecodePriority::Background, &|| false).unwrap();
        assert_eq!(gate.state.lock().unwrap().active, 1);
        drop(permit);
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }

    #[test]
    fn multiple_decodes_leave_capacity_for_selected_image() {
        let gate = DecodeGate::with_capacity(3);
        let first = gate.acquire(DecodePriority::Visible, &|| false).unwrap();
        let second = gate.acquire(DecodePriority::Background, &|| false).unwrap();
        let selected = gate.acquire(DecodePriority::Foreground, &|| false).unwrap();
        assert_eq!(gate.state.lock().unwrap().active, 3);
        drop((first, second, selected));
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }

    #[test]
    fn old_background_never_overtakes_current_selection() {
        let waiters = VecDeque::from([
            DecodeWaiter {
                ticket: 0,
                priority: DecodePriority::Background,
                queued_at: Instant::now() - Duration::from_secs(60),
            },
            DecodeWaiter {
                ticket: 1,
                priority: DecodePriority::Foreground,
                queued_at: Instant::now(),
            },
        ]);
        assert_eq!(selected_ticket(&waiters), Some(1));
    }

    #[test]
    fn file_lock_reuses_identity_and_does_not_block_different_sources() {
        let first = file_lock("test-file-lock-identity-a");
        let first_guard = first.lock().unwrap();
        assert!(matches!(
            first.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        // Acquiring another source while the first is held must not retain
        // the registry mutex or accidentally serialize all source work.
        let other = file_lock("test-file-lock-identity-b");
        let other_guard = other.lock().unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
        drop(other_guard);
        drop(first_guard);

        let again = file_lock("test-file-lock-identity-a");
        let again_guard = again.lock().unwrap();
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
            let source = file_lock("test-file-lock-poison");
            let _guard = source.lock().unwrap();
            panic!("simulate a failed source decode");
        });
        assert!(result.is_err());
        let source = file_lock("test-file-lock-poison");
        let guard = source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The weak registry may reclaim the poisoned mutex when the panicking
        // owner drops its final Arc; either way, subsequent work can proceed.
        assert!(matches!(
            source.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        drop(guard);
    }

    #[test]
    fn waiting_for_source_lock_observes_cancellation() {
        let source = file_lock("test-file-lock-cancellation");
        let _guard = source.lock().unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let started = Instant::now();
        let result = acquire_file_lock(&source, &|| {
            if started.elapsed() >= Duration::from_millis(40) {
                cancelled.store(true, Ordering::Release);
            }
            cancelled.load(Ordering::Acquire)
        });

        assert!(matches!(result, Err(crate::MediaError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn file_lock_registry_reclaims_unowned_entries() {
        let key = "test-file-lock-reclamation";
        {
            let _source = file_lock(key);
            assert!(DECODE_LOCKS.lock().unwrap().contains_key(key));
        }
        let mut locks = DECODE_LOCKS.lock().unwrap();
        reclaim_file_locks(&mut locks);
        assert!(!locks.contains_key(key));
    }

    #[test]
    fn decode_gate_prefers_foreground_then_visible_then_background() {
        // The gate is now format-agnostic; the priority contract (loupe first,
        // visible second, overscan last) is preserved unchanged.
        let gate = Arc::new(DecodeGate::new());
        let active = gate.acquire(DecodePriority::Background, &|| false).unwrap();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let spawn_waiter = |priority| {
            let gate = gate.clone();
            let acquired_tx = acquired_tx.clone();
            thread::spawn(move || {
                let _permit = gate.acquire(priority, &|| false).unwrap();
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
            let foreground = state
                .waiters
                .iter()
                .filter(|waiter| waiter.priority == DecodePriority::Foreground)
                .count();
            let visible = state
                .waiters
                .iter()
                .filter(|waiter| waiter.priority == DecodePriority::Visible)
                .count();
            if foreground == 1 && visible == 1 {
                // Queued higher priorities cannot preempt the active permit.
                assert_eq!(state.active, 1);
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
        let active = gate.acquire(DecodePriority::Foreground, &|| false).unwrap();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let spawn_waiter = |priority| {
            let gate = gate.clone();
            let acquired_tx = acquired_tx.clone();
            thread::spawn(move || {
                let _permit = gate.acquire(priority, &|| false).unwrap();
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
            if state
                .waiters
                .iter()
                .filter(|waiter| waiter.priority == DecodePriority::Visible)
                .count()
                == 1
            {
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
