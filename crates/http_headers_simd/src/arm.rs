// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `AArch64` NEON implementations of shared byte scanners.

use core::arch::aarch64::*;

use crate::list;
use crate::list::{EmptyMembers, ListScan, TokenListScan};
use crate::range::{RangeMasks, WINDOW};

/// NEON byte vectors contain sixteen lanes.
const WIDTH: usize = 16;

/// The same width counted in mask lanes.
const WIDTH_LANES: u32 = 16;

/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn is_token(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        if !all_set(token_mask(value)) {
            return false;
        }
        offset += WIDTH;
    }
    crate::scalar::all_token_bytes(&bytes[offset..])
}

/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn is_token68(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset + 2 * WIDTH <= bytes.len() {
        let first_pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + 2 * WIDTH <= bytes.len()` bounds this unaligned load.
        let first = unsafe { vld1q_u8(first_pointer) };
        let second_pointer = bytes.as_ptr().wrapping_add(offset + WIDTH);
        // SAFETY: `offset + 2 * WIDTH <= bytes.len()` bounds this unaligned load.
        let second = unsafe { vld1q_u8(second_pointer) };
        if !all_set(token68_data_mask(first)) || !all_set(token68_data_mask(second)) {
            return crate::scalar::token68_tail(&bytes[offset..], offset != 0);
        }
        offset += 2 * WIDTH;
    }
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        if !all_set(token68_data_mask(value)) {
            return crate::scalar::token68_tail(&bytes[offset..], offset != 0);
        }
        offset += WIDTH;
    }
    crate::scalar::token68_tail(&bytes[offset..], offset != 0)
}

/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn is_field_value(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        let control = vandq_u8(vcltq_u8(value, vdupq_n_u8(0x20)), vmvnq_u8(vceqq_u8(value, vdupq_n_u8(9))));
        let invalid = vorrq_u8(control, vceqq_u8(value, vdupq_n_u8(0x7f)));
        if any_set(invalid) {
            return false;
        }
        offset += WIDTH;
    }
    crate::scalar::is_field_value(&bytes[offset..])
}

/// # Safety
///
/// The processor must support NEON, and `right.len()` must equal `left.len()`
/// because vector loads from both slices are bounded using `left.len()`.
#[target_feature(enable = "neon")]
pub(super) unsafe fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    let mut offset = 0;
    while offset + WIDTH <= left.len() {
        let left_pointer = left.as_ptr().wrapping_add(offset);
        // SAFETY: the dispatcher supplies equal-length slices and the loop bounds this load.
        let left_value = unsafe { vld1q_u8(left_pointer) };
        let right_pointer = right.as_ptr().wrapping_add(offset);
        // SAFETY: the dispatcher supplies equal-length slices and the loop bounds this load.
        let right_value = unsafe { vld1q_u8(right_pointer) };
        if !all_set(vceqq_u8(lower(left_value), lower(right_value))) {
            return false;
        }
        offset += WIDTH;
    }
    crate::scalar::eq_ignore_ascii_case(&left[offset..], &right[offset..])
}

/// Checks that every byte is a base64 alphabet character.
///
/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn all_base64_alphabet(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        if !all_set(base64_accept_mask(value)) {
            return false;
        }
        offset += WIDTH;
    }
    crate::base64::all_base64_alphabet(&bytes[offset..])
}

/// Scans a reference whose origin-relative prefix the dispatcher already checked.
///
/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn is_simple_uri_tail(bytes: &[u8]) -> bool {
    let mut offset = 0;
    let mut hashes = 0_u32;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        let accepted = simple_uri_accept_mask(value);
        if !all_set(accepted) {
            let fragments = vceqq_u8(value, vdupq_n_u8(b'#'));
            if !all_set(vorrq_u8(accepted, fragments)) {
                return false;
            }
            hashes += u32::from(hash_count(value));
            if hashes > 1 {
                return false;
            }
        }
        offset += WIDTH;
    }
    crate::uri::simple_uri_tail(&bytes[offset..], hashes)
}

/// Scans a comma-separated token list one block at a time.
///
/// # Safety
///
/// The processor must support NEON, and `bytes.len()` must be at least `WIDTH`
/// because the trailing run is folded from a full vector of input.
#[target_feature(enable = "neon")]
pub(super) unsafe fn scan_token_list(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    let mut state = ListScan::new();
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        let (tokens, ows, commas) = list_masks(value);
        if !state.push_block(tokens, ows, commas) {
            return TokenListScan::Rejected;
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return state.finish(&[], empty);
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset);
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { vld1q_u8(pointer) };
    let (tokens, ows, commas) = list_masks(value);
    let shift = u32::try_from(offset - final_offset).unwrap_or(WIDTH_LANES);
    if !state.push_partial_block(tokens >> shift, ows >> shift, commas >> shift, WIDTH_LANES - shift) {
        return TokenListScan::Rejected;
    }
    state.finish(&[], empty)
}

/// Scans a token list of one to two vectors as a single fold.
///
/// # Safety
///
/// The processor must support NEON, `bytes.len()` must be at least `WIDTH` and
/// at most twice it, and `TWO` must say whether the line runs past the first
/// vector, because both vectors are loaded whole.
#[target_feature(enable = "neon")]
pub(super) unsafe fn scan_short_token_list<const TWO: bool>(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    let last = bytes.len() - WIDTH;
    let head = bytes.as_ptr();
    let tail = bytes.as_ptr().wrapping_add(last);
    // SAFETY: the caller guarantees a whole vector at offset zero.
    let first = unsafe { vld1q_u8(head) };
    // SAFETY: the caller guarantees a whole vector at `last`.
    let final_value = unsafe { vld1q_u8(tail) };
    let (first_tokens, first_ows, first_commas) = list_masks(first);
    let mut state = ListScan::new();
    let folded = if TWO {
        let (last_tokens, last_ows, last_commas) = list_masks(final_value);
        let lanes = u32::try_from(last).unwrap_or(WIDTH_LANES);
        let shift = WIDTH_LANES - lanes;
        state.push_line(
            list::join_lanes(first_tokens, last_tokens, shift),
            list::join_lanes(first_ows, last_ows, shift),
            list::join_lanes(first_commas, last_commas, shift),
            WIDTH_LANES + lanes,
        )
    } else {
        state.push_block(first_tokens, first_ows, first_commas)
    };
    if !folded {
        return TokenListScan::Rejected;
    }
    state.finish(&[], empty)
}

/// Splits one block into the token, whitespace, and comma lanes a list needs.
///
/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
fn list_masks(value: uint8x16_t) -> (u32, u32, u32) {
    let tokens = movemask(token_mask(value));
    let ows = movemask(vorrq_u8(vceqq_u8(value, vdupq_n_u8(b' ')), vceqq_u8(value, vdupq_n_u8(b'\t'))));
    let commas = movemask(vceqq_u8(value, vdupq_n_u8(b',')));
    (tokens, ows, commas)
}

/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn find_interesting(bytes: &[u8]) -> Option<usize> {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        let matches = b",;\"\\ \t"
            .iter()
            .fold(vdupq_n_u8(0), |mask, byte| vorrq_u8(mask, vceqq_u8(value, vdupq_n_u8(*byte))));
        if any_set(matches) {
            let mut lanes = [0_u8; WIDTH];
            // SAFETY: `lanes` has exactly 16 writable bytes.
            unsafe { vst1q_u8(lanes.as_mut_ptr(), matches) };
            return lanes.iter().position(|byte| *byte != 0).map(|index| offset + index);
        }
        offset += WIDTH;
    }
    crate::scalar::find_interesting(&bytes[offset..]).map(|index| offset + index)
}

/// # Safety
///
/// The processor must support NEON.
#[target_feature(enable = "neon")]
pub(super) unsafe fn find_either(bytes: &[u8], first: u8, second: u8) -> Option<usize> {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset);
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { vld1q_u8(pointer) };
        let matches = vorrq_u8(vceqq_u8(value, vdupq_n_u8(first)), vceqq_u8(value, vdupq_n_u8(second)));
        if any_set(matches) {
            let mut lanes = [0_u8; WIDTH];
            // SAFETY: `lanes` has exactly 16 writable bytes.
            unsafe { vst1q_u8(lanes.as_mut_ptr(), matches) };
            return lanes.iter().position(|byte| *byte != 0).map(|index| offset + index);
        }
        offset += WIDTH;
    }
    crate::scalar::find_either(&bytes[offset..], first, second).map(|index| offset + index)
}

#[target_feature(enable = "neon")]
fn token_mask(value: uint8x16_t) -> uint8x16_t {
    let mut mask = vorrq_u8(in_range(value, b'0', b'9'), in_range(value, b'A', b'Z'));
    mask = vorrq_u8(mask, in_range(value, b'a', b'z'));
    b"!#$%&'*+-.^_`|~"
        .iter()
        .fold(mask, |mask, byte| vorrq_u8(mask, vceqq_u8(value, vdupq_n_u8(*byte))))
}

#[target_feature(enable = "neon")]
fn token68_data_mask(value: uint8x16_t) -> uint8x16_t {
    let digit = in_range(value, b'0', b'9');
    let folded = vorrq_u8(value, vdupq_n_u8(0x20));
    let alpha = in_range(folded, b'a', b'z');
    let plus_to_slash = in_range(value, b'+', b'/');
    let symbols = vbicq_u8(plus_to_slash, vceqq_u8(value, vdupq_n_u8(b',')));
    let underscore = vceqq_u8(value, vdupq_n_u8(b'_'));
    let tilde = vceqq_u8(value, vdupq_n_u8(b'~'));
    vorrq_u8(vorrq_u8(digit, alpha), vorrq_u8(symbols, vorrq_u8(underscore, tilde)))
}

/// Marks the lanes inside the base64 alphabet.
///
/// `/` and the digits are contiguous, so the alphabet needs only three ranges and one compare.
#[target_feature(enable = "neon")]
fn base64_accept_mask(value: uint8x16_t) -> uint8x16_t {
    let digit_or_slash = in_range(value, b'/', b'9');
    let upper = in_range(value, b'A', b'Z');
    let lower_case = in_range(value, b'a', b'z');
    let plus = vceqq_u8(value, vdupq_n_u8(b'+'));
    vorrq_u8(vorrq_u8(digit_or_slash, upper), vorrq_u8(lower_case, plus))
}

/// Marks the lanes inside the separator-free URI subset.
///
/// The subset is three ranges plus a handful of isolated bytes, so it needs no
/// table: `&`..`?` covers the sub-delims, digits, `:`, and `;` once `<` and `>`
/// are removed, case folding merges both letter ranges, and `!`/`#` share a
/// single compare after the low bit is forced on.
#[target_feature(enable = "neon")]
fn simple_uri_accept_mask(value: uint8x16_t) -> uint8x16_t {
    let raised = vorrq_u8(value, vdupq_n_u8(2));
    let punctuation = vbicq_u8(in_range(value, b'&', b'?'), vceqq_u8(raised, vdupq_n_u8(0x3e)));
    let bang = vceqq_u8(value, vdupq_n_u8(b'!'));
    let dollar = vceqq_u8(value, vdupq_n_u8(b'$'));
    let letter = in_range(vorrq_u8(value, vdupq_n_u8(0x20)), b'a', b'z');
    let at = vceqq_u8(value, vdupq_n_u8(b'@'));
    let underscore = vceqq_u8(value, vdupq_n_u8(b'_'));
    let tilde = vceqq_u8(value, vdupq_n_u8(b'~'));
    vorrq_u8(
        vorrq_u8(punctuation, vorrq_u8(bang, dollar)),
        vorrq_u8(letter, vorrq_u8(at, vorrq_u8(underscore, tilde))),
    )
}

/// Packs one bit per lane, lane zero in bit zero, from an all-ones lane mask.
///
/// NEON has no move-mask instruction, so each lane keeps a single distinct bit
/// and the halves are summed across, which costs two horizontal adds.
#[target_feature(enable = "neon")]
fn movemask(mask: uint8x16_t) -> u32 {
    const BITS: [u8; WIDTH] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];

    // SAFETY: `BITS` is exactly 16 bytes, so this unaligned load stays in bounds.
    let bits = unsafe { vld1q_u8(BITS.as_ptr()) };
    let selected = vandq_u8(mask, bits);
    let low = u32::from(vaddv_u8(vget_low_u8(selected)));
    let high = u32::from(vaddv_u8(vget_high_u8(selected)));
    low | (high << 8)
}

