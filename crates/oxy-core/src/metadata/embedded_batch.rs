//! The runtime-owned coordinator joins its dispatcher, lanes and native sessions.
use super::{
    MetadataService,
    embedded::FileSync,
    types::{EmbeddedSyncFailure, EmbeddedSyncOptions, EmbeddedSyncProgress, EmbeddedSyncResult},
};
use crate::{
    browse::types::AssetSummary, events::CoreEvent, files::observe_file, jobs::JobTicket,
    resources::types::MetadataRequestPriority,
};
use oxy_media::types::AssetKind;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

struct Completion {
    index: usize,
    path: PathBuf,
    outcome: Result<Option<FileSync>, EmbeddedSyncFailure>,
}

#[derive(Default)]
struct FileTurn {
    done: Mutex<bool>,
    changed: Condvar,
}

struct PreparedFile {
    index: usize,
    asset: AssetSummary,
    previous: Option<Arc<FileTurn>>,
    finished: Arc<FileTurn>,
}

struct FinishTurn<'a> {
    turn: &'a FileTurn,
    stopped: &'a AtomicBool,
}

impl Drop for FinishTurn<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.stopped.store(true, Ordering::Release);
        }
        *self
            .turn
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.turn.changed.notify_all();
    }
}

impl MetadataService {
    pub(super) fn run_embedded_batch(
        &self,
        paths: Vec<PathBuf>,
        request_id: String,
        ticket: &JobTicket,
        options: EmbeddedSyncOptions,
    ) -> EmbeddedSyncResult {
        let mut seen = HashSet::new();
        let paths = paths
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .collect::<Vec<_>>();
        let mut result = EmbeddedSyncResult {
            job_id: ticket.id.clone(),
            total: paths.len(),
            completed: 0,
            updated: 0,
            unchanged: 0,
            skipped: 0,
            cancelled: false,
            failures: Vec::new(),
        };
        let publish = |result: &EmbeddedSyncResult| {
            let _ = self
                .runtime
                .publish(CoreEvent::EmbeddedSyncProgress(EmbeddedSyncProgress {
                    request_id: request_id.clone(),
                    job_id: result.job_id.clone(),
                    total: result.total,
                    completed: result.completed,
                    updated: result.updated,
                    unchanged: result.unchanged,
                    skipped: result.skipped,
                }));
        };
        publish(&result);
        let _mutation = self
            .embedded_mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cancelled = || self.runtime.is_stopping() || ticket.is_cancelled();
        if cancelled() || paths.is_empty() {
            result.cancelled = cancelled();
            return result;
        }
        let stopped = AtomicBool::new(false);
        let mut refresh = Vec::new();
        let mut last_refresh = Instant::now();
        let mut failures = Vec::new();
        thread::scope(|scope| {
            // Rendezvous admission holds no backlog of prepared writes. The
            // dispatcher alone observes paths, in input order, outside queue locks.
            let (tasks, receiver) = mpsc::sync_channel(0);
            let receiver = Arc::new(Mutex::new(receiver));
            let (sender, completions) = mpsc::channel();
            for lane in 0..options.concurrency.min(paths.len()) {
                let receiver = Arc::clone(&receiver);
                let output = sender.clone();
                let stopped = &stopped;
                if let Err(error) = thread::Builder::new()
                    .name(format!("metadata-embedded-{lane}"))
                    .spawn_scoped(scope, move || {
                        self.run_embedded_lane(ticket, stopped, receiver, output);
                    })
                {
                    stopped.store(true, Ordering::Release);
                    send_failure(
                        &sender,
                        0,
                        paths[0].clone(),
                        format!("cannot start metadata writer: {error}"),
                    );
                    break;
                }
            }
            // Retaining this receiver would leave a dispatcher blocked after
            // cancellation when all lanes have already released their receivers.
            drop(receiver);
            if !stopped.load(Ordering::Acquire) {
                let output = sender.clone();
                let stopped = &stopped;
                if let Err(error) = thread::Builder::new()
                    .name("metadata-embedded-dispatch".into())
                    .spawn_scoped(scope, || {
                        self.dispatch_embedded(&paths, ticket, stopped, tasks, output);
                    })
                {
                    stopped.store(true, Ordering::Release);
                    send_failure(
                        &sender,
                        0,
                        paths[0].clone(),
                        format!("cannot start metadata dispatcher: {error}"),
                    );
                }
            } else {
                drop(tasks);
            }
            drop(sender);
            for completion in completions {
                let mut invalidate = false;
                match completion.outcome {
                    Ok(Some(FileSync::Updated)) => {
                        result.updated += 1;
                        invalidate = true;
                    }
                    Ok(Some(FileSync::Unchanged)) => result.unchanged += 1,
                    Ok(Some(FileSync::Cancelled)) => continue,
                    Ok(None) => result.skipped += 1,
                    Err(error) => {
                        invalidate = error.uncertain;
                        failures.push((completion.index, error));
                    }
                }
                if invalidate {
                    if let Some(parent) = completion.path.parent() {
                        self.files.invalidate_directory(parent);
                    }
                    self.queue.invalidate_path(&completion.path);
                    refresh.push(completion.path);
                }
                result.completed += 1;
                publish(&result);
                if refresh.len() >= 16 || last_refresh.elapsed() >= Duration::from_millis(250) {
                    self.refresh_written(std::mem::take(&mut refresh));
                    last_refresh = Instant::now();
                }
            }
            // Channel closure follows session drop in every lane. Scope also
            // joins dispatcher/lane threads before this runtime worker can finish.
        });
        failures.sort_by_key(|(index, _)| *index);
        result.failures = failures.into_iter().map(|(_, failure)| failure).collect();
        result.cancelled = cancelled();
        self.refresh_written(refresh);
        result
    }

