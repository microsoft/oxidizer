// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Guaranteed-valid scanner for the common `bytes=` byte-range-set.
//!
//! A field line such as `bytes=0-499, 1000-` is a comma separated list of
//! `first-last`, `first-`, and `-suffix` specifications made only of decimal
//! digits, a hyphen, commas, and optional whitespace. The scanner classifies
//! one sixteen byte window with a handful of masks and then decides the whole
//! grammar — including `first <= last` — with branch-free bit arithmetic, so a
//! well formed line never walks the general parser.
//!
//! Every answer is conservative: `false` means "not provably a valid range
//! set", never "malformed", so callers fall back to the parser that produces
//! the diagnostic.

/// Bytes classified in one pass.
pub(super) const WINDOW: usize = 16;

/// Byte that fills the lanes before a line shorter than one window.
///
/// It belongs to no class the scan reads, so it can neither open a member nor
/// satisfy a rule.
pub(super) const FILLER: u8 = b'x';

/// Per-lane classification of one window.
///
/// Each field holds one bit per lane, with lane zero in the least significant
/// bit. The vector kernels only fill these in; every grammar decision is made
/// once, in [`accepts`], so the scalar, SSE2, and NEON paths cannot disagree
/// about anything but the masks themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RangeMasks {
    /// Lanes holding `0`–`9`.
    pub(super) digits: u32,
    /// Lanes holding `-`.
    pub(super) dashes: u32,
    /// Lanes holding `,`.
    pub(super) commas: u32,
    /// Lanes holding a space or a horizontal tab.
    pub(super) spaces: u32,
    /// Lanes holding `0`.
    pub(super) zeros: u32,
}

impl RangeMasks {
    /// Classifies a window without vector instructions.
    #[inline]
    pub(super) fn scalar(window: &[u8; WINDOW]) -> Self {
        let mut masks = Self {
            digits: 0,
            dashes: 0,
            commas: 0,
            spaces: 0,
            zeros: 0,
        };
        for (lane, &byte) in window.iter().enumerate() {
            let bit = 1_u32 << lane;
            match byte {
                b'0'..=b'9' => {
                    masks.digits |= bit;
                    if byte == b'0' {
                        masks.zeros |= bit;
                    }
                }
                b'-' => masks.dashes |= bit,
                b',' => masks.commas |= bit,
                b' ' | b'\t' => masks.spaces |= bit,
                _other => {}
            }
        }
        masks
    }
}

/// Returns whether the last `len` lanes of `window` are a valid range set.
///
/// The window is classified by `masks`, and only its final `len` lanes belong
/// to the payload, so a caller that holds a longer field line can pass the
/// window that ends with it and leave the earlier lanes alone.
///
/// Every rule contributes to one rejection mask that is tested once, so a line
/// that is going to be accepted takes a single branch on its way through.
#[expect(clippy::inline_always, reason = "the padded path pays for the call and the masks it cannot fold")]
#[inline(always)]
pub(super) fn accepts(window: &[u8; WINDOW], masks: &RangeMasks, len: usize) -> bool {
    if len == 0 || len > WINDOW {
        return false;
    }
    let payload = (0xffff_u64 << (WINDOW - len)) & 0xffff;
    let digits = u64::from(masks.digits) & payload;
    let dashes = u64::from(masks.dashes) & payload;
    let commas = u64::from(masks.commas) & payload;
    let spaces = u64::from(masks.spaces) & payload;
    let after_digit = digits << 1;
    let before_digit = digits >> 1;

    // Members are the runs of digits and hyphens, and every one of them walks
    // over its leading digits onto the hyphen that splits it. A run that walks
    // onto a comma, onto whitespace, or off the end of the line holds no
    // hyphen, and a second hyphen is neither the landing of its member nor the
    // start of one.
    let member = digits | dashes;
    let starts = member & !(member << 1);
    let landing = digits.wrapping_add(starts & digits) & !digits;

    let rejected =
        // A byte outside the four classes leaves the subset.
        (payload & !(member | commas | spaces))
        // Whitespace is only recognized where it follows a comma.
        | (spaces & !(commas << 1))
        // Every member holds exactly one hyphen.
        | (landing & !dashes)
        | (dashes & !landing & !starts)
        // A hyphen needs a digit on at least one side.
        | (dashes & !after_digit & !before_digit)
        // A run that starts with a zero and continues would compare as a
        // shorter number than it is, so leave those to the general parser.
        | (u64::from(masks.zeros) & payload & before_digit & !after_digit)
        // A line of nothing but separators holds no member at all.
        | (starts.wrapping_sub(1) >> (u64::BITS - 1));
    if rejected != 0 {
        return false;
    }

    // Only a specification with digits on both sides of its hyphen can be
    // inverted; the rest are already ordered.
    let mut compare = dashes & after_digit & before_digit;
    while compare != 0 {
        if !ordered(window, !digits, compare.trailing_zeros()) {
            return false;
        }
        compare &= compare - 1;
    }
    true
}