/// Classifies one window for the byte-range grammar.
///
/// Digits fall out of one biased unsigned comparison, and the three delimiters
/// and the leading-zero marker are plain equality comparisons.
///
/// # Safety
///
/// The processor must support NEON.
#[inline]
#[target_feature(enable = "neon")]
pub(super) unsafe fn range_masks(window: &[u8; WINDOW]) -> RangeMasks {
    // SAFETY: the window is exactly 16 bytes, so this unaligned load stays in bounds.
    let value = unsafe { vld1q_u8(window.as_ptr()) };
    let biased = vsubq_u8(value, vdupq_n_u8(b'0'));
    let digits = vcleq_u8(biased, vdupq_n_u8(9));
    let zeros = vceqq_u8(value, vdupq_n_u8(b'0'));
    let dashes = vceqq_u8(value, vdupq_n_u8(b'-'));
    let commas = vceqq_u8(value, vdupq_n_u8(b','));
    let spaces = vorrq_u8(vceqq_u8(value, vdupq_n_u8(b' ')), vceqq_u8(value, vdupq_n_u8(b'\t')));
    RangeMasks {
        digits: movemask(digits),
        dashes: movemask(dashes),
        commas: movemask(commas),
        spaces: movemask(spaces),
        zeros: movemask(zeros),
    }
}

