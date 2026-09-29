//! Three independently scheduled stages with bounded ownership hand-offs.
//! At most one prepared frame and one inferred result wait between workers.
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    time::Duration,
};

fn measured<T>(stage: &str, work: impl FnOnce() -> T) -> T {
    if std::env::var_os("OXY_ANALYSIS_TIMING").is_none() {
        return work();
    }
    let started = std::time::Instant::now();
    let value = work();
    eprintln!(
        "analysis_timing stage={stage} ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.
    );
    value
}

const POLL: Duration = Duration::from_millis(10);

fn send<T>(sender: &SyncSender<T>, mut value: T, stopped: &impl Fn() -> bool) -> bool {
    loop {
        if stopped() {
            return false;
        }
        match sender.try_send(value) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                value = returned;
                std::thread::sleep(POLL);
            }
        }
    }
}

fn receive<T>(receiver: &Receiver<T>, stopped: &impl Fn() -> bool) -> Option<T> {
    loop {
        if stopped() {
            return None;
        }
        match receiver.recv_timeout(POLL) {
            Ok(value) => return Some(value),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn guarded(stop: &AtomicBool, work: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let result = catch_unwind(AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err("人物流水线阶段异常退出".into()));
    if result.is_err() {
        stop.store(true, Ordering::Release);
    }
    result
}

/// Preparation and inference get dedicated scoped threads; the caller saves.
/// Shutdown joins both workers, including native calls already in progress.
pub(super) fn execute<P: Send, O: Send>(
    mut prepare_next: impl FnMut() -> Result<Option<P>, String> + Send,
    mut infer: impl FnMut(P) -> O + Send,
    mut save: impl FnMut(O) -> Result<(), String>,
    cancelled: impl Fn() -> bool + Sync,
) -> Result<(), String> {
    let stop = AtomicBool::new(false);
    let stopped = || stop.load(Ordering::Acquire) || cancelled();
    let (prepared_tx, prepared_rx) = mpsc::sync_channel(1);
    let (inferred_tx, inferred_rx) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let stop = &stop;
        let stopped = &stopped;
        let producer = scope.spawn(move || {
            guarded(stop, || {
                while !stopped() {
                    let Some(prepared) = measured("prepare_claim", &mut prepare_next)? else {
                        break;
                    };
                    if !measured("prepare_send_wait", || {
                        send(&prepared_tx, prepared, &stopped)
                    }) {
                        break;
                    }
                }
                drop(prepared_tx);
                Ok(())
            })
        });
        let predictor = scope.spawn(move || {
            guarded(stop, || {
                while let Some(prepared) =
                    measured("infer_receive_wait", || receive(&prepared_rx, &stopped))
                {
                    let inferred = measured("infer", || infer(prepared));
                    if !measured("infer_send_wait", || send(&inferred_tx, inferred, &stopped)) {
                        break;
                    }
                }
                drop(inferred_tx);
                Ok(())
            })
        });
        let saved = guarded(stop, || {
            while let Some(result) = receive(&inferred_rx, &stopped) {
                if stopped() {
                    break;
                }
                measured("save", || save(result))?;
            }
            Ok(())
        });
        // A save failure/cancellation wakes blocked senders before joining.
        let prepared = producer.join().map_err(|_| "图片准备线程异常")?;
        let inferred = predictor.join().map_err(|_| "模型推理线程异常")?;
        saved.and(prepared).and(inferred)
    })
}
