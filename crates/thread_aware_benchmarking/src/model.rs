// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Object model shared by the wall-clock and instruction-count relocation benchmarks.
//!
//! Both harnesses relocate the same two subjects — a bare `Arc<Payload, PerThread>`
//! and a multi-layer object tree — so their reported shapes describe the same
//! object. Defining that object once keeps the two targets from drifting apart.

#![allow(dead_code, reason = "each bench target uses a different subset of the shared model")]

use std::sync::atomic::{AtomicU64, Ordering};

use thread_aware::{Arc, PerThread, Unaware};

/// Source of distinct strategy-partition identities.
static NEXT_VALUE_ID: AtomicU64 = AtomicU64::new(0);

/// Stand-in for strategy-partitioned state behind an
/// `Arc<T, PerThread>`, such as a connection pipeline or a cache shard.
///
/// Relocation never inspects the payload. The identity exists so the setup can
/// verify that a relocation really swapped in the destination thread's value,
/// and so the measured loop has something observable to consume.
#[derive(Debug)]
pub struct Payload {
    /// Identity of this strategy partition's value.
    pub id: u64,
}

#[cfg_attr(test, mutants::skip)] // Benchmark support is exercised by its harnesses, not unit tests.
impl Payload {
    /// Creates a payload with a distinct identity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Benchmark support is exercised by its harnesses, not unit tests.
impl Default for Payload {
    fn default() -> Self {
        Self::new()
    }
}

/// Depth of the object tree, and therefore the number of distinct storage maps a
/// single tree relocation reads.
///
/// Relocation is a graph walk, so what a caller pays is set by the number of
/// thread-aware nodes reachable from the message, not by the cost of one call.
/// A deliberately modest stand-in for a real message: a request carrying a
/// session, which holds a connection pool, which holds a resolver, which holds a
/// metrics sink.
///
/// This remains nonzero because every [`Tree`] has a root layer.
pub const TREE_DEPTH: usize = 5;

/// Strategy-partitioned state behind one `Arc<_, PerThread>` node of the tree.
#[derive(Debug)]
struct Leaf {
    id: u64,
}

#[cfg_attr(test, mutants::skip)] // Benchmark support is exercised by its harnesses, not unit tests.
impl Leaf {
    fn new() -> Self {
        Self {
            id: NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// One layer of [`Tree`].
///
/// The field mix is the point of the type. `id` and `name` are thread-aware with
/// a no-op relocation, `flags` opts out entirely, and `shared` is a genuine
/// strategy-partitioned node whose relocation reads a keyed entry. Every layer owns
/// a separate storage map, so relocating the tree walks plain data and reads
/// exactly one cell per layer, which is how relocation cost accrues in a consumer.
#[derive(Debug, Clone, thread_aware::ThreadAware)]
struct Layer {
    id: u64,
    name: &'static str,
    flags: Unaware<u32>,
    shared: Arc<Leaf, PerThread>,
    child: Option<Box<Self>>,
}

/// A message-shaped object tree of [`TREE_DEPTH`] layers.
///
/// This is the subject the multithreaded groups relocate, because a runtime
/// relocates whole messages rather than individual values.
#[derive(Debug, Clone, thread_aware::ThreadAware)]
pub struct Tree {
    root: Box<Layer>,
}

#[cfg_attr(test, mutants::skip)] // Benchmark support is exercised by its harnesses, not unit tests.
impl Tree {
    /// Creates the benchmark object tree.
    #[must_use]
    pub fn new() -> Self {
        // Build the innermost layer first so a root always exists without a fallible unwrap.
        let mut root = Box::new(Layer {
            id: 0,
            name: "layer",
            flags: Unaware(0),
            shared: Arc::<Leaf, PerThread>::new(Leaf::new),
            child: None,
        });

        for depth in 1..TREE_DEPTH {
            root = Box::new(Layer {
                id: depth as u64,
                name: "layer",
                flags: Unaware(0),
                shared: Arc::<Leaf, PerThread>::new(Leaf::new),
                child: Some(root),
            });
        }

        Self { root }
    }

    /// Number of `Arc<_, PerThread>` nodes a relocation of this tree has to visit.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.leaf_ids().len()
    }

    /// Identity of the root `Arc<_, PerThread>` node.
    #[must_use]
    pub fn leaf_id(&self) -> u64 {
        self.root.shared.id
    }

    /// Identity of every `Arc<_, PerThread>` node, in layer order.
    #[must_use]
    pub fn leaf_ids(&self) -> Vec<u64> {
        let mut ids = Vec::with_capacity(TREE_DEPTH);
        let mut layer = Some(&self.root);

        while let Some(current) = layer {
            ids.push(current.shared.id);
            layer = current.child.as_ref();
        }

        ids
    }
}

#[cfg_attr(test, mutants::skip)] // Benchmark support is exercised by its harnesses, not unit tests.
impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}
