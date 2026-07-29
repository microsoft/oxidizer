// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Minimum input length for vector scanning.
///
/// SSE2 and NEON process 16 bytes per iteration. Requiring two complete vectors
/// amortizes vector setup while keeping the short, common-query benchmark on
/// the lower-overhead scalar path. The long-ASCII query benchmark exercises the
/// vector path; changing this value requires comparing both benchmark groups.
pub(crate) const SIMD_THRESHOLD: usize = 32;

#[inline]
// SIMD selection is performance-only; scalar/SIMD results are checked
// exhaustively at vector boundaries below.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn find_byte<const WIDE: bool>(bytes: &[u8], needle: u8) -> Option<usize> {
    if WIDE && bytes.len() >= SIMD_THRESHOLD {
        #[cfg(target_arch = "x86_64")]
        {
            return find_byte_sse2(bytes, needle);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return find_byte_neon(bytes, needle);
        }
    }
    find_byte_scalar(bytes, needle)
}

#[inline]
#[cfg_attr(test, mutants::skip)]
pub(crate) fn contains_either<const WIDE: bool>(bytes: &[u8], first: u8, second: u8) -> bool {
    if WIDE && bytes.len() >= SIMD_THRESHOLD {
        #[cfg(target_arch = "x86_64")]
        {
            return contains_either_sse2(bytes, first, second);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return contains_either_neon(bytes, first, second);
        }
    }
    contains_either_scalar(bytes, first, second)
}

#[inline]
fn find_byte_scalar(bytes: &[u8], needle: u8) -> Option<usize> {
    bytes.iter().position(|&byte| byte == needle)
}

#[inline]
fn contains_either_scalar(bytes: &[u8], first: u8, second: u8) -> bool {
    bytes.iter().any(|&byte| byte == first || byte == second)
}

#[cfg(target_arch = "x86_64")]
#[inline]
#[cfg_attr(test, mutants::skip)]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "each vector block shares one in-bounds load precondition"
)]
fn find_byte_sse2(bytes: &[u8], needle: u8) -> Option<usize> {
    use core::arch::x86_64::{_mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8};

    let mut offset = 0;
    while offset + 16 <= bytes.len() {
        // SAFETY: SSE2 is part of the x86_64 baseline, and the loop guard
        // guarantees a complete in-bounds vector at `offset`.
        let mask = unsafe {
            let chunk = _mm_loadu_si128(bytes.as_ptr().add(offset).cast());
            _mm_movemask_epi8(_mm_cmpeq_epi8(chunk, _mm_set1_epi8(needle.cast_signed())))
        };
        if mask != 0 {
            return Some(offset + mask.trailing_zeros() as usize);
        }
        offset += 16;
    }
    find_byte_scalar(&bytes[offset..], needle).map(|index| offset + index)
}

#[cfg(target_arch = "x86_64")]
#[inline]
#[cfg_attr(test, mutants::skip)]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "each vector block shares one in-bounds load precondition"
)]
fn contains_either_sse2(bytes: &[u8], first: u8, second: u8) -> bool {
    use core::arch::x86_64::{_mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8};

    let mut offset = 0;
    while offset + 16 <= bytes.len() {
        // SAFETY: SSE2 is part of the x86_64 baseline, and the loop guard
        // guarantees a complete in-bounds vector at `offset`.
        let mask = unsafe {
            let chunk = _mm_loadu_si128(bytes.as_ptr().add(offset).cast());
            let first_matches = _mm_cmpeq_epi8(chunk, _mm_set1_epi8(first.cast_signed()));
            let second_matches = _mm_cmpeq_epi8(chunk, _mm_set1_epi8(second.cast_signed()));
            _mm_movemask_epi8(_mm_or_si128(first_matches, second_matches))
        };
        if mask != 0 {
            return true;
        }
        offset += 16;
    }
    contains_either_scalar(&bytes[offset..], first, second)
}

#[cfg(target_arch = "aarch64")]
#[inline]
#[cfg_attr(test, mutants::skip)]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "each vector block shares one in-bounds load precondition"
)]
fn find_byte_neon(bytes: &[u8], needle: u8) -> Option<usize> {
    use core::arch::aarch64::{vceqq_u8, vdupq_n_u8, vget_lane_u64, vld1q_u8, vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16};

    let mut offset = 0;
    while offset + 16 <= bytes.len() {
        // SAFETY: NEON is part of the aarch64 baseline, and the loop guard
        // guarantees a complete in-bounds vector at `offset`.
        let mask = unsafe {
            let chunk = vld1q_u8(bytes.as_ptr().add(offset));
            let matches = vceqq_u8(chunk, vdupq_n_u8(needle));
            let nibbles = vshrn_n_u16::<4>(vreinterpretq_u16_u8(matches));
            vget_lane_u64::<0>(vreinterpret_u64_u8(nibbles))
        };
        if mask != 0 {
            return Some(offset + (mask.trailing_zeros() >> 2) as usize);
        }
        offset += 16;
    }
    find_byte_scalar(&bytes[offset..], needle).map(|index| offset + index)
}

#[cfg(target_arch = "aarch64")]
#[inline]
#[cfg_attr(test, mutants::skip)]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "each vector block shares one in-bounds load precondition"
)]
fn contains_either_neon(bytes: &[u8], first: u8, second: u8) -> bool {
    use core::arch::aarch64::{vceqq_u8, vdupq_n_u8, vld1q_u8, vmaxvq_u8, vorrq_u8};

    let mut offset = 0;
    while offset + 16 <= bytes.len() {
        // SAFETY: NEON is part of the aarch64 baseline, and the loop guard
        // guarantees a complete in-bounds vector at `offset`.
        let matched = unsafe {
            let chunk = vld1q_u8(bytes.as_ptr().add(offset));
            let first_matches = vceqq_u8(chunk, vdupq_n_u8(first));
            let second_matches = vceqq_u8(chunk, vdupq_n_u8(second));
            vmaxvq_u8(vorrq_u8(first_matches, second_matches))
        };
        if matched != 0 {
            return true;
        }
        offset += 16;
    }
    contains_either_scalar(&bytes[offset..], first, second)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn scalar_contains_either_reports_no_match() {
        assert!(!contains_either_scalar(b"plain", b'%', b'+'));
    }

    #[test]
    fn scanning_matches_scalar_at_vector_boundaries() {
        for length in 0..80 {
            for position in 0..=length {
                let mut bytes = vec![b'a'; length];
                if position < length {
                    bytes[position] = b'&';
                }
                assert_eq!(find_byte::<true>(&bytes, b'&'), find_byte_scalar(&bytes, b'&'));
                assert_eq!(
                    contains_either::<true>(&bytes, b'&', b'%'),
                    contains_either_scalar(&bytes, b'&', b'%')
                );
            }
        }
    }
}