/// Returns whether the specification whose hyphen sits at `dash` is ordered.
///
/// `gaps` marks every lane that is not a digit, so shifting it until the lane
/// beside the hyphen reaches the end of the word turns each run length into a
/// count of leading or trailing zeros. Neither run carries a leading zero by
/// the time this runs, so the shorter run is the smaller number and equal runs
/// compare byte by byte.
#[inline]
fn ordered(window: &[u8; WINDOW], gaps: u64, dash: u32) -> bool {
    let first_len = (gaps << (u64::BITS - dash)).leading_zeros();
    let last_len = (gaps >> (dash + 1)).trailing_zeros();
    if first_len != last_len {
        return first_len < last_len;
    }
    same_length_ordered(window, dash, first_len)
}

/// Compares two runs of the same length byte by byte.
///
/// Equal lengths are the only case that survives the length comparison, and a
/// well formed set usually spells its bounds with different widths, so this
/// stays out of the straight-line path.
#[cold]
#[inline(never)]
fn same_length_ordered(window: &[u8; WINDOW], dash: u32, len: u32) -> bool {
    let first = (dash - len) as usize;
    let last = dash as usize + 1;
    for step in 0..len as usize {
        let left = window[first + step];
        let right = window[last + step];
        if left != right {
            return left < right;
        }
    }
    true
}

/// Scans a field line for a range set without vector instructions.
///
/// The dispatcher reaches the scalar classifier directly, so this entry only
/// serves the differential tests and the benchmark harness.
#[cfg(any(test, feature = "benchmarking"))]
pub(super) fn scan_byte_range_set(bytes: &[u8], start: usize) -> bool {
    with_window(bytes, start, |window, len| accepts(window, &RangeMasks::scalar(window), len))
}

/// Calls `scan` with the window that ends the field line, if one exists.
///
/// A line at least [`WINDOW`] bytes long lends its own tail, and a shorter one
/// is copied into a padded window whose leading lanes fall outside the
/// payload; either way the payload occupies the final `len` lanes.
#[cfg(any(test, feature = "benchmarking"))]
#[inline]
pub(super) fn with_window(bytes: &[u8], start: usize, scan: fn(&[u8; WINDOW], usize) -> bool) -> bool {
    let Some(len) = bytes.len().checked_sub(start) else {
        return false;
    };
    if len == 0 || len > WINDOW {
        return false;
    }
    if let Some(window) = window_of(bytes) {
        return scan(window, len);
    }
    let mut padded = [FILLER; WINDOW];
    if !fill_window(bytes, &mut padded) {
        return false;
    }
    scan(&padded, len)
}

