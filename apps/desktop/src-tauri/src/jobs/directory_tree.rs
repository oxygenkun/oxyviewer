use oxy_domain::{DebugQueueItem, DebugQueueState, DirectoryTreeSnapshot};
use oxy_fs::FsCatalog;
use oxy_runtime::CoalescingPriorityQueue;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};
use tauri::{AppHandle, Emitter};

pub const DIRECTORY_TREE_UPDATED_EVENT: &str = "directory-tree-updated";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    session_id: String,
    directory: PathBuf,
}

#[derive(Debug, Clone)]
struct ActiveDirectory {
    session_id: String,
    directory: PathBuf,
}

#[derive(Default)]
struct WorkState {
    pending: CoalescingPriorityQueue<RequestKey, (), i64>,
    pending_order: HashMap<RequestKey, u64>,
    next_order: u64,
    active: Option<ActiveDirectory>,
    loading: Option<RequestKey>,
}

impl WorkState {
    fn enqueue(&mut self, key: RequestKey) {
        if let Some(&order) = self.pending_order.get(&key) {
            let score = priority_score(self.active.as_ref(), &key, order);
            self.pending.reprioritize_if_present(&key, score);
            return;
        }

        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        let score = priority_score(self.active.as_ref(), &key, order);
        self.pending.push(key.clone(), (), score);
        self.pending_order.insert(key, order);
    }

    fn set_active(&mut self, active: ActiveDirectory) {
        self.active = Some(active);
        let pending = self
            .pending_order
            .iter()
            .map(|(key, order)| (key.clone(), *order))
            .collect::<Vec<_>>();
        for (key, order) in pending {
            let score = priority_score(self.active.as_ref(), &key, order);
            self.pending.reprioritize_if_present(&key, score);
        }
    }

    fn pop(&mut self) -> Option<RequestKey> {
        let request = self.pending.pop().map(|(key, _, _)| key);
        if let Some(key) = &request {
            self.pending_order.remove(key);
        }
        request
    }
}

#[derive(Clone)]
pub struct DirectoryTreeQueue {
    files: Arc<FsCatalog>,
    work: Arc<(Mutex<WorkState>, Condvar)>,
}

impl DirectoryTreeQueue {
    pub fn new(app: AppHandle, files: Arc<FsCatalog>) -> Self {
        let queue = Self {
            files,
            work: Arc::new((Mutex::new(WorkState::default()), Condvar::new())),
        };
        queue.spawn_worker(app);
        queue
    }

    pub fn enqueue(&self, session_id: String, directory: PathBuf) {
        let key = RequestKey {
            session_id,
            directory,
        };
        let mut work = self.work.0.lock().expect("directory tree queue poisoned");
        work.enqueue(key);
        drop(work);
        self.work.1.notify_one();
    }

    pub fn set_active(&self, session_id: String, directory: PathBuf) {
        let mut work = self.work.0.lock().expect("directory tree queue poisoned");
        work.set_active(ActiveDirectory {
            session_id,
            directory,
        });
    }

    pub fn debug_snapshot(&self) -> DebugQueueState {
        let work = self.work.0.lock().expect("directory tree queue poisoned");
        let pending = work
            .pending
            .entries()
            .map(|(key, _, score)| directory_debug_item(key, score))
            .collect::<Vec<_>>();
        let active = work
            .loading
            .as_ref()
            .map(|key| {
                vec![directory_debug_item(
                    key,
                    priority_score(work.active.as_ref(), key, 0),
                )]
            })
            .unwrap_or_default();
        DebugQueueState {
            name: "directoryTree".into(),
            concurrency: 1,
            pending,
            active,
        }
    }

    fn spawn_worker(&self, app: AppHandle) {
        let queue = self.clone();
        std::thread::Builder::new()
            .name("oxy-directory-tree".into())
            .spawn(move || {
                loop {
                    let key = {
                        let mut work = queue.work.0.lock().expect("directory tree queue poisoned");
                        while work.pending.is_empty() {
                            work = queue
                                .work
                                .1
                                .wait(work)
                                .expect("directory tree queue poisoned");
                        }
                        let request = work.pop();
                        work.loading.clone_from(&request);
                        request
                    };
                    let Some(key) = key else {
                        continue;
                    };
                    match queue
                        .files
                        .load_directory_children(&key.session_id, &key.directory)
                    {
                        Ok(snapshot) => publish(&app, snapshot),
                        Err(error) => {
                            if let Ok(snapshot) = queue.files.directory_tree(&key.session_id) {
                                publish(&app, snapshot);
                            }
                            eprintln!(
                                "directory tree load failed for {}: {error}",
                                key.directory.display()
                            );
                        }
                    }
                    queue
                        .work
                        .0
                        .lock()
                        .expect("directory tree queue poisoned")
                        .loading = None;
                }
            })
            .expect("failed to start directory tree worker");
    }
}

fn directory_debug_item(key: &RequestKey, score: i64) -> DebugQueueItem {
    let tier = if score > 2_000_000 {
        3
    } else if score > 1_000_000 {
        2
    } else {
        1
    };
    let rank = tier * 1_000_000 - score;
    DebugQueueItem {
        key: format!("{}:{}", key.session_id, key.directory.display()),
        path: Some(key.directory.clone()),
        root_path: None,
        stage: "children".into(),
        priority: match tier {
            3 => "active",
            2 => "session",
            _ => "background",
        }
        .into(),
        rank: Some(rank),
        consumers: 1,
        pending_count: None,
        asset_count: None,
        directory_count: None,
    }
}

fn publish(app: &AppHandle, snapshot: DirectoryTreeSnapshot) {
    let _ = app.emit(DIRECTORY_TREE_UPDATED_EVENT, snapshot);
}

fn priority_score(active: Option<&ActiveDirectory>, key: &RequestKey, order: u64) -> i64 {
    let tier = match active {
        Some(active)
            if active.session_id == key.session_id && active.directory == key.directory =>
        {
            3
        }
        Some(active) if active.session_id == key.session_id => 2,
        _ => 1,
    };
    i64::from(tier) * 1_000_000 - i64::try_from(order.min(999_999)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_directory_precedes_its_session_and_other_favorites() {
        let active = ActiveDirectory {
            session_id: "active".into(),
            directory: PathBuf::from("/active/current"),
        };
        let current = RequestKey {
            session_id: "active".into(),
            directory: PathBuf::from("/active/current"),
        };
        let sibling = RequestKey {
            session_id: "active".into(),
            directory: PathBuf::from("/active/sibling"),
        };
        let other = RequestKey {
            session_id: "other".into(),
            directory: PathBuf::from("/other"),
        };

        assert!(
            priority_score(Some(&active), &current, 2) > priority_score(Some(&active), &sibling, 0)
        );
        assert!(
            priority_score(Some(&active), &sibling, 2) > priority_score(Some(&active), &other, 0)
        );
    }

    #[test]
    fn changing_favorites_reorders_pending_node_loads() {
        let old_favorite = RequestKey {
            session_id: "old".into(),
            directory: PathBuf::from("/old/child"),
        };
        let new_favorite = RequestKey {
            session_id: "new".into(),
            directory: PathBuf::from("/new/current"),
        };
        let mut work = WorkState::default();

        work.set_active(ActiveDirectory {
            session_id: "old".into(),
            directory: PathBuf::from("/old/current"),
        });
        work.enqueue(old_favorite);
        work.enqueue(new_favorite.clone());
        work.set_active(ActiveDirectory {
            session_id: "new".into(),
            directory: PathBuf::from("/new/current"),
        });

        assert_eq!(work.pop(), Some(new_favorite));
    }
}
