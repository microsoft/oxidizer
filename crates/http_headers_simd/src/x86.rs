// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! x86 SSE2, SSSE3, and SSE4.2 implementations of shared byte scanners.

#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

use crate::list;
use crate::list::{EmptyMembers, ListScan, TokenListScan};
use crate::range::{RangeMasks, WINDOW};

/// All x86 backends in this module process one 128-bit vector at a time.
const WIDTH: usize = 16;

/// The same width counted in mask lanes.
const WIDTH_LANES: u32 = 16;

/// # Safety
///
/// The processor must support SSE2.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn is_token(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        if token_mask(value) != 0xffff {
            return false;
        }
        offset += WIDTH;
    }
    if offset == 0 || bytes.len() - offset < 8 {
        return crate::scalar::all_token_bytes(&bytes[offset..]);
    }
    let pointer = bytes.as_ptr().wrapping_add(bytes.len() - WIDTH).cast();
    // SAFETY: at least one full vector was consumed and this load ends at the slice end.
    token_mask(unsafe { _mm_loadu_si128(pointer) }) == 0xffff
}

/// # Safety
///
/// The processor must support SSE2.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn is_token68_sse2(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset + 2 * WIDTH <= bytes.len() {
        let first_pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + 2 * WIDTH <= bytes.len()` bounds this unaligned load.
        let first = unsafe { _mm_loadu_si128(first_pointer) };
        let second_pointer = bytes.as_ptr().wrapping_add(offset + WIDTH).cast();
        // SAFETY: `offset + 2 * WIDTH <= bytes.len()` bounds this unaligned load.
        let second = unsafe { _mm_loadu_si128(second_pointer) };
        if token68_data_mask(first) != 0xffff || token68_data_mask(second) != 0xffff {
            return crate::scalar::token68_tail(&bytes[offset..], offset != 0);
        }
        offset += 2 * WIDTH;
    }
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        if token68_data_mask(value) != 0xffff {
            return crate::scalar::token68_tail(&bytes[offset..], offset != 0);
        }
        offset += WIDTH;
    }
    crate::scalar::token68_tail(&bytes[offset..], offset != 0)
}

/// # Safety
///
/// The processor must support SSE4.2.
#[target_feature(enable = "sse4.2")]
pub(super) unsafe fn is_token68_sse42(bytes: &[u8]) -> bool {
    if bytes.len() < WIDTH {
        return crate::scalar::is_token68(bytes);
    }
    let mut offset = 0;
    while offset + 2 * WIDTH <= bytes.len() {
        let first_pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + 2 * WIDTH <= bytes.len()` bounds this unaligned load.
        let first = unsafe { _mm_loadu_si128(first_pointer) };
        let second_pointer = bytes.as_ptr().wrapping_add(offset + WIDTH).cast();
        // SAFETY: `offset + 2 * WIDTH <= bytes.len()` bounds this unaligned load.
        let second = unsafe { _mm_loadu_si128(second_pointer) };
        if token68_valid_mask_sse42(first) != 0xffff || token68_valid_mask_sse42(second) != 0xffff {
            return crate::scalar::token68_tail(&bytes[offset..], offset != 0);
        }
        offset += 2 * WIDTH;
    }
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        if token68_valid_mask_sse42(value) != 0xffff {
            return crate::scalar::token68_tail(&bytes[offset..], offset != 0);
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return offset != 0;
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
    // SAFETY: `bytes.len() >= WIDTH` and this load ends exactly at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let valid = token68_valid_mask_sse42(value);
    if valid == 0xffff {
        return true;
    }
    let equals = _mm_movemask_epi8(_mm_cmpeq_epi8(value, _mm_set1_epi8(b'='.cast_signed())));
    if equals == 0 {
        return false;
    }
    let first = equals.trailing_zeros() as usize;
    let expected = (0xffff_i32 << first) & 0xffff;
    valid | equals == 0xffff && equals == expected && final_offset + first != 0
}

/// # Safety
///
/// The processor must support SSE2.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn is_field_value(bytes: &[u8]) -> bool {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let ascii = _mm_cmpgt_epi8(value, _mm_set1_epi8(-1));
        let control = _mm_and_si128(ascii, _mm_cmpgt_epi8(_mm_set1_epi8(0x20), value));
        let invalid_control = _mm_andnot_si128(_mm_cmpeq_epi8(value, _mm_set1_epi8(9)), control);
        let del = _mm_cmpeq_epi8(value, _mm_set1_epi8(0x7f));
        if _mm_movemask_epi8(_mm_or_si128(invalid_control, del)) != 0 {
            return false;
        }
        offset += WIDTH;
    }
    if offset == 0 || bytes.len() - offset == 1 {
        return crate::scalar::is_field_value(&bytes[offset..]);
    }
    let pointer = bytes.as_ptr().wrapping_add(bytes.len() - WIDTH).cast();
    // SAFETY: at least one full vector was consumed and this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let ascii = _mm_cmpgt_epi8(value, _mm_set1_epi8(-1));
    let control = _mm_and_si128(ascii, _mm_cmpgt_epi8(_mm_set1_epi8(0x20), value));
    let invalid_control = _mm_andnot_si128(_mm_cmpeq_epi8(value, _mm_set1_epi8(9)), control);
    let del = _mm_cmpeq_epi8(value, _mm_set1_epi8(0x7f));
    _mm_movemask_epi8(_mm_or_si128(invalid_control, del)) == 0
}

/// # Safety
///
/// The processor must support SSE2, and `right.len()` must equal `left.len()`
/// because vector loads from both slices are bounded using `left.len()`.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    let mut offset = 0;
    while offset + WIDTH <= left.len() {
        let left_pointer = left.as_ptr().wrapping_add(offset).cast();
        // SAFETY: the dispatcher supplies equal-length slices and the loop bounds this load.
        let left_value = unsafe { _mm_loadu_si128(left_pointer) };
        let right_pointer = right.as_ptr().wrapping_add(offset).cast();
        // SAFETY: the dispatcher supplies equal-length slices and the loop bounds this load.
        let right_value = unsafe { _mm_loadu_si128(right_pointer) };
        if _mm_movemask_epi8(_mm_cmpeq_epi8(lower(left_value), lower(right_value))) != 0xffff {
            return false;
        }
        offset += WIDTH;
    }
    crate::scalar::eq_ignore_ascii_case(&left[offset..], &right[offset..])
}

