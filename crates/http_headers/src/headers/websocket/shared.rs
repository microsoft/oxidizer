// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::super::{invalid_syntax, trim_ows as header_trim_ows};
use crate::source::FieldLines;
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef};

pub(super) fn encode_fixed_base64(input: &[u8]) -> FieldValue {
    FieldValue::try_from(STANDARD.encode(input)).expect("the standard base64 alphabet is always a valid field value")
}

fn base64_encoded_length(decoded_length: usize) -> Option<usize> {
    decoded_length.checked_add(2).and_then(|length| (length / 3).checked_mul(4))
}

pub(super) fn increment_item_count(value: usize, name: &'static FieldName) -> Result<usize, DecodeError> {
    value
        .checked_add(1)
        .ok_or_else(|| DecodeError::new(name, DecodeErrorKind::InvalidNumber))
}

#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the constant lengths into the scan"
)]
#[inline(always)]
pub(super) fn validate_canonical_base64(bytes: &[u8], decoded_length: usize, name: &'static FieldName) -> Result<(), DecodeError> {
    let encoded_length = base64_encoded_length(decoded_length).ok_or_else(|| DecodeError::new(name, DecodeErrorKind::InvalidNumber))?;
    let padding = (3 - decoded_length % 3) % 3;
    if bytes.len() != encoded_length {
        return Err(invalid_syntax(name));
    }
    let data_end = encoded_length - padding;
    let (data, pad) = bytes.split_at(data_end);
    if !http_headers_simd::all_base64_alphabet(data) || !all_padding(pad) {
        return Err(invalid_syntax(name));
    }
    let canonical_tail = if padding == 0 {
        true
    } else if padding == 1 {
        data.last().is_some_and(|byte| is_canonical_tail_pad1(*byte))
    } else {
        data.last().is_some_and(|byte| is_canonical_tail_pad2(*byte))
    };
    if canonical_tail { Ok(()) } else { Err(invalid_syntax(name)) }
}

/// Reduces the padding test to one branch over the whole run.
///
/// Each byte contributes a nonzero bit only when it differs from `=`, so the
/// loop carries no data-dependent control flow and the fixed nonce and digest
/// lengths let it collapse into a couple of compares.
#[expect(
    clippy::inline_always,
    reason = "specializing per call site folds the constant lengths into the scan"
)]
#[inline(always)]
fn all_padding(bytes: &[u8]) -> bool {
    bytes.iter().fold(0_u8, |differs, byte| differs | (byte ^ b'=')) == 0
}

/// Classifies every byte as a canonical final data character.
///
/// Bit zero is set when the byte may end a run padded with a single `=`, and
/// bit one when it may end a run padded with two. The table is derived from
/// [`base64_sextet`], so a probe is exactly the arithmetic test it replaces
/// while costing one load rather than a shift-and-mask chain.
static CANONICAL_TAIL: [u8; 256] = canonical_tail_table();

const fn canonical_tail_table() -> [u8; 256] {
    let mut table = [0_u8; 256];
    let mut byte = 0_u8;
    loop {
        if let Some(sextet) = base64_sextet(byte) {
            if sextet.trailing_zeros() >= 2 {
                table[byte as usize] |= 1;
            }
            if sextet.trailing_zeros() >= 4 {
                table[byte as usize] |= 2;
            }
        }
        if byte == u8::MAX {
            return table;
        }
        byte += 1;
    }
}

/// Accepts the last data character of a run padded with a single `=`.
///
/// One pad character means the final sextet carries only two significant
/// bits beyond the byte boundary, so the encoding is canonical exactly when
/// the low two bits of the sextet are clear.
#[inline]
fn is_canonical_tail_pad1(byte: u8) -> bool {
    CANONICAL_TAIL[usize::from(byte)] & 1 != 0
}

/// Accepts the last data character of a run padded with two `=`.
///
/// Two pad characters leave only two significant bits in the final sextet, so
/// the encoding is canonical exactly when the low four bits are clear.
#[inline]
fn is_canonical_tail_pad2(byte: u8) -> bool {
    CANONICAL_TAIL[usize::from(byte)] & 2 != 0
}

const fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

pub(super) fn validate_bare_list(
    values: &FieldLines<'_>,
    accepts_bare_item: fn(&[u8]) -> bool,
    validate_item: fn(&[u8]) -> Result<(), DecodeError>,
) -> Result<(), DecodeError> {
    values.validate_list_item_limit(b',', true)?;
    for value in values.repeated() {
        if !accepts_bare_item(value.as_bytes()) {
            let mut validate_item = validate_item;
            return validate_list(values, &mut validate_item);
        }
    }
    Ok(())
}