    fn dispatch_embedded(
        &self,
        paths: &[PathBuf],
        ticket: &JobTicket,
        stopped: &AtomicBool,
        tasks: SyncSender<PreparedFile>,
        output: Sender<Completion>,
    ) {
        let mut targets = HashSet::new();
        let mut identities: HashMap<String, Arc<FileTurn>> = HashMap::new();
        for (index, path) in paths.iter().enumerate() {
            if self.runtime.is_stopping()
                || ticket.is_cancelled()
                || stopped.load(Ordering::Acquire)
            {
                break;
            }
            let asset = match self.files.get_asset(path) {
                Ok(asset) => asset,
                Err(error) => {
                    send_failure(&output, index, path.clone(), error.to_string());
                    continue;
                }
            };
            if !matches!(asset.kind, AssetKind::Jpeg | AssetKind::Heif) || !asset.has_sidecar {
                let _ = output.send(Completion {
                    index,
                    path: path.clone(),
                    outcome: Ok(None),
                });
                continue;
            }
            // Resolve aliases before admission, in input order, so the same
            // canonical target is never sent to two writers under different names.
            let observation = match observe_file(path) {
                Ok(observation) => observation,
                Err(error) => {
                    send_failure(&output, index, path.clone(), error.to_string());
                    continue;
                }
            };
            if !targets.insert(observation.canonical_path) {
                let _ = output.send(Completion {
                    index,
                    path: path.clone(),
                    outcome: Ok(None),
                });
                continue;
            }
            // Preserve input order for hard links/case aliases without discarding
            // either path's sidecar. No queue or object mutex spans native I/O.
            let finished = Arc::new(FileTurn::default());
            let previous = identities.insert(observation.file_identity, Arc::clone(&finished));
            if self.runtime.is_stopping()
                || ticket.is_cancelled()
                || stopped.load(Ordering::Acquire)
                || tasks
                    .send(PreparedFile {
                        index,
                        asset,
                        previous,
                        finished,
                    })
                    .is_err()
            {
                break;
            }
        }
    }

