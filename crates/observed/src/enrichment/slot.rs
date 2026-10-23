// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local enrichment storage: linked-list nodes, RAII guards, and cross-thread transfer.
//!
//! See the [Enrichment](crate#enrichment) section for a high-level overview.

use std::any::type_name;
use std::cell::RefCell;

use performables::arc::Arc;
use smallvec::{SmallVec, smallvec};
use thread_aware::{Thread, ThreadAware};
use thread_local::ThreadLocal;

use crate::SinkId;
use crate::enrichment::{Enrichment, EnrichmentEntry};

pub(crate) type OptEnrichmentNode = Option<Arc<EnrichmentNode>>;

/// Inline capacity for per-sink enrichment slot lists. Composite sinks
/// typically fan out to only a handful of children, so 3 covers the common
/// case without heap-spilling; larger fan-outs spill to the heap transparently.
const INLINE_SLOT_CAPACITY: usize = 3;

/// A single node in the linked enrichment chain.
#[derive(Debug, Clone)]
pub(crate) struct EnrichmentNode {
    pub(crate) entries: Arc<[EnrichmentEntry]>,
    pub(crate) parent: OptEnrichmentNode,
}

/// Thread-local slot per sink, holding the current enrichment chain tail.
#[derive(Clone)]
pub(crate) struct Slot(Arc<ThreadLocal<RefCell<OptEnrichmentNode>>>);

impl thread_aware::ThreadAware for Slot {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        // Enrichment slot is thread local, it doesn't need to be relocated
    }
}

impl Slot {
    pub(crate) fn new() -> Self {
        Self(Arc::new(ThreadLocal::new()))
    }

    pub(crate) fn current(&self) -> OptEnrichmentNode {
        let cell = self.0.get()?;
        cell.borrow().clone()
    }

    /// Returns `true` if both handles address the same thread-local storage.
    ///
    /// Slot identity is preserved by `Sink::clone` and by folding a leaf into a
    /// composite, so this identifies the underlying leaf sink.
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn replace(&self, node: OptEnrichmentNode) -> OptEnrichmentNode {
        let cell = self.0.get_or(|| RefCell::new(None));
        cell.replace(node)
    }

    /// Installs a reusable node with one TLS lookup and one mutable borrow.
    ///
    /// The replacement is built before the slot changes, so an allocation
    /// panic leaves the current chain intact.
    fn push_reusable(&self, entries: &Arc<[EnrichmentEntry]>, node: &mut Option<Arc<EnrichmentNode>>) -> OptEnrichmentNode {
        let cell = self.0.get_or(|| RefCell::new(None));
        let mut current = cell.borrow_mut();
        let parent = current.clone();
        let pushed = if let Some(cached) = node {
            if let Some(cached_node) = Arc::get_mut(cached) {
                cached_node.parent = parent;
                Arc::clone(cached)
            } else {
                let pushed = Arc::new(EnrichmentNode {
                    entries: Arc::clone(entries),
                    parent,
                });
                *cached = Arc::clone(&pushed);
                pushed
            }
        } else {
            let pushed = Arc::new(EnrichmentNode {
                entries: Arc::clone(entries),
                parent,
            });
            *node = Some(Arc::clone(&pushed));
            pushed
        };
        current.replace(pushed)
    }

    /// Pushes entries onto the enrichment chain and returns a guard.
    ///
    /// An empty layer installs no chain node and records no restoration, so the
    /// storage boundary holds for callers that reach it without passing through
    /// [`Sink::push_enrichment`](crate::Sink).
    pub(crate) fn push(&self, entries: Arc<[EnrichmentEntry]>) -> Guard {
        if entries.is_empty() {
            return Guard::empty();
        }

        let prev = {
            let cell = self.0.get_or(|| RefCell::new(None));
            let next = Arc::new(EnrichmentNode {
                entries,
                parent: cell.borrow().clone(),
            });
            cell.borrow_mut().replace(next)
        };
        Guard {
            slots: smallvec![(self.clone(), prev)],
        }
    }
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// RAII guard that restores previous enrichment heads when dropped.
///
/// Holds one `(slot, prev)` pair per mutation. Drop unwinds them in reverse
/// mutation order, so nested pushes onto the same slot restore correctly even
/// if a caller manages to record two mutations of one slot in a single guard.
#[must_use]
pub(crate) struct Guard {
    slots: SmallVec<[(Slot, OptEnrichmentNode); INLINE_SLOT_CAPACITY]>,
}

impl Guard {
    pub(crate) fn empty() -> Self {
        Self { slots: SmallVec::new() }
    }

