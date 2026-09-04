// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump heap configuration and usage information.

const BUMP_SEGMENT_SIZE: usize = 32 * 1024;

/// Runtime options for a bump heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Options {
    max_allocation_bytes: usize,
    max_alignment: usize,
    retained_chunks: usize,
    max_retained_chunks: usize,
}

impl Options {
    /// The default maximum bump allocation size: 32 KiB.
    pub(crate) const DEFAULT_MAX_ALLOCATION_BYTES: usize = 32 * 1024;
    /// The default maximum bump allocation alignment: 4 KiB.
    pub(crate) const DEFAULT_MAX_ALIGNMENT: usize = 4 * 1024;
    /// The default number of chunks retained by pooled bump heaps.
    pub(crate) const DEFAULT_RETAINED_CHUNKS: usize = 4;
    /// The default maximum number of chunks retained after recent demand.
    pub(crate) const DEFAULT_MAX_RETAINED_CHUNKS: usize = 16;

    /// Returns the standard bump heap options.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            max_allocation_bytes: Self::DEFAULT_MAX_ALLOCATION_BYTES,
            max_alignment: Self::DEFAULT_MAX_ALIGNMENT,
            retained_chunks: Self::DEFAULT_RETAINED_CHUNKS,
            max_retained_chunks: Self::DEFAULT_MAX_RETAINED_CHUNKS,
        }
    }

    /// Sets the largest allocation eligible for bump allocation.
    ///
    /// # Panics
    ///
    /// Panics if `bytes` is zero or exceeds 32 KiB.
    #[must_use]
    pub(crate) const fn with_max_allocation_bytes(mut self, bytes: usize) -> Self {
        assert!(
            bytes != 0 && bytes <= BUMP_SEGMENT_SIZE,
            "bump maximum allocation bytes must be from 1 byte through 32 KiB"
        );
        self.max_allocation_bytes = bytes;
        self
    }

    /// Sets the largest alignment eligible for bump allocation.
    ///
    /// # Panics
    ///
    /// Panics unless `alignment` is a nonzero power of two through 32 KiB.
    #[must_use]
    pub(crate) const fn with_max_alignment(mut self, alignment: usize) -> Self {
        assert!(
            alignment != 0 && alignment <= BUMP_SEGMENT_SIZE && alignment.is_power_of_two(),
            "bump maximum alignment must be a power of two through 32 KiB"
        );
        self.max_alignment = alignment;
        self
    }

    /// Sets a fixed number of chunks retained when backing state returns to a pool.
    ///
    /// # Panics
    ///
    /// Panics if `chunks` is zero.
    #[must_use]
    pub(crate) const fn with_retained_chunks(mut self, chunks: usize) -> Self {
        assert!(chunks != 0, "a bump heap must retain at least its root chunk");
        self.retained_chunks = chunks;
        self.max_retained_chunks = chunks;
        self
    }

    /// Allows adaptive retention to grow through the given chunk count.
    ///
    /// # Panics
    ///
    /// Panics if `chunks` is below the configured retained minimum.
    #[must_use]
    pub(crate) const fn with_max_retained_chunks(mut self, chunks: usize) -> Self {
        assert!(
            chunks >= self.retained_chunks,
            "maximum retained chunks must not be below the retained minimum"
        );
        self.max_retained_chunks = chunks;
        self
    }

    /// Returns the largest bump allocation size.
    #[must_use]
    pub(crate) const fn max_allocation_bytes(self) -> usize {
        self.max_allocation_bytes
    }

    /// Returns the largest bump allocation alignment.
    #[must_use]
    pub(crate) const fn max_alignment(self) -> usize {
        self.max_alignment
    }

    /// Returns the minimum retained chunk count.
    #[must_use]
    pub(crate) const fn retained_chunks(self) -> usize {
        self.retained_chunks
    }

    /// Returns the maximum retained chunk count.
    #[must_use]
    pub(crate) const fn max_retained_chunks(self) -> usize {
        self.max_retained_chunks
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_the_standard_bump_configuration() {
        assert_eq!(Options::default(), Options::new());
    }
}
