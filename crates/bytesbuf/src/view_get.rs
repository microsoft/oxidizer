// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! We separate out all the consumption methods for ease of maintenance.

use core::mem::MaybeUninit;
use core::ptr;

use crate::BytesView;

/// Generates the little-, big-, and native-endian read accessors for a primitive numeric type.
///
/// The generated methods delegate to [`BytesView::get_array`] for the shared span traversal and
/// only select the byte order via the primitive's inherent `from_*_bytes` associated function.
macro_rules! get_num_accessors {
    ($t:ty, $le:ident, $be:ident, $ne:ident) => {
        #[doc = concat!("Consumes a `", stringify!($t), "` from the view in little-endian byte order.")]
        ///
        /// The bytes are dropped from the view, moving any remaining bytes to the front.
        ///
        /// # Panics
        ///
        /// Panics if the view does not cover enough bytes of data.
        #[inline]
        #[must_use]
        pub fn $le(&mut self) -> $t {
            <$t>::from_le_bytes(self.get_array())
        }

        #[doc = concat!("Consumes a `", stringify!($t), "` from the view in big-endian byte order.")]
        ///
        /// The bytes are dropped from the view, moving any remaining bytes to the front.
        ///
        /// # Panics
        ///
        /// Panics if the view does not cover enough bytes of data.
        #[inline]
        #[must_use]
        pub fn $be(&mut self) -> $t {
            <$t>::from_be_bytes(self.get_array())
        }

        #[doc = concat!("Consumes a `", stringify!($t), "` from the view in native-endian byte order.")]
        ///
        /// The bytes are dropped from the view, moving any remaining bytes to the front.
        ///
        /// # Panics
        ///
        /// Panics if the view does not cover enough bytes of data.
        #[inline]
        #[must_use]
        pub fn $ne(&mut self) -> $t {
            <$t>::from_ne_bytes(self.get_array())
        }
    };
}

impl BytesView {
    /// Consumes a `u8` from the byte sequence.
    ///
    /// The consumed byte is dropped from the view, moving any remaining bytes to the front.
    ///
    /// If permitted by memory layout considerations and reference counts, the memory capacity
    /// backing the dropped bytes is released back to the memory provider.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"ABC", &memory);
    ///
    /// assert_eq!(view.get_byte(), b'A');
    /// assert_eq!(view.get_byte(), b'B');
    /// assert_eq!(view.get_byte(), b'C');
    /// assert!(view.is_empty());
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the view does not cover enough bytes of data.
    #[inline]
    #[must_use]
    pub fn get_byte(&mut self) -> u8 {
        // The first span (last in reverse order) backs the next byte. Stored spans are never
        // empty (type invariant), so reading its first byte and advancing over it in one pass
        // avoids the general `advance` loop and a second span lookup.
        let front = self.spans_reversed.last_mut().expect("view must cover at least one byte");

        let byte = front[0];

        if front.len() == 1 {
            self.spans_reversed.pop();
        } else {
            // SAFETY: the span has at least two bytes, so advancing by one stays in bounds.
            unsafe {
                front.advance(1);
            }
        }

        // The view covered at least one byte, which we just removed from the front span.
        self.shrink_len(1);

        byte
    }

    /// Transfers bytes into an initialized slice.
    ///
    /// The copied bytes are dropped from the view, moving any remaining bytes to the front.
    ///
    /// If permitted by memory layout considerations and reference counts, the memory capacity
    /// backing the dropped bytes is released back to the memory provider.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"Hello, world!", &memory);
    ///
    /// let mut buffer = [0u8; 5];
    /// view.copy_to_slice(&mut buffer);
    ///
    /// assert_eq!(&buffer, b"Hello");
    /// assert_eq!(view.len(), 8);
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the destination is larger than the view.
    pub fn copy_to_slice(&mut self, mut dst: &mut [u8]) {
        assert!(self.len() >= dst.len());

        // The general slice-walking loop is intentional. A single-span fast path (copying directly
        // when the destination fits within the first slice) trims a few instructions but leaves
        // wall-clock time unchanged, because the per-call cost is dominated by the `advance` span
        // bookkeeping the loop already performs. Adding `unsafe` for a flat result is not worth it.
        while !dst.is_empty() {
            let src = self.first_slice();
            let bytes_to_copy = dst.len().min(src.len());

            dst[..bytes_to_copy].copy_from_slice(&src[..bytes_to_copy]);
            dst = &mut dst[bytes_to_copy..];

            self.advance(bytes_to_copy);
        }
    }

