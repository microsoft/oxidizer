// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local enrichment storage: linked-list nodes, RAII guards, and cross-thread transfer.
//!
//! See the [Enrichment](crate#enrichment) section for a high-level overview.

use std::any::type_name;
use std::cell::RefCell;
use std::sync::Arc;

use smallvec::{SmallVec, smallvec};
use thread_aware::ThreadAware;
use thread_local::ThreadLocal;

use crate::enrichment::{Enrichment, EnrichmentEntry};

pub(crate) type OptEnrichmentNode = Option<Arc<EnrichmentNode>>;

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
    fn relocate(&mut self, _source: Option<thread_aware::affinity::Affinity>, _destination: thread_aware::affinity::Affinity) {
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

    fn replace(&self, node: OptEnrichmentNode) -> OptEnrichmentNode {
        let cell = self.0.get_or(|| RefCell::new(None));
        if let Some(node) = node {
            cell.borrow_mut().replace(node)
        } else {
            cell.borrow_mut().take()
        }
    }

    /// Pushes entries onto the enrichment chain and returns a guard.
    pub(crate) fn push(&self, entries: Arc<[EnrichmentEntry]>) -> Guard {
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
/// Holds one `(slot, prev)` pair per slot that was mutated. Drop walks them
/// in order and writes each `prev` back into its slot's `RefCell`. Slots are
/// independent, so order across distinct slots doesn't matter.
#[must_use]
pub(crate) struct Guard {
    slots: SmallVec<[(Slot, OptEnrichmentNode); 3]>,
}

impl Guard {
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
        for (slot, prev) in self.slots.drain(..) {
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
/// [`Transfer::apply`](crate::context::Transfer::apply); each captured
/// slot's head is restored independently, so divergent state across
/// composite children round-trips correctly.
#[derive(Clone, Default, ThreadAware)]
pub(crate) struct EnrichmentTransfer {
    #[thread_aware(skip)] // immutable Arc-shared state; no locks involved
    slots: SmallVec<[(Slot, OptEnrichmentNode); 3]>,
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
    pub(crate) fn push(&mut self, additional_enrichment: impl Enrichment) {
        if self.slots.is_empty() {
            return;
        }

        let entries = Arc::<[EnrichmentEntry]>::from(additional_enrichment.into_entries());
        for (_, node) in &mut self.slots {
            *node = Some(Arc::new(EnrichmentNode {
                entries: Arc::clone(&entries),
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