    fn run_embedded_lane(
        &self,
        ticket: &JobTicket,
        stopped: &AtomicBool,
        tasks: Arc<Mutex<Receiver<PreparedFile>>>,
        output: Sender<Completion>,
    ) {
        let cancelled = || {
            self.runtime.is_stopping() || ticket.is_cancelled() || stopped.load(Ordering::Acquire)
        };
        let mut session = None;
        while !cancelled() {
            let task = tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .recv();
            let Ok(PreparedFile {
                index,
                asset,
                previous,
                finished,
            }) = task
            else {
                break;
            };
            let _turn = FinishTurn {
                turn: &finished,
                stopped,
            };
            if let Some(previous) = previous {
                let mut done = previous
                    .done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*done && !cancelled() {
                    done = previous
                        .changed
                        .wait_timeout(done, Duration::from_millis(20))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
            }
            if cancelled() {
                break;
            }
            let outcome = self
                .metadata
                .sync_embedded_file(&asset.path, &mut session, &cancelled)
                .map(Some)
                .map_err(|error| EmbeddedSyncFailure {
                    path: asset.path.clone(),
                    error: error.error.to_string(),
                    uncertain: error.uncertain,
                });
            // A broken lane stops admission globally, while other in-flight
            // writes still finish and report their partial successes. No replay.
            if session
                .as_ref()
                .is_some_and(super::exiftool_session::Session::is_broken)
            {
                stopped.store(true, Ordering::Release);
            }
            if output
                .send(Completion {
                    index,
                    path: asset.path,
                    outcome,
                })
                .is_err()
            {
                stopped.store(true, Ordering::Release);
                break;
            }
        }
        drop(session);
    }

    fn refresh_written(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() || self.runtime.is_stopping() {
            return;
        }
        if let Err(error) = self.request(paths, MetadataRequestPriority::Background) {
            eprintln!("embedded metadata applied; summary refresh unavailable: {error}");
        }
    }
}

fn send_failure(output: &Sender<Completion>, index: usize, path: PathBuf, error: String) {
    let _ = output.send(Completion {
        index,
        path: path.clone(),
        outcome: Err(EmbeddedSyncFailure {
            path,
            error,
            uncertain: false,
        }),
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{AppPaths, OxyApp, metadata::types::MetadataPatch};
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    struct ReleaseOnDrop(PathBuf);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            for index in 0..8 {
                let _ = fs::write(self.0.join(format!("release-photo-{index}.jpg")), b"finish");
            }
        }
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for owned writer"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn progress(events: &Receiver<CoreEvent>, completed: usize) -> EmbeddedSyncProgress {
        loop {
            if let CoreEvent::EmbeddedSyncProgress(value) =
                events.recv_timeout(Duration::from_secs(5)).unwrap()
                && value.completed == completed
            {
                return value;
            }
        }
    }

    fn fixture() -> (tempfile::TempDir, OxyApp, Receiver<CoreEvent>, Vec<PathBuf>) {
        let temp = tempfile::tempdir().unwrap();
        let (app, events) = OxyApp::open(AppPaths::from_roots(
            temp.path().join("data"),
            temp.path().join("cache"),
        ))
        .unwrap();
        let mut paths = Vec::new();
        for index in 0..8 {
            let path = temp.path().join(format!("photo-{index}.jpg"));
            image::RgbImage::new(8, 8).save(&path).unwrap();
            app.metadata_service
                .metadata
                .patch_metadata(
                    &path,
                    AssetKind::Jpeg,
                    &MetadataPatch {
                        rating: Some(Some(4)),
                        ..MetadataPatch::default()
                    },
                )
                .unwrap();
            paths.push(path);
        }
        let executable = temp.path().join("fake-exiftool");
        fs::write(
            &executable,
            r#"#!/bin/sh
root=$(dirname "$0")
while IFS= read -r arg; do
  case "$arg" in
    *.jpg) file=${arg##*/} ;;
    -stay_open) IFS= read -r flag; [ "$flag" = False ] && exit 0 ;;
    -execute*)
      id=${arg#-execute}
      printf '%s %s\n' "$$" "$file" >> "$root/admissions"
      touch "$root/entered-$file"
      while [ ! -f "$root/release-$file" ]; do sleep 0.01; done
      [ -f "$root/break-$file" ] && exit 0
      printf 'oxy-status-%s:0\n' "$id" >&2
      printf '{ready%s}\n' "$id"
      ;;
  esac
done
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        app.metadata_service.metadata.set_exiftool(Some(executable));
        (temp, app, events, paths)
    }

    fn entered(root: &Path, index: usize) -> bool {
        root.join(format!("entered-photo-{index}.jpg")).is_file()
    }

    #[test]
    fn two_active_writes_finish_after_cancel_and_shutdown_waits_for_both() {
        let (temp, app, events, paths) = fixture();
        let _release = ReleaseOnDrop(temp.path().to_owned());
        let receiver = app
            .metadata_service
            .sync_to_embedded(paths, "cancel-two".into())
            .unwrap();
        let id = progress(&events, 0).job_id;
        wait_for(|| entered(temp.path(), 0) && entered(temp.path(), 1));
        assert!(!entered(temp.path(), 2));
        let admissions = fs::read_to_string(temp.path().join("admissions")).unwrap();
        assert_eq!(
            admissions
                .lines()
                .map(|line| line.split_whitespace().next().unwrap())
                .collect::<HashSet<_>>()
                .len(),
            2
        );
        assert!(app.jobs.cancel(&id));
        fs::write(temp.path().join("release-photo-1.jpg"), b"finish").unwrap();
        assert_eq!(progress(&events, 1).updated, 1);
        assert!(!app.shutdown(Duration::ZERO));
        fs::write(temp.path().join("release-photo-0.jpg"), b"finish").unwrap();
        let result = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.completed, 2);
        assert_eq!(result.updated, 2);
        assert!(result.failures.is_empty());
        assert_eq!(
            fs::read_to_string(temp.path().join("admissions"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }

    #[test]
    fn broken_lane_stops_new_admission_but_keeps_other_in_flight_success() {
        let (temp, app, events, paths) = fixture();
        let _release = ReleaseOnDrop(temp.path().to_owned());
        fs::write(temp.path().join("break-photo-0.jpg"), b"break").unwrap();
        let receiver = app
            .metadata_service
            .sync_to_embedded(paths, "broken-two".into())
            .unwrap();
        wait_for(|| entered(temp.path(), 0) && entered(temp.path(), 1));
        fs::write(temp.path().join("release-photo-0.jpg"), b"finish").unwrap();
        // Wait for the failed result before releasing the other native writer.
        assert_eq!(progress(&events, 1).updated, 0);
        fs::write(temp.path().join("release-photo-1.jpg"), b"finish").unwrap();
        let result = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(!result.cancelled);
        assert_eq!(result.total, 8);
        assert_eq!(result.completed, 2);
        assert_eq!(result.updated, 1);
        assert_eq!(result.failures.len(), 1);
        assert!(result.failures[0].uncertain);
        assert!(result.failures[0].path.ends_with("photo-0.jpg"));
        assert_eq!(
            fs::read_to_string(temp.path().join("admissions"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }

    #[test]
    fn canonical_alias_is_skipped_and_invalid_concurrency_is_rejected_before_admission() {
        let (temp, app, _events, paths) = fixture();
        let _release = ReleaseOnDrop(temp.path().to_owned());
        for concurrency in [0, 5, usize::MAX] {
            assert!(
                app.metadata_service
                    .sync_to_embedded_with_options(
                        paths.clone(),
                        "invalid".into(),
                        EmbeddedSyncOptions { concurrency }
                    )
                    .is_err()
            );
        }
        let alias = temp.path().join("alias.jpg");
        std::os::unix::fs::symlink(&paths[0], &alias).unwrap();
        fs::copy(paths[0].with_extension("xmp"), alias.with_extension("xmp")).unwrap();
        fs::write(temp.path().join("release-photo-0.jpg"), b"finish").unwrap();
        let result = app
            .metadata_service
            .sync_to_embedded(vec![paths[0].clone(), alias], "alias".into())
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(result.completed, 2);
        assert_eq!(result.updated, 1);
        assert_eq!(result.skipped, 1);
        assert_eq!(
            fs::read_to_string(temp.path().join("admissions"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(app.shutdown(Duration::from_secs(3)));
    }

    #[test]
    fn hard_links_keep_both_sidecars_and_write_in_input_order() {
        let (temp, app, events, paths) = fixture();
        let _release = ReleaseOnDrop(temp.path().to_owned());
        fs::remove_file(&paths[1]).unwrap();
        fs::hard_link(&paths[0], &paths[1]).unwrap();
        let receiver = app
            .metadata_service
            .sync_to_embedded_with_options(
                paths[..3].to_vec(),
                "hard-links".into(),
                EmbeddedSyncOptions { concurrency: 3 },
            )
            .unwrap();
        wait_for(|| entered(temp.path(), 0) && entered(temp.path(), 2));
        assert!(!entered(temp.path(), 1));
        fs::write(temp.path().join("release-photo-2.jpg"), b"finish").unwrap();
        assert_eq!(progress(&events, 1).updated, 1);
        assert!(!entered(temp.path(), 1));
        fs::write(temp.path().join("release-photo-0.jpg"), b"finish").unwrap();
        wait_for(|| entered(temp.path(), 1));
        fs::write(temp.path().join("release-photo-1.jpg"), b"finish").unwrap();
        let result = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(result.updated, 3);
        assert_eq!(result.skipped, 0);
        assert!(result.failures.is_empty());
        assert!(app.shutdown(Duration::from_secs(3)));
    }
}