/// Validates every comma-delimited member of a header's field lines.
///
/// Quoting only ever affects where members start and end, so members holding
/// no quote or backslash are split by a tight scan that needs no per-item
/// iterator state. On the first quoted member, the delimiter-aware scan
/// resumes at that member and continues through the remaining field lines.
#[inline(never)]
pub(super) fn validate_list(
    values: &FieldLines<'_>,
    validate_item: &mut dyn FnMut(&[u8]) -> Result<(), DecodeError>,
) -> Result<(), DecodeError> {
    values.validate_list_item_limit(b',', true)?;
    let mut present = false;
    let mut lines = values.repeated().enumerate();
    while let Some((value_index, value)) = lines.next() {
        let mut item_start = 0_usize;
        for raw_item in value.as_bytes().split(|byte| *byte == b',') {
            if raw_item.iter().any(|byte| matches!(byte, b'"' | b'\\')) {
                validate_quoted_line(
                    values.name(),
                    &value.as_bytes()[item_start..],
                    value_index,
                    validate_item,
                    &mut present,
                )?;
                for (value_index, value) in lines {
                    validate_quoted_line(values.name(), value.as_bytes(), value_index, validate_item, &mut present)?;
                }
                return Ok(());
            }
            let item = trim_ows(raw_item);
            if !item.is_empty() {
                validate_item(item)?;
                present = true;
            }
            item_start += raw_item.len() + 1;
        }
    }
    if present {
        Ok(())
    } else {
        Err(DecodeError::new(values.name(), DecodeErrorKind::MissingValue))
    }
}

#[inline(never)]
fn validate_quoted_line(
    name: &'static FieldName,
    bytes: &[u8],
    value_index: usize,
    validate_item: &mut dyn FnMut(&[u8]) -> Result<(), DecodeError>,
    present: &mut bool,
) -> Result<(), DecodeError> {
    let mut start = 0_usize;
    let mut position = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    while position < bytes.len() {
        let byte = bytes[position];
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if !quoted && byte == b',' {
            let item = trim_ows(&bytes[start..position]);
            *present |= validate_present_item(item, validate_item)?;
            start = position + 1;
        }
        position += 1;
    }
    if quoted || escaped {
        return Err(DecodeError::new(name, DecodeErrorKind::UnterminatedQuote).at_value(value_index));
    }
    let item = trim_ows(&bytes[start..]);
    *present |= validate_present_item(item, validate_item)?;
    Ok(())
}

fn validate_present_item(item: &[u8], validate_item: &mut dyn FnMut(&[u8]) -> Result<(), DecodeError>) -> Result<bool, DecodeError> {
    if item.is_empty() {
        return Ok(false);
    }
    validate_item(item)?;
    Ok(true)
}

/// Removes optional whitespace without the bounds checks of range slicing.
#[inline]
pub(super) fn trim_ows(bytes: &[u8]) -> &[u8] {
    let mut trimmed = bytes;
    while let [b' ' | b'\t', rest @ ..] = trimmed {
        trimmed = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = trimmed {
        trimmed = rest;
    }
    trimmed
}

pub(super) fn validate_header_value_list(
    value: FieldValueRef<'_>,
    name: &'static FieldName,
    validate_item: fn(&[u8]) -> Result<(), DecodeError>,
) -> Result<(), DecodeError> {
    let mut count = 0_usize;
    for item in CommaItems::new(value.as_bytes()) {
        validate_item(item)?;
        count = increment_item_count(count, name)?;
    }
    if count == 0 { Err(invalid_syntax(name)) } else { Ok(()) }
}

pub(super) struct CommaItems<'a> {
    bytes: &'a [u8],
    start: usize,
    position: usize,
    finished: bool,
}

impl<'a> CommaItems<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            start: 0,
            position: 0,
            finished: false,
        }
    }
}

