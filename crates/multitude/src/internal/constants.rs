// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared sizing constants for the chunk allocator.

/// Smallest cacheable chunk total allocation size in bytes (header + payload).
pub(crate) const MIN_CHUNK_BYTES: usize = 512;

/// Largest cacheable chunk total allocation size in bytes (header + payload).
///
/// Anything strictly larger is "oversized": sized exactly to fit the request
/// (plus header and drop-list rounding) and bypasses the cache entirely.
pub(crate) const MAX_CHUNK_BYTES: usize = 65_536;

/// Required alignment for every [`SharedChunk`](super::shared_chunk::SharedChunk)
/// allocation. Matches [`MAX_CHUNK_BYTES`] so that for any pointer to a
/// non-oversized value in the chunk, the chunk header's address can be
/// recovered by subtracting the low `CHUNK_ALIGN - 1` bits of the pointer.
///
/// This in turn allows [`Box`](crate::Box) and similar smart pointers
/// to store a single value pointer without separately tracking the
/// chunk header.
pub(crate) const CHUNK_ALIGN: usize = MAX_CHUNK_BYTES;

/// Maximum alignment accepted by smart-pointer / `Allocator::allocate`
/// allocations. Values at or above this cap can no longer be guaranteed
/// to lie strictly inside the first [`CHUNK_ALIGN`] bytes of their
/// chunk, which would break the header-recovery mask used by
/// `Drop` / `deallocate`.
#[cfg_attr(test, mutants::skip)] // `/ → *` lets over-aligned requests spin → OOM
#[inline]
pub(crate) const fn max_smart_ptr_align() -> usize {
    CHUNK_ALIGN / 2
}

/// Number of cacheable size classes (powers of two from [`MIN_CHUNK_BYTES`]
/// up to [`MAX_CHUNK_BYTES`] inclusive).
pub(crate) const NUM_CHUNK_CLASSES: u8 = 8;

/// Default value of the per-arena `max_normal_alloc` knob.
pub(crate) const MAX_NORMAL_ALLOC: usize = 16 * 1024;

/// Cache size-class index, range `0..NUM_CHUNK_CLASSES`.
///
/// Wraps the raw `u8` to make invalid classes harder to construct
/// accidentally and to centralize the
/// [`bytes`](Self::bytes)/[`saturating_inc`](Self::saturating_inc)
/// helpers. `#[repr(transparent)]` so that `AtomicU8` cache slots in
/// [`ChunkProvider`](super::chunk_provider::ChunkProvider) can keep
/// storing the raw byte without conversion.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub(crate) struct SizeClass(u8);

impl SizeClass {
    pub(crate) const ZERO: Self = Self(0);
    pub(crate) const MAX: Self = Self(NUM_CHUNK_CLASSES - 1);

    /// Construct a `SizeClass` from a raw index, checking the range in
    /// debug builds.
    #[inline]
    #[must_use]
    pub(crate) const fn new(c: u8) -> Self {
        debug_assert!(c < NUM_CHUNK_CLASSES, "class out of range");
        Self(c)
    }

    /// Raw class index.
    #[inline]
    #[must_use]
    pub(crate) const fn raw(self) -> u8 {
        self.0
    }

    /// Total allocation size in bytes for this class (header + payload).
    #[inline]
    #[must_use]
    pub(crate) const fn bytes(self) -> usize {
        MIN_CHUNK_BYTES << self.0
    }

    /// Smallest size class whose total allocation is at least `bytes`.
    /// Saturates at [`Self::MAX`] when `bytes > MAX_CHUNK_BYTES`.
    #[inline]
    #[must_use]
    pub(crate) const fn min_for_bytes(bytes: usize) -> Self {
        if bytes <= MIN_CHUNK_BYTES {
            return Self::ZERO;
        }
        if bytes >= MAX_CHUNK_BYTES {
            return Self::MAX;
        }
        let ratio = bytes.div_ceil(MIN_CHUNK_BYTES);
        let mut c: u8 = 0;
        let mut v: usize = 1;
        while v < ratio {
            v <<= 1;
            c += 1;
        }
        Self(c)
    }

    /// Saturating increment, clamped at [`Self::MAX`].
    #[inline]
    #[must_use]
    pub(crate) const fn saturating_inc(self) -> Self {
        let next = self.0.saturating_add(1);
        if next >= NUM_CHUNK_CLASSES { Self::MAX } else { Self(next) }
    }

    /// Returns the larger of two classes.
    #[inline]
    #[must_use]
    pub(crate) const fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }

    /// Clamp to at most `cap`.
    #[inline]
    #[must_use]
    pub(crate) const fn min(self, cap: Self) -> Self {
        if self.0 <= cap.0 { self } else { cap }
    }
}

/// Aborts the process on chunk-refcount overflow.
///
/// A refcount that wraps to zero would let live pointers race with a free,
/// so the only sound response is to terminate the process.
#[cold]
#[inline(never)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // unreachable: refcount overflow requires usize::MAX live refs
pub(crate) fn refcount_overflow_abort() -> ! {
    // Under `cfg(test)` we panic instead of aborting so the overflow-guard
    // call sites (otherwise unreachable without `usize::MAX` live references)
    // can be exercised by `#[should_panic]` unit tests. Production builds are
    // never compiled with `cfg(test)`, so the abort behavior below is the only
    // one that ships.
    #[cfg(test)]
    {
        panic!("multitude: refcount overflow (test)");
    }
    #[cfg(all(feature = "std", not(test)))]
    {
        std::process::abort();
    }
    #[cfg(all(not(feature = "std"), not(test)))]
    {
        struct ForceAbort;
        impl Drop for ForceAbort {
            fn drop(&mut self) {
                panic!("multitude: chunk refcount overflow (abort)");
            }
        }
        let _force = ForceAbort;
        panic!("multitude: chunk refcount overflow");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invokes `max_smart_ptr_align` at runtime (not in a const context)
    // so coverage instrumentation records its body.
    #[test]
    fn max_smart_ptr_align_is_half_chunk_align() {
        assert_eq!(max_smart_ptr_align(), CHUNK_ALIGN / 2);
    }

    /// `raw()` must return the same byte that was passed to `new()` for every
    /// valid class — pins the trivial accessor so mutants that hard-code a
    /// constant (e.g. always-1) are caught.
    #[test]
    fn size_class_raw_round_trips_every_index() {
        for i in 0..NUM_CHUNK_CLASSES {
            assert_eq!(SizeClass::new(i).raw(), i);
        }
    }
}
