// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reproduces the 64-bit widening-multiply `FxHasher::write` variant from
//! rustc-hash 2.1.3 for default threaded corpus restoration on 32-bit targets.
//! `FxBuildHasher` uses `usize` internally and offers no fixed-width variant;
//! tagging the bare string-sequence wire format instead would invalidate
//! existing corpus serialization.

const SEED1: u64 = 0x243f_6a88_85a3_08d3;
const SEED2: u64 = 0x1319_8a2e_0370_7344;
const ZERO_GUARD: u64 = 0xa409_3822_299f_31d0;
const MULTIPLIER: u64 = 0xf135_7aea_2e62_a9c5;

#[expect(clippy::cast_possible_truncation, reason = "mix intentionally XORs the low and high 64-bit halves")]
fn mix(left: u64, right: u64) -> u64 {
    let product = u128::from(left) * u128::from(right);
    product as u64 ^ (product >> 64) as u64
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("caller passes exactly eight bytes"))
}

/// Hashes string bytes with the 64-bit widening-multiply Fx variant used on
/// most 64-bit targets. Native `sparc64` and `wasm64` use a different variant.
pub(crate) fn hash_bytes_64(bytes: &[u8]) -> u64 {
    let len = bytes.len();
    let (mut left, mut right) = (SEED1, SEED2);

    if len <= 16 {
        if len >= 8 {
            left ^= read_u64(&bytes[..8]);
            right ^= read_u64(&bytes[len - 8..]);
        } else if len >= 4 {
            left ^= u64::from(u32::from_le_bytes(
                bytes[..4].try_into().expect("length checked to be at least four"),
            ));
            right ^= u64::from(u32::from_le_bytes(
                bytes[len - 4..].try_into().expect("length checked to be at least four"),
            ));
        } else if len > 0 {
            left ^= u64::from(bytes[0]);
            right ^= u64::from(u16::from_le_bytes([bytes[len / 2], bytes[len - 1]]));
        }
    } else {
        for chunk in bytes[..len - 1].chunks_exact(16) {
            let next = mix(left ^ read_u64(&chunk[..8]), ZERO_GUARD ^ read_u64(&chunk[8..]));
            left = right;
            right = next;
        }
        let suffix = &bytes[len - 16..];
        left ^= read_u64(&suffix[..8]);
        right ^= read_u64(&suffix[8..]);
    }

    (mix(left, right) ^ len as u64).wrapping_mul(MULTIPLIER).rotate_left(26)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::hash_bytes_64;

    #[test]
    fn matches_published_64_bit_fx_fixtures() {
        assert_eq!(hash_bytes_64(b""), 17_606_491_139_363_777_937);
        assert_eq!(hash_bytes_64(b"uwu"), 7_168_164_714_682_931_527);
        assert_eq!(
            hash_bytes_64(b"These are some bytes for testing rustc_hash."),
            2_349_210_501_944_688_211
        );
    }

    #[test]
    fn matches_64_bit_fx_fixtures_at_length_boundaries() {
        for (len, expected) in [
            (1_u8, 5_448_590_020_104_574_886_u64),
            (3, 9_516_350_524_388_822_273),
            (4, 11_674_893_537_475_458_668),
            (7, 7_949_690_358_762_399_999),
            (8, 8_209_357_599_162_978_632),
            (15, 6_504_237_124_707_187_323),
            (16, 9_425_631_185_673_857_818),
            (17, 1_541_009_452_916_434_481),
            (31, 1_726_081_568_318_747_045),
            (32, 17_447_610_628_299_870_898),
        ] {
            let bytes: alloc::vec::Vec<u8> = (0..len).collect();
            assert_eq!(hash_bytes_64(&bytes), expected, "length {len}");
        }
    }

    #[cfg(all(target_pointer_width = "64", not(any(target_arch = "sparc64", target_arch = "wasm64"))))]
    #[test]
    fn matches_native_fx_for_varied_lengths() {
        use core::hash::{BuildHasher, Hasher};

        use rustc_hash::FxBuildHasher;

        for len in 0_u8..80 {
            let bytes: alloc::vec::Vec<u8> = (0..len).collect();
            let mut native = FxBuildHasher.build_hasher();
            native.write(&bytes);
            assert_eq!(hash_bytes_64(&bytes), native.finish(), "length {len}");
        }
    }
}
