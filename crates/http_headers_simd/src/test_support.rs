// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

const BYTE_BOUNDARIES: &[u8] = b"\0\t\n\r\x1f !\"#$%&'()*+,-./09:;<=>?@AZ[\\]^_`az{|}~\x7f\x80\xff";

pub(crate) fn lane_bytes(length: usize, lane: usize) -> impl Iterator<Item = u8> {
    probe_bytes(length, lane, cfg!(miri))
}

fn probe_bytes(length: usize, lane: usize, reduced: bool) -> impl Iterator<Item = u8> {
    // Separate byte classification from lane placement under Miri. Native runs
    // retain the Cartesian product; Miri retains every byte across two blocks
    // and every lane with the byte-class and scanner-state boundaries.
    let exhaustive = !reduced || matches!(length, 16 | 32 | 33) && matches!(lane, 0 | 15 | 16 | 31 | 32);
    (u8::MIN..=u8::MAX)
        .take(if exhaustive { 256 } else { 0 })
        .chain(
            BYTE_BOUNDARIES
                .iter()
                .copied()
                .take(if exhaustive { 0 } else { BYTE_BOUNDARIES.len() }),
        )
}

pub(crate) fn case_partners(left: u8) -> impl Iterator<Item = u8> {
    let reduced = cfg!(miri);
    (u8::MIN..=u8::MAX).take(if reduced { 0 } else { 256 }).chain(
        [
            left,
            left.to_ascii_lowercase(),
            left.to_ascii_uppercase(),
            left.wrapping_add(1),
            left ^ 0x80,
        ]
        .into_iter()
        .take(if reduced { 5 } else { 0 }),
    )
}

#[test]
fn native_probes_keep_every_byte_in_every_lane() {
    for length in [16, 17, 31, 32, 33, 48, 49] {
        for lane in 0..length {
            assert!(probe_bytes(length, lane, false).eq(u8::MIN..=u8::MAX));
        }
    }
}

#[test]
fn reduced_probes_keep_byte_classes_and_vector_boundaries() {
    for length in [16, 17, 31, 32, 33, 48, 49] {
        for lane in 0..length {
            let probes: std::vec::Vec<_> = probe_bytes(length, lane, true).collect();
            assert!(BYTE_BOUNDARIES.iter().all(|byte| probes.contains(byte)));
        }
    }
    for length in [32, 33] {
        for lane in [0, 15, 16, 31] {
            assert!(probe_bytes(length, lane, true).eq(u8::MIN..=u8::MAX));
        }
        for lane in [0, 15] {
            assert!(probe_bytes(16, lane, true).eq(u8::MIN..=u8::MAX));
        }
    }
    assert!(probe_bytes(33, 32, true).eq(u8::MIN..=u8::MAX));
    assert!(probe_bytes(33, 1, true).count() < 256);
}
