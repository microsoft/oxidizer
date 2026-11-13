// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::{BytesBuf, BytesView};

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

    buf.put_u32_le(0x1234_5678);
    buf.put_u32_be(0x9ABC_DEF0);
    buf.put_u64_ne(0x1122_3344_5566_7788);

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

#[test]
fn put_num_signed_and_float_round_trip() {
    let memory = TransparentMemory::new();
    let mut buf = memory.reserve(64);

    buf.put_i16_le(-12345);
    buf.put_i32_be(-2_000_000_000);
    buf.put_i64_le(-9_000_000_000_000);
    buf.put_i128_be(i128::MIN);
    buf.put_f32_le(std::f32::consts::PI);
    buf.put_f64_be(std::f64::consts::E);

    let mut view = buf.consume_all();

    assert_eq!(view.get_i16_le(), -12345);
    assert_eq!(view.get_i32_be(), -2_000_000_000);
    assert_eq!(view.get_i64_le(), -9_000_000_000_000);
    assert_eq!(view.get_i128_be(), i128::MIN);
    assert_eq!(view.get_f32_le().to_bits(), std::f32::consts::PI.to_bits());
    assert_eq!(view.get_f64_be().to_bits(), std::f64::consts::E.to_bits());
    assert!(view.is_empty());
}