/// Right aligns a line shorter than one window inside a padded window.
///
/// Two eight byte copies place the line without the branch tree a variable
/// length copy needs; they overlap for lines shorter than sixteen bytes, which
/// writes the same bytes twice and costs nothing. Lines of fewer than eight
/// bytes have no such pair and are left to the caller's own parser, and the
/// filler is a byte no rule reads, so the lanes before the line decide
/// nothing.
#[inline]
pub(super) fn fill_window(bytes: &[u8], window: &mut [u8; WINDOW]) -> bool {
    if !(8..=WINDOW).contains(&bytes.len()) {
        return false;
    }
    let offset = WINDOW - bytes.len();
    let tail = bytes.len() - 8;
    window[offset..offset + 8].copy_from_slice(&bytes[..8]);
    window[WINDOW - 8..].copy_from_slice(&bytes[tail..]);
    true
}

/// Borrows the window a line ends with, if the line is long enough.
///
/// Asking the slice for its final chunk keeps the bound check to the one
/// comparison the length needs, where re-slicing and converting would also
/// make the compiler prove the pointer arithmetic afresh.
pub(super) fn window_of(bytes: &[u8]) -> Option<&[u8; WINDOW]> {
    bytes.last_chunk::<WINDOW>()
}

/// Mirrors the general parser closely enough to check the scanner against it.
#[cfg(test)]
pub(super) fn oracle(payload: &[u8]) -> bool {
    fn number(bytes: &[u8]) -> Option<u64> {
        if bytes.is_empty() {
            return None;
        }
        bytes.iter().try_fold(0_u64, |value, byte| {
            value
                .checked_mul(10)?
                .checked_add(u64::from(byte.checked_sub(b'0').filter(|d| *d <= 9)?))
        })
    }

    fn trim(bytes: &[u8]) -> &[u8] {
        let mut slice = bytes;
        while let [b' ' | b'\t', rest @ ..] = slice {
            slice = rest;
        }
        while let [rest @ .., b' ' | b'\t'] = slice {
            slice = rest;
        }
        slice
    }

    let mut count = 0_usize;
    for item in payload.split(|byte| *byte == b',') {
        let item = trim(item);
        if item.is_empty() {
            continue;
        }
        let Some(dash) = item.iter().position(|byte| *byte == b'-') else {
            return false;
        };
        if dash == 0 {
            if number(&item[1..]).is_none() {
                return false;
            }
        } else {
            if item[dash + 1..].contains(&b'-') {
                return false;
            }
            let Some(first) = number(&item[..dash]) else {
                return false;
            };
            let last = &item[dash + 1..];
            if !last.is_empty() {
                let Some(last) = number(last) else {
                    return false;
                };
                if last < first {
                    return false;
                }
            }
        }
        count += 1;
    }
    count != 0
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ops::RangeInclusive;
    #[cfg(not(feature = "std"))]
    use std::vec::Vec;

    use super::*;

    /// Runs the scanner the way a field line reaches it.
    fn scan(payload: &[u8]) -> bool {
        let mut line = Vec::from(&b"bytes="[..]);
        line.extend_from_slice(payload);
        scan_byte_range_set(&line, 6)
    }

    #[test]
    fn accepts_the_specification_examples() {
        for payload in [
            &b"0-499"[..],
            b"0-499, 1000-",
            b"1000-",
            b"-500",
            b"0-0",
            b"9-9",
            b"0-499,500-999",
            b"0-1,\t2-3",
            b"0-1,,2-3",
            b"0-1,",
            b",0-1",
            b"12-345",
            b"499-499",
        ] {
            assert!(scan(payload), "rejected {payload:?}");
            assert!(oracle(payload), "oracle rejected {payload:?}");
        }
    }

    #[test]
    fn leaves_everything_unusual_to_the_general_parser() {
        for payload in [
            &b""[..],
            b"-",
            b"0",
            b"500-100",
            b"0--1",
            b"0-1-2",
            b"0- 1",
            b"0 - 1",
            b"00-1",
            b"1-09",
            b"bytes=0-1",
            b"0-1;q=1",
        ] {
            assert!(!scan(payload), "accepted {payload:?}");
        }
    }

    #[test]
    fn helpers_reject_unrepresentable_windows_and_offsets() {
        fn expected_window(window: &[u8; WINDOW], len: usize) -> bool {
            len == WINDOW && window == b"0123456789abcdef"
        }

        let mut padded = [FILLER; WINDOW];
        assert!(!accepts(&padded, &RangeMasks::scalar(&padded), 0));
        assert!(!accepts(&padded, &RangeMasks::scalar(&padded), WINDOW + 1));
        assert!(!with_window(b"short", 6, expected_window));
        assert!(!with_window(b"payload", 7, expected_window));
        assert!(!with_window(b"0123456789abcdefg", 0, expected_window));
        assert!(!fill_window(b"1234567", &mut padded));
        assert!(!fill_window(b"0123456789abcdefg", &mut padded));

        assert!(fill_window(b"1234-678", &mut padded));
        assert_eq!(&padded[WINDOW - 8..], b"1234-678");
        assert!(with_window(b"prefix0123456789abcdef", 6, expected_window));
    }

    #[test]
    fn oracle_rejects_each_malformed_number_shape() {
        for payload in [
            &b", ,\t,"[..],
            b"1",
            b"-",
            b"-x",
            b"x-1",
            b"1-2-3",
            b"1-x",
            b"2-1",
            b"18446744073709551616-",
            b"1-18446744073709551616",
            b"184467440737095516150-",
            b"1-184467440737095516150",
        ] {
            assert!(!oracle(payload), "{payload:?}");
        }
        assert!(oracle(b" \t1-2\t "));
    }

    #[test]
    fn comma_before_payload_does_not_legalize_leading_space() {
        let mut window = [FILLER; WINDOW];
        window[WINDOW - 5] = b',';
        window[WINDOW - 4..].copy_from_slice(b" 0-1");
        assert!(!accepts(&window, &RangeMasks::scalar(&window), 4));
    }

    #[test]
    fn every_acceptance_is_one_the_general_parser_shares() {
        const ALPHABET: &[u8] = b"0-,1 9\t2";
        const NARROW: &[u8] = b"0-,1 ";

        fn sweep(alphabet: &[u8], lengths: RangeInclusive<usize>) {
            let mut payload = Vec::new();
            for length in lengths {
                let total = alphabet.len().pow(u32::try_from(length).unwrap_or(0));
                for index in 0..total {
                    payload.clear();
                    let mut rest = index;
                    for _step in 0..length {
                        payload.push(alphabet[rest % alphabet.len()]);
                        rest /= alphabet.len();
                    }
                    if scan(&payload) {
                        assert!(oracle(&payload), "accepted invalid {payload:?}");
                    }
                }
            }
        }

        // The wide alphabet covers every class, and the narrow one reaches the
        // lengths that hold two specifications and a separator between them.
        let wide_max = if cfg!(miri) { 3 } else { 5 };
        let narrow_max = if cfg!(miri) { 7 } else { 8 };
        sweep(ALPHABET, 0..=wide_max);
        sweep(NARROW, 6..=narrow_max);
    }

    #[test]
    fn ordering_is_decided_for_every_short_pair() {
        fn digits(value: u32, into: &mut Vec<u8>) {
            if value >= 10 {
                digits(value / 10, into);
            }
            into.push(b'0' + u8::try_from(value % 10).unwrap_or(0));
        }

        let mut payload = Vec::new();
        for first in 0..=120_u32 {
            for last in 0..=120_u32 {
                payload.clear();
                digits(first, &mut payload);
                payload.push(b'-');
                digits(last, &mut payload);
                assert_eq!(scan(&payload), first <= last, "{payload:?}");
                assert_eq!(oracle(&payload), first <= last, "{payload:?}");
            }
        }
    }

    #[test]
    fn the_window_must_cover_the_whole_payload() {
        // Seventeen payload bytes cannot be classified in one window.
        assert!(!scan(b"10000000-20000000"));
        assert!(scan(b"1000000-2000000"));
    }
}