    /// Flattens several guards into one. Each input guard is consumed and its
    /// `Drop` is disarmed (`mem::take` empties its slots), so the merged guard
    /// owns the restoration responsibility.
    pub(crate) fn merge(guards: impl IntoIterator<Item = Self>) -> Self {
        let mut slots = SmallVec::new();
        for mut g in guards {
            slots.extend(std::mem::take(&mut g.slots));
        }
        Self { slots }
    }
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Reverse order: the last mutation must be undone first, otherwise two
        // mutations of one slot would restore the older head and leave the
        // newer one installed.
        for (slot, prev) in self.slots.drain(..).rev() {
            let _ = slot.replace(prev);
        }
    }
}

#[derive(Debug)]
struct PushSlot {
    slot: Slot,
    node: Option<Arc<EnrichmentNode>>,
}

/// Reusable enrichment nodes and restoration scratch for an enriched future.
///
/// Holds at most one cached node per immutable sink slot and is owned by the
/// future. A node is evicted when another owner captures it, preserving the
/// captured chain as an immutable snapshot.
#[derive(Debug)]
pub(crate) struct PushState {
    entries: Arc<[EnrichmentEntry]>,
    slots: SmallVec<[PushSlot; INLINE_SLOT_CAPACITY]>,
    previous_heads: SmallVec<[OptEnrichmentNode; INLINE_SLOT_CAPACITY]>,
}

impl PushState {
    pub(crate) fn new(entries: Arc<[EnrichmentEntry]>, slots: impl IntoIterator<Item = Slot>) -> Self {
        let slots = if entries.is_empty() {
            SmallVec::new()
        } else {
            slots
                .into_iter()
                .map(|slot| PushSlot {
                    slot,
                    node: Some(Arc::new(EnrichmentNode {
                        entries: Arc::clone(&entries),
                        parent: None,
                    })),
                })
                .collect()
        };

        Self {
            entries,
            previous_heads: SmallVec::with_capacity(slots.len()),
            slots,
        }
    }

    pub(crate) fn apply(&mut self) -> PushedGuard<'_> {
        debug_assert!(self.previous_heads.is_empty());
        for slot in &mut self.slots {
            let PushSlot { slot, node } = slot;
            self.previous_heads.push(slot.push_reusable(&self.entries, node));
        }
        PushedGuard { state: self }
    }
}

/// Restores slot heads pushed by a [`PushState`].
#[must_use]
pub(crate) struct PushedGuard<'a> {
    state: &'a mut PushState,
}

impl Drop for PushedGuard<'_> {
    fn drop(&mut self) {
        debug_assert_eq!(self.state.slots.len(), self.state.previous_heads.len());
        while let Some(previous) = self.state.previous_heads.pop() {
            let slot = &mut self.state.slots[self.state.previous_heads.len()];
            drop(slot.slot.replace(previous));

            if let Some(node) = slot.node.as_mut()
                && let Some(node) = Arc::get_mut(node)
            {
                node.parent = None;
                continue;
            }
            slot.node = None;
        }
    }
}

/// Reusable restoration scratch space for an attached future.
///
/// Capacity is fixed from the transfer's immutable slot count and retained
/// only until the attached future is dropped.
#[derive(Debug)]
pub(crate) struct ApplyState {
    previous_heads: SmallVec<[OptEnrichmentNode; INLINE_SLOT_CAPACITY]>,
}

impl ApplyState {
    fn new(slot_count: usize) -> Self {
        Self {
            previous_heads: SmallVec::with_capacity(slot_count),
        }
    }
}

/// Restores slot heads installed by an [`EnrichmentTransfer`].
///
/// Borrows reusable storage owned by the attached future so transfers with
/// more than [`INLINE_SLOT_CAPACITY`] slots do not allocate on every poll.
#[must_use]
pub(crate) struct AppliedGuard<'a> {
    slots: &'a [(Slot, OptEnrichmentNode)],
    state: &'a mut ApplyState,
}

impl std::fmt::Debug for AppliedGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Drop for AppliedGuard<'_> {
    fn drop(&mut self) {
        debug_assert_eq!(self.slots.len(), self.state.previous_heads.len());
        while let Some(previous) = self.state.previous_heads.pop() {
            let slot = &self.slots[self.state.previous_heads.len()].0;
            let _ = slot.replace(previous);
        }
    }
}

