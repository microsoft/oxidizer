// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! We separate out the mutation functions for ease of maintenance.

use std::borrow::Borrow;
use std::ptr;

use num_traits::ToBytes;

use crate::{BytesBuf, BytesView};

impl BytesBuf {
    /// Appends a slice of bytes to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    ///
    /// buf.put_slice(*b"Hello, ");
    /// buf.put_slice(*b"world!");
    ///
    /// assert_eq!(buf.consume_all(), b"Hello, world!");
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub fn put_slice(&mut self, src: impl Borrow<[u8]>) {
        let mut src = src.borrow();

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
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub fn put_bytes(&mut self, view: BytesView) {
        self.append(view);
    }

    /// Appends a `u8` to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(8);
    ///
    /// buf.put_byte(0xCA);
    /// buf.put_byte(0xFE);
    ///
    /// assert_eq!(buf.consume_all(), &[0xCA, 0xFE]);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub fn put_byte(&mut self, value: u8) {
        self.put_num_ne(value);
    }

    /// Appends multiple repetitions of a `u8` to the buffer.
    ///
    /// # Example
    ///
    /// ```
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

    /// Appends a number of type `T` in little-endian representation to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(16);
    ///
    /// buf.put_num_le(0x1234_u16);
    /// buf.put_num_le(0xDEAD_BEEF_u32);
    ///
    /// let data = buf.consume_all();
    /// // Little-endian: least significant byte first.
    /// assert_eq!(data.first_slice(), &[0x34, 0x12, 0xEF, 0xBE, 0xAD, 0xDE]);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    #[expect(clippy::needless_pass_by_value, reason = "tiny numeric types, fine to always pass by value")]
    pub fn put_num_le<T: ToBytes>(&mut self, value: T) {
        let bytes = value.to_le_bytes();
        self.put_slice(bytes);
    }

    /// Appends a number of type `T` in big-endian representation to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(16);
    ///
    /// buf.put_num_be(0xCAFE_u16);
    /// buf.put_num_be(0xBABE_u16);
    ///
    /// let data = buf.consume_all();
    /// // Big-endian: most significant byte first.
    /// assert_eq!(data, &[0xCA, 0xFE, 0xBA, 0xBE]);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    #[expect(clippy::needless_pass_by_value, reason = "tiny numeric types, fine to always pass by value")]
    pub fn put_num_be<T: ToBytes>(&mut self, value: T) {
        let bytes = value.to_be_bytes();
        self.put_slice(bytes);
    }

    /// Appends a number of type `T` in native-endian representation to the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(16);
    ///
    /// buf.put_num_ne(0xABCD_u16);
    /// buf.put_num_ne(0x1234_5678_9ABC_DEF0_u64);
    ///
    /// let mut view = buf.consume_all();
    /// // Reading back in native-endian gives the original values.
    /// assert_eq!(view.get_num_ne::<u16>(), 0xABCD);
    /// assert_eq!(view.get_num_ne::<u64>(), 0x1234_5678_9ABC_DEF0);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    #[expect(clippy::needless_pass_by_value, reason = "tiny numeric types, fine to always pass by value")]
    pub fn put_num_ne<T: ToBytes>(&mut self, value: T) {
        let bytes = value.to_ne_bytes();
        self.put_slice(bytes);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn put_slice() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        let data = [1u8, 2, 3, 4, 5];
        buf.put_slice(data);

        assert_eq!(buf.len(), 5);
        assert_eq!(buf.remaining_capacity(), 95);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes, &data);
    }

    #[test]
    fn put_slice_empty() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        buf.put_slice([]);

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.remaining_capacity(), 100);
    }

    #[test]
    fn put_view_single_span() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        let data = [10_u8, 20, 30, 40, 50];
        let view = BytesView::copied_from_slice(&data, &memory);

        buf.put_bytes(view);

        assert_eq!(buf.len(), 5);
        // Appending a view brings along its existing memory capacity, consuming none.
        assert_eq!(buf.remaining_capacity(), 100);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes, &data);
    }

    #[test]
    fn put_view_multi_span() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        let data_part1 = [10_u8, 20];
        let data_part2 = [30_u8, 40, 50];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let view_combined = BytesView::from_views([view_part1, view_part2]);

        buf.put_bytes(view_combined);

        assert_eq!(buf.len(), 5);
        // Appending a view brings along its existing memory capacity, consuming none.
        assert_eq!(buf.remaining_capacity(), 100);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes, &[10_u8, 20, 30, 40, 50]);
    }

    #[test]
    fn put_view_empty() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        let view = BytesView::new();

        buf.put_bytes(view);

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.remaining_capacity(), 100);
    }

    #[test]
    fn put_view_peeked_from_self() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(100);

        let data = [1u8, 2, 3, 4, 5];
        buf.put_slice(data);

        let peeked = buf.peek();
        assert_eq!(peeked.len(), 5);

        buf.put_bytes(peeked);

        assert_eq!(buf.len(), 10);
        // The peeked view brings along its existing memory capacity, consuming none.
        assert_eq!(buf.remaining_capacity(), 95);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 10);
        assert_eq!(bytes, &[1u8, 2, 3, 4, 5, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn put_byte() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(10);

        buf.put_byte(0xAB);
        buf.put_byte(0xCD);

        assert_eq!(buf.len(), 2);
        assert_eq!(buf.remaining_capacity(), 8);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 2);
        assert_eq!(bytes, &[0xAB, 0xCD]);
    }

    #[test]
    fn put_bytes() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(10);

        buf.put_byte_repeated(0xFF, 5);

        assert_eq!(buf.len(), 5);
        assert_eq!(buf.remaining_capacity(), 5);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes, &[0xFF; 5]);
    }

    #[test]
    fn put_bytes_into_multi_span() {
        let memory = TransparentMemory::new();
        let mut buf = BytesBuf::new();

        // Result: 5
        buf.reserve(5, &memory);
        // Result: 5 + 5
        buf.reserve(10, &memory);

        buf.put_byte_repeated(0xAA, 10);

        assert_eq!(buf.len(), 10);
        assert_eq!(buf.remaining_capacity(), 0);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 10);
        assert_eq!(bytes, &[0xAA; 10]);
    }

    #[test]
    fn put_num() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(16);

        buf.put_num_le(0x1234_5678_u32);
        buf.put_num_be(0x9ABC_DEF0_u32);
        buf.put_num_ne(0x1122_3344_5566_7788_u64);

        assert_eq!(buf.len(), 16);
        assert_eq!(buf.remaining_capacity(), 0);

        let bytes = buf.consume_all();

        assert_eq!(bytes.len(), 16);

        if cfg!(target_endian = "big") {
            assert_eq!(
                bytes,
                &[
                    0x78, 0x56, 0x34, 0x12, // Little-endian 0x12345678
                    0x9A, 0xBC, 0xDE, 0xF0, // Big-endian 0x9ABCDEF0
                    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88 // Native-endian 0x1122334455667788
                ]
            );
        } else {
            assert_eq!(
                bytes,
                &[
                    0x78, 0x56, 0x34, 0x12, // Little-endian 0x12345678
                    0x9A, 0xBC, 0xDE, 0xF0, // Big-endian 0x9ABCDEF0
                    0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11 // Native-endian 0x1122334455667788
                ]
            );
        }
    }
}
