// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! What an engine draws on, rather than what it is configured to do.
//!
//! Compression needs two things from its caller that have nothing to do with the format: somewhere
//! to allocate output buffers, and somewhere to keep engine state between messages. Both are
//! shared, both are cloneable handles, and both belong to the calling application rather than to
//! any one message -- so they travel together, as [`Resources`].

use std::sync::OnceLock;

use bytesbuf::mem::{GlobalPool, MemoryShared, OpaqueMemory};

use crate::pool::Pool;

/// The memory and engine state a compressor or decompressor draws on.
///
/// Everything a builder carries describes what to do; this describes what to do it with. Hold one
/// per application -- or per subsystem that wants its own memory accounting -- and hand it to every
/// compressor and decompressor. Cloning is cheap, and every clone draws on the same memory and the same engines.
///
/// # Recycling
///
/// Building a compressor allocates and initializes a substantial amount of state: on a small
/// message, as much work as the compression itself. These resources recycle that state between
/// messages, so a service that compresses many small bodies spends its budget compressing rather
/// than getting ready to compress. It is on by default, and
/// [`with_pool_capacity(0)`][Resources::with_pool_capacity] turns it off for the rare caller that
/// wants to measure what it is worth, or that compresses one message and exits.
///
/// Recycling is transparent: it applies to the engines that benefit and quietly skips the rest, so
/// calling code never has to know which is which, and which engines those are can change without
/// any change to calling code.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use compressors::{Resources, gzip};
///
/// // One shared instance: the process-wide memory provider and process-wide engine recycling.
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
    /// The engines belong to the returned value, so every clone of it shares them, and separately
    /// constructed resources have independent engine pools. Whether they also share memory is a
    /// property of the [`MemoryShared`] provider handed in, not of this type.
    #[must_use]
    pub fn new(memory: impl MemoryShared) -> Self {
        Self {
            memory: OpaqueMemory::new(memory),
            pool: Pool::new(),
        }
    }

    /// Sets how many idle engines are kept per distinct configuration, where zero stops recycling.
    ///
    /// Recycling is already on after [`new`][Resources::new] at a capacity that suits ordinary
    /// request traffic. Set this to the number of messages expected to be in flight at once, or to
    /// zero when compression is rare enough that retaining engine state costs more than rebuilding
    /// it.
    ///
    /// The capacity bounds what is kept, not what can be used: a burst beyond it still compresses,
    /// building engines it then drops instead of storing.
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
    /// This is the right answer for an application that has no reason to account for memory per
    /// subsystem, and it is what makes recycling the easy path rather than the deliberate one.
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
        all(
            not(test),
            not(any(
                test,
                feature = "brotli",
                feature = "deflate",
                feature = "gzip",
                feature = "zlib",
                feature = "zstd"
            ))
        ),
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
        assert!(recycling.pool().capacity() > 0, "recycling should be the default");

        let plain = recycling.with_pool_capacity(0);
        assert_eq!(plain.pool().capacity(), 0, "a capacity of zero must stop recycling");

        assert_eq!(plain.with_pool_capacity(4).pool().capacity(), 4, "the capacity must be honoured");
    }

    #[test]
    fn the_global_resources_are_one_instance() {
        assert!(
            std::ptr::eq(Resources::global(), Resources::global()),
            "every caller must see the same global resources"
        );
        assert!(
            Resources::default().pool().capacity() > 0,
            "the default is the global handle, which recycles"
        );
    }

    #[test]
    fn memory_is_available_for_input_as_well_as_output() {
        let resources = Resources::new(GlobalPool::new());

        assert!(
            resources.memory().reserve(16).remaining_capacity() >= 16,
            "the memory provider must be reachable"
        );
        assert!(format!("{resources:?}").contains("Resources"));
    }
}