/// # Safety
///
/// The processor must support SSE2.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn find_interesting(bytes: &[u8]) -> Option<usize> {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let matches = b",;\"\\ \t".iter().fold(_mm_setzero_si128(), |mask, byte| {
            _mm_or_si128(mask, _mm_cmpeq_epi8(value, _mm_set1_epi8(byte.cast_signed())))
        });
        let found = _mm_movemask_epi8(matches);
        if found != 0 {
            return Some(offset + found.trailing_zeros() as usize);
        }
        offset += WIDTH;
    }
    crate::scalar::find_interesting(&bytes[offset..]).map(|index| offset + index)
}

/// # Safety
///
/// The processor must support SSE2.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn find_either(bytes: &[u8], first: u8, second: u8) -> Option<usize> {
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let matches = _mm_or_si128(
            _mm_cmpeq_epi8(value, _mm_set1_epi8(first.cast_signed())),
            _mm_cmpeq_epi8(value, _mm_set1_epi8(second.cast_signed())),
        );
        let found = _mm_movemask_epi8(matches);
        if found != 0 {
            return Some(offset + found.trailing_zeros() as usize);
        }
        offset += WIDTH;
    }
    crate::scalar::find_either(&bytes[offset..], first, second).map(|index| offset + index)
}

/// Scans a reference with one nibble-table lookup per lane.
///
/// # Safety
///
/// The processor must support SSSE3, and `bytes.len()` must be at least
/// `WIDTH` because every block is loaded relative to a full vector of input.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn is_simple_uri_tail_ssse3(bytes: &[u8]) -> bool {
    if bytes.len() <= 2 * WIDTH {
        let head_pointer = bytes.as_ptr().cast();
        // SAFETY: the caller guarantees at least `WIDTH` bytes.
        let head = unsafe { _mm_loadu_si128(head_pointer) };
        let final_offset = bytes.len() - WIDTH;
        let tail_pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
        // SAFETY: this load ends exactly at the end of the slice.
        let tail = unsafe { _mm_loadu_si128(tail_pointer) };
        let head_rejects = simple_uri_reject_mask_ssse3(head);
        let tail_rejects = simple_uri_reject_mask_ssse3(tail);
        // Lanes shared by both blocks are rejected in `head` too, so the overlap only has to
        // be masked off once a rejected lane forces the fragment count to be exact.
        if head_rejects | tail_rejects == 0 {
            return true;
        }
        let repeated = (1_u32 << (2 * WIDTH - bytes.len())) - 1;
        let (Some(head_hashes), Some(tail_hashes)) = (fragment_count(head, head_rejects), fragment_count(tail, tail_rejects & !repeated))
        else {
            return false;
        };
        return head_hashes + tail_hashes <= 1;
    }
    let mut offset = 0;
    let mut hashes = 0_u32;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let rejects = simple_uri_reject_mask_ssse3(value);
        if rejects != 0 {
            let Some(count) = fragment_count(value, rejects) else {
                return false;
            };
            hashes += count;
            if hashes > 1 {
                return false;
            }
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return true;
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let repeated = (1_u32 << (offset - final_offset)) - 1;
    let rejects = simple_uri_reject_mask_ssse3(value) & !repeated;
    if rejects == 0 {
        return true;
    }
    let Some(count) = fragment_count(value, rejects) else {
        return false;
    };
    hashes + count <= 1
}

/// Checks that every byte is a base64 alphabet character.
///
/// # Safety
///
/// The processor must support SSE4.2, and `bytes.len()` must be at least `WIDTH`
/// because every block is loaded relative to a full vector of input.
#[target_feature(enable = "sse4.2")]
pub(super) unsafe fn all_base64_alphabet_sse42(bytes: &[u8]) -> bool {
    if bytes.len() <= 2 * WIDTH {
        let head_pointer = bytes.as_ptr().cast();
        // SAFETY: the caller guarantees at least `WIDTH` bytes.
        let head = unsafe { _mm_loadu_si128(head_pointer) };
        let tail_pointer = bytes.as_ptr().wrapping_add(bytes.len() - WIDTH).cast();
        // SAFETY: this load ends exactly at the end of the slice.
        let tail = unsafe { _mm_loadu_si128(tail_pointer) };
        return base64_reject_mask_sse42(head) | base64_reject_mask_sse42(tail) == 0;
    }
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        if base64_reject_mask_sse42(value) != 0 {
            return false;
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return true;
    }
    let pointer = bytes.as_ptr().wrapping_add(bytes.len() - WIDTH).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    base64_reject_mask_sse42(value) == 0
}

/// Checks that every byte is a base64 alphabet character using range compares.
///
/// # Safety
///
/// The processor must support SSE2, and `bytes.len()` must be at least `WIDTH`
/// because every block is loaded relative to a full vector of input.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn all_base64_alphabet_sse2(bytes: &[u8]) -> bool {
    if bytes.len() <= 2 * WIDTH {
        let head_pointer = bytes.as_ptr().cast();
        // SAFETY: the caller guarantees at least `WIDTH` bytes.
        let head = unsafe { _mm_loadu_si128(head_pointer) };
        let tail_pointer = bytes.as_ptr().wrapping_add(bytes.len() - WIDTH).cast();
        // SAFETY: this load ends exactly at the end of the slice.
        let tail = unsafe { _mm_loadu_si128(tail_pointer) };
        return base64_reject_mask(head) | base64_reject_mask(tail) == 0;
    }
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        if base64_reject_mask(value) != 0 {
            return false;
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return true;
    }
    let pointer = bytes.as_ptr().wrapping_add(bytes.len() - WIDTH).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    base64_reject_mask(value) == 0
}

