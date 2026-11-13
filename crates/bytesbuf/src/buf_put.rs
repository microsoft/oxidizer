// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! We separate out the mutation functions for ease of maintenance.

use core::borrow::Borrow;
use core::ptr;

use crate::{BytesBuf, BytesView};

/// Generates the little-, big-, and native-endian write accessors for a primitive numeric type.
///
/// Each generated method serializes the value with the primitive's inherent `to_*_bytes` method
/// and appends the resulting bytes via [`BytesBuf::put_slice`].
macro_rules! put_num_accessors {
    ($t:ty, $le:ident, $be:ident, $ne:ident) => {
        #[doc = concat!("Appends a `", stringify!($t), "` to the buffer in little-endian byte order.")]
        ///
        /// # Panics
        ///
        /// Panics if there is insufficient remaining capacity in the buffer.
        #[inline]
        pub fn $le(&mut self, value: $t) {
            self.put_slice(value.to_le_bytes());
        }

        #[doc = concat!("Appends a `", stringify!($t), "` to the buffer in big-endian byte order.")]
        ///
        /// # Panics
        ///
        /// Panics if there is insufficient remaining capacity in the buffer.
        #[inline]
        pub fn $be(&mut self, value: $t) {
            self.put_slice(value.to_be_bytes());
        }

        #[doc = concat!("Appends a `", stringify!($t), "` to the buffer in native-endian byte order.")]
        ///
        /// # Panics
        ///
        /// Panics if there is insufficient remaining capacity in the buffer.
        #[inline]
        pub fn $ne(&mut self, value: $t) {
            self.put_slice(value.to_ne_bytes());
        }
    };
}

