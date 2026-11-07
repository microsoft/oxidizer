// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

fn longs(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = u64>) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

fn words(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = u32>) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

pub(super) fn image(schema: u16, activity: [u64; 6]) -> Vec<u8> {
    let mut bytes = b"SEISRUNT".to_vec();
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&schema.to_le_bytes());
    words(&mut bytes, [1, u32::from(schema >= 3)]);
    longs(&mut bytes, [1]);
    bytes.extend_from_slice(&[1, 1, 0, 0]);
    words(&mut bytes, [4]);
    longs(&mut bytes, [10, 0]);
    words(&mut bytes, [7, 1, 1, 0]);
    longs(&mut bytes, [7, 2, 21, 22, 23, 24, 25, 0, 0, 0]);
    bytes.extend_from_slice(b"primary");
    longs(&mut bytes, [2]);
    bytes.extend_from_slice(&[1, 1, 0, 0]);
    words(&mut bytes, [3]);
    longs(&mut bytes, [9, 4]);
    longs(&mut bytes, [4, 3, 5]);
    words(&mut bytes, [2, 0]);
    if schema >= 3 {
        longs(&mut bytes, [11, 2, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
    }
    if schema >= 4 {
        longs(&mut bytes, activity[..if schema >= 5 { 6 } else { 5 }].iter().copied());
    }
    if schema >= 6 {
        longs(&mut bytes, [418]);
    }
    longs(&mut bytes, [0x1234, 0x5678]);
    if schema >= 3 {
        longs(&mut bytes, [0x1234]);
        words(&mut bytes, [14, 10, 42, 7]);
        bytes.extend_from_slice(b"example::spawn");
        bytes.extend_from_slice(b"example.rs");
    }
    bytes
}
