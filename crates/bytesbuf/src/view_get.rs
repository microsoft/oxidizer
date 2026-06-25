// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! We separate out all the consumption methods for ease of maintenance.

use std::mem::MaybeUninit;
use std::ptr;

use num_traits::FromBytes;

use crate::BytesView;

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
        let byte = *self.first_slice().first().expect("view must cover at least one byte");
        self.advance(1);
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

    /// Consumes a number of type `T` in little-endian representation.
    ///
    /// The bytes of the `T` are dropped from the view, moving any remaining bytes to the front.
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
    /// // Little-endian: least significant byte first.
    /// let data: &[u8] = &[
    ///     0x34, 0x12, // u16: 0x1234
    ///     0x78, 0x56, 0x34, 0x12, // u32: 0x12345678
    /// ];
    /// let mut view = BytesView::copied_from_slice(data, &memory);
    ///
    /// assert_eq!(view.get_num_le::<u16>(), 0x1234);
    /// assert_eq!(view.get_num_le::<u32>(), 0x12345678);
    /// assert!(view.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the view does not cover enough bytes of data.
    #[inline]
    #[must_use]
    pub fn get_num_le<T: FromBytes>(&mut self) -> T
    where
        T::Bytes: Sized,
    {
        let size = size_of::<T::Bytes>();
        assert!(self.len() >= size);

        if let Some(bytes) = self.first_slice().get(..size) {
            // SAFETY: `get(..size)` guarantees the slice covers exactly `size_of::<T::Bytes>()`
            // bytes, so reading a `T::Bytes` from its start stays in bounds. We use an unaligned
            // read because the slice is backed by a byte buffer with no alignment guarantee, while
            // `T::Bytes` may demand a larger alignment; the read materializes a properly aligned
            // local that we then borrow.
            let bytes_array = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<T::Bytes>()) };

            let result = T::from_le_bytes(&bytes_array);
            self.advance(size);
            return result;
        }

        // If we got here, there were not enough bytes in the first slice, so we need
        // to go collect bytes into an intermediate buffer and deserialize it from there.
        // SAFETY: We guarantee the view covers enough bytes - we checked it above.
        unsafe { self.get_num_le_buffered() }
    }

    /// # Safety
    ///
    /// The caller is responsible for ensuring that the view covers enough bytes.
    /// We do not duplicate length checking as this method is only assumed to be
    /// called as a fallback when non-buffered reading proved impossible.
    #[cold] // Most reads will not straddle a slice boundary and not require buffering.
    unsafe fn get_num_le_buffered<T: FromBytes>(&mut self) -> T
    where
        T::Bytes: Sized,
    {
        let mut buffer: MaybeUninit<T::Bytes> = MaybeUninit::uninit();
        let mut buffer_cursor = buffer.as_mut_ptr().cast::<u8>();

        let mut bytes_remaining = size_of::<T::Bytes>();

        while bytes_remaining > 0 {
            let first_slice = self.first_slice();
            let bytes_to_copy = bytes_remaining.min(first_slice.len());

            // SAFETY: The caller has guaranteed that the view covers enough bytes.
            // We only copy up to bytes_remaining, which is at most size_of::<T::Bytes>(),
            // so we will not overflow the buffer.
            // Both sides are byte arrays/slices so there are no alignment concerns.
            unsafe {
                ptr::copy_nonoverlapping(first_slice.as_ptr(), buffer_cursor, bytes_to_copy);
            }

            // This cannot overflow because we it is guarded by min() above.
            bytes_remaining = bytes_remaining.wrapping_sub(bytes_to_copy);

            // SAFETY: We are advancing the cursor in-bounds of the buffer.
            buffer_cursor = unsafe { buffer_cursor.add(bytes_to_copy) };

            self.advance(bytes_to_copy);
        }

        // SAFETY: We have filled the buffer with data, initializing it fully.
        T::from_le_bytes(&unsafe { buffer.assume_init() })
    }

    /// Consumes a number of type `T` in big-endian representation.
    ///
    /// The bytes of the `T` are dropped from the view, moving any remaining bytes to the front.
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
    /// // Big-endian: most significant byte first.
    /// let data: &[u8] = &[
    ///     0x12, 0x34, 0x56, 0x78, // u32: 0x12345678
    ///     0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, // u64: 0x0123456789ABCDEF
    /// ];
    /// let mut view = BytesView::copied_from_slice(data, &memory);
    ///
    /// assert_eq!(view.get_num_be::<u32>(), 0x12345678);
    /// assert_eq!(view.get_num_be::<u64>(), 0x0123456789ABCDEF);
    /// assert!(view.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the view does not cover enough bytes of data.
    #[inline]
    #[must_use]
    pub fn get_num_be<T: FromBytes>(&mut self) -> T
    where
        T::Bytes: Sized,
    {
        let size = size_of::<T::Bytes>();
        assert!(self.len() >= size);

        if let Some(bytes) = self.first_slice().get(..size) {
            // SAFETY: `get(..size)` guarantees the slice covers exactly `size_of::<T::Bytes>()`
            // bytes, so reading a `T::Bytes` from its start stays in bounds. We use an unaligned
            // read because the slice is backed by a byte buffer with no alignment guarantee, while
            // `T::Bytes` may demand a larger alignment; the read materializes a properly aligned
            // local that we then borrow.
            let bytes_array = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<T::Bytes>()) };

            let result = T::from_be_bytes(&bytes_array);
            self.advance(size);
            return result;
        }

        // If we got here, there were not enough bytes in the first slice, so we need
        // to go collect bytes into an intermediate buffer and deserialize it from there.
        // SAFETY: We guarantee the view covers enough bytes - we checked it above.
        unsafe { self.get_num_be_buffered() }
    }

    /// # Safety
    ///
    /// The caller is responsible for ensuring that the view covers enough bytes.
    /// We do not duplicate length checking as this method is only assumed to be
    /// called as a fallback when non-buffered reading proved impossible.
    #[cold] // Most reads will not straddle a slice boundary and not require buffering.
    unsafe fn get_num_be_buffered<T: FromBytes>(&mut self) -> T
    where
        T::Bytes: Sized,
    {
        let mut buffer: MaybeUninit<T::Bytes> = MaybeUninit::uninit();
        let mut buffer_cursor = buffer.as_mut_ptr().cast::<u8>();

        let mut bytes_remaining = size_of::<T::Bytes>();

        while bytes_remaining > 0 {
            let first_slice = self.first_slice();
            let bytes_to_copy = bytes_remaining.min(first_slice.len());

            // SAFETY: The caller has guaranteed that the view covers enough bytes.
            // We only copy up to bytes_remaining, which is at most size_of::<T::Bytes>(),
            // so we will not overflow the buffer.
            // Both sides are byte arrays/slices so there are no alignment concerns.
            unsafe {
                ptr::copy_nonoverlapping(first_slice.as_ptr(), buffer_cursor, bytes_to_copy);
            }

            // This cannot overflow because we it is guarded by min() above.
            bytes_remaining = bytes_remaining.wrapping_sub(bytes_to_copy);

            // SAFETY: We are advancing the cursor in-bounds of the buffer.
            buffer_cursor = unsafe { buffer_cursor.add(bytes_to_copy) };

            self.advance(bytes_to_copy);
        }

        // SAFETY: We have filled the buffer with data, initializing it fully.
        T::from_be_bytes(&unsafe { buffer.assume_init() })
    }

    /// Consumes a number of type `T` in native-endian representation.
    ///
    /// The bytes of the `T` are dropped from the view, moving any remaining bytes to the front.
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
    /// // Native-endian: byte order matches the platform.
    /// let value1: u16 = 0x1234;
    /// let value2: u64 = 0x0123456789ABCDEF;
    ///
    /// let mut data = Vec::new();
    /// data.extend_from_slice(&value1.to_ne_bytes());
    /// data.extend_from_slice(&value2.to_ne_bytes());
    ///
    /// let mut view = BytesView::copied_from_slice(&data, &memory);
    ///
    /// assert_eq!(view.get_num_ne::<u16>(), 0x1234);
    /// assert_eq!(view.get_num_ne::<u64>(), 0x0123456789ABCDEF);
    /// assert!(view.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the view does not cover enough bytes of data.
    #[inline]
    #[must_use]
    pub fn get_num_ne<T: FromBytes>(&mut self) -> T
    where
        T::Bytes: Sized,
    {
        let size = size_of::<T::Bytes>();
        assert!(self.len() >= size);

        if let Some(bytes) = self.first_slice().get(..size) {
            // SAFETY: `get(..size)` guarantees the slice covers exactly `size_of::<T::Bytes>()`
            // bytes, so reading a `T::Bytes` from its start stays in bounds. We use an unaligned
            // read because the slice is backed by a byte buffer with no alignment guarantee, while
            // `T::Bytes` may demand a larger alignment; the read materializes a properly aligned
            // local that we then borrow.
            let bytes_array = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<T::Bytes>()) };

            let result = T::from_ne_bytes(&bytes_array);
            self.advance(size);
            return result;
        }

        // If we got here, there were not enough bytes in the first slice, so we need
        // to go collect bytes into an intermediate buffer and deserialize it from there.
        // SAFETY: We guarantee the view covers enough bytes - we checked it above.
        unsafe { self.get_num_ne_buffered() }
    }

    /// # Safety
    ///
    /// The caller is responsible for ensuring that the view covers enough bytes.
    /// We do not duplicate length checking as this method is only assumed to be
    /// called as a fallback when non-buffered reading proved impossible.
    #[cold] // Most reads will not straddle a slice boundary and not require buffering.
    unsafe fn get_num_ne_buffered<T: FromBytes>(&mut self) -> T
    where
        T::Bytes: Sized,
    {
        let mut buffer: MaybeUninit<T::Bytes> = MaybeUninit::uninit();
        let mut buffer_cursor = buffer.as_mut_ptr().cast::<u8>();

        let mut bytes_remaining = size_of::<T::Bytes>();

        while bytes_remaining > 0 {
            let first_slice = self.first_slice();
            let bytes_to_copy = bytes_remaining.min(first_slice.len());

            // SAFETY: The caller has guaranteed that the view covers enough bytes.
            // We only copy up to bytes_remaining, which is at most size_of::<T::Bytes>(),
            // so we will not overflow the buffer.
            // Both sides are byte arrays/slices so there are no alignment concerns.
            unsafe {
                ptr::copy_nonoverlapping(first_slice.as_ptr(), buffer_cursor, bytes_to_copy);
            }

            // This cannot overflow because we it is guarded by min() above.
            bytes_remaining = bytes_remaining.wrapping_sub(bytes_to_copy);

            // SAFETY: We are advancing the cursor in-bounds of the buffer.
            buffer_cursor = unsafe { buffer_cursor.add(bytes_to_copy) };

            self.advance(bytes_to_copy);
        }

        // SAFETY: We have filled the buffer with data, initializing it fully.
        T::from_ne_bytes(&unsafe { buffer.assume_init() })
    }
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
    fn get_num_le() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

        assert_eq!(view.get_num_le::<u16>(), 0x1234);
        assert_eq!(view.get_num_le::<u16>(), 0x5678);

        assert!(view.is_empty());
    }

    #[test]
    fn get_num_le_multi_span() {
        let memory = TransparentMemory::new();
        let data_part1 = [0x78_u8, 0x56];
        let data_part2 = [0x34_u8, 0x12];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view_combined.get_num_le::<u32>(), 0x1234_5678);

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_num_be() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x12, 0x34, 0x56, 0x78], &memory);

        assert_eq!(view.get_num_be::<u16>(), 0x1234);
        assert_eq!(view.get_num_be::<u16>(), 0x5678);

        assert!(view.is_empty());
    }

    #[test]
    fn get_num_be_multi_span() {
        let memory = TransparentMemory::new();
        let data_part1 = [0x12_u8, 0x34];
        let data_part2 = [0x56_u8, 0x78];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view_combined.get_num_be::<u32>(), 0x1234_5678);

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_num_ne() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

        if cfg!(target_endian = "big") {
            assert_eq!(view.get_num_ne::<u16>(), 0x3412);
            assert_eq!(view.get_num_ne::<u16>(), 0x7856);
        } else {
            assert_eq!(view.get_num_ne::<u16>(), 0x1234);
            assert_eq!(view.get_num_ne::<u16>(), 0x5678);
        }

        assert!(view.is_empty());
    }

    #[test]
    fn get_num_ne_multi_span() {
        let memory = TransparentMemory::new();
        let data_part1 = [0x78_u8, 0x56];
        let data_part2 = [0x34_u8, 0x12];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        if cfg!(target_endian = "big") {
            assert_eq!(view_combined.get_num_ne::<u32>(), 0x7856_3412);
        } else {
            assert_eq!(view_combined.get_num_ne::<u32>(), 0x1234_5678);
        }

        assert!(view_combined.is_empty());
    }

    /// Soundness coverage for `FromBytes` implementations whose `Bytes` associated type does not
    /// match the value type `T` in size or alignment.
    ///
    /// The standard integer and float types use `Bytes = [u8; size_of::<T>()]`, so for them
    /// `size_of::<T>() == size_of::<T::Bytes>()` and `align_of::<T::Bytes>() == 1`. The numeric
    /// reads must size, advance, and bound by `T::Bytes` (not `T`) and must tolerate any source
    /// alignment, which only these custom types can exercise.
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod from_bytes_soundness {
        use std::borrow::{Borrow, BorrowMut};

        use num_traits::FromBytes;

        use crate::BytesView;
        use crate::mem::testing::TransparentMemory;

        /// `Bytes` is wider than the value it produces: `size_of::<NarrowValue>() == 1` while the
        /// serialized form is four bytes. Sizing reads by `size_of::<T>()` would read past a
        /// one-byte first-span slice and leave the buffered path's `MaybeUninit<[u8; 4]>` only
        /// partially initialized before `assume_init`.
        #[derive(Debug, PartialEq, Eq, Clone, Copy)]
        struct NarrowValue(u8);

        impl FromBytes for NarrowValue {
            type Bytes = [u8; 4];

            fn from_le_bytes(bytes: &[u8; 4]) -> Self {
                Self(bytes[0])
            }

            fn from_be_bytes(bytes: &[u8; 4]) -> Self {
                Self(bytes[3])
            }
        }

        /// `Bytes` is narrower than the value it produces: `size_of::<WideValue>() == 8` while the
        /// serialized form is two bytes. Sizing reads by `size_of::<T>()` would over-advance the
        /// view and, in the buffered path, copy eight bytes into a two-byte `MaybeUninit<[u8; 2]>`
        /// buffer.
        #[derive(Debug, PartialEq, Eq, Clone, Copy)]
        struct WideValue(u64);

        impl FromBytes for WideValue {
            type Bytes = [u8; 2];

            fn from_le_bytes(bytes: &[u8; 2]) -> Self {
                Self(u16::from_le_bytes(*bytes).into())
            }

            fn from_be_bytes(bytes: &[u8; 2]) -> Self {
                Self(u16::from_be_bytes(*bytes).into())
            }
        }

        /// A `Bytes` type whose alignment exceeds one. Constructing a `&OverAligned` reference from
        /// an unaligned byte-slice pointer is undefined behavior, so the read must materialize the
        /// value through an alignment-agnostic copy. `NumBytes` is satisfied via its blanket impl
        /// over the manual `AsRef`/`AsMut`/`Borrow`/`BorrowMut` and the derived comparison traits.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(align(8))]
        struct OverAligned([u8; 8]);

        impl AsRef<[u8]> for OverAligned {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl AsMut<[u8]> for OverAligned {
            fn as_mut(&mut self) -> &mut [u8] {
                &mut self.0
            }
        }

        impl Borrow<[u8]> for OverAligned {
            fn borrow(&self) -> &[u8] {
                &self.0
            }
        }

        impl BorrowMut<[u8]> for OverAligned {
            fn borrow_mut(&mut self) -> &mut [u8] {
                &mut self.0
            }
        }

        #[derive(Debug, PartialEq, Eq, Clone, Copy)]
        struct OverAlignedValue(u64);

        impl FromBytes for OverAlignedValue {
            type Bytes = OverAligned;

            fn from_le_bytes(bytes: &OverAligned) -> Self {
                Self(u64::from_le_bytes(bytes.0))
            }

            fn from_be_bytes(bytes: &OverAligned) -> Self {
                Self(u64::from_be_bytes(bytes.0))
            }
        }

        #[test]
        fn narrow_bytes_le_single_span() {
            let memory = TransparentMemory::new();
            let mut view = BytesView::copied_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD], &memory);

            // Four bytes are consumed even though the value occupies a single byte.
            assert_eq!(view.get_num_le::<NarrowValue>(), NarrowValue(0xAA));
            assert!(view.is_empty());
        }

        #[test]
        fn narrow_bytes_be_single_span() {
            let memory = TransparentMemory::new();
            let mut view = BytesView::copied_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD], &memory);

            assert_eq!(view.get_num_be::<NarrowValue>(), NarrowValue(0xDD));
            assert!(view.is_empty());
        }

        #[test]
        fn narrow_bytes_le_multi_span() {
            let memory = TransparentMemory::new();
            let view_part1 = BytesView::copied_from_slice(&[0xAA, 0xBB], &memory);
            let view_part2 = BytesView::copied_from_slice(&[0xCC, 0xDD], &memory);
            let mut view = BytesView::from_views([view_part1, view_part2]);

            // The value straddles the span boundary, forcing the buffered path to fill all four
            // bytes of the `MaybeUninit<[u8; 4]>` before `assume_init`.
            assert_eq!(view.get_num_le::<NarrowValue>(), NarrowValue(0xAA));
            assert!(view.is_empty());
        }

        #[test]
        fn wide_bytes_le_single_span() {
            let memory = TransparentMemory::new();
            let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

            // Each read advances by exactly two bytes (the size of `Bytes`), not eight.
            assert_eq!(view.get_num_le::<WideValue>(), WideValue(0x1234));
            assert_eq!(view.get_num_le::<WideValue>(), WideValue(0x5678));
            assert!(view.is_empty());
        }

        #[test]
        fn wide_bytes_be_single_span() {
            let memory = TransparentMemory::new();
            let mut view = BytesView::copied_from_slice(&[0x12, 0x34, 0x56, 0x78], &memory);

            assert_eq!(view.get_num_be::<WideValue>(), WideValue(0x1234));
            assert_eq!(view.get_num_be::<WideValue>(), WideValue(0x5678));
            assert!(view.is_empty());
        }

        #[test]
        fn wide_bytes_le_multi_span() {
            let memory = TransparentMemory::new();
            let view_part1 = BytesView::copied_from_slice(&[0x34], &memory);
            let view_part2 = BytesView::copied_from_slice(&[0x12], &memory);
            let mut view = BytesView::from_views([view_part1, view_part2]);

            // The buffered path must size its copy by `Bytes` (two bytes), not by the eight-byte
            // value type, to avoid overflowing the `MaybeUninit<[u8; 2]>` buffer.
            assert_eq!(view.get_num_le::<WideValue>(), WideValue(0x1234));
            assert!(view.is_empty());
        }

        const OVER_ALIGNED_VALUE: u64 = 0x0123_4567_89AB_CDEF;

        #[test]
        fn over_aligned_le_single_span() {
            let memory = TransparentMemory::new();
            let mut view = BytesView::copied_from_slice(&OVER_ALIGNED_VALUE.to_le_bytes(), &memory);

            assert_eq!(view.get_num_le::<OverAlignedValue>(), OverAlignedValue(OVER_ALIGNED_VALUE));
            assert!(view.is_empty());
        }

        #[test]
        fn over_aligned_le_misaligned_single_span() {
            let memory = TransparentMemory::new();

            // A single leading byte that we drop, leaving the eight value bytes at an odd offset
            // from the allocation base. That address can never satisfy an eight-byte alignment, so
            // the fast path reads from a guaranteed-misaligned pointer.
            let mut data = vec![0xFF_u8];
            data.extend_from_slice(&OVER_ALIGNED_VALUE.to_le_bytes());
            let mut view = BytesView::copied_from_slice(&data, &memory);

            assert_eq!(view.get_byte(), 0xFF);
            assert_eq!(view.get_num_le::<OverAlignedValue>(), OverAlignedValue(OVER_ALIGNED_VALUE));
            assert!(view.is_empty());
        }

        #[test]
        fn over_aligned_le_multi_span() {
            let memory = TransparentMemory::new();
            let bytes = OVER_ALIGNED_VALUE.to_le_bytes();
            let view_part1 = BytesView::copied_from_slice(&bytes[..3], &memory);
            let view_part2 = BytesView::copied_from_slice(&bytes[3..], &memory);
            let mut view = BytesView::from_views([view_part1, view_part2]);

            // The buffered path assembles the value in a properly aligned `MaybeUninit<OverAligned>`.
            assert_eq!(view.get_num_le::<OverAlignedValue>(), OverAlignedValue(OVER_ALIGNED_VALUE));
            assert!(view.is_empty());
        }

        #[test]
        fn over_aligned_ne_misaligned_single_span() {
            let memory = TransparentMemory::new();

            let mut data = vec![0xFF_u8];
            data.extend_from_slice(&OVER_ALIGNED_VALUE.to_ne_bytes());
            let mut view = BytesView::copied_from_slice(&data, &memory);

            assert_eq!(view.get_byte(), 0xFF);
            assert_eq!(view.get_num_ne::<OverAlignedValue>(), OverAlignedValue(OVER_ALIGNED_VALUE));
            assert!(view.is_empty());
        }
    }
}
