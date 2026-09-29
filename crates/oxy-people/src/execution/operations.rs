//! One bounded background person operation per application. Locks protect only
//! small snapshots; downloads, decoding, inference and database work run outside.

use oxy_domain::{PersonOperationState, PersonOperationStatus};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub struct PersonOperations {
    current: Mutex<Option<Arc<PersonOperation>>>,
}

pub struct PersonOperation {
    status: Mutex<PersonOperationStatus>,
    cancelled: oxy_runtime::CancellationToken,
    finished: AtomicBool,
}

impl PersonOperations {
    pub fn reserve(&self, status: PersonOperationStatus) -> Result<Arc<PersonOperation>, String> {
        let mut current = self.current.lock().map_err(|error| error.to_string())?;
        if current
            .as_ref()
            .is_some_and(|item| !item.finished.load(Ordering::Acquire))
        {
            return Err("已有下载或识别任务正在运行，请等待完成或取消。".into());
        }
        let operation = Arc::new(PersonOperation {
            status: Mutex::new(status),
            cancelled: oxy_runtime::CancellationToken::default(),
            finished: AtomicBool::new(false),
        });
        *current = Some(Arc::clone(&operation));
        Ok(operation)
    }

    pub fn snapshot(&self) -> Result<Option<PersonOperationStatus>, String> {
        let current = self
            .current
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        current.map(|item| item.snapshot()).transpose()
    }

    pub fn cancel(&self, operation_id: &str) -> Result<(), String> {
        let current = self
            .current
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        let Some(current) = current else {
            return Err("任务不存在".into());
        };
        if current.snapshot()?.operation_id != operation_id {
            return Err("任务已变更，请刷新后重试".into());
        }
        current.cancelled.cancel();
        Ok(())
    }
}

impl PersonOperation {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.is_cancelled()
    }

    pub fn cancellation(&self) -> &oxy_runtime::CancellationToken {
        &self.cancelled
    }

    pub fn snapshot(&self) -> Result<PersonOperationStatus, String> {
        Ok(self
            .status
            .lock()
            .map_err(|error| error.to_string())?
            .clone())
    }

    pub fn update(&self, update: impl FnOnce(&mut PersonOperationStatus)) {
        if let Ok(mut status) = self.status.lock() {
            update(&mut status);
        }
    }

    /// Called only once all worker resources have been released, so cancel
    /// cannot admit a second worker while the first is still inside inference.
    pub fn finish(&self, result: Result<(), String>) {
        self.update(|status| {
            if self.is_cancelled() {
                status.state = PersonOperationState::Cancelled;
                status.detail = "已取消".into();
            } else if let Err(error) = result {
                status.state = PersonOperationState::Failed;
                status.detail = "任务失败，请查看下方原因后重试。".into();
                status.error = Some(match status.error.take() {
                    Some(detail) if detail != error => format!("{error} {detail}"),
                    _ => error,
                });
            } else {
                status.state = PersonOperationState::Completed;
            }
        });
        self.finished.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: &str) -> PersonOperationStatus {
        PersonOperationStatus {
            operation_id: id.into(),
            folder_path: None,
            state: PersonOperationState::Preparing,
            completed: 0,
            total: 0,
            detail: String::new(),
            error: None,
            run: None,
        }
    }

    #[test]
    fn cancellation_waits_for_worker_exit_and_old_ids_cannot_cancel_new_work() {
        let operations = PersonOperations::default();
        let first = operations.reserve(status("first")).unwrap();
        operations.cancel("first").unwrap();
        assert!(first.is_cancelled());
        assert!(operations.reserve(status("second")).is_err());
        first.finish(Ok(()));
        let second = operations.reserve(status("second")).unwrap();
        assert!(operations.cancel("first").is_err());
        assert!(!second.is_cancelled());
    }
}