impl BytesBuf {
    /// Appends a slice of bytes to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    ///
    /// buf.put_slice(*b"Hello, ");
    /// buf.put_slice(*b"world!");
    ///
    /// assert_eq!(buf.consume_all(), b"Hello, world!");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub fn put_slice(&mut self, src: impl Borrow<[u8]>) {
        let src = src.borrow();
        if !self.put_small(src) {
            self.put_slice_looped(src);
        }
    }

    /// Writes a slice that may span multiple capacity slices, copying one slice at a time.
    ///
    /// This is the general path used by [`put_slice()`] when [`put_small()`] declines a write that
    /// does not fit within the first unfilled slice.
    ///
    /// [`put_slice()`]: Self::put_slice
    /// [`put_small()`]: Self::put_small
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub(crate) fn put_slice_looped(&mut self, mut src: &[u8]) {
        assert!(self.remaining_capacity() >= src.len());

        while !src.is_empty() {
            let dst = self.first_unfilled_slice();

            let to_copy_len = dst.len().min(src.len());

            // Sanity check - we verified lengths above but let's be defensive.
            debug_assert_ne!(to_copy_len, 0);

            let (to_copy, remainder) = src.split_at(to_copy_len);

            // SAFETY: Both are byte slices, so no alignment concerns.
            // We guard against length overflow via min() to constrain to slice length.
            unsafe {
                ptr::copy_nonoverlapping(to_copy.as_ptr(), dst.as_mut_ptr().cast(), to_copy_len);
            }

            // SAFETY: Yes, we really did just write `to_copy_len` bytes.
            unsafe {
                self.advance(to_copy_len);
            }

            src = remainder;
        }

        // Sanity check to protect against silly mutations.
        debug_assert!(self.len() >= src.len());
    }

    /// Appends a byte sequence to the buffer.
    ///
    /// This reuses the existing capacity of the view being appended.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// use bytesbuf::BytesView;
    /// use bytesbuf::mem::Memory;
    /// # use bytesbuf::mem::GlobalPool;
    ///
    /// # let memory = GlobalPool::new();
    /// let mut buf = memory.reserve(16);
    ///
    /// // Create a view with its own memory.
    /// let header = BytesView::copied_from_slice(b"HDR", &memory);
    ///
    /// buf.put_slice(*b"data");
    /// buf.put_bytes(header); // Zero-copy append
    ///
    /// assert_eq!(buf.consume_all(), b"dataHDR");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the resulting total buffer capacity would be greater than `usize::MAX`.
    pub fn put_bytes(&mut self, view: BytesView) {
        self.append(view);
    }

    /// Appends a `u8` to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(8);
    ///
    /// buf.put_byte(0xCA);
    /// buf.put_byte(0xFE);
    ///
    /// assert_eq!(buf.consume_all(), &[0xCA, 0xFE]);
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub fn put_byte(&mut self, value: u8) {
        self.put_slice([value]);
    }

    /// Appends multiple repetitions of a `u8` to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(16);
    ///
    /// // Write a header followed by zero-padding.
    /// buf.put_slice(*b"HDR:");
    /// buf.put_byte_repeated(0x00, 12);
    ///
    /// let data = buf.consume_all();
    /// assert_eq!(data.len(), 16);
    /// assert_eq!(
    ///     data.first_slice(),
    ///     b"HDR:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00"
    /// );
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub fn put_byte_repeated(&mut self, value: u8, mut count: usize) {
        assert!(self.remaining_capacity() >= count);

        while count > 0 {
            let dst = self.first_unfilled_slice();
            let to_fill_len = dst.len().min(count);

            // Sanity check - we verified lengths above but let's be defensive.
            debug_assert_ne!(to_fill_len, 0);

            // SAFETY: We are writing bytes, which is always valid, and we have
            // guarded against overflow via min() to constrain to slice length.
            unsafe {
                ptr::write_bytes(dst.as_mut_ptr(), value, to_fill_len);
            }

            // SAFETY: Yes, we really did just write `to_fill_len` bytes.
            unsafe {
                self.advance(to_fill_len);
            }

            // Will never overflow because it is guarded by min().
            count = count.wrapping_sub(to_fill_len);
        }

        // Sanity check to protect against silly mutations.
        debug_assert!(self.len() >= count);
    }

    put_num_accessors!(u16, put_u16_le, put_u16_be, put_u16_ne);
    put_num_accessors!(i16, put_i16_le, put_i16_be, put_i16_ne);
    put_num_accessors!(u32, put_u32_le, put_u32_be, put_u32_ne);
    put_num_accessors!(i32, put_i32_le, put_i32_be, put_i32_ne);
    put_num_accessors!(u64, put_u64_le, put_u64_be, put_u64_ne);
    put_num_accessors!(i64, put_i64_le, put_i64_be, put_i64_ne);
    put_num_accessors!(u128, put_u128_le, put_u128_be, put_u128_ne);
    put_num_accessors!(i128, put_i128_le, put_i128_be, put_i128_ne);
    put_num_accessors!(f32, put_f32_le, put_f32_be, put_f32_ne);
    put_num_accessors!(f64, put_f64_le, put_f64_be, put_f64_ne);
}

#[cfg(test)]
mod tests {
    // Only the private fast-path return value distinguishes handled writes from fallback.
    use std::num::NonZero;

    use crate::mem::testing::{FixedBlockMemory, TransparentMemory};

    #[test]
    fn put_small_takes_fast_path_when_value_fits() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        // The value fits entirely within the first unfilled slice, so the fast path handles it.
        let data = [1u8, 2, 3, 4, 5];
        assert!(buf.put_small(&data));

        assert_eq!(buf.len(), 5);
        assert_eq!(buf.remaining_capacity(), 95);

        assert_eq!(buf.consume_all(), &data);
    }

    #[test]
    fn put_small_declines_when_value_straddles_capacity_boundary() {
        // Four-byte blocks force a capacity boundary after every four bytes.
        let memory = FixedBlockMemory::new(NonZero::new(4).unwrap());
        let mut buf = memory.reserve(8);

        // The value does not fit within the first four-byte unfilled slice, so the fast path
        // declines and leaves the buffer untouched for the caller to fall back to the looped path.
        assert!(!buf.put_small(&[1, 2, 3, 4, 5]));

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.remaining_capacity(), 8);
    }
}