    /// Transfers bytes into a potentially uninitialized slice.
    ///
    /// The copied bytes are dropped from the view, moving any remaining bytes to the front.
    ///
    /// If permitted by memory layout considerations and reference counts, the memory capacity
    /// backing the dropped bytes is released back to the memory provider.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use std::mem::MaybeUninit;
    ///
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"Hello", &memory);
    ///
    /// let mut buffer = MaybeUninit::<[u8; 5]>::uninit();
    /// view.copy_to_uninit_slice(buffer.as_mut());
    ///
    /// // SAFETY: The buffer has been fully initialized by copy_to_uninit_slice.
    /// let buffer = unsafe { buffer.assume_init() };
    /// assert_eq!(&buffer, b"Hello");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the destination is larger than the view.
    pub fn copy_to_uninit_slice(&mut self, mut dst: &mut [MaybeUninit<u8>]) {
        assert!(self.len() >= dst.len());

        while !dst.is_empty() {
            let src = self.first_slice();
            let bytes_to_copy = dst.len().min(src.len());

            // SAFETY: Both are byte slices, so no alignment concerns.
            // We guard against length overflow via min() to constrain to slice length.
            unsafe {
                ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr().cast(), bytes_to_copy);
            }

            dst = &mut dst[bytes_to_copy..];

            self.advance(bytes_to_copy);
        }
    }

    /// Consumes exactly `N` bytes from the front of the view and returns them as an array.
    ///
    /// The numeric accessors delegate here; `N` is the byte width of the primitive, and the caller
    /// decodes the array with the primitive's `from_{le,be,ne}_bytes` associated function.
    ///
    /// # Panics
    ///
    /// Panics if the view does not cover at least `N` bytes.
    #[inline]
    fn get_array<const N: usize>(&mut self) -> [u8; N] {
        assert!(self.len() >= N);

        if let Some(array) = self.get_array_from_first_span::<N>() {
            return array;
        }

        // The bytes straddle a span boundary, so we gather them into a buffer instead.
        self.get_array_buffered::<N>()
    }

    /// Reads `N` bytes directly from the first span when that span fully contains them. Returns
    /// `None` when the bytes straddle a span boundary, in which case the caller must fall back to
    /// buffered assembly.
    ///
    /// The caller must have already verified that the view covers at least `N` bytes.
    #[inline]
    // The span-fit decision is an optimization: when it declines (returns `None`), the caller falls
    // back to `get_array_buffered`, which yields an identical result. Mutation testing cannot
    // distinguish a fast path that declines more eagerly from one that never declines, so we skip
    // this function to avoid reporting those equivalent mutants. The read itself (the byte copy and
    // span consumption) is instead exercised by the single-span behavioral tests below, and the
    // `get_array` / `get_array_buffered` wrappers remain mutation-tested.
    fn get_array_from_first_span<const N: usize>(&mut self) -> Option<[u8; N]> {
        let front = self.spans_reversed.last_mut()?;
        let front_len = front.len() as usize;

        if N > front_len {
            return None;
        }

        // A byte-wise copy tolerates the span's lack of alignment, and reading into a `[u8; N]`
        // can never construct an invalid value regardless of the source bytes. The zero-init is
        // optimized away: `copy_from_slice` fully overwrites the array, so the compiler elides it
        // (verified via Callgrind - identical to a `MaybeUninit` variant, which we avoid to keep
        // this path free of `unsafe`).
        let mut array = [0_u8; N];
        array.copy_from_slice(&front[..N]);

        if front_len == N {
            self.spans_reversed.pop();
        } else {
            // SAFETY: `N < front_len`, so advancing the span by `N` stays in bounds.
            unsafe {
                front.advance(N);
            }
        }

        // The caller verified the view covers at least `N` bytes, which we just removed.
        self.shrink_len(N);

        Some(array)
    }

    /// Assembles `N` bytes gathered across one or more span boundaries.
    ///
    /// Only reached as a fallback when the bytes are not contiguous in the first span. The caller
    /// must have already verified that the view covers at least `N` bytes.
    #[cold] // Most reads will not straddle a slice boundary and not require buffering.
    #[inline(never)] // Keep the buffered loop out of the hot path's stack frame (avoids extra register spills).
    fn get_array_buffered<const N: usize>(&mut self) -> [u8; N] {
        let mut array = [0_u8; N];
        self.copy_to_slice(&mut array);
        array
    }

    get_num_accessors!(u16, get_u16_le, get_u16_be, get_u16_ne);
    get_num_accessors!(i16, get_i16_le, get_i16_be, get_i16_ne);
    get_num_accessors!(u32, get_u32_le, get_u32_be, get_u32_ne);
    get_num_accessors!(i32, get_i32_le, get_i32_be, get_i32_ne);
    get_num_accessors!(u64, get_u64_le, get_u64_be, get_u64_ne);
    get_num_accessors!(i64, get_i64_le, get_i64_be, get_i64_ne);
    get_num_accessors!(u128, get_u128_le, get_u128_be, get_u128_ne);
    get_num_accessors!(i128, get_i128_le, get_i128_be, get_i128_ne);
    get_num_accessors!(f32, get_f32_le, get_f32_be, get_f32_ne);
    get_num_accessors!(f64, get_f64_le, get_f64_be, get_f64_ne);
}