#[target_feature(enable = "neon")]
fn hash_count(value: uint8x16_t) -> u8 {
    vaddvq_u8(vshrq_n_u8::<7>(vceqq_u8(value, vdupq_n_u8(b'#'))))
}

#[target_feature(enable = "neon")]
fn lower(value: uint8x16_t) -> uint8x16_t {
    vorrq_u8(value, vandq_u8(in_range(value, b'A', b'Z'), vdupq_n_u8(0x20)))
}

#[target_feature(enable = "neon")]
fn in_range(value: uint8x16_t, start: u8, end: u8) -> uint8x16_t {
    vandq_u8(vcgeq_u8(value, vdupq_n_u8(start)), vcleq_u8(value, vdupq_n_u8(end)))
}

#[target_feature(enable = "neon")]
fn any_set(value: uint8x16_t) -> bool {
    vmaxvq_u8(value) != 0
}

#[target_feature(enable = "neon")]
fn all_set(value: uint8x16_t) -> bool {
    vminvq_u8(value) == u8::MAX
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Puts every byte value in every lane of a token-list block.
    #[test]
    fn token_list_lanes_match_scalar_for_every_byte() {
        if !std::arch::is_aarch64_feature_detected!("neon") {
            return;
        }
        for lane in 0..2 * WIDTH {
            for byte in u8::MIN..=u8::MAX {
                let mut block = [b'a'; 2 * WIDTH];
                block[lane] = byte;
                for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                    // SAFETY: runtime detection above establishes NEON support.
                    let list = unsafe { scan_token_list(&block, empty) };
                    assert_eq!(list, crate::list::scan_token_list(&block, empty), "byte {byte:#04x} lane {lane}");
                }
            }
        }
    }

    #[test]
    fn neon_matches_scalar() {
        let available = std::arch::is_aarch64_feature_detected!("neon");
        if !available {
            return;
        }
        bolero::check!()
            .with_iterations(4_096)
            .with_test_time(Duration::from_millis(400))
            .with_type::<(Vec<u8>, Vec<u8>)>()
            .for_each(|(left, right)| {
                // SAFETY: runtime detection above establishes NEON support.
                let token = unsafe { is_token(left) };
                assert_eq!(token, crate::scalar::is_token(left));
                // SAFETY: runtime detection above establishes NEON support.
                let token68 = unsafe { is_token68(left) };
                assert_eq!(token68, crate::scalar::is_token68(left));
                // SAFETY: runtime detection above establishes NEON support.
                let field_value = unsafe { is_field_value(left) };
                assert_eq!(field_value, crate::scalar::is_field_value(left));
                if left.len() == right.len() {
                    // SAFETY: runtime detection establishes NEON support; lengths are equal.
                    let equal = unsafe { eq_ignore_ascii_case(left, right) };
                    assert_eq!(equal, crate::scalar::eq_ignore_ascii_case(left, right));
                }
                // SAFETY: runtime detection above establishes NEON support.
                let interesting = unsafe { find_interesting(left) };
                assert_eq!(interesting, crate::scalar::find_interesting(left));
                // SAFETY: runtime detection above establishes NEON support.
                let base64 = unsafe { all_base64_alphabet(left) };
                assert_eq!(base64, crate::base64::all_base64_alphabet(left));
                // SAFETY: runtime detection above establishes NEON support.
                let simple = unsafe { is_simple_uri_tail(left) };
                assert_eq!(simple, crate::uri::simple_uri_tail(left, 0));
                if left.len() >= WIDTH {
                    for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                        let expected = crate::list::scan_token_list(left, empty);
                        // SAFETY: runtime detection establishes NEON support; the length is checked.
                        let list = unsafe { scan_token_list(left, empty) };
                        assert_eq!(list, expected);
                        if left.len() == WIDTH {
                            // SAFETY: detection above, and the line is exactly one vector.
                            let short = unsafe { scan_short_token_list::<false>(left, empty) };
                            assert_eq!(short, expected);
                        } else if left.len() <= 2 * WIDTH {
                            // SAFETY: detection above, and the line spans two vectors.
                            let short = unsafe { scan_short_token_list::<true>(left, empty) };
                            assert_eq!(short, expected);
                        }
                    }
                }
            });
    }

    #[test]
    fn range_lanes_match_scalar_for_every_byte() {
        if !std::arch::is_aarch64_feature_detected!("neon") {
            return;
        }
        for lane in 0..WINDOW {
            for byte in u8::MIN..=u8::MAX {
                let mut window = [b'0'; WINDOW];
                window[lane] = byte;
                // SAFETY: runtime detection above establishes NEON support.
                let masks = unsafe { range_masks(&window) };
                assert_eq!(masks, RangeMasks::scalar(&window), "byte {byte:#04x} lane {lane}");
            }
        }
    }

    #[test]
    fn base64_and_uri_lanes_match_neon() {
        if !std::arch::is_aarch64_feature_detected!("neon") {
            return;
        }
        for lane in 0..2 * WIDTH {
            for byte in u8::MIN..=u8::MAX {
                let mut base64_block = [b'A'; 2 * WIDTH];
                base64_block[lane] = byte;
                assert_eq!(
                    // SAFETY: runtime detection above establishes NEON support.
                    unsafe { all_base64_alphabet(&base64_block) },
                    crate::base64::all_base64_alphabet(&base64_block),
                    "base64 byte {byte:#04x} lane {lane}"
                );

                let mut uri_block = [b'a'; 2 * WIDTH];
                uri_block[lane] = byte;
                assert_eq!(
                    // SAFETY: runtime detection above establishes NEON support.
                    unsafe { is_simple_uri_tail(&uri_block) },
                    crate::uri::simple_uri_tail(&uri_block, 0),
                    "URI byte {byte:#04x} lane {lane}"
                );
            }
        }
    }
}
