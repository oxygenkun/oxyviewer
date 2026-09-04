use oxy_domain::{JobId, JobPriority};
use parking_lot::RwLock;
use std::{
    cmp::Ordering as CmpOrdering,
    collections::BinaryHeap,
    collections::HashMap,
    hash::Hash,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

/// Priority queue for source-derived work. One pending entry exists per key;
/// repeated requests replace its payload and can only raise its priority.
/// Stale heap entries created by promotion are discarded during `pop`.
pub struct CoalescingPriorityQueue<K, V, P> {
    pending: HashMap<K, PendingEntry<V, P>>,
    heap: BinaryHeap<HeapEntry<K, P>>,
    next_sequence: u64,
}

struct PendingEntry<V, P> {
    value: V,
    priority: P,
    generation: u64,
    sequence: u64,
}

struct HeapEntry<K, P> {
    key: K,
    priority: P,
    generation: u64,
    sequence: u64,
}

impl<K, P: Ord> PartialEq for HeapEntry<K, P> {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl<K, P: Ord> Eq for HeapEntry<K, P> {}

impl<K, P: Ord> PartialOrd for HeapEntry<K, P> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl<K, P: Ord> Ord for HeapEntry<K, P> {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl<K, V, P> Default for CoalescingPriorityQueue<K, V, P> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            heap: BinaryHeap::new(),
            next_sequence: 0,
        }
    }
}

impl<K, V, P> CoalescingPriorityQueue<K, V, P>
where
    K: Clone + Eq + Hash,
    P: Copy + Ord,
{
    pub fn push(&mut self, key: K, value: V, priority: P) {
        self.push_or_merge(key, value, priority, |current, replacement| {
            *current = replacement;
        });
    }

    pub fn push_or_merge(&mut self, key: K, value: V, priority: P, merge: impl FnOnce(&mut V, V)) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let (accepted_priority, generation) = if let Some(current) = self.pending.get_mut(&key) {
            merge(&mut current.value, value);
            current.priority = current.priority.max(priority);
            current.generation = current.generation.wrapping_add(1);
            current.sequence = sequence;
            (current.priority, current.generation)
        } else {
            self.pending.insert(
                key.clone(),
                PendingEntry {
                    value,
                    priority,
                    generation: 0,
                    sequence,
                },
            );
            (priority, 0)
        };
        self.heap.push(HeapEntry {
            key,
            priority: accepted_priority,
            generation,
            sequence,
        });
    }

    pub fn pop(&mut self) -> Option<(K, V, P)> {
        while let Some(candidate) = self.heap.pop() {
            let Some(current) = self.pending.get(&candidate.key) else {
                continue;
            };
            if current.generation != candidate.generation
                || current.sequence != candidate.sequence
                || current.priority != candidate.priority
            {
                continue;
            }
            let current = self.pending.remove(&candidate.key)?;
            return Some((candidate.key, current.value, current.priority));
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.pending.contains_key(key)
    }

    pub fn update_if_present(&mut self, key: &K, priority: P, update: impl FnOnce(&mut V)) -> bool {
        let Some(current) = self.pending.get_mut(key) else {
            return false;
        };
        update(&mut current.value);
        current.priority = current.priority.max(priority);
        current.generation = current.generation.wrapping_add(1);
        current.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.heap.push(HeapEntry {
            key: key.clone(),
            priority: current.priority,
            generation: current.generation,
            sequence: current.sequence,
        });
        true
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

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

    #[test]
    fn coalescing_queue_promotes_and_replaces_without_duplicate_work() {
        let mut queue = CoalescingPriorityQueue::default();
        queue.push("a", 1, JobPriority::DirectoryBackground);
        queue.push("b", 2, JobPriority::VisibleThumbnail);
        queue.push("a", 3, JobPriority::LoupePreview);

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop(), Some(("a", 3, JobPriority::LoupePreview)));
        assert_eq!(queue.pop(), Some(("b", 2, JobPriority::VisibleThumbnail)));
        assert!(queue.is_empty());
    }

    #[test]
    fn pending_request_can_attach_a_consumer_without_rebuilding_its_payload() {
        let mut queue = CoalescingPriorityQueue::default();
        queue.push("asset", vec!["grid"], JobPriority::DirectoryBackground);

        assert!(
            queue.update_if_present(&"asset", JobPriority::LoupePreview, |consumers| consumers
                .push("loupe"))
        );
        assert_eq!(
            queue.pop(),
            Some(("asset", vec!["grid", "loupe"], JobPriority::LoupePreview))
        );
    }
}
