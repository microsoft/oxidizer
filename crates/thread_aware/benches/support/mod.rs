// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Object model shared by the wall-clock (`thread_aware_relocate`) and
//! instruction-count (`thread_aware_relocate_cg`) relocation benchmarks.
//!
//! Both harnesses relocate the same two subjects — a bare `Arc<Payload, PerCore>`
//! and a multi-layer object tree — so their reported shapes describe the same
//! object. Defining that object once keeps the two targets from drifting apart.
//!
//! This module is included into each bench binary with `#[path]`, so every item
//! is used by at least one target but not necessarily by both. The harness-specific
//! setup (materialization across a variable affinity set for Criterion, fixed-tuple
//! inputs for Gungraun) stays in each bench file.

#![allow(dead_code, reason = "each bench target uses a different subset of the shared model")]

use std::sync::atomic::{AtomicU64, Ordering};

use thread_aware::{Arc, PerCore, Unaware};

/// Source of distinct per-affinity identities.
pub(crate) static NEXT_VALUE_ID: AtomicU64 = AtomicU64::new(0);

/// Stand-in for the per-affinity state a consumer keeps behind an
/// `Arc<T, PerCore>`, such as a connection pipeline or a cache shard.
///
/// Relocation never inspects the payload. The identity exists so the setup can
/// verify that a relocation really swapped in the destination affinity's value,
/// and so the measured loop has something observable to consume.
#[derive(Debug)]
pub(crate) struct Payload {
    pub(crate) id: u64,
}

impl Payload {
    pub(crate) fn new() -> Self {
        Self {
            id: NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// Depth of the object tree, and therefore the number of distinct slot tables a
/// single tree relocation reads.
///
/// Relocation is a graph walk, so what a caller pays is set by the number of
/// thread-aware nodes reachable from the message, not by the cost of one call.
/// A deliberately modest stand-in for a real message: a request carrying a
/// session, which holds a connection pool, which holds a resolver, which holds a
/// metrics sink.
pub(crate) const TREE_DEPTH: usize = 5;

/// Per-affinity state held behind one `Arc<_, PerCore>` node of the tree.
#[derive(Debug)]
pub(crate) struct Leaf {
    pub(crate) id: u64,
}

impl Leaf {
    pub(crate) fn new() -> Self {
        Self {
            id: NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// One layer of [`Tree`].
///
/// The field mix is the point of the type. `id` and `name` are thread-aware with
/// a no-op relocation, `flags` opts out entirely, and `shared` is a genuine
/// per-affinity node whose relocation reads a slot cell. Every layer owns a separate
/// slot table, so relocating the tree walks plain data and reads exactly one
/// cell per layer, which is how relocation cost actually accrues in a consumer.
#[derive(Debug, Clone, thread_aware::ThreadAware)]
pub(crate) struct Layer {
    id: u64,
    name: &'static str,
    flags: Unaware<u32>,
    shared: Arc<Leaf, PerCore>,
    child: Option<Box<Self>>,
}

/// A message-shaped object tree of [`TREE_DEPTH`] layers.
///
/// This is the subject the multithreaded groups relocate, because a runtime
/// relocates whole messages rather than individual values.
#[derive(Debug, Clone, thread_aware::ThreadAware)]
pub(crate) struct Tree {
    root: Box<Layer>,
}

impl Tree {
    pub(crate) fn new() -> Self {
        let mut layer = None;

        for depth in 0..TREE_DEPTH {
            layer = Some(Box::new(Layer {
                id: depth as u64,
                name: "layer",
                flags: Unaware(0),
                shared: Arc::<Leaf, PerCore>::new(Leaf::new),
                child: layer,
            }));
        }

        Self {
            root: layer.expect("the loop runs at least once because TREE_DEPTH is nonzero"),
        }
    }

    /// Number of `Arc<_, PerCore>` nodes a relocation of this tree has to visit.
    pub(crate) fn node_count(&self) -> usize {
        self.leaf_ids().len()
    }

    /// Identity of the root `Arc<_, PerCore>` node.
    pub(crate) fn leaf_id(&self) -> u64 {
        self.root.shared.id
    }

    /// Identity of every `Arc<_, PerCore>` node, in layer order.
    pub(crate) fn leaf_ids(&self) -> Vec<u64> {
        let mut ids = Vec::with_capacity(TREE_DEPTH);
        let mut layer = Some(&self.root);

        while let Some(current) = layer {
            ids.push(current.shared.id);
            layer = current.child.as_ref();
        }

        ids
    }
}
