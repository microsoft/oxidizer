// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytes::{BufMut, Bytes, BytesMut};
use bytesbuf::BytesView;

#[test]
fn test_bytes_to_view() {
    let bytes = Bytes::from_static(b"Hello, world!");

    let bytes_data_ptr = bytes.as_ptr();

    let view: BytesView = bytes.into();

    assert_eq!(view.len(), 13);
    assert_eq!(view, b"Hello, world!");

    // We expect this to be zero-copy - Bytes to BytesView always is.
    assert_eq!(view.first_slice().as_ptr(), bytes_data_ptr);
}

#[test]
fn zero_sized_bytes() {
    let bytes = Bytes::new();
    let view: BytesView = bytes.into();

    assert_eq!(view.len(), 0);
    assert!(view.is_empty());
}

#[test]
fn test_giant_bytes_to_view() {
    // This test requires at least 5 GB of memory to run. The publishing pipeline runs on a system
    // where this may not be available, so we skip this test in that environment.
    #[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
    if crate::support::system_memory() < 10_000_000_000 {
        eprintln!("Skipping giant allocation test due to insufficient memory.");
        return;
    }

    let mut bytes = BytesMut::new();

    // This is bigger than fits into a single memory block, so will be split in two blocks.
    bytes.put_bytes(0, 5_000_000_000);

    let bytes = bytes.freeze();

    let view: BytesView = bytes.into();
    assert_eq!(view.len(), 5_000_000_000);
    assert_eq!(view.first_slice().len(), u32::MAX as usize);
    assert_eq!(view.slices().count(), 2);
}