impl<'a> Iterator for CommaItems<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            let mut quoted = false;
            let mut escaped = false;
            while let Some(byte) = self.bytes.get(self.position).copied() {
                if escaped {
                    escaped = false;
                } else if quoted && byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = !quoted;
                } else if !quoted && byte == b',' {
                    let item = header_trim_ows(&self.bytes[self.start..self.position]);
                    self.position += 1;
                    self.start = self.position;
                    if item.is_empty() {
                        continue;
                    }
                    return Some(item);
                }
                self.position += 1;
            }
            self.finished = true;
            let item = header_trim_ows(&self.bytes[self.start..]);
            if !item.is_empty() {
                return Some(item);
            }
        }
        None
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        CommaItems, base64_encoded_length, base64_sextet, canonical_tail_table, encode_fixed_base64, increment_item_count,
        is_canonical_tail_pad1, is_canonical_tail_pad2, trim_ows, validate_bare_list, validate_canonical_base64,
        validate_header_value_list, validate_list, validate_present_item,
    };
    use crate::source::FieldLines;
    use crate::{DecodeErrorKind, FieldName, FieldValue};

    fn validate_alpha(item: &[u8]) -> Result<(), crate::DecodeError> {
        item.iter()
            .all(u8::is_ascii_alphabetic)
            .then_some(())
            .ok_or_else(|| crate::DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::InvalidToken))
    }

    fn reject_named_item(item: &[u8]) -> Result<(), crate::DecodeError> {
        if item == b"reject" {
            Err(crate::DecodeError::new(
                &FieldName::SecWebSocketExtensions,
                DecodeErrorKind::InvalidToken,
            ))
        } else {
            Ok(())
        }
    }

    #[test]
    fn canonical_tail_table_matches_the_sextet_arithmetic() {
        let build: fn() -> [u8; 256] = canonical_tail_table;
        let runtime_table = std::hint::black_box(build)();
        for byte in 0..=u8::MAX {
            let sextet = base64_sextet(byte);
            assert_eq!(runtime_table[usize::from(byte)], super::CANONICAL_TAIL[usize::from(byte)]);
            assert_eq!(
                is_canonical_tail_pad1(byte),
                sextet.is_some_and(|value| value.trailing_zeros() >= 2),
                "one-pad classification differs for {byte:#04x}"
            );
            assert_eq!(
                is_canonical_tail_pad2(byte),
                sextet.is_some_and(|value| value.trailing_zeros() >= 4),
                "two-pad classification differs for {byte:#04x}"
            );
        }
    }

    #[test]
    fn fixed_base64_encoding_and_validation_cover_all_padding_shapes() {
        assert_eq!(encode_fixed_base64(b"a").as_bytes(), b"YQ==");
        assert_eq!(encode_fixed_base64(b"ab").as_bytes(), b"YWI=");
        assert_eq!(encode_fixed_base64(b"abc").as_bytes(), b"YWJj");
        assert_eq!(base64_encoded_length(0), Some(0));
        assert_eq!(base64_encoded_length(3), Some(4));
        assert_eq!(base64_encoded_length(usize::MAX - 2), None);
        assert_eq!(base64_encoded_length(usize::MAX), None);
        assert_eq!(
            increment_item_count(usize::MAX - 1, &FieldName::SecWebSocketProtocol).expect("last count is representable"),
            usize::MAX
        );
        assert_eq!(
            increment_item_count(usize::MAX, &FieldName::SecWebSocketProtocol)
                .expect_err("count overflow is rejected")
                .kind(),
            DecodeErrorKind::InvalidNumber
        );

        assert_eq!(validate_canonical_base64(b"YWJj", 3, &FieldName::SecWebSocketKey), Ok(()));

        let first_slow = [FieldValue::from_static("alpha, beta")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketProtocol, &first_slow).expect("nonempty values");
        assert_eq!(
            validate_bare_list(&values, |item| !item.contains(&b','), |_| Ok::<_, crate::DecodeError>(()),),
            Ok(())
        );

        let later_slow = [FieldValue::from_static("alpha"), FieldValue::from_static("beta, gamma")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketProtocol, &later_slow).expect("nonempty values");
        assert_eq!(
            validate_bare_list(&values, |item| !item.contains(&b','), |_| Ok::<_, crate::DecodeError>(()),),
            Ok(())
        );
        assert_eq!(validate_canonical_base64(b"YQ==", 1, &FieldName::SecWebSocketKey), Ok(()));
        assert_eq!(validate_canonical_base64(b"YWI=", 2, &FieldName::SecWebSocketKey), Ok(()));
        for (wire, decoded) in [
            (b"YR==".as_slice(), 1),
            (b"YWJ=".as_slice(), 2),
            (b"YW!j".as_slice(), 3),
            (b"YQ=A".as_slice(), 1),
            (b"short".as_slice(), 3),
        ] {
            let _error = validate_canonical_base64(wire, decoded, &FieldName::SecWebSocketKey).expect_err("noncanonical base64 must fail");
        }
        assert_eq!(
            validate_canonical_base64(b"", usize::MAX, &FieldName::SecWebSocketKey)
                .expect_err("length arithmetic overflows")
                .kind(),
            DecodeErrorKind::InvalidNumber
        );
    }

    #[test]
    fn list_validation_switches_between_bare_and_quoted_paths() {
        assert_eq!(validate_present_item(b"", &mut validate_alpha), Ok(false));

        let stored = [FieldValue::from_static("alpha"), FieldValue::from_static("beta, gamma")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketProtocol, &stored).expect("nonempty values");
        assert_eq!(
            validate_bare_list(&values, |item| item.iter().all(u8::is_ascii_alphabetic), validate_alpha,),
            Ok(())
        );

        let invalid = [FieldValue::from_static("alpha, bad value")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketProtocol, &invalid).expect("nonempty values");
        assert_eq!(
            validate_bare_list(&values, |item| item.iter().all(u8::is_ascii_alphabetic), validate_alpha,)
                .expect_err("invalid member propagates")
                .kind(),
            DecodeErrorKind::InvalidToken
        );

        let quoted = [FieldValue::from_static("alpha, ext=\"a,b\""), FieldValue::from_static("z")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &quoted).expect("nonempty values");
        let mut seen = Vec::new();
        let mut collect_item = |item: &[u8]| {
            seen.push(item.to_vec());
            Ok(())
        };
        validate_list(&values, &mut collect_item).expect("quoted comma remains in one item");
        assert_eq!(seen, [b"alpha".to_vec(), b"ext=\"a,b\"".to_vec(), b"z".to_vec()]);

        let unterminated = [FieldValue::from_static("alpha"), FieldValue::from_static("\"beta")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &unterminated).expect("nonempty values");
        let error = validate_list(&values, &mut |_| Ok::<_, crate::DecodeError>(())).expect_err("unterminated quote");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        assert_eq!(error.value_index(), Some(1));

        let quoted_then_unterminated = [FieldValue::from_static("ext=\"a,b\""), FieldValue::from_static("\"unterminated")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &quoted_then_unterminated).expect("nonempty values");
        let error = validate_list(&values, &mut |_| Ok::<_, crate::DecodeError>(())).expect_err("later unterminated quote");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        assert_eq!(error.value_index(), Some(1));

        let quoted = [FieldValue::from_static("reject, ext=\"a,b\"")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &quoted).expect("nonempty values");
        assert_eq!(
            validate_list(&values, &mut reject_named_item)
                .expect_err("delimiter item error propagates")
                .kind(),
            DecodeErrorKind::InvalidToken
        );

        let quoted = [FieldValue::from_static("ext=\"a,b\", tail")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &quoted).expect("nonempty values");
        assert_eq!(
            validate_list(&values, &mut |_| {
                Err::<(), _>(crate::DecodeError::new(
                    &FieldName::SecWebSocketExtensions,
                    DecodeErrorKind::InvalidToken,
                ))
            })
            .expect_err("quoted delimiter item error propagates")
            .kind(),
            DecodeErrorKind::InvalidToken
        );

        let quoted = [FieldValue::from_static("ext=\"a,b\", reject")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &quoted).expect("nonempty values");
        assert_eq!(
            validate_list(&values, &mut reject_named_item)
                .expect_err("final item error propagates")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
    }

    #[test]
    fn comma_items_header_value_lists_and_ows_handle_empty_and_quoted_members() {
        assert_eq!(trim_ows(b" \t value \t "), b"value");
        assert_eq!(
            CommaItems::new(b", alpha, ext=\"a,b\",, omega ").collect::<Vec<_>>(),
            [b"alpha".as_slice(), b"ext=\"a,b\"".as_slice(), b"omega".as_slice()]
        );

        let escaped = [FieldValue::from_static("ext=\"a\\,b\", tail")];
        let values = FieldLines::from_slice(&FieldName::SecWebSocketExtensions, &escaped).expect("nonempty values");
        let mut seen = Vec::new();
        let mut collect_item = |item: &[u8]| {
            seen.push(item.to_vec());
            Ok(())
        };
        validate_list(&values, &mut collect_item).expect("escaped comma remains quoted");
        assert_eq!(seen, [b"ext=\"a\\,b\"".to_vec(), b"tail".to_vec()]);

        let value = FieldValue::from_static("alpha, beta");
        assert_eq!(
            CommaItems::new(b"ext=\"a\\,b\", tail").collect::<Vec<_>>(),
            [b"ext=\"a\\,b\"".as_slice(), b"tail".as_slice()]
        );
        assert_eq!(
            validate_header_value_list(value.as_field_value_ref(), &FieldName::SecWebSocketProtocol, validate_alpha,),
            Ok(())
        );
        let empty = FieldValue::from_static(",,,");
        assert_eq!(
            validate_header_value_list(empty.as_field_value_ref(), &FieldName::SecWebSocketProtocol, validate_alpha,)
                .expect_err("empty list")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}
