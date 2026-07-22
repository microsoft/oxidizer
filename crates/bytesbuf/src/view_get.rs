// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! We separate out all the consumption methods for ease of maintenance.

use std::mem::MaybeUninit;
use std::ptr;

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
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"ABC", &memory);
    ///
    /// assert_eq!(view.get_byte(), b'A');
    /// assert_eq!(view.get_byte(), b'B');
    /// assert_eq!(view.get_byte(), b'C');
    /// assert!(view.is_empty());
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
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"Hello, world!", &memory);
    ///
    /// let mut buffer = [0u8; 5];
    /// view.copy_to_slice(&mut buffer);
    ///
    /// assert_eq!(&buffer, b"Hello");
    /// assert_eq!(view.len(), 8); // ", world!" remains
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
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use std::mem::MaybeUninit;
    ///
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"Hello", &memory);
    ///
    /// let mut buffer: [MaybeUninit<u8>; 5] = [const { MaybeUninit::uninit() }; 5];
    /// view.copy_to_uninit_slice(&mut buffer);
    ///
    /// // SAFETY: The buffer has been fully initialized by copy_to_uninit_slice.
    /// let buffer: [u8; 5] = unsafe { std::mem::transmute(buffer) };
    /// assert_eq!(&buffer, b"Hello");
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
    #[cfg_attr(test, mutants::skip)]
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn get_byte() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        assert_eq!(view.get_byte(), 1);
        assert_eq!(view.get_byte(), 2);
        assert_eq!(view.get_byte(), 3);
        assert_eq!(view.get_byte(), 4);
        assert!(view.is_empty());
    }

    #[test]
    fn copy_to_slice() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [0u8; 4];
        view.copy_to_slice(&mut dst);

        assert_eq!(dst, [1, 2, 3, 4]);
        assert!(view.is_empty());
    }

    #[test]
    fn copy_to_smaller_slice_copies_partially() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [0u8; 3];
        view.copy_to_slice(&mut dst);

        assert_eq!(dst, [1, 2, 3]);
        assert_eq!(view.len(), 1);
    }

    #[test]
    #[should_panic]
    fn copy_to_bigger_slice_panics() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [0u8; 8];
        view.copy_to_slice(&mut dst);
    }

    #[test]
    fn copy_to_uninit_slice() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [MaybeUninit::<u8>::uninit(); 4];
        view.copy_to_uninit_slice(&mut dst);

        // SAFETY: It has now been initialized.
        let dst = unsafe { std::mem::transmute::<[MaybeUninit<u8>; 4], [u8; 4]>(dst) };

        assert_eq!(dst, [1, 2, 3, 4]);
        assert!(view.is_empty());
    }

    #[test]
    fn copy_to_uninit_smaller_slice_copies_partially() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [MaybeUninit::<u8>::uninit(); 3];
        view.copy_to_uninit_slice(&mut dst);

        // SAFETY: It has now been initialized.
        let dst = unsafe { std::mem::transmute::<[MaybeUninit<u8>; 3], [u8; 3]>(dst) };

        assert_eq!(dst, [1, 2, 3]);
        assert_eq!(view.len(), 1);
    }

    #[test]
    #[should_panic]
    fn copy_to_uninit_bigger_slice_panics() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [MaybeUninit::<u8>::uninit(); 8];
        view.copy_to_uninit_slice(&mut dst);
    }

    #[test]
    fn copy_to_slice_multi_span() {
        let memory = TransparentMemory::new();
        let data_part1 = [10_u8, 20];
        let data_part2 = [30_u8, 40, 50];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        let mut dst = [0u8; 5];
        view_combined.copy_to_slice(&mut dst);
        assert!(view_combined.is_empty());

        assert_eq!(dst, [10_u8, 20, 30, 40, 50]);
    }

    #[test]
    fn copy_to_uninit_slice_multi_span() {
        let memory = TransparentMemory::new();
        let data_part1 = [10_u8, 20];
        let data_part2 = [30_u8, 40, 50];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        let mut dst = [MaybeUninit::<u8>::uninit(); 5];
        view_combined.copy_to_uninit_slice(&mut dst);
        assert!(view_combined.is_empty());

        // SAFETY: It has now been initialized.
        let dst = unsafe { std::mem::transmute::<[MaybeUninit<u8>; 5], [u8; 5]>(dst) };

        assert_eq!(dst, [10_u8, 20, 30, 40, 50]);
    }

    #[test]
    fn get_u16_le() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

        assert_eq!(view.get_u16_le(), 0x1234);
        assert_eq!(view.get_u16_le(), 0x5678);

        assert!(view.is_empty());
    }

    #[test]
    fn get_u32_le_multi_span() {
        let memory = TransparentMemory::new();
        let view_part1 = BytesView::copied_from_slice(&[0x78_u8, 0x56], &memory);
        let view_part2 = BytesView::copied_from_slice(&[0x34_u8, 0x12], &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view_combined.get_u32_le(), 0x1234_5678);

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_u16_be() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x12, 0x34, 0x56, 0x78], &memory);

        assert_eq!(view.get_u16_be(), 0x1234);
        assert_eq!(view.get_u16_be(), 0x5678);

        assert!(view.is_empty());
    }

    #[test]
    fn get_u32_be_multi_span() {
        let memory = TransparentMemory::new();
        let view_part1 = BytesView::copied_from_slice(&[0x12_u8, 0x34], &memory);
        let view_part2 = BytesView::copied_from_slice(&[0x56_u8, 0x78], &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view_combined.get_u32_be(), 0x1234_5678);

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_u16_ne() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

        if cfg!(target_endian = "big") {
            assert_eq!(view.get_u16_ne(), 0x3412);
            assert_eq!(view.get_u16_ne(), 0x7856);
        } else {
            assert_eq!(view.get_u16_ne(), 0x1234);
            assert_eq!(view.get_u16_ne(), 0x5678);
        }

        assert!(view.is_empty());
    }

    #[test]
    fn get_u32_ne_multi_span() {
        let memory = TransparentMemory::new();
        let view_part1 = BytesView::copied_from_slice(&[0x78_u8, 0x56], &memory);
        let view_part2 = BytesView::copied_from_slice(&[0x34_u8, 0x12], &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        if cfg!(target_endian = "big") {
            assert_eq!(view_combined.get_u32_ne(), 0x7856_3412);
        } else {
            assert_eq!(view_combined.get_u32_ne(), 0x1234_5678);
        }

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_signed_and_float_round_trip() {
        let memory = TransparentMemory::new();

        let mut data = Vec::new();
        data.extend_from_slice(&(-12345_i16).to_le_bytes());
        data.extend_from_slice(&(-2_000_000_000_i32).to_be_bytes());
        data.extend_from_slice(&(-9_000_000_000_000_i64).to_le_bytes());
        data.extend_from_slice(&i128::MIN.to_be_bytes());
        data.extend_from_slice(&std::f32::consts::PI.to_le_bytes());
        data.extend_from_slice(&std::f64::consts::E.to_be_bytes());

        let mut view = BytesView::copied_from_slice(&data, &memory);

        assert_eq!(view.get_i16_le(), -12345);
        assert_eq!(view.get_i32_be(), -2_000_000_000);
        assert_eq!(view.get_i64_le(), -9_000_000_000_000);
        assert_eq!(view.get_i128_be(), i128::MIN);
        assert_eq!(view.get_f32_le().to_bits(), std::f32::consts::PI.to_bits());
        assert_eq!(view.get_f64_be().to_bits(), std::f64::consts::E.to_bits());
        assert!(view.is_empty());
    }

    #[test]
    fn get_wide_value_multi_span() {
        let memory = TransparentMemory::new();
        let value: u128 = 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210;
        let bytes = value.to_le_bytes();

        // Split at an odd offset so the value straddles the span boundary and exercises buffering.
        let view_part1 = BytesView::copied_from_slice(&bytes[..5], &memory);
        let view_part2 = BytesView::copied_from_slice(&bytes[5..], &memory);
        let mut view = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view.get_u128_le(), value);
        assert!(view.is_empty());
    }

    #[test]
    fn get_misaligned_single_span() {
        let memory = TransparentMemory::new();

        // Drop a single leading byte so the eight value bytes sit at an odd offset from the
        // allocation base, exercising the fast path against a misaligned source.
        let value: u64 = 0x0123_4567_89AB_CDEF;
        let mut data = vec![0xFF_u8];
        data.extend_from_slice(&value.to_le_bytes());
        let mut view = BytesView::copied_from_slice(&data, &memory);

        assert_eq!(view.get_byte(), 0xFF);
        assert_eq!(view.get_u64_le(), value);
        assert!(view.is_empty());
    }

    #[test]
    #[should_panic]
    fn get_num_insufficient_bytes_panics() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x00, 0x01, 0x02], &memory);

        // Only three bytes are available, so reading a four-byte value must panic.
        let _ = view.get_u32_le();
    }
}