/// Captured enrichment state for cross-thread transfer.
///
/// Created by [`Sink::transfer_context`](crate::Sink::transfer_context).
/// Carries one `(slot, head)` pair per leaf sink that contributed to the
/// capture - slot identity (the `Arc<ThreadLocal<…>>` inside [`Slot`])
/// is preserved across `Sink::clone`, so each pair addresses the *same*
/// enrichment chain on whichever thread the transfer is applied on.
///
/// Apply on the target thread via
/// [`Transfer::apply_current_thread`](crate::context::Transfer::apply_current_thread); each captured
/// slot's head is restored independently, so divergent state across
/// composite children round-trips correctly.
#[derive(Clone, Default, ThreadAware)]
pub(crate) struct EnrichmentTransfer {
    #[thread_aware(skip)] // immutable Arc-shared state; no locks involved
    slots: SmallVec<[(Slot, OptEnrichmentNode); INLINE_SLOT_CAPACITY]>,
}

impl EnrichmentTransfer {
    /// Appends a slot's current chain head to this transfer.
    pub(crate) fn add_slot(&mut self, slot: &Slot) {
        self.slots.push((slot.clone(), slot.current()));
    }

    /// Restores every captured `(slot, head)` on this thread, returning a
    /// guard that pops each slot back to its prior head on drop.
    ///
    /// Borrows `self` so the same transfer can be applied repeatedly (e.g.
    /// from a future's `poll` loop) without cloning the captured slots.
    pub(crate) fn apply_state(&self) -> ApplyState {
        ApplyState::new(self.slots.len())
    }

    /// Restores captured heads using scratch storage retained by the caller.
    pub(crate) fn apply<'a>(&'a self, state: &'a mut ApplyState) -> AppliedGuard<'a> {
        debug_assert!(state.previous_heads.is_empty());
        for (slot, head) in &self.slots {
            let previous = slot.replace(head.clone());
            state.previous_heads.push(previous);
        }
        AppliedGuard { slots: &self.slots, state }
    }

    /// Restores captured heads with an owned guard that can outlive this transfer.
    pub(crate) fn apply_owned(&self) -> Guard {
        let mut slots = SmallVec::with_capacity(self.slots.len());
        for (slot, head) in &self.slots {
            let prev = slot.replace(head.clone());
            slots.push((slot.clone(), prev));
        }
        Guard { slots }
    }

    /// Pushes an additional enrichment node onto every captured chain.
    /// Broadcast within the transfer's known slots; transfers with no
    /// captured slots are left unchanged.
    ///
    /// Emptiness is tested while the entries are still an owned `Vec`, so an
    /// empty layer skips the shared-slice allocation as well as the chain
    /// nodes. [`push_entries`](Self::push_entries) relies on this and on the
    /// same check in [`push_for`](Self::push_for) for its non-empty input.
    pub(crate) fn push(&mut self, additional_enrichment: impl Enrichment) {
        if self.slots.is_empty() {
            return;
        }

        let entries = additional_enrichment.into_entries();
        if entries.is_empty() {
            return;
        }

        self.push_entries(&Arc::from(entries));
    }

    /// Same as [`push`](Self::push), but marks every entry as targeted at
    /// `target`, so only that sink observes it. Mirrors the `with_target`
    /// mapping that `EnrichFutureExt::enrich_for` applies.
    pub(crate) fn push_for(&mut self, target: SinkId, additional_enrichment: impl Enrichment) {
        if self.slots.is_empty() {
            return;
        }

        let entries = additional_enrichment.into_entries();
        if entries.is_empty() {
            return;
        }

        self.push_entries(&entries.into_iter().map(|entry| entry.with_target(target)).collect());
    }

    /// Layers `entries` onto every captured chain as a single new node.
    fn push_entries(&mut self, entries: &Arc<[EnrichmentEntry]>) {
        if self.slots.is_empty() {
            return;
        }

        for (_, node) in &mut self.slots {
            *node = Some(Arc::new(EnrichmentNode {
                entries: Arc::clone(entries),
                parent: node.take(),
            }));
        }
    }
}

impl std::fmt::Debug for EnrichmentTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod coverage_tests {
    use super::*;

