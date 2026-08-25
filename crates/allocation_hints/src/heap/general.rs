// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! General-purpose heap configuration and usage information.

const MEDIUM_SLICE_BYTES: usize = 64 * 1024;
const MAX_LOCALITY_SEGMENT_BYTES: usize = 1024 * 1024 * 1024;
const MAX_MEDIUM_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// Runtime options for a general-purpose heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options {
    locality_segment_bytes: usize,
    medium_cache_max_bytes: usize,
}

/// Cheap information for a general-purpose heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Info {
    options: Options,
    thread_target: bool,
}

/// Usage totals for one allocation category.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocationUsage {
    live_allocations: usize,
    requested_bytes: usize,
    usable_bytes: usize,
}

/// A general-purpose heap usage snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Usage {
    small: AllocationUsage,
    medium: AllocationUsage,
    direct: AllocationUsage,
    cached_medium_bytes: usize,
    slab_count: usize,
    slice_count: usize,
}

impl Options {
    /// The default locality segment size: 4 MiB.
    pub const DEFAULT_LOCALITY_SEGMENT_BYTES: usize = 4 * 1024 * 1024;
    /// The default and largest supported per-heap medium cache entry: 8 MiB.
    pub const DEFAULT_MEDIUM_CACHE_MAX_BYTES: usize = MAX_MEDIUM_CACHE_BYTES;

    /// Returns the standard general-purpose heap options.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            locality_segment_bytes: Self::DEFAULT_LOCALITY_SEGMENT_BYTES,
            medium_cache_max_bytes: Self::DEFAULT_MEDIUM_CACHE_MAX_BYTES,
        }
    }

    /// Sets the locality segment size.
    ///
    /// # Panics
    ///
    /// Panics unless `bytes` is a power of two from 64 KiB through 1 GiB.
    #[must_use]
    pub const fn with_locality_segment_bytes(mut self, bytes: usize) -> Self {
        assert!(
            bytes >= MEDIUM_SLICE_BYTES && bytes <= MAX_LOCALITY_SEGMENT_BYTES && bytes.is_power_of_two(),
            "locality segment bytes must be a power of two from 64 KiB through 1 GiB"
        );
        self.locality_segment_bytes = bytes;
        self
    }

    /// Sets the largest power-of-two medium span retained in the local cache.
    ///
    /// # Panics
    ///
    /// Panics unless `bytes` is zero or a power of two from 64 KiB through 8 MiB.
    #[must_use]
    pub const fn with_medium_cache_max_bytes(mut self, bytes: usize) -> Self {
        assert!(
            bytes == 0 || (bytes >= MEDIUM_SLICE_BYTES && bytes <= MAX_MEDIUM_CACHE_BYTES && bytes.is_power_of_two()),
            "medium cache maximum bytes must be zero or a power of two from 64 KiB through 8 MiB"
        );
        self.medium_cache_max_bytes = bytes;
        self
    }

    /// Returns the locality segment size.
    #[must_use]
    pub const fn locality_segment_bytes(self) -> usize {
        self.locality_segment_bytes
    }

    /// Returns the largest locally cached medium span.
    #[must_use]
    pub const fn medium_cache_max_bytes(self) -> usize {
        self.medium_cache_max_bytes
    }

    #[doc(hidden)]
    /// Creates provider-facing options from validated primitive values.
    ///
    /// # Panics
    ///
    /// Panics when either value violates the corresponding public builder's
    /// contract.
    #[must_use]
    pub const fn from_values(locality_segment_bytes: usize, medium_cache_max_bytes: usize) -> Self {
        Self::new()
            .with_locality_segment_bytes(locality_segment_bytes)
            .with_medium_cache_max_bytes(medium_cache_max_bytes)
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

impl Info {
    /// Returns the heap options.
    #[must_use]
    pub const fn options(&self) -> Options {
        self.options
    }

    /// Returns whether this describes an implicit thread heap target.
    #[must_use]
    pub const fn is_thread_target(&self) -> bool {
        self.thread_target
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn new(options: Options, thread_target: bool) -> Self {
        Self { options, thread_target }
    }
}

impl AllocationUsage {
    /// Returns the number of live allocations.
    #[must_use]
    pub const fn live_allocations(&self) -> usize {
        self.live_allocations
    }

    /// Returns the total requested bytes.
    #[must_use]
    pub const fn requested_bytes(&self) -> usize {
        self.requested_bytes
    }

    /// Returns the total usable bytes.
    #[must_use]
    pub const fn usable_bytes(&self) -> usize {
        self.usable_bytes
    }

    #[doc(hidden)]
    pub fn add(&mut self, live_allocations: usize, requested_bytes: usize, usable_bytes: usize) {
        self.live_allocations += live_allocations;
        self.requested_bytes += requested_bytes;
        self.usable_bytes += usable_bytes;
    }
}

impl Usage {
    /// Returns small-allocation usage.
    #[must_use]
    pub const fn small(&self) -> &AllocationUsage {
        &self.small
    }

    /// Returns medium-allocation usage.
    #[must_use]
    pub const fn medium(&self) -> &AllocationUsage {
        &self.medium
    }

    /// Returns direct-allocation usage.
    #[must_use]
    pub const fn direct(&self) -> &AllocationUsage {
        &self.direct
    }

    /// Returns bytes held in the heap's medium cache.
    #[must_use]
    pub const fn cached_medium_bytes(&self) -> usize {
        self.cached_medium_bytes
    }

    /// Returns the number of slabs owned by the heap.
    #[must_use]
    pub const fn slab_count(&self) -> usize {
        self.slab_count
    }

    /// Returns the number of slices owned by the heap.
    #[must_use]
    pub const fn slice_count(&self) -> usize {
        self.slice_count
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        small: AllocationUsage,
        medium: AllocationUsage,
        direct: AllocationUsage,
        cached_medium_bytes: usize,
        slab_count: usize,
        slice_count: usize,
    ) -> Self {
        Self {
            small,
            medium,
            direct,
            cached_medium_bytes,
            slab_count,
            slice_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_constructor_enforces_public_option_invariants() {
        let options = Options::from_values(MEDIUM_SLICE_BYTES, 0);
        assert_eq!(options.locality_segment_bytes(), MEDIUM_SLICE_BYTES);
        assert_eq!(options.medium_cache_max_bytes(), 0);

        std::panic::catch_unwind(|| Options::from_values(MEDIUM_SLICE_BYTES - 1, 0)).unwrap_err();
        std::panic::catch_unwind(|| Options::from_values(MEDIUM_SLICE_BYTES, 1)).unwrap_err();
    }
}