/// Scans a reference with one packed range comparison per block.
///
/// # Safety
///
/// The processor must support SSE4.2, and `bytes.len()` must be at least `WIDTH`
/// because every block is loaded relative to a full vector of input.
#[target_feature(enable = "sse4.2")]
pub(super) unsafe fn is_simple_uri_tail_sse42(bytes: &[u8]) -> bool {
    if bytes.len() <= 2 * WIDTH {
        let head_pointer = bytes.as_ptr().cast();
        // SAFETY: the caller guarantees at least `WIDTH` bytes.
        let head = unsafe { _mm_loadu_si128(head_pointer) };
        let final_offset = bytes.len() - WIDTH;
        let tail_pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
        // SAFETY: this load ends exactly at the end of the slice.
        let tail = unsafe { _mm_loadu_si128(tail_pointer) };
        let head_rejects = simple_uri_reject_mask_sse42(head);
        let tail_rejects = simple_uri_reject_mask_sse42(tail);
        // Lanes shared by both blocks are rejected in `head` too, so the overlap only has to
        // be masked off once a rejected lane forces the fragment count to be exact.
        if head_rejects | tail_rejects == 0 {
            return true;
        }
        let repeated = (1_u32 << (2 * WIDTH - bytes.len())) - 1;
        let (Some(head_hashes), Some(tail_hashes)) = (fragment_count(head, head_rejects), fragment_count(tail, tail_rejects & !repeated))
        else {
            return false;
        };
        return head_hashes + tail_hashes <= 1;
    }
    let mut offset = 0;
    let mut hashes = 0_u32;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let rejects = simple_uri_reject_mask_sse42(value);
        if rejects != 0 {
            let Some(count) = fragment_count(value, rejects) else {
                return false;
            };
            hashes += count;
            if hashes > 1 {
                return false;
            }
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return true;
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let repeated = (1_u32 << (offset - final_offset)) - 1;
    let rejects = simple_uri_reject_mask_sse42(value) & !repeated;
    if rejects == 0 {
        return true;
    }
    let Some(count) = fragment_count(value, rejects) else {
        return false;
    };
    hashes + count <= 1
}

/// Scans a reference with range compares instead of a nibble table.
///
/// # Safety
///
/// The processor must support SSE2, and `bytes.len()` must be at least `WIDTH`
/// because every block is loaded relative to a full vector of input.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn is_simple_uri_tail_sse2(bytes: &[u8]) -> bool {
    if bytes.len() <= 2 * WIDTH {
        let head_pointer = bytes.as_ptr().cast();
        // SAFETY: the caller guarantees at least `WIDTH` bytes.
        let head = unsafe { _mm_loadu_si128(head_pointer) };
        let final_offset = bytes.len() - WIDTH;
        let tail_pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
        // SAFETY: this load ends exactly at the end of the slice.
        let tail = unsafe { _mm_loadu_si128(tail_pointer) };
        let head_rejects = simple_uri_reject_mask(head);
        let tail_rejects = simple_uri_reject_mask(tail);
        // Lanes shared by both blocks are rejected in `head` too, so the overlap only has to
        // be masked off once a rejected lane forces the fragment count to be exact.
        if head_rejects | tail_rejects == 0 {
            return true;
        }
        let repeated = (1_u32 << (2 * WIDTH - bytes.len())) - 1;
        let (Some(head_hashes), Some(tail_hashes)) = (fragment_count(head, head_rejects), fragment_count(tail, tail_rejects & !repeated))
        else {
            return false;
        };
        return head_hashes + tail_hashes <= 1;
    }
    let mut offset = 0;
    let mut hashes = 0_u32;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let rejects = simple_uri_reject_mask(value);
        if rejects != 0 {
            let Some(count) = fragment_count(value, rejects) else {
                return false;
            };
            hashes += count;
            if hashes > 1 {
                return false;
            }
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return true;
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let repeated = (1_u32 << (offset - final_offset)) - 1;
    let rejects = simple_uri_reject_mask(value) & !repeated;
    if rejects == 0 {
        return true;
    }
    let Some(count) = fragment_count(value, rejects) else {
        return false;
    };
    hashes + count <= 1
}

/// Scans a comma-separated token list with one packed range comparison per block.
///
/// # Safety
///
/// The processor must support SSE4.2, and `bytes.len()` must be at least
/// `WIDTH` because the trailing run is folded from a full vector of input.
#[target_feature(enable = "sse4.2")]
pub(super) unsafe fn scan_token_list_sse42(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    let mut state = ListScan::new();
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        if !state.push_block(token_mask_sse42(value), ows_mask(value), comma_mask(value)) {
            return TokenListScan::Rejected;
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return state.finish(&[], empty);
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let shift = shift_of(offset, final_offset);
    if !state.push_partial_block(
        token_mask_sse42(value) >> shift,
        ows_mask(value) >> shift,
        comma_mask(value) >> shift,
        WIDTH_LANES - shift,
    ) {
        return TokenListScan::Rejected;
    }
    state.finish(&[], empty)
}

/// Scans a token list of one to two vectors as a single fold.
///
/// # Safety
///
/// The processor must support SSE4.2, `bytes.len()` must be at least `WIDTH`
/// and at most twice it, and `two` must say whether the line runs past the
/// first vector, because both vectors are loaded whole.
#[target_feature(enable = "sse4.2")]
#[inline]
pub(super) unsafe fn scan_short_token_list_sse42(bytes: &[u8], empty: EmptyMembers, two: bool) -> TokenListScan {
    let last = bytes.len() - WIDTH;
    // SAFETY: the caller guarantees at least `WIDTH` bytes from either end.
    let (first, final_value) = unsafe { short_windows(bytes, last) };
    let mut state = ListScan::new();
    let folded = if two {
        let shift = WIDTH_LANES - lanes_of(last);
        state.push_line(
            list::join_lanes(token_mask_sse42(first), token_mask_sse42(final_value), shift),
            list::join_lanes(ows_mask(first), ows_mask(final_value), shift),
            list::join_lanes(comma_mask(first), comma_mask(final_value), shift),
            WIDTH_LANES + lanes_of(last),
        )
    } else {
        state.push_block(token_mask_sse42(first), ows_mask(first), comma_mask(first))
    };
    if !folded {
        return TokenListScan::Rejected;
    }
    state.finish(&[], empty)
}

/// Scans a token list of one to two vectors as a single fold.
///
/// # Safety
///
/// The processor must support SSE2, `bytes.len()` must be at least `WIDTH`
/// and at most twice it, and `two` must say whether the line runs past the
/// first vector, because both vectors are loaded whole.
#[target_feature(enable = "sse2")]
#[inline]
pub(super) unsafe fn scan_short_token_list_sse2(bytes: &[u8], empty: EmptyMembers, two: bool) -> TokenListScan {
    let last = bytes.len() - WIDTH;
    // SAFETY: the caller guarantees at least `WIDTH` bytes from either end.
    let (first, final_value) = unsafe { short_windows(bytes, last) };
    let mut state = ListScan::new();
    let folded = if two {
        let shift = WIDTH_LANES - lanes_of(last);
        state.push_line(
            list::join_lanes(token_mask(first).cast_unsigned(), token_mask(final_value).cast_unsigned(), shift),
            list::join_lanes(ows_mask(first), ows_mask(final_value), shift),
            list::join_lanes(comma_mask(first), comma_mask(final_value), shift),
            WIDTH_LANES + lanes_of(last),
        )
    } else {
        state.push_block(token_mask(first).cast_unsigned(), ows_mask(first), comma_mask(first))
    };
    if !folded {
        return TokenListScan::Rejected;
    }
    state.finish(&[], empty)
}

/// Loads the vector a line opens with and the vector it ends with.
///
/// # Safety
///
/// The processor must support SSE2, and `bytes` must hold at least `WIDTH`
/// bytes from offset zero and at least `WIDTH` bytes from `last`.
#[target_feature(enable = "sse2")]
unsafe fn short_windows(bytes: &[u8], last: usize) -> (__m128i, __m128i) {
    let head = bytes.as_ptr().cast();
    let tail = bytes.as_ptr().wrapping_add(last).cast();
    // SAFETY: the caller guarantees a whole vector at offset zero.
    let first = unsafe { _mm_loadu_si128(head) };
    // SAFETY: the caller guarantees a whole vector at `last`.
    let final_value = unsafe { _mm_loadu_si128(tail) };
    (first, final_value)
}

/// The lane count an offset within one vector stands for.
fn lanes_of(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(WIDTH_LANES)
}

/// The number of lanes an overlapping final block repeats.
///
/// Both offsets index the same line and the reload starts no later than the
/// bytes still owed, so the difference is smaller than one vector and the
/// masks keep every lane the scan has not folded yet.
fn shift_of(offset: usize, final_offset: usize) -> u32 {
    u32::try_from(offset - final_offset).unwrap_or(WIDTH_LANES)
}

/// Scans a comma-separated token list with range compares instead of a nibble table.
///
/// # Safety
///
/// The processor must support SSE2, and `bytes.len()` must be at least `WIDTH`
/// because the trailing run is folded from a full vector of input.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn scan_token_list_sse2(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    let mut state = ListScan::new();
    let mut offset = 0;
    while offset + WIDTH <= bytes.len() {
        let pointer = bytes.as_ptr().wrapping_add(offset).cast();
        // SAFETY: `offset + WIDTH <= bytes.len()` permits this unaligned 16-byte load.
        let value = unsafe { _mm_loadu_si128(pointer) };
        let tokens = token_mask(value).cast_unsigned();
        if !state.push_block(tokens, ows_mask(value), comma_mask(value)) {
            return TokenListScan::Rejected;
        }
        offset += WIDTH;
    }
    if offset == bytes.len() {
        return state.finish(&[], empty);
    }
    let final_offset = bytes.len() - WIDTH;
    let pointer = bytes.as_ptr().wrapping_add(final_offset).cast();
    // SAFETY: the caller guarantees `bytes.len() >= WIDTH`, so this load ends at the slice end.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let shift = shift_of(offset, final_offset);
    if !state.push_partial_block(
        token_mask(value).cast_unsigned() >> shift,
        ows_mask(value) >> shift,
        comma_mask(value) >> shift,
        WIDTH_LANES - shift,
    ) {
        return TokenListScan::Rejected;
    }
    state.finish(&[], empty)
}

/// Classifies one window for the byte-range grammar.
///
/// Digits fall out of one biased unsigned clamp, and the three delimiters and
/// the leading-zero marker are plain equality comparisons, so the whole
/// classification is five compares and five movemasks.
///
/// # Safety
///
/// The processor must support SSE2.
#[inline]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn range_masks(window: &[u8; WINDOW]) -> RangeMasks {
    let pointer = window.as_ptr().cast();
    // SAFETY: the window is exactly 16 bytes, so this unaligned load stays in bounds.
    let value = unsafe { _mm_loadu_si128(pointer) };
    let biased = _mm_sub_epi8(value, _mm_set1_epi8(b'0'.cast_signed()));
    let clamped = _mm_min_epu8(biased, _mm_set1_epi8(9));
    let digits = _mm_cmpeq_epi8(clamped, biased);
    let zeros = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'0'.cast_signed()));
    let dashes = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'-'.cast_signed()));
    let commas = _mm_cmpeq_epi8(value, _mm_set1_epi8(b','.cast_signed()));
    let spaces = _mm_or_si128(
        _mm_cmpeq_epi8(value, _mm_set1_epi8(b' '.cast_signed())),
        _mm_cmpeq_epi8(value, _mm_set1_epi8(b'\t'.cast_signed())),
    );
    RangeMasks {
        digits: _mm_movemask_epi8(digits).cast_unsigned(),
        dashes: _mm_movemask_epi8(dashes).cast_unsigned(),
        commas: _mm_movemask_epi8(commas).cast_unsigned(),
        spaces: _mm_movemask_epi8(spaces).cast_unsigned(),
        zeros: _mm_movemask_epi8(zeros).cast_unsigned(),
    }
}