    struct TestEnrichment(&'static str);

    impl Enrichment for TestEnrichment {
        fn into_entries(self) -> Vec<EnrichmentEntry> {
            vec![EnrichmentEntry::unclassified(self.0, 1_i64)]
        }
    }

    struct EmptyEnrichment;

    impl Enrichment for EmptyEnrichment {
        fn into_entries(self) -> Vec<EnrichmentEntry> {
            Vec::new()
        }
    }

    struct UnreachableEnrichment;

    impl Enrichment for UnreachableEnrichment {
        fn into_entries(self) -> Vec<EnrichmentEntry> {
            panic!("an empty transfer must not materialize enrichment entries")
        }
    }

    /// Flattens a chain into its entries, innermost node first.
    fn chain_entries(head: &OptEnrichmentNode) -> Vec<EnrichmentEntry> {
        let mut entries = Vec::new();
        let mut cursor = head.as_ref();
        while let Some(node) = cursor {
            entries.extend(node.entries.iter().cloned());
            cursor = node.parent.as_ref();
        }
        entries
    }

    #[test]
    fn ptr_eq_identifies_shared_thread_local_storage() {
        // Slot identity is what lets a transfer address the same chain after a
        // `Sink::clone`, so a clone must compare equal and a fresh slot must not.
        let slot = Slot::new();

        assert!(slot.ptr_eq(&slot.clone()));
        assert!(!slot.ptr_eq(&Slot::new()));
    }

    #[test]
    fn guard_restores_the_previous_head_on_drop() {
        let slot = Slot::new();
        assert!(slot.current().is_none());

        {
            let _guard = slot.push(Arc::from(vec![EnrichmentEntry::unclassified("k", 1_i64)]));
            assert!(slot.current().is_some());
        }

        assert!(slot.current().is_none(), "dropping the guard must pop the pushed node");
    }

    #[test]
    fn empty_push_returns_noop_guard_without_touching_the_slot() {
        let slot = Slot::new();
        let empty_guard = slot.push(Arc::from(Vec::new()));
        assert!(slot.current().is_none());

        let real_guard = slot.push(Arc::from(vec![EnrichmentEntry::unclassified("k", 1_i64)]));
        let merged = Guard::merge([empty_guard, real_guard]);
        assert!(slot.current().is_some());

        drop(merged);
        assert!(slot.current().is_none());
    }

    #[test]
    fn push_state_reuses_nodes_without_retaining_the_parent() {
        let slot = Slot::new();
        let parent_guard = slot.push(Arc::from(vec![EnrichmentEntry::unclassified("parent", 1_i64)]));
        let mut state = PushState::new(Arc::from(vec![EnrichmentEntry::unclassified("pushed", 2_i64)]), [slot.clone()]);
        let node = Arc::as_ptr(state.slots[0].node.as_ref().unwrap());

        for _ in 0..2 {
            {
                let _guard = state.apply();
                assert_eq!(chain_entries(&slot.current())[0].key().as_str(), "pushed");
            }

            assert_eq!(chain_entries(&slot.current())[0].key().as_str(), "parent");
            assert_eq!(Arc::as_ptr(state.slots[0].node.as_ref().unwrap()), node);
            assert!(state.slots[0].node.as_ref().unwrap().parent.is_none());
        }

        drop(parent_guard);
    }

    #[test]
    fn push_state_restores_the_parent_during_unwind() {
        let slot = Slot::new();
        let parent_guard = slot.push(Arc::from(vec![EnrichmentEntry::unclassified("parent", 1_i64)]));
        let parent = slot.current().unwrap();
        let mut state = PushState::new(Arc::from(vec![EnrichmentEntry::unclassified("pushed", 2_i64)]), [slot.clone()]);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.apply();
            panic!("exercise pushed guard unwind restoration");
        }));

        assert!(result.is_err());
        assert!(Arc::ptr_eq(slot.current().as_ref().unwrap(), &parent));
        drop(parent_guard);
    }

    #[test]
    fn push_state_evicts_a_node_captured_by_a_transfer() {
        let slot = Slot::new();
        let mut state = PushState::new(Arc::from(vec![EnrichmentEntry::unclassified("pushed", 1_i64)]), [slot.clone()]);

        let captured = {
            let _guard = state.apply();
            slot.current().unwrap()
        };
        assert!(state.slots[0].node.is_none());

        {
            let _guard = state.apply();
            assert!(!Arc::ptr_eq(slot.current().as_ref().unwrap(), &captured));
        }
        assert_eq!(chain_entries(&Some(captured))[0].key().as_str(), "pushed");
    }

    #[test]
    fn push_reusable_replaces_a_shared_cached_node() {
        let slot = Slot::new();
        let entries = Arc::from(vec![EnrichmentEntry::unclassified("pushed", 1_i64)]);
        let shared = Arc::new(EnrichmentNode {
            entries: Arc::clone(&entries),
            parent: None,
        });
        let mut cached = Some(Arc::clone(&shared));

        let previous = slot.push_reusable(&entries, &mut cached);

        assert!(previous.is_none());
        assert!(!Arc::ptr_eq(slot.current().as_ref().unwrap(), &shared));
        assert!(!Arc::ptr_eq(cached.as_ref().unwrap(), &shared));
        drop(slot.replace(previous));
    }

    #[test]
    fn transfer_captures_a_slot_and_layers_further_enrichment_onto_it() {
        let slot = Slot::new();
        let mut transfer = EnrichmentTransfer::default();
        transfer.add_slot(&slot);

        transfer.push(TestEnrichment("global"));
        transfer.push_for(SinkId::new("target"), TestEnrichment("targeted"));

        let mut state = transfer.apply_state();
        let guard = transfer.apply(&mut state);
        let entries = chain_entries(&slot.current());

        assert_eq!(entries.len(), 2, "both pushes must reach the captured slot");
        assert!(
            entries.iter().any(|entry| entry.target().is_none()),
            "`push` must add an untargeted entry"
        );
        assert!(
            entries.iter().any(|entry| entry.target() == Some(SinkId::new("target"))),
            "`push_for` must mark its entry as targeted"
        );

        drop(guard);
        assert!(slot.current().is_none());
    }

    static_assertions::assert_impl_all!(Slot: thread_aware::ThreadAware);

    #[test]
    fn relocation_preserves_thread_local_storage_identity() {
        let slot = Slot::new();
        let mut relocated = slot.clone();
        let destination = thread_aware::ThreadBuilder::default().build(std::thread::current().id());

        relocated.relocate(None, &destination);

        assert!(slot.ptr_eq(&relocated));
    }

    #[test]
    fn transfer_ignores_empty_enrichment_layers() {
        let slot = Slot::new();
        let mut transfer = EnrichmentTransfer::default();
        transfer.add_slot(&slot);

        transfer.push(EmptyEnrichment);
        transfer.push_for(SinkId::new("target"), EmptyEnrichment);

        let mut state = transfer.apply_state();
        let guard = transfer.apply(&mut state);
        assert!(slot.current().is_none());

        drop(guard);
        assert!(slot.current().is_none());
    }

    #[test]
    fn applied_guard_restores_spilled_slot_storage() {
        const SLOT_COUNT: usize = INLINE_SLOT_CAPACITY + 1;

        let slots: [_; SLOT_COUNT] = std::array::from_fn(|_| Slot::new());
        let captured_guards: Vec<_> = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                slot.push(Arc::from(vec![EnrichmentEntry::unclassified(
                    "captured",
                    i64::try_from(index).unwrap(),
                )]))
            })
            .collect();
        let mut transfer = EnrichmentTransfer::default();
        for slot in &slots {
            transfer.add_slot(slot);
        }
        drop(captured_guards);

        let previous_guards: Vec<_> = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                slot.push(Arc::from(vec![EnrichmentEntry::unclassified(
                    "previous",
                    i64::try_from(index).unwrap(),
                )]))
            })
            .collect();

        let mut state = transfer.apply_state();
        let capacity = state.previous_heads.capacity();
        for _ in 0..2 {
            {
                let _guard = transfer.apply(&mut state);
                for slot in &slots {
                    assert_eq!(chain_entries(&slot.current())[0].key().as_str(), "captured");
                }
            }
            assert!(state.previous_heads.is_empty());
            assert_eq!(state.previous_heads.capacity(), capacity);
            for slot in &slots {
                assert_eq!(chain_entries(&slot.current())[0].key().as_str(), "previous");
            }
        }

        for slot in &slots {
            assert_eq!(chain_entries(&slot.current())[0].key().as_str(), "previous");
        }
        drop(previous_guards);
        for slot in &slots {
            assert!(slot.current().is_none());
        }
    }

    #[test]
    fn empty_transfer_does_not_materialize_enrichment() {
        let mut transfer = EnrichmentTransfer::default();

        transfer.push(UnreachableEnrichment);
        transfer.push_for(SinkId::new("target"), UnreachableEnrichment);
    }

    #[test]
    fn push_entries_ignores_an_empty_transfer() {
        let mut transfer = EnrichmentTransfer::default();
        let entries = Arc::from(vec![EnrichmentEntry::unclassified("ignored", 1_i64)]);

        transfer.push_entries(&entries);

        assert!(transfer.slots.is_empty());
    }

    #[test]
    fn debug_impls() {
        let slot = Slot::new();
        assert!(format!("{slot:?}").contains("Slot"));

        let guard = slot.push(Arc::from(vec![EnrichmentEntry::unclassified("k", 1_i64)]));
        assert!(format!("{guard:?}").contains("Guard"));
        drop(guard);

        let mut transfer = EnrichmentTransfer::default();
        transfer.add_slot(&slot);
        assert!(format!("{transfer:?}").contains("EnrichmentTransfer"));

        let mut state = transfer.apply_state();
        assert!(format!("{state:?}").contains("ApplyState"));
        let applied_guard = transfer.apply(&mut state);
        assert!(format!("{applied_guard:?}").contains("AppliedGuard"));
    }
}
