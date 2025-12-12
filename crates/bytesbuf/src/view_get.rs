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
    /// The byte is removed from the front of the view, shrinking it.
    ///
    /// # Panics
    ///
    /// Panics if the view does not cover enough bytes of data.
    #[inline]
    #[must_use]
    pub fn get_byte(&mut self) -> u8 {
        assert!(!self.is_empty());

        // SAFETY: We asserted above that the view covers at least one byte,
        // so the first slice must also have at least one byte.
        let byte = unsafe { *self.first_slice().get_unchecked(0) };
        self.advance(1);
        byte
    }

    /// Transfers bytes into an initialized slice.
    ///
    /// The bytes are removed from the front of the view, shrinking it.
    ///
    /// # Panics
    ///
    /// Panics if the view and destination slice have different lengths.
    pub fn copy_to_slice(&mut self, mut dst: &mut [u8]) {
        assert!(self.len() == dst.len());

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
    /// The bytes are removed from the front of the view, shrinking it.
    ///
    /// # Panics
    ///
    /// Panics if the view and destination slice have different lengths.
    pub fn copy_to_uninit_slice(&mut self, mut dst: &mut [MaybeUninit<u8>]) {
        assert!(self.len() == dst.len());

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
    /// The bytes of the `T` are removed from the front of the view, shrinking it.
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
        let size = size_of::<T>();
        assert!(self.len() >= size);

        if let Some(bytes) = self.first_slice().get(..size) {
            let bytes_array_ptr = bytes.as_ptr().cast::<T::Bytes>();

            // SAFETY: The block is only entered if there are enough bytes in the first slice.
            // The target type is an array of bytes, so has no alignment requirements.
            let bytes_array_maybe = unsafe { bytes_array_ptr.as_ref() };
            // SAFETY: This is never a null pointer because it came from a reference.
            let bytes_array = unsafe { bytes_array_maybe.unwrap_unchecked() };

            let result = T::from_le_bytes(bytes_array);
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

        let mut bytes_remaining = size_of::<T>();

        while bytes_remaining > 0 {
            let first_slice = self.first_slice();
            let bytes_to_copy = bytes_remaining.min(first_slice.len());

            // SAFETY: The caller has guaranteed that the view covers enough bytes.
            // We only copy up to bytes_remaining, which is at most size_of::<T>(),
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
    /// The bytes of the `T` are removed from the front of the view, shrinking it.
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
        let size = size_of::<T>();
        assert!(self.len() >= size);

        if let Some(bytes) = self.first_slice().get(..size) {
            let bytes_array_ptr = bytes.as_ptr().cast::<T::Bytes>();

            // SAFETY: The block is only entered if there are enough bytes in the first slice.
            // The target type is an array of bytes, so has no alignment requirements.
            let bytes_array_maybe = unsafe { bytes_array_ptr.as_ref() };
            // SAFETY: This is never a null pointer because it came from a reference.
            let bytes_array = unsafe { bytes_array_maybe.unwrap_unchecked() };

            let result = T::from_be_bytes(bytes_array);
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

        let mut bytes_remaining = size_of::<T>();

        while bytes_remaining > 0 {
            let first_slice = self.first_slice();
            let bytes_to_copy = bytes_remaining.min(first_slice.len());

            // SAFETY: The caller has guaranteed that the view covers enough bytes.
            // We only copy up to bytes_remaining, which is at most size_of::<T>(),
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
    /// The bytes of the `T` are removed from the front of the view, shrinking it.
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
        let size = size_of::<T>();
        assert!(self.len() >= size);

        if let Some(bytes) = self.first_slice().get(..size) {
            let bytes_array_ptr = bytes.as_ptr().cast::<T::Bytes>();

            // SAFETY: The block is only entered if there are enough bytes in the first slice.
            // The target type is an array of bytes, so has no alignment requirements.
            let bytes_array_maybe = unsafe { bytes_array_ptr.as_ref() };
            // SAFETY: This is never a null pointer because it came from a reference.
            let bytes_array = unsafe { bytes_array_maybe.unwrap_unchecked() };

            let result = T::from_ne_bytes(bytes_array);
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

        let mut bytes_remaining = size_of::<T>();

        while bytes_remaining > 0 {
            let first_slice = self.first_slice();
            let bytes_to_copy = bytes_remaining.min(first_slice.len());

            // SAFETY: The caller has guaranteed that the view covers enough bytes.
            // We only copy up to bytes_remaining, which is at most size_of::<T>(),
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
    use crate::TransparentTestMemory;

    #[test]
    fn get_byte() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        assert_eq!(view.get_byte(), 1);
        assert_eq!(view.get_byte(), 2);
        assert_eq!(view.get_byte(), 3);
        assert_eq!(view.get_byte(), 4);
        assert!(view.is_empty());
    }

    #[test]
    fn copy_to_slice() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [0u8; 4];
        view.copy_to_slice(&mut dst);

        assert_eq!(dst, [1, 2, 3, 4]);
        assert!(view.is_empty());
    }

    #[test]
    #[should_panic]
    fn copy_to_smaller_slice_panics() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [0u8; 3];
        view.copy_to_slice(&mut dst);
    }

    #[test]
    #[should_panic]
    fn copy_to_bigger_slice_panics() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [0u8; 8];
        view.copy_to_slice(&mut dst);
    }

    #[test]
    fn copy_to_uninit_slice() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [MaybeUninit::<u8>::uninit(); 4];
        view.copy_to_uninit_slice(&mut dst);

        // SAFETY: It has now been initialized.
        let dst = unsafe { std::mem::transmute::<[MaybeUninit<u8>; 4], [u8; 4]>(dst) };

        assert_eq!(dst, [1, 2, 3, 4]);
        assert!(view.is_empty());
    }

    #[test]
    #[should_panic]
    fn copy_to_uninit_smaller_slice_panics() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [MaybeUninit::<u8>::uninit(); 3];
        view.copy_to_uninit_slice(&mut dst);
    }

    #[test]
    #[should_panic]

    fn copy_to_uninit_bigger_slice_panics() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

        let mut dst = [MaybeUninit::<u8>::uninit(); 8];
        view.copy_to_uninit_slice(&mut dst);
    }

    #[test]
    fn copy_to_slice_multi_span() {
        let memory = TransparentTestMemory::new();
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
        let memory = TransparentTestMemory::new();
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
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

        assert_eq!(view.get_num_le::<u16>(), 0x1234);
        assert_eq!(view.get_num_le::<u16>(), 0x5678);

        assert!(view.is_empty());
    }

    #[test]
    fn get_num_le_multi_span() {
        let memory = TransparentTestMemory::new();
        let data_part1 = [0x78_u8, 0x56];
        let data_part2 = [0x34_u8, 0x12];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view_combined.get_num_le::<u32>(), 0x12345678);

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_num_be() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(&[0x12, 0x34, 0x56, 0x78], &memory);

        assert_eq!(view.get_num_be::<u16>(), 0x1234);
        assert_eq!(view.get_num_be::<u16>(), 0x5678);

        assert!(view.is_empty());
    }

    #[test]
    fn get_num_be_multi_span() {
        let memory = TransparentTestMemory::new();
        let data_part1 = [0x12_u8, 0x34];
        let data_part2 = [0x56_u8, 0x78];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        assert_eq!(view_combined.get_num_be::<u32>(), 0x12345678);

        assert!(view_combined.is_empty());
    }

    #[test]
    fn get_num_ne() {
        let memory = TransparentTestMemory::new();
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
        let memory = TransparentTestMemory::new();
        let data_part1 = [0x78_u8, 0x56];
        let data_part2 = [0x34_u8, 0x12];
        let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
        let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
        let mut view_combined = BytesView::from_views([view_part1, view_part2]);

        if cfg!(target_endian = "big") {
            assert_eq!(view_combined.get_num_ne::<u32>(), 0x78563412);
        } else {
            assert_eq!(view_combined.get_num_ne::<u32>(), 0x12345678);
        }

        assert!(view_combined.is_empty());
    }
}