/// Marks the `tchar` lanes with one packed range comparison and one compare.
///
/// The eight ranges hold every `tchar` except `|` and `~`, which are the only
/// two bytes that survive forcing bit one on and comparing against `~`. A `NUL`
/// lane truncates the implicit string, so lanes after it report no token; that
/// only ever removes token lanes, and a `NUL` is outside every list class, so
/// the block is rejected before the classification is consulted.
#[target_feature(enable = "sse4.2")]
fn token_mask_sse42(value: __m128i) -> u32 {
    const RANGES: &[u8; 16] = b"!!#'*+-.09AZ^`az";

    let ranges_pointer = RANGES.as_ptr().cast();
    // SAFETY: `RANGES` is exactly 16 bytes, so this unaligned load stays in bounds.
    let ranges = unsafe { _mm_loadu_si128(ranges_pointer) };
    let in_ranges = _mm_cmpistrm::<{ _SIDD_UBYTE_OPS | _SIDD_CMP_RANGES | _SIDD_UNIT_MASK | _SIDD_POSITIVE_POLARITY }>(ranges, value);
    let raised = _mm_or_si128(value, _mm_set1_epi8(2));
    let bar_or_tilde = _mm_cmpeq_epi8(raised, _mm_set1_epi8(b'~'.cast_signed()));
    _mm_movemask_epi8(_mm_or_si128(in_ranges, bar_or_tilde)).cast_unsigned()
}

