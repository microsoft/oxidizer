// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Memory and reusable state for compression engines.
//!
//! Compression needs a memory provider for output and a pool for reusable engine state.
//! [`Resources`] stores shared handles to both.

use std::sync::OnceLock;

use bytesbuf::mem::{GlobalPool, MemoryShared, OpaqueMemory};
use thread_aware::{Thread, ThreadAware};

use crate::pool::Pool;

/// The memory and engine state a compressor or decompressor draws on.
///
/// A builder describes engine behavior. `Resources` supplies memory and reusable engine state.
/// Keep one value per application or per subsystem with separate memory accounting. Clones share
/// the same memory provider and engine pool.
///
/// # Recycling
///
/// Building a compressor allocates and initializes engine state. For a small message, this setup
/// costs about as much as compression. Recycling keeps this state between messages. Recycling is
/// enabled by default. [`with_pool_capacity(0)`][Resources::with_pool_capacity] disables it.
///
/// The public contract does not specify which engines are recycled.
///
/// # Relocation
///
/// [`ThreadAware`] lets a runtime report movement between threads. `Resources` relocates the memory
/// provider. A NUMA-aware or per-thread provider can then use memory near the destination thread.
/// The engine pool stays shared because idle engine state has no thread affinity.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use compressors::{Resources, gzip};
///
/// // Use the process-wide memory provider and engine pool.
/// let resources = Resources::global();
///
/// let compressed = gzip::compress(b"hello", resources)?;
/// assert_eq!(
///     gzip::decompress(compressed, resources)?.to_vec(),
///     b"hello".to_vec()
/// );
/// # }
/// # Ok::<(), compressors::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Resources {
    memory: OpaqueMemory,
    pool: Pool,
}

impl Resources {
    /// Draws output buffers from `memory`, recycling engine state between messages.
    ///
    /// Clones of the returned value share its engine pool. Separately constructed values have
    /// separate engine pools. The [`MemoryShared`] implementation determines whether they share
    /// memory.
    #[must_use]
    pub fn new(memory: impl MemoryShared) -> Self {
        Self {
            memory: OpaqueMemory::new(memory),
            pool: Pool::new(),
        }
    }

    /// Sets how many idle engines are kept per interchangeable group, where zero stops recycling.
    ///
    /// What counts as a group depends on the engine. The deflate family's compressors are keyed by
    /// container and level because a reset preserves both. Zstd contexts have no key because any
    /// idle context can serve any level after reset. Therefore, the ceiling on retained
    /// engines is this capacity times the number of groups a workload actually reaches, not this
    /// capacity alone.
    ///
    /// [`new`][Resources::new] enables recycling. Set the capacity to the expected number of
    /// concurrent messages. Set it to zero to disable recycling.
    ///
    /// The capacity bounds retained idle engines, not active engines. A larger burst builds
    /// additional engines and drops them after use.
    #[must_use]
    pub fn with_pool_capacity(mut self, capacity: usize) -> Self {
        self.pool = match capacity {
            0 => Pool::disabled().clone(),
            capacity => Pool::with_capacity(capacity),
        };
        self
    }

    /// The shared resources of the process: one memory provider, one set of recycled engines.
    ///
    /// Use this value when the application does not need separate memory accounting.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<Resources> = OnceLock::new();

        GLOBAL.get_or_init(|| Self::new(global_memory().clone()))
    }

    /// The memory provider output buffers are drawn from.
    ///
    /// Also the provider to build input with, so that a message is allocated out of the same memory
    /// it is compressed into.
    #[must_use]
    pub fn memory(&self) -> &OpaqueMemory {
        &self.memory
    }

    /// The engines a compressor or decompressor built from these resources checks out of, and back into.
    #[cfg_attr(
        not(any(test, any_format)),
        expect(dead_code, reason = "only a format module's build method checks an engine out")
    )]
    pub(crate) fn pool(&self) -> &Pool {
        &self.pool
    }
}

impl Default for Resources {
    /// The [`global`][Resources::global] resources, as an owned handle.
    fn default() -> Self {
        Self::global().clone()
    }
}

/// Relocates the memory provider but keeps the shared engine pool.
impl ThreadAware for Resources {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        // A NUMA-aware or per-thread provider can use memory near the destination thread.
        self.memory.relocate(source, destination);

        // Every clone shares one pool. Idle engine state has no thread affinity, so relocation does
        // not move or drain the pool.
    }
}

/// The one global memory provider this crate creates, shared by every [`Resources`] that does not
/// name its own.
fn global_memory() -> &'static GlobalPool {
    static MEMORY: OnceLock<GlobalPool> = OnceLock::new();

    MEMORY.get_or_init(GlobalPool::new)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recycling_is_on_by_default_and_its_capacity_is_adjustable() {
        let recycling = Resources::new(GlobalPool::new());
        assert!(recycling.pool().capacity() > 0, "recycling must be the default");

        let plain = recycling.with_pool_capacity(0);
        assert_eq!(plain.pool().capacity(), 0, "a capacity of zero must stop recycling");

        assert_eq!(plain.with_pool_capacity(4).pool().capacity(), 4, "the capacity must be honored");
    }

    #[test]
    fn relocating_moves_the_memory_provider_and_leaves_the_pool_alone() {
        use thread_aware::Relocator;

        use crate::testing::counting_memory;

        let (memory, activity) = counting_memory();
        let mut resources = Resources::new(memory).with_pool_capacity(4);

        _ = Relocator::between_threads().relocate(&mut resources);

        assert_eq!(activity.relocations(), 1, "the memory provider must be told where it now runs");
        assert_eq!(
            resources.pool().capacity(),
            4,
            "relocation must not disturb the pool, whose whole purpose is to outlive a move"
        );
    }
}
