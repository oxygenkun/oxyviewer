use oxy_domain::{JobId, JobPriority};
mod foreground;
pub use foreground::{ForegroundGate, ForegroundGuard};
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

/// Maximum concurrent image workers, based on the CPUs available to this process.
pub fn image_worker_count() -> usize {
    static COUNT: std::sync::LazyLock<usize> =
        std::sync::LazyLock::new(|| std::thread::available_parallelism().map_or(1, usize::from));
    *COUNT
}

/// Cloneable cooperative cancellation shared by queues and blocking workers.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulePosition {
    pub tier: u16,
    pub rank: u32,
}

impl SchedulePosition {
    pub const fn new(tier: u16, rank: u32) -> Self {
        Self { tier, rank }
    }
}

impl PartialOrd for SchedulePosition {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for SchedulePosition {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .tier
            .cmp(&self.tier)
            .then_with(|| other.rank.cmp(&self.rank))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePlacement {
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmittedIntentPolicy {
    Release,
    Demote {
        tier: u16,
        placement: QueuePlacement,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveScheduleChange<K> {
    pub key: K,
    pub position: Option<SchedulePosition>,
}

struct ScopeSchedule<K> {
    epoch: u64,
    intents: HashMap<K, SchedulePosition>,
}

/// Aggregates ordered task intents owned by independent scopes. Lower tier and
/// rank values are more important; `SchedulePosition::Ord` reverses that order
/// so the effective position can be selected with `max` and used in a max-heap.
pub struct ScopedIntentScheduler<K, S> {
    scopes: HashMap<S, ScopeSchedule<K>>,
}

impl<K, S> Default for ScopedIntentScheduler<K, S> {
    fn default() -> Self {
        Self {
            scopes: HashMap::new(),
        }
    }
}

impl<K, S> ScopedIntentScheduler<K, S>
where
    K: Clone + Eq + Hash,
    S: Clone + Eq + Hash,
{
    pub fn effective_position(&self, key: &K) -> Option<SchedulePosition> {
        self.scopes
            .values()
            .filter_map(|scope| scope.intents.get(key).copied())
            .max()
    }

    pub fn reconcile(
        &mut self,
        scope_id: S,
        epoch: u64,
        intents: impl IntoIterator<Item = (K, SchedulePosition)>,
        omitted: OmittedIntentPolicy,
    ) -> Option<Vec<EffectiveScheduleChange<K>>> {
        if self
            .scopes
            .get(&scope_id)
            .is_some_and(|scope| scope.epoch > epoch)
        {
            return None;
        }

        let previous = self
            .scopes
            .get(&scope_id)
            .map_or_else(HashMap::new, |scope| scope.intents.clone());
        let mut next = intents.into_iter().collect::<HashMap<_, _>>();
        let mut omitted_items = previous
            .iter()
            .filter(|(key, _)| !next.contains_key(*key))
            .map(|(key, position)| (key.clone(), *position))
            .collect::<Vec<_>>();
        omitted_items.sort_by_key(|(_, position)| (position.tier, position.rank));

        if let OmittedIntentPolicy::Demote { tier, placement } = omitted {
            let back_start = next
                .values()
                .filter(|position| position.tier == tier)
                .map(|position| position.rank)
                .max()
                .map_or(0, |rank| rank.saturating_add(1));
            let omitted_count = u32::try_from(omitted_items.len()).unwrap_or(u32::MAX);
            if placement == QueuePlacement::Front && omitted_count > 0 {
                for position in next.values_mut().filter(|position| position.tier == tier) {
                    position.rank = position.rank.saturating_add(omitted_count);
                }
            }
            for (index, (key, _)) in omitted_items.into_iter().enumerate() {
                let offset = u32::try_from(index).unwrap_or(u32::MAX);
                let rank = match placement {
                    QueuePlacement::Front => offset,
                    QueuePlacement::Back => back_start.saturating_add(offset),
                };
                next.insert(key, SchedulePosition::new(tier, rank));
            }
        }

        let mut affected = previous.keys().cloned().collect::<Vec<_>>();
        affected.extend(
            next.keys()
                .filter(|key| !previous.contains_key(*key))
                .cloned(),
        );
        let before = affected
            .iter()
            .map(|key| (key.clone(), self.effective_position(key)))
            .collect::<HashMap<_, _>>();
        self.scopes.insert(
            scope_id,
            ScopeSchedule {
                epoch,
                intents: next,
            },
        );
        Some(self.changed_effective_positions(affected, before))
    }

    pub fn upsert(
        &mut self,
        scope_id: S,
        epoch: u64,
        key: K,
        tier: u16,
        placement: QueuePlacement,
    ) -> Option<Vec<EffectiveScheduleChange<K>>> {
        if self
            .scopes
            .get(&scope_id)
            .is_some_and(|scope| scope.epoch > epoch)
        {
            return None;
        }
        let mut intents = self
            .scopes
            .get(&scope_id)
            .map_or_else(HashMap::new, |scope| scope.intents.clone());
        intents.remove(&key);
        let mut tier_items = intents
            .iter()
            .filter(|(_, position)| position.tier == tier)
            .map(|(key, position)| (key.clone(), *position))
            .collect::<Vec<_>>();
        tier_items.sort_by_key(|(_, position)| position.rank);
        let mut ordered = Vec::with_capacity(tier_items.len() + 1);
        if placement == QueuePlacement::Front {
            ordered.push(key.clone());
        }
        ordered.extend(tier_items.into_iter().map(|(key, _)| key));
        if placement == QueuePlacement::Back {
            ordered.push(key.clone());
        }
        for (rank, tier_key) in ordered.into_iter().enumerate() {
            intents.insert(
                tier_key,
                SchedulePosition::new(tier, u32::try_from(rank).unwrap_or(u32::MAX)),
            );
        }
        self.reconcile(scope_id, epoch, intents, OmittedIntentPolicy::Release)
    }

    pub fn release(
        &mut self,
        scope_id: &S,
        epoch: u64,
        key: &K,
    ) -> Option<Vec<EffectiveScheduleChange<K>>> {
        let scope = self.scopes.get(scope_id)?;
        if scope.epoch > epoch || !scope.intents.contains_key(key) {
            return None;
        }
        let mut intents = scope.intents.clone();
        intents.remove(key);
        self.reconcile(
            scope_id.clone(),
            epoch,
            intents,
            OmittedIntentPolicy::Release,
        )
    }

    pub fn release_scope(&mut self, scope_id: &S) -> Vec<EffectiveScheduleChange<K>> {
        let Some(scope) = self.scopes.remove(scope_id) else {
            return Vec::new();
        };
        let affected = scope.intents.keys().cloned().collect::<Vec<_>>();
        let before = affected
            .iter()
            .map(|key| {
                let previous = scope.intents.get(key).copied();
                let remaining = self.effective_position(key);
                (key.clone(), previous.max(remaining))
            })
            .collect::<HashMap<_, _>>();
        self.changed_effective_positions(affected, before)
    }

    fn changed_effective_positions(
        &self,
        affected: Vec<K>,
        before: HashMap<K, Option<SchedulePosition>>,
    ) -> Vec<EffectiveScheduleChange<K>> {
        affected
            .into_iter()
            .filter_map(|key| {
                let position = self.effective_position(&key);
                (before.get(&key).copied().flatten() != position)
                    .then_some(EffectiveScheduleChange { key, position })
            })
            .collect()
    }
}

/// Priority queue for source-derived work. One pending entry exists per key;
/// repeated requests merge its payload, while explicit schedule updates may
/// move it in either direction. Stale heap entries are discarded during `pop`.
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

    /// Updates a pending payload and derives its replacement priority from the
    /// updated value, allowing consumer-aware promotion and demotion.
    pub fn update_priority_if_present(
        &mut self,
        key: &K,
        update: impl FnOnce(&mut V) -> P,
    ) -> bool {
        let Some(current) = self.pending.get_mut(key) else {
            return false;
        };
        current.priority = update(&mut current.value);
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

    /// Updates a pending payload and either assigns its replacement priority
    /// or removes it entirely. Stale heap entries are discarded by `pop`.
    pub fn update_or_remove_if_present(
        &mut self,
        key: &K,
        update: impl FnOnce(&mut V) -> Option<P>,
    ) -> bool {
        let Some(current) = self.pending.get_mut(key) else {
            return false;
        };
        let replacement = update(&mut current.value);
        let Some(priority) = replacement else {
            self.pending.remove(key);
            return true;
        };
        let current = self
            .pending
            .get_mut(key)
            .expect("pending entry disappeared during update");
        current.priority = priority;
        current.generation = current.generation.wrapping_add(1);
        current.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.heap.push(HeapEntry {
            key: key.clone(),
            priority,
            generation: current.generation,
            sequence: current.sequence,
        });
        true
    }

    /// Replaces a pending entry's priority, allowing a coordinator to demote
    /// work that is no longer active as well as promote newly active work.
    pub fn reprioritize_if_present(&mut self, key: &K, priority: P) -> bool {
        let Some(current) = self.pending.get_mut(key) else {
            return false;
        };
        current.priority = priority;
        current.generation = current.generation.wrapping_add(1);
        current.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.heap.push(HeapEntry {
            key: key.clone(),
            priority,
            generation: current.generation,
            sequence: current.sequence,
        });
        true
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Removes matching logical entries and returns their payloads. Heap nodes
    /// are left stale and discarded by `pop`, matching priority updates.
    pub fn remove_if(&mut self, mut predicate: impl FnMut(&K, &V) -> bool) -> Vec<(K, V, P)> {
        let keys = self
            .pending
            .iter()
            .filter(|(key, entry)| predicate(key, &entry.value))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| {
                self.pending
                    .remove(&key)
                    .map(|entry| (key, entry.value, entry.priority))
            })
            .collect()
    }

    /// Visits the current logical entries without exposing stale heap nodes.
    /// Intended for low-frequency diagnostics while the caller owns its queue lock.
    pub fn entries(&self) -> impl Iterator<Item = (&K, &V, P)> {
        self.pending
            .iter()
            .map(|(key, entry)| (key, &entry.value, entry.priority))
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
    fn scoped_scheduler_aggregates_consumers_and_rejects_stale_epochs() {
        let mut scheduler = ScopedIntentScheduler::default();
        let task = "asset";
        assert!(
            scheduler
                .reconcile(
                    "grid",
                    2,
                    [(task, SchedulePosition::new(1, 4))],
                    OmittedIntentPolicy::Release,
                )
                .is_some()
        );
        assert!(
            scheduler
                .reconcile(
                    "preload",
                    1,
                    [(task, SchedulePosition::new(3, 0))],
                    OmittedIntentPolicy::Release,
                )
                .is_some()
        );
        assert_eq!(
            scheduler.effective_position(&task),
            Some(SchedulePosition::new(1, 4))
        );

        assert!(
            scheduler
                .reconcile(
                    "grid",
                    1,
                    [(task, SchedulePosition::new(3, 0))],
                    OmittedIntentPolicy::Release,
                )
                .is_none()
        );
        assert_eq!(
            scheduler.effective_position(&task),
            Some(SchedulePosition::new(1, 4))
        );

        let changes = scheduler.release_scope(&"grid");
        assert_eq!(
            changes,
            vec![EffectiveScheduleChange {
                key: task,
                position: Some(SchedulePosition::new(3, 0)),
            }]
        );
        assert_eq!(
            scheduler.effective_position(&task),
            Some(SchedulePosition::new(3, 0))
        );
    }

    #[test]
    fn reconcile_demotes_omitted_work_to_the_requested_edge() {
        let mut scheduler = ScopedIntentScheduler::default();
        scheduler.reconcile(
            "viewport",
            1,
            [
                ("old-a", SchedulePosition::new(1, 0)),
                ("old-b", SchedulePosition::new(1, 1)),
            ],
            OmittedIntentPolicy::Release,
        );
        scheduler.reconcile(
            "viewport",
            2,
            [("visible", SchedulePosition::new(1, 0))],
            OmittedIntentPolicy::Demote {
                tier: 3,
                placement: QueuePlacement::Back,
            },
        );

        assert_eq!(
            scheduler.effective_position(&"visible"),
            Some(SchedulePosition::new(1, 0))
        );
        assert_eq!(
            scheduler.effective_position(&"old-a"),
            Some(SchedulePosition::new(3, 0))
        );
        assert_eq!(
            scheduler.effective_position(&"old-b"),
            Some(SchedulePosition::new(3, 1))
        );
    }

    #[test]
    fn point_upsert_supports_front_and_back_placement() {
        let mut scheduler = ScopedIntentScheduler::default();
        scheduler.upsert("selection", 1, "a", 0, QueuePlacement::Back);
        scheduler.upsert("selection", 2, "b", 0, QueuePlacement::Front);
        scheduler.upsert("selection", 3, "c", 0, QueuePlacement::Back);

        assert_eq!(
            scheduler.effective_position(&"b"),
            Some(SchedulePosition::new(0, 0))
        );
        assert_eq!(
            scheduler.effective_position(&"a"),
            Some(SchedulePosition::new(0, 1))
        );
        assert_eq!(
            scheduler.effective_position(&"c"),
            Some(SchedulePosition::new(0, 2))
        );
    }

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

    #[test]
    fn pending_work_can_be_demoted_when_the_active_context_changes() {
        let mut queue = CoalescingPriorityQueue::default();
        queue.push("old-active", (), 3);
        queue.push("new-active", (), 1);

        assert!(queue.reprioritize_if_present(&"old-active", 1));
        assert!(queue.reprioritize_if_present(&"new-active", 3));
        assert_eq!(queue.pop(), Some(("new-active", (), 3)));
        assert_eq!(queue.pop(), Some(("old-active", (), 1)));
    }

    #[test]
    fn payload_derived_priority_preserves_the_highest_remaining_consumer() {
        let mut queue = CoalescingPriorityQueue::default();
        queue.push("asset", vec![3, 2], 3);

        assert!(queue.update_priority_if_present(&"asset", |priorities| {
            priorities[0] = 0;
            *priorities.iter().max().unwrap()
        }));
        assert_eq!(queue.pop(), Some(("asset", vec![0, 2], 2)));
    }

    #[test]
    fn pending_work_can_be_removed_while_stale_heap_entries_remain() {
        let mut queue = CoalescingPriorityQueue::default();
        queue.push("old", vec!["viewport"], 3);
        queue.push("new", vec!["viewport"], 2);

        assert!(queue.update_or_remove_if_present(&"old", |consumers| {
            consumers.clear();
            None
        }));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop(), Some(("new", vec!["viewport"], 2)));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn matching_pending_work_can_be_removed_in_bulk() {
        let mut queue = CoalescingPriorityQueue::default();
        queue.push("active", 1, 3);
        queue.push("inactive-a", 2, 2);
        queue.push("inactive-b", 3, 1);

        let mut removed = queue.remove_if(|key, _| key.starts_with("inactive"));
        removed.sort_by_key(|(_, value, _)| *value);

        assert_eq!(removed, vec![("inactive-a", 2, 2), ("inactive-b", 3, 1)]);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop(), Some(("active", 1, 3)));
        assert!(queue.pop().is_none());
    }
}