/// Marks the optional-whitespace lanes.
#[target_feature(enable = "sse2")]
fn ows_mask(value: __m128i) -> u32 {
    let space = _mm_cmpeq_epi8(value, _mm_set1_epi8(b' '.cast_signed()));
    let tab = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'\t'.cast_signed()));
    _mm_movemask_epi8(_mm_or_si128(space, tab)).cast_unsigned()
}

/// Marks the comma lanes.
#[target_feature(enable = "sse2")]
fn comma_mask(value: __m128i) -> u32 {
    _mm_movemask_epi8(_mm_cmpeq_epi8(value, _mm_set1_epi8(b','.cast_signed()))).cast_unsigned()
}

#[target_feature(enable = "sse2")]
fn token_mask(value: __m128i) -> i32 {
    let mut mask = _mm_or_si128(in_range(value, b'0', b'9'), in_range(value, b'A', b'Z'));
    mask = _mm_or_si128(mask, in_range(value, b'a', b'z'));
    let mask = b"!#$%&'*+-.^_`|~".iter().fold(mask, |mask, byte| {
        _mm_or_si128(mask, _mm_cmpeq_epi8(value, _mm_set1_epi8(byte.cast_signed())))
    });
    _mm_movemask_epi8(mask)
}

#[target_feature(enable = "sse2")]
fn token68_data_mask(value: __m128i) -> i32 {
    let digit = in_range(value, b'0', b'9');
    let folded = _mm_or_si128(value, _mm_set1_epi8(0x20));
    let alpha = in_range(folded, b'a', b'z');
    let plus_to_slash = in_range(value, b'+', b'/');
    let symbols = _mm_andnot_si128(_mm_cmpeq_epi8(value, _mm_set1_epi8(b','.cast_signed())), plus_to_slash);
    let underscore = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'_'.cast_signed()));
    let tilde = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'~'.cast_signed()));
    _mm_movemask_epi8(_mm_or_si128(
        _mm_or_si128(digit, alpha),
        _mm_or_si128(symbols, _mm_or_si128(underscore, tilde)),
    ))
}

/// Marks the token68 data lanes with a single packed range comparison.
///
/// The `=` padding is deliberately excluded so callers can locate it themselves.
#[target_feature(enable = "sse4.2")]
fn token68_valid_mask_sse42(value: __m128i) -> i32 {
    const RANGES: [u8; 16] = [
        b'0', b'9', b'A', b'Z', b'a', b'z', b'+', b'+', b'-', b'/', b'_', b'_', b'~', b'~', 0, 0,
    ];

    let ranges_pointer = RANGES.as_ptr().cast();
    // SAFETY: `RANGES` is exactly 16 bytes, so this unaligned load stays in bounds.
    let ranges = unsafe { _mm_loadu_si128(ranges_pointer) };
    let matched = _mm_cmpistrm::<{ _SIDD_UBYTE_OPS | _SIDD_CMP_RANGES | _SIDD_UNIT_MASK | _SIDD_POSITIVE_POLARITY }>(ranges, value);
    _mm_movemask_epi8(matched)
}

/// Counts the `#` lanes among the rejected ones, or reports a lane that cannot start a fragment.
///
/// Returns `None` when any rejected lane holds something other than `#`, which means the
/// reference is outside the guaranteed-valid subset and needs the full parser.
#[target_feature(enable = "sse2")]
fn fragment_count(value: __m128i, rejects: u32) -> Option<u32> {
    let hashes = hash_mask(value) & rejects;
    (rejects & !hashes == 0).then(|| hashes.count_ones())
}

/// Marks the lanes outside the base64 alphabet with one packed range comparison.
///
/// `/` and the digits are contiguous, so the alphabet needs only four ranges. A `NUL` lane ends
/// the implicit string and forces every later lane to be reported as rejected, which is the
/// right answer because `NUL` is outside the alphabet and already rejects the whole input.
#[target_feature(enable = "sse4.2")]
fn base64_reject_mask_sse42(value: __m128i) -> u32 {
    const RANGES: [u8; 16] = [b'+', b'+', b'/', b'9', b'A', b'Z', b'a', b'z', 0, 0, 0, 0, 0, 0, 0, 0];

    let ranges_pointer = RANGES.as_ptr().cast();
    // SAFETY: `RANGES` is exactly 16 bytes, so this unaligned load stays in bounds.
    let ranges = unsafe { _mm_loadu_si128(ranges_pointer) };
    let rejected = _mm_cmpistrm::<{ _SIDD_UBYTE_OPS | _SIDD_CMP_RANGES | _SIDD_BIT_MASK | _SIDD_NEGATIVE_POLARITY }>(ranges, value);
    _mm_cvtsi128_si32(rejected).cast_unsigned()
}

/// Marks the lanes outside the base64 alphabet with range compares.
#[target_feature(enable = "sse2")]
fn base64_reject_mask(value: __m128i) -> u32 {
    let digit_or_slash = in_range(value, b'/', b'9');
    let letter = in_range(_mm_or_si128(value, _mm_set1_epi8(0x20)), b'a', b'z');
    let plus = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'+'.cast_signed()));
    let accepted = _mm_or_si128(_mm_or_si128(digit_or_slash, letter), plus);
    !_mm_movemask_epi8(accepted).cast_unsigned() & 0xffff
}

