// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytesbuf::BytesView;

#[test]
fn vec_into_view() {
    let vec = vec![1, 2, 3, 4, 5];
    let mut view: BytesView = vec.into();
    assert_eq!(view.len(), 5);

    assert_eq!(view.get_byte(), 1);
    assert_eq!(view.get_byte(), 2);
    assert_eq!(view.get_byte(), 3);
    assert_eq!(view.get_byte(), 4);
    assert_eq!(view.get_byte(), 5);

    assert!(view.is_empty());
}

#[test]
fn zero_sized_vec() {
    let vec = Vec::<u8>::new();
    let view: BytesView = vec.into();

    assert_eq!(view.len(), 0);
    assert!(view.is_empty());
}

#[test]
fn test_vec_to_view() {
    let vec = vec![b'H', b'e', b'l', b'l', b'o', b',', b' ', b'w', b'o', b'r', b'l', b'd', b'!'];

    let vec_data_ptr = vec.as_ptr();

    let view: BytesView = vec.into();

    assert_eq!(view.len(), 13);
    assert_eq!(view, b"Hello, world!");

    // We expect this to be zero-copy - Vec to BytesView always is.
    assert_eq!(view.first_slice().as_ptr(), vec_data_ptr);
}

#[test]
fn test_giant_vec_to_view() {
    // This test requires at least 5 GB of memory to run. The publishing pipeline runs on a system
    // where this may not be available, so we skip this test in that environment.
    #[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
    if crate::support::system_memory() < 10_000_000_000 {
        eprintln!("Skipping giant allocation test due to insufficient memory.");
        return;
    }

    let vec = vec![0u8; 5_000_000_000];

    let view: BytesView = vec.into();
    assert_eq!(view.len(), 5_000_000_000);
    assert_eq!(view.first_slice().len(), u32::MAX as usize);
    assert_eq!(view.slices().count(), 2);
}
