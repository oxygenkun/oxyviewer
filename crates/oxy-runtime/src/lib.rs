use oxy_domain::{JobId, JobPriority};
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Debug, Clone)]
pub struct JobTicket {
    pub id: JobId,
    pub priority: JobPriority,
    cancelled: Arc<AtomicBool>,
}

impl JobTicket {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct JobRegistry {
    next_id: AtomicU64,
    jobs: RwLock<HashMap<JobId, Arc<AtomicBool>>>,
}

impl JobRegistry {
    pub fn register(&self, priority: JobPriority) -> JobTicket {
        let id = format!("job-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.jobs.write().insert(id.clone(), cancelled.clone());
        JobTicket {
            id,
            priority,
            cancelled,
        }
    }

    pub fn cancel(&self, id: &str) -> bool {
        let jobs = self.jobs.read();
        let Some(flag) = jobs.get(id) else {
            return false;
        };
        flag.store(true, Ordering::Relaxed);
        true
    }

    pub fn finish(&self, id: &str) {
        self.jobs.write().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_can_be_cancelled_and_finished() {
        let registry = JobRegistry::default();
        let ticket = registry.register(JobPriority::VisibleThumbnail);
        assert!(registry.cancel(&ticket.id));
        assert!(ticket.is_cancelled());
        registry.finish(&ticket.id);
        assert!(!registry.cancel(&ticket.id));
    }
}