/// Marks the lanes outside the separator-free URI subset with one packed range comparison.
///
/// The subset is exactly eight ranges, which is the widest set `pcmpistrm` can hold, so a single
/// instruction classifies a whole block. `#` is excluded because callers count fragments
/// separately. A `NUL` lane ends the implicit string, and every lane past it is reported as
/// rejected, which only ever sends the caller to the full parser.
#[target_feature(enable = "sse4.2")]
fn simple_uri_reject_mask_sse42(value: __m128i) -> u32 {
    const RANGES: &[u8; 16] = b"!!$$&;==?Z__az~~";

    let ranges_pointer = RANGES.as_ptr().cast();
    // SAFETY: `RANGES` is exactly 16 bytes, so this unaligned load stays in bounds.
    let ranges = unsafe { _mm_loadu_si128(ranges_pointer) };
    let rejected = _mm_cmpistrm::<{ _SIDD_UBYTE_OPS | _SIDD_CMP_RANGES | _SIDD_BIT_MASK | _SIDD_NEGATIVE_POLARITY }>(ranges, value);
    _mm_cvtsi128_si32(rejected).cast_unsigned()
}

/// Marks the lanes outside the separator-free URI subset with one nibble-table lookup.
#[target_feature(enable = "ssse3")]
fn simple_uri_reject_mask_ssse3(value: __m128i) -> u32 {
    let low_nibble = _mm_set_epi64x(0x7cd4_5c54_5cfc_fcfc, 0xfcfc_f8fc_f8f8_fcb8_u64.cast_signed());
    let high_nibble = _mm_set_epi64x(0, 0x8040_2010_0804_0201_u64.cast_signed());
    let low = _mm_and_si128(value, _mm_set1_epi8(0x0f));
    let high = _mm_and_si128(_mm_srli_epi16::<4>(value), _mm_set1_epi8(0x0f));
    let accepted = _mm_and_si128(_mm_shuffle_epi8(low_nibble, low), _mm_shuffle_epi8(high_nibble, high));
    _mm_movemask_epi8(_mm_cmpeq_epi8(accepted, _mm_setzero_si128())).cast_unsigned() & 0xffff
}

/// Marks the lanes outside the separator-free URI subset.
///
/// The `&`..`?` range covers sub-delims, digits, `:`, and `;` after excluding
/// `<` and `>`. Case folding merges the letter ranges; isolated bytes use
/// equality comparisons. `#` is excluded for separate fragment counting.
#[target_feature(enable = "sse2")]
fn simple_uri_reject_mask(value: __m128i) -> u32 {
    let raised = _mm_or_si128(value, _mm_set1_epi8(2));
    let punctuation = _mm_andnot_si128(_mm_cmpeq_epi8(raised, _mm_set1_epi8(0x3e)), in_range(value, b'&', b'?'));
    let bang = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'!'.cast_signed()));
    let dollar = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'$'.cast_signed()));
    let letter = in_range(_mm_or_si128(value, _mm_set1_epi8(0x20)), b'a', b'z');
    let at = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'@'.cast_signed()));
    let underscore = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'_'.cast_signed()));
    let tilde = _mm_cmpeq_epi8(value, _mm_set1_epi8(b'~'.cast_signed()));
    let accepted = _mm_or_si128(
        _mm_or_si128(punctuation, _mm_or_si128(bang, dollar)),
        _mm_or_si128(letter, _mm_or_si128(at, _mm_or_si128(underscore, tilde))),
    );
    !_mm_movemask_epi8(accepted).cast_unsigned() & 0xffff
}

#[target_feature(enable = "sse2")]
fn hash_mask(value: __m128i) -> u32 {
    _mm_movemask_epi8(_mm_cmpeq_epi8(value, _mm_set1_epi8(b'#'.cast_signed()))).cast_unsigned()
}

#[target_feature(enable = "sse2")]
fn lower(value: __m128i) -> __m128i {
    _mm_or_si128(value, _mm_and_si128(in_range(value, b'A', b'Z'), _mm_set1_epi8(0x20)))
}

