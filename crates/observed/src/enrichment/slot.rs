// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local enrichment storage: linked-list nodes, RAII guards, and cross-thread transfer.
//!
//! See the [Enrichment](crate#enrichment) section for a high-level overview.

use std::any::type_name;
use std::cell::RefCell;
use std::sync::Arc;

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
    #[cfg_attr(coverage_nightly, coverage(off))]
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
        if let Some(node) = node {
            cell.borrow_mut().replace(node)
        } else {
            cell.borrow_mut().take()
        }
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
    pub(crate) fn apply(&self) -> Guard {
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
    fn transfer_captures_a_slot_and_layers_further_enrichment_onto_it() {
        let slot = Slot::new();
        let mut transfer = EnrichmentTransfer::default();
        transfer.add_slot(&slot);

        transfer.push(TestEnrichment("global"));
        transfer.push_for(SinkId::new("target"), TestEnrichment("targeted"));

        let guard = transfer.apply();
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
    fn transfer_ignores_empty_enrichment_layers() {
        let slot = Slot::new();
        let mut transfer = EnrichmentTransfer::default();
        transfer.add_slot(&slot);

        transfer.push(EmptyEnrichment);
        transfer.push_for(SinkId::new("target"), EmptyEnrichment);

        let guard = transfer.apply();
        assert!(slot.current().is_none());

        drop(guard);
        assert!(slot.current().is_none());
    }

    #[test]
    fn debug_impls() {
        let slot = Slot::new();
        assert!(format!("{slot:?}").contains("Slot"));

        let mut transfer = EnrichmentTransfer::default();
        transfer.add_slot(&slot);
        assert!(format!("{transfer:?}").contains("EnrichmentTransfer"));

        let guard = transfer.apply();
        assert!(format!("{guard:?}").contains("Guard"));
    }
}
