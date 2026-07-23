// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared HTTP field-value grammar helpers.

pub(in crate::route) const MAX_QUALITY: u16 = 1000;

/// Parses RFC `qvalue` syntax into an integer in `0..=1000`.
pub(in crate::route) fn parse_quality(value: &[u8]) -> Option<u16> {
    let (&whole, fraction) = value.split_first()?;
    if whole != b'0' && whole != b'1' {
        return None;
    }
    if fraction.is_empty() {
        return Some(if whole == b'1' { MAX_QUALITY } else { 0 });
    }
    let digits = fraction.strip_prefix(b".")?;
    if digits.len() > 3 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if whole == b'1' {
        return digits.iter().all(|digit| *digit == b'0').then_some(MAX_QUALITY);
    }
    let mut quality = 0;
    let mut scale = 100;
    for digit in digits {
        quality += u16::from(*digit - b'0') * scale;
        scale /= 10;
    }
    Some(quality)
}

/// Returns whether `byte` is an RFC `tchar`.
pub(in crate::route) const fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}