#[target_feature(enable = "sse2")]
fn in_range(value: __m128i, start: u8, end: u8) -> __m128i {
    let above_start = _mm_cmpgt_epi8(value, _mm_set1_epi8(start.wrapping_sub(1).cast_signed()));
    let below_end = _mm_cmpgt_epi8(_mm_set1_epi8(end.wrapping_add(1).cast_signed()), value);
    _mm_and_si128(above_start, below_end)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::arch;
    use std::time::Duration;
    #[cfg(not(feature = "std"))]
    use std::vec;
    #[cfg(not(feature = "std"))]
    use std::vec::Vec;

    use super::*;

    #[test]
    fn sse2_matches_scalar() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let sse42 = arch::is_x86_feature_detected!("sse4.2");
        let ssse3 = arch::is_x86_feature_detected!("ssse3");
        bolero::check!()
            .with_iterations(4_096)
            .with_test_time(Duration::from_millis(400))
            .with_type::<(Vec<u8>, Vec<u8>)>()
            .for_each(|(left, right)| {
                // SAFETY: runtime detection above establishes SSE2 support.
                let token = unsafe { is_token(left) };
                assert_eq!(token, crate::scalar::is_token(left));
                // SAFETY: runtime detection above establishes SSE2 support.
                let token68 = unsafe { is_token68_sse2(left) };
                assert_eq!(token68, crate::scalar::is_token68(left));
                assert_eq!(
                    sse42.then(|| {
                        // SAFETY: runtime detection above establishes SSE4.2 support.
                        unsafe { is_token68_sse42(left) }
                    }),
                    sse42.then(|| crate::scalar::is_token68(left))
                );
                // SAFETY: runtime detection above establishes SSE2 support.
                let field_value = unsafe { is_field_value(left) };
                assert_eq!(field_value, crate::scalar::is_field_value(left));
                if left.len() == right.len() {
                    // SAFETY: runtime detection establishes SSE2 support; lengths are equal.
                    let equal = unsafe { eq_ignore_ascii_case(left, right) };
                    assert_eq!(equal, crate::scalar::eq_ignore_ascii_case(left, right));
                }
                // SAFETY: runtime detection above establishes SSE2 support.
                let interesting = unsafe { find_interesting(left) };
                assert_eq!(interesting, crate::scalar::find_interesting(left));
                if left.len() >= WIDTH {
                    let expected = crate::base64::all_base64_alphabet(left);
                    assert_eq!(
                        sse42.then(|| {
                            // SAFETY: runtime detection establishes SSE4.2 support.
                            unsafe { all_base64_alphabet_sse42(left) }
                        }),
                        sse42.then_some(expected)
                    );
                    // SAFETY: runtime detection establishes SSE2 support; the length is checked.
                    let base64 = unsafe { all_base64_alphabet_sse2(left) };
                    assert_eq!(base64, expected);
                    // SAFETY: runtime detection establishes SSE2 support; the length is checked.
                    let simple = unsafe { is_simple_uri_tail_sse2(left) };
                    assert_eq!(simple, crate::uri::simple_uri_tail(left, 0));
                    let expected = crate::uri::simple_uri_tail(left, 0);
                    assert_eq!(
                        sse42.then(|| {
                            // SAFETY: runtime detection establishes SSE4.2 support.
                            unsafe { is_simple_uri_tail_sse42(left) }
                        }),
                        sse42.then_some(expected)
                    );
                    assert_eq!(
                        ssse3.then(|| {
                            // SAFETY: runtime detection establishes SSSE3 support.
                            unsafe { is_simple_uri_tail_ssse3(left) }
                        }),
                        ssse3.then_some(expected)
                    );
                }
                if left.len() >= WIDTH {
                    for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                        let expected = crate::list::scan_token_list(left, empty);
                        // SAFETY: runtime detection establishes SSE2 support; the length is checked.
                        let list = unsafe { scan_token_list_sse2(left, empty) };
                        assert_eq!(list, expected);
                        assert_eq!(
                            sse42.then(|| {
                                // SAFETY: runtime detection establishes SSE4.2 support.
                                unsafe { scan_token_list_sse42(left, empty) }
                            }),
                            sse42.then_some(expected)
                        );
                        if left.len() == WIDTH {
                            // SAFETY: detection above, and the line is exactly one vector.
                            let short = unsafe { scan_short_token_list_sse2(left, empty, false) };
                            assert_eq!(short, expected);
                            assert_eq!(
                                sse42.then(|| {
                                    // SAFETY: detection above, and the line is exactly one vector.
                                    unsafe { scan_short_token_list_sse42(left, empty, false) }
                                }),
                                sse42.then_some(expected)
                            );
                        } else if left.len() <= 2 * WIDTH {
                            // SAFETY: detection above, and the line spans two vectors.
                            let short = unsafe { scan_short_token_list_sse2(left, empty, true) };
                            assert_eq!(short, expected);
                            assert_eq!(
                                sse42.then(|| {
                                    // SAFETY: detection above, and the line spans two vectors.
                                    unsafe { scan_short_token_list_sse42(left, empty, true) }
                                }),
                                sse42.then_some(expected)
                            );
                        }
                    }
                }
            });
    }

    /// Puts every byte value in every lane of a token-list block.
    ///
    /// Each kernel classifies a lane into one of four roles, so a single
    /// misplaced range or compare shows up as one byte in one lane disagreeing
    /// with the scalar state machine, which no random search reliably finds.
    #[test]
    fn token_list_lanes_match_scalar_for_every_byte() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let sse42 = arch::is_x86_feature_detected!("sse4.2");
        for lane in 0..2 * WIDTH {
            for byte in u8::MIN..=u8::MAX {
                let mut block = [b'a'; 2 * WIDTH];
                block[lane] = byte;
                for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                    let expected = crate::list::scan_token_list(&block, empty);
                    // SAFETY: runtime detection above establishes SSE2 support.
                    let list = unsafe { scan_token_list_sse2(&block, empty) };
                    assert_eq!(list, expected, "byte {byte:#04x} lane {lane}");
                    assert_eq!(
                        sse42.then(|| {
                            // SAFETY: runtime detection establishes SSE4.2 support.
                            unsafe { scan_token_list_sse42(&block, empty) }
                        }),
                        sse42.then_some(expected),
                        "byte {byte:#04x} lane {lane}"
                    );
                }
            }
        }
    }

    #[test]
    fn range_lanes_match_scalar_for_every_byte() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }
        for lane in 0..WINDOW {
            for byte in u8::MIN..=u8::MAX {
                let mut window = [b'0'; WINDOW];
                window[lane] = byte;
                // SAFETY: runtime detection above establishes SSE2 support.
                let masks = unsafe { range_masks(&window) };
                assert_eq!(masks, RangeMasks::scalar(&window), "byte {byte:#04x} lane {lane}");
            }
        }
    }

    #[test]
    fn base64_and_uri_lanes_match_every_x86_backend() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let sse42 = arch::is_x86_feature_detected!("sse4.2");
        let ssse3 = arch::is_x86_feature_detected!("ssse3");
        for lane in 0..2 * WIDTH {
            for byte in u8::MIN..=u8::MAX {
                let mut base64_block = [b'A'; 2 * WIDTH];
                base64_block[lane] = byte;
                let expected_base64 = crate::base64::all_base64_alphabet(&base64_block);
                assert_eq!(
                    // SAFETY: runtime detection above establishes SSE2 support.
                    unsafe { all_base64_alphabet_sse2(&base64_block) },
                    expected_base64,
                    "base64 byte {byte:#04x} lane {lane}"
                );
                assert_eq!(
                    sse42.then(|| {
                        // SAFETY: runtime detection establishes SSE4.2 support.
                        unsafe { all_base64_alphabet_sse42(&base64_block) }
                    }),
                    sse42.then_some(expected_base64),
                    "SSE4.2 base64 byte {byte:#04x} lane {lane}"
                );

                let mut uri_block = [b'a'; 2 * WIDTH];
                uri_block[lane] = byte;
                let expected_uri = crate::uri::simple_uri_tail(&uri_block, 0);
                assert_eq!(
                    // SAFETY: runtime detection above establishes SSE2 support.
                    unsafe { is_simple_uri_tail_sse2(&uri_block) },
                    expected_uri,
                    "URI byte {byte:#04x} lane {lane}"
                );
                assert_eq!(
                    ssse3.then(|| {
                        // SAFETY: runtime detection establishes SSSE3 support.
                        unsafe { is_simple_uri_tail_ssse3(&uri_block) }
                    }),
                    ssse3.then_some(expected_uri),
                    "SSSE3 URI byte {byte:#04x} lane {lane}"
                );
                assert_eq!(
                    sse42.then(|| {
                        // SAFETY: runtime detection establishes SSE4.2 support.
                        unsafe { is_simple_uri_tail_sse42(&uri_block) }
                    }),
                    sse42.then_some(expected_uri),
                    "SSE4.2 URI byte {byte:#04x} lane {lane}"
                );
            }
        }
    }

    #[test]
    fn long_sse2_scanners_cover_full_blocks_and_overlapping_tails() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }

        let valid_token68 = [b'a'; 64];
        // SAFETY: runtime detection above establishes SSE2 support.
        assert!(unsafe { is_token68_sse2(&valid_token68) });
        let mut invalid_token68 = valid_token68;
        invalid_token68[20] = b':';
        // SAFETY: runtime detection above establishes SSE2 support.
        assert!(!unsafe { is_token68_sse2(&invalid_token68) });
        let mut padded_token68 = valid_token68;
        padded_token68[48..].fill(b'=');
        // SAFETY: runtime detection above establishes SSE2 support.
        assert!(unsafe { is_token68_sse2(&padded_token68) });
        let valid_token68_tail = [b'a'; 48];
        // SAFETY: runtime detection above establishes SSE2 support.
        assert!(unsafe { is_token68_sse2(&valid_token68_tail) });
        let mut invalid_token68_tail = valid_token68_tail;
        invalid_token68_tail[40] = b':';
        // SAFETY: runtime detection above establishes SSE2 support.
        assert!(!unsafe { is_token68_sse2(&invalid_token68_tail) });

        for length in [48, 49] {
            let valid_base64 = vec![b'A'; length];
            // SAFETY: runtime detection above establishes SSE2 support.
            assert!(unsafe { all_base64_alphabet_sse2(&valid_base64) });
            let mut invalid_base64 = valid_base64;
            invalid_base64[length - 1] = b'=';
            // SAFETY: runtime detection above establishes SSE2 support.
            assert!(!unsafe { all_base64_alphabet_sse2(&invalid_base64) });
        }

        let exact_list = [b'a'; 48];
        assert_eq!(
            // SAFETY: runtime detection above establishes SSE2 support.
            unsafe { scan_token_list_sse2(&exact_list, EmptyMembers::Skip) },
            TokenListScan::Members
        );
        let mut tailed_list = [b'a'; 49];
        tailed_list[47] = b',';
        assert_eq!(
            // SAFETY: runtime detection above establishes SSE2 support.
            unsafe { scan_token_list_sse2(&tailed_list, EmptyMembers::Skip) },
            TokenListScan::Members
        );
        tailed_list[48] = b' ';
        assert_eq!(
            // SAFETY: runtime detection above establishes SSE2 support.
            unsafe { scan_token_list_sse2(&tailed_list, EmptyMembers::Reject) },
            TokenListScan::Rejected
        );
        tailed_list[48] = b'(';
        assert_eq!(
            // SAFETY: runtime detection above establishes SSE2 support.
            unsafe { scan_token_list_sse2(&tailed_list, EmptyMembers::Skip) },
            TokenListScan::Rejected
        );
    }

    #[test]
    fn long_uri_paths_cover_every_available_x86_backend() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }

        fn exercise(scan: unsafe fn(&[u8]) -> bool) {
            let valid_exact = [b'a'; 48];
            // SAFETY: the caller supplies a scanner whose feature was detected.
            assert!(unsafe { scan(&valid_exact) });

            let valid_tail = [b'a'; 49];
            // SAFETY: the caller supplies a scanner whose feature was detected.
            assert!(unsafe { scan(&valid_tail) });

            let mut invalid_block = valid_exact;
            invalid_block[17] = b'%';
            // SAFETY: the caller supplies a scanner whose feature was detected.
            assert!(!unsafe { scan(&invalid_block) });

            let mut repeated_fragment = valid_exact;
            repeated_fragment[1] = b'#';
            repeated_fragment[33] = b'#';
            // SAFETY: the caller supplies a scanner whose feature was detected.
            assert!(!unsafe { scan(&repeated_fragment) });

            let mut invalid_tail = valid_tail;
            invalid_tail[48] = b'%';
            // SAFETY: the caller supplies a scanner whose feature was detected.
            assert!(!unsafe { scan(&invalid_tail) });

            let mut fragment_tail = valid_tail;
            fragment_tail[1] = b'#';
            fragment_tail[48] = b'#';
            // SAFETY: the caller supplies a scanner whose feature was detected.
            assert!(!unsafe { scan(&fragment_tail) });
        }

        exercise(is_simple_uri_tail_sse2);
        let _ = arch::is_x86_feature_detected!("ssse3").then(|| {
            exercise(is_simple_uri_tail_ssse3);
        });
        let _ = arch::is_x86_feature_detected!("sse4.2").then(|| {
            exercise(is_simple_uri_tail_sse42);
        });
    }

    #[test]
    fn short_sse2_list_folds_and_lane_fallbacks_are_explicit() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }
        let one = [b'a'; WIDTH];
        assert_eq!(
            // SAFETY: runtime detection above establishes SSE2 support.
            unsafe { scan_short_token_list_sse2(&one, EmptyMembers::Skip, false) },
            TokenListScan::Members
        );
        let two = [b'a'; 2 * WIDTH];
        assert_eq!(
            // SAFETY: runtime detection above establishes SSE2 support.
            unsafe { scan_short_token_list_sse2(&two, EmptyMembers::Skip, true) },
            TokenListScan::Members
        );
        #[cfg(target_pointer_width = "32")]
        assert_eq!(lanes_of(usize::MAX), u32::MAX);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(lanes_of(usize::MAX), WIDTH_LANES);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            shift_of(usize::try_from(u64::from(u32::MAX) + 1).expect("value fits a 64-bit usize"), 0),
            WIDTH_LANES
        );
    }
}
