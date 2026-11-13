// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg_attr(coverage_nightly, coverage(off))] // Test-only wire fixture construction.
fn longs(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = u64>) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Test-only wire fixture construction.
fn words(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = u32>) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Test-only wire fixture construction.
pub(super) fn image(activity: [u64; 6]) -> Vec<u8> {
    let mut bytes = b"SEISRUNT".to_vec();
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&9_u16.to_le_bytes());
    words(&mut bytes, [1, 1]);
    longs(&mut bytes, [1]);
    bytes.extend_from_slice(&[1, 1, 0, 0]);
    words(&mut bytes, [4]);
    longs(&mut bytes, [10, 0]);
    words(&mut bytes, [7, 1, 1, 0]);
    longs(&mut bytes, [7, 2, 21, 22, 23, 24, 25]);
    bytes.extend_from_slice(b"primary");
    longs(&mut bytes, [2]);
    bytes.extend_from_slice(&[1, 1, 0, 0]);
    words(&mut bytes, [3]);
    longs(&mut bytes, [9, 4]);
    longs(&mut bytes, [4, 3, 5]);
    words(&mut bytes, [0, 0]);
    longs(&mut bytes, [12, 2, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
    longs(&mut bytes, activity);
    longs(&mut bytes, [418, 0, 0x1234]);
    words(&mut bytes, [14, 10, 42, 7]);
    bytes.extend_from_slice(b"example::spawn");
    bytes.extend_from_slice(b"example.rs");
    bytes
}
