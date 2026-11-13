// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytes::{Buf, Bytes};
use bytesbuf::BytesView;

use crate::test_memory::TestMemory;

#[test]
fn view_to_bytes() {
    let mut builder = TestMemory::new().reserve(100);

    builder.put_slice(1234_u64.to_ne_bytes());
    builder.put_slice(5678_u64.to_ne_bytes());

    let span1 = builder.consume(8);
    let span2 = builder.consume(8);

    let view_single_span = BytesView::from_views(vec![span1.clone()]);
    let view_multi_span = BytesView::from_views(vec![span1, span2]);

    let mut bytes = view_single_span.to_bytes();
    assert_eq!(8, bytes.len());
    assert_eq!(1234, bytes.get_u64_ne());

    let mut bytes = view_single_span.to_bytes();
    assert_eq!(8, bytes.len());
    assert_eq!(1234, bytes.get_u64_ne());

    let mut bytes = view_multi_span.to_bytes();
    assert_eq!(16, bytes.len());
    assert_eq!(1234, bytes.get_u64_ne());
    assert_eq!(5678, bytes.get_u64_ne());
}

#[test]
fn empty_view_to_bytes() {
    let view = BytesView::default();
    let bytes = view.to_bytes();
    assert_eq!(0, bytes.len());
}

#[test]
fn into_bytes() {
    let memory = TestMemory::new();
    let view = BytesView::copied_from_slice(b"Hello, world!", &memory);

    let bytes: Bytes = view.into();

    assert_eq!(bytes.as_ref(), b"Hello, world!");
}

#[test]
fn test_view_to_bytes() {
    let memory = TestMemory::new();

    let view = BytesView::copied_from_slice(b"Hello, world!", &memory);

    let view_chunk_ptr = view.first_slice().as_ptr();

    let bytes = view.to_bytes();

    assert_eq!(bytes.as_ref(), b"Hello, world!");

    // We expect this to be zero-copy since we used the passthrough allocator.
    assert_eq!(bytes.as_ptr(), view_chunk_ptr);
}

#[test]
fn test_multi_block_view_to_bytes() {
    let memory = TestMemory::new();

    let hello = BytesView::copied_from_slice(b"Hello, ", &memory);
    let world = BytesView::copied_from_slice(b"world!", &memory);
    let view = BytesView::from_views([hello, world]);

    let bytes = view.to_bytes();
    assert_eq!(bytes.as_ref(), b"Hello, world!");
}
