// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::mem::MaybeUninit;

use bytesbuf::BytesView;

use crate::test_memory::TestMemory;

#[test]
fn get_byte() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    assert_eq!(view.get_byte(), 1);
    assert_eq!(view.get_byte(), 2);
    assert_eq!(view.get_byte(), 3);
    assert_eq!(view.get_byte(), 4);
    assert!(view.is_empty());
}

#[test]
fn copy_to_slice() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    let mut dst = [0u8; 4];
    view.copy_to_slice(&mut dst);

    assert_eq!(dst, [1, 2, 3, 4]);
    assert!(view.is_empty());
}

#[test]
fn copy_to_smaller_slice_copies_partially() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    let mut dst = [0u8; 3];
    view.copy_to_slice(&mut dst);

    assert_eq!(dst, [1, 2, 3]);
    assert_eq!(view.len(), 1);
}

#[test]
#[should_panic]
fn copy_to_bigger_slice_panics() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    let mut dst = [0u8; 8];
    view.copy_to_slice(&mut dst);
}

#[test]
fn copy_to_uninit_slice() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    let mut dst = MaybeUninit::<[u8; 4]>::uninit();
    view.copy_to_uninit_slice(dst.as_mut());

    // SAFETY: It has now been initialized.
    let dst = unsafe { dst.assume_init() };

    assert_eq!(dst, [1, 2, 3, 4]);
    assert!(view.is_empty());
}

#[test]
fn copy_to_uninit_smaller_slice_copies_partially() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    let mut dst = MaybeUninit::<[u8; 3]>::uninit();
    view.copy_to_uninit_slice(dst.as_mut());

    // SAFETY: It has now been initialized.
    let dst = unsafe { dst.assume_init() };

    assert_eq!(dst, [1, 2, 3]);
    assert_eq!(view.len(), 1);
}

#[test]
#[should_panic]
fn copy_to_uninit_bigger_slice_panics() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[1, 2, 3, 4], &memory);

    let mut dst = [MaybeUninit::<u8>::uninit(); 8];
    view.copy_to_uninit_slice(&mut dst);
}

#[test]
fn copy_to_slice_multi_span() {
    let memory = TestMemory::new();
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
    let memory = TestMemory::new();
    let data_part1 = [10_u8, 20];
    let data_part2 = [30_u8, 40, 50];
    let view_part1 = BytesView::copied_from_slice(&data_part1, &memory);
    let view_part2 = BytesView::copied_from_slice(&data_part2, &memory);
    let mut view_combined = BytesView::from_views([view_part1, view_part2]);

    let mut dst = MaybeUninit::<[u8; 5]>::uninit();
    view_combined.copy_to_uninit_slice(dst.as_mut());
    assert!(view_combined.is_empty());

    // SAFETY: It has now been initialized.
    let dst = unsafe { dst.assume_init() };

    assert_eq!(dst, [10_u8, 20, 30, 40, 50]);
}

#[test]
fn get_u16_le() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[0x34, 0x12, 0x78, 0x56], &memory);

    assert_eq!(view.get_u16_le(), 0x1234);
    assert_eq!(view.get_u16_le(), 0x5678);

    assert!(view.is_empty());
}

#[test]
fn get_u32_le_multi_span() {
    let memory = TestMemory::new();
    let view_part1 = BytesView::copied_from_slice(&[0x78_u8, 0x56], &memory);
    let view_part2 = BytesView::copied_from_slice(&[0x34_u8, 0x12], &memory);
    let mut view_combined = BytesView::from_views([view_part1, view_part2]);

    assert_eq!(view_combined.get_u32_le(), 0x1234_5678);

    assert!(view_combined.is_empty());
}

#[test]
fn get_u16_be() {
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[0x12, 0x34, 0x56, 0x78], &memory);

    assert_eq!(view.get_u16_be(), 0x1234);
    assert_eq!(view.get_u16_be(), 0x5678);

    assert!(view.is_empty());
}

#[test]
fn get_u32_be_multi_span() {
    let memory = TestMemory::new();
    let view_part1 = BytesView::copied_from_slice(&[0x12_u8, 0x34], &memory);
    let view_part2 = BytesView::copied_from_slice(&[0x56_u8, 0x78], &memory);
    let mut view_combined = BytesView::from_views([view_part1, view_part2]);

    assert_eq!(view_combined.get_u32_be(), 0x1234_5678);

    assert!(view_combined.is_empty());
}

#[test]
fn get_u16_ne() {
    let memory = TestMemory::new();
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
    let memory = TestMemory::new();
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
    let memory = TestMemory::new();

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
    let memory = TestMemory::new();
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
    let memory = TestMemory::new();

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
    let memory = TestMemory::new();
    let mut view = BytesView::copied_from_slice(&[0x00, 0x01, 0x02], &memory);

    // Only three bytes are available, so reading a four-byte value must panic.
    let _ = view.get_u32_le();
}
