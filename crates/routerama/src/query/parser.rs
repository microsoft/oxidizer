// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use super::scan::{SIMD_THRESHOLD, contains_either, find_byte};
use super::{Decoded, Error, ErrorKind, QueryLimits};

#[derive(Debug)]
pub(crate) struct Pair<'q> {
    pub(crate) key: Decoded<'q>,
    pub(crate) value: Decoded<'q>,
    pub(crate) offset: usize,
}

pub(crate) struct Parser<'q, const WIDE: bool> {
    input: &'q str,
    cursor: usize,
    pairs: usize,
    decoded: usize,
    limits: QueryLimits,
}

struct PairScan {
    separator: Option<usize>,
    equals: Option<usize>,
    key_encoded: bool,
    value_encoded: bool,
}

impl<'q, const WIDE: bool> Parser<'q, WIDE> {
    #[inline]
    pub(crate) fn new(input: &'q str, limits: QueryLimits) -> Result<Self, Error> {
        if input.len() > limits.max_query_length {
            return Err(Error::parsing(None, 0, ErrorKind::QueryTooLong));
        }
        Ok(Self {
            input,
            cursor: 0,
            pairs: 0,
            decoded: 0,
            limits,
        })
    }

    #[expect(
        clippy::inline_always,
        reason = "Callgrind shows forced inlining cuts common query parsing from 1,121 to 824 instructions"
    )]
    #[inline(always)]
    // Arithmetic mutations of cursor advancement create non-terminating loops;
    // boundary and malformed-input behavior is pinned by unit tests below.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn next_pair(&mut self) -> Result<Option<Pair<'q>>, Error> {
        if !WIDE {
            return self.next_pair_scalar();
        }
        loop {
            if self.input.as_bytes().get(self.cursor).is_none() {
                return Ok(None);
            }
            let offset = self.cursor;
            let rest = &self.input[offset..];
            if rest.len() < SIMD_THRESHOLD {
                return self.next_pair_scalar();
            }
            let separator = find_byte::<true>(rest.as_bytes(), b'&');
            let pair_len = separator.unwrap_or(rest.len());
            let raw_pair = &rest[..pair_len];
            self.cursor = separator.map_or(self.input.len(), |_| offset + pair_len + 1);
            if raw_pair.is_empty() {
                continue;
            }

            self.pairs += 1;
            if self.pairs > self.limits.max_pairs {
                return Err(Error::parsing(None, offset, ErrorKind::TooManyPairs));
            }

            let equals = find_byte::<true>(raw_pair.as_bytes(), b'=');
            let (key, value) = match equals {
                Some(index) => (&raw_pair[..index], &raw_pair[index + 1..]),
                None => (raw_pair, ""),
            };
            let key = decode::<true>(key, offset, &mut self.decoded, self.limits)?;
            let value_offset = offset + equals.map_or(raw_pair.len(), |index| index + 1);
            let value = decode::<true>(value, value_offset, &mut self.decoded, self.limits)?;
            return Ok(Some(Pair { key, value, offset }));
        }
    }

    #[expect(
        clippy::inline_always,
        reason = "Callgrind shows inlining the fused scan reduces common query parsing below 750 instructions"
    )]
    #[inline(always)]
    fn next_pair_scalar(&mut self) -> Result<Option<Pair<'q>>, Error> {
        loop {
            if self.input.as_bytes().get(self.cursor).is_none() {
                return Ok(None);
            }
            let offset = self.cursor;
            let rest = &self.input[offset..];
            let scan = scan_pair(rest.as_bytes());
            let pair_len = scan.separator.unwrap_or(rest.len());
            let raw_pair = &rest[..pair_len];
            self.cursor = scan.separator.map_or(self.input.len(), |_| advance(offset, advance(pair_len, 1)));
            if raw_pair.is_empty() {
                continue;
            }

            self.pairs += 1;
            if self.pairs > self.limits.max_pairs {
                return Err(Error::parsing(None, offset, ErrorKind::TooManyPairs));
            }

            let (key, value) = match scan.equals {
                Some(index) => (&raw_pair[..index], &raw_pair[index + 1..]),
                None => (raw_pair, ""),
            };
            if !scan.key_encoded && !scan.value_encoded {
                let decoded_len = unencoded_pair_decoded_len(raw_pair, scan.equals);
                if let Some(total) = self.decoded.checked_add(decoded_len)
                    && total <= self.limits.max_decoded_length
                {
                    self.decoded = total;
                    return Ok(Some(Pair {
                        key: Cow::Borrowed(key),
                        value: Cow::Borrowed(value),
                        offset,
                    }));
                }
            }
            let key = decode_known(key, offset, &mut self.decoded, self.limits, scan.key_encoded)?;
            let value_offset = offset + scan.equals.map_or(raw_pair.len(), |index| index + 1);
            let value = decode_known(value, value_offset, &mut self.decoded, self.limits, scan.value_encoded)?;
            return Ok(Some(Pair { key, value, offset }));
        }
    }
}

// Short queries use one pass for `&`, the first `=`, and encoding markers.
// Keeping this separate from the SIMD path reduced the common case from 817
// to 720 instructions without regressing escaped or repeated query parsing.
fn scan_pair(bytes: &[u8]) -> PairScan {
    let mut equals = None;
    let mut key_encoded = false;
    let mut value_encoded = false;
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'&' => {
                return PairScan {
                    separator: Some(index),
                    equals,
                    key_encoded,
                    value_encoded,
                };
            }
            b'=' if equals.is_none() => equals = Some(index),
            b'%' | b'+' if equals.is_some() => value_encoded = true,
            b'%' | b'+' => key_encoded = true,
            _ => {}
        }
    }
    PairScan {
        separator: None,
        equals,
        key_encoded,
        value_encoded,
    }
}

#[expect(
    clippy::inline_always,
    reason = "Callgrind shows forced inlining cuts common query parsing from 1,062 to 824 instructions"
)]
#[inline(always)]
// Arithmetic mutations of the byte cursor create non-terminating loops.
#[cfg_attr(test, mutants::skip)]
fn decode<'q, const WIDE: bool>(
    input: &'q str,
    offset: usize,
    decoded_total: &mut usize,
    limits: QueryLimits,
) -> Result<Cow<'q, str>, Error> {
    if !contains_either::<WIDE>(input.as_bytes(), b'%', b'+') {
        add_decoded(decoded_total, input.len(), offset, limits)?;
        return Ok(Cow::Borrowed(input));
    }

    decode_encoded(input, offset, decoded_total, limits)
}

#[expect(
    clippy::inline_always,
    reason = "the caller's fused scalar scan makes the encoded flag a hot-path constant"
)]
#[inline(always)]
fn decode_known<'q>(
    input: &'q str,
    offset: usize,
    decoded_total: &mut usize,
    limits: QueryLimits,
    encoded: bool,
) -> Result<Cow<'q, str>, Error> {
    if !encoded {
        add_decoded(decoded_total, input.len(), offset, limits)?;
        return Ok(Cow::Borrowed(input));
    }

    decode_encoded(input, offset, decoded_total, limits)
}

#[inline]
fn decode_encoded<'q>(input: &'q str, offset: usize, decoded_total: &mut usize, limits: QueryLimits) -> Result<Cow<'q, str>, Error> {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(bytes.len());
    let mut index = 0;
    while bytes.get(index).is_some() {
        let Some(relative) = bytes[index..].iter().position(|&byte| matches!(byte, b'%' | b'+')) else {
            output.push_str(&input[index..]);
            break;
        };
        let special = advance(index, relative);
        output.push_str(&input[index..special]);
        if bytes[special] == b'+' {
            output.push(' ');
            index = advance(special, 1);
        } else {
            let Some((&high, rest)) = bytes.get(advance(special, 1)).zip(bytes.get(advance(special, 2)..)) else {
                return Err(Error::parsing(None, offset + special, ErrorKind::InvalidEncoding));
            };
            let Some(&low) = rest.first() else {
                return Err(Error::parsing(None, offset + special, ErrorKind::InvalidEncoding));
            };
            let Some(high) = hex(high) else {
                return Err(Error::parsing(None, offset + special, ErrorKind::InvalidEncoding));
            };
            let Some(low) = hex(low) else {
                return Err(Error::parsing(None, offset + special, ErrorKind::InvalidEncoding));
            };
            let byte = high * 16 + low;
            // ASCII escapes keep the String valid without a second UTF-8
            // pass. Non-ASCII bytes continue in the same allocation and
            // validate the completed byte sequence below.
            if !byte.is_ascii() {
                return decode_encoded_bytes(input, offset, decoded_total, limits, output.into_bytes(), special);
            }
            output.push(char::from(byte));
            index = advance(special, 3);
        }
    }
    add_decoded(decoded_total, output.len(), offset, limits)?;
    Ok(Cow::Owned(output))
}

fn decode_encoded_bytes<'q>(
    input: &'q str,
    offset: usize,
    decoded_total: &mut usize,
    limits: QueryLimits,
    mut output: Vec<u8>,
    mut index: usize,
) -> Result<Cow<'q, str>, Error> {
    let bytes = input.as_bytes();
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index = advance(index, 1);
            }
            b'%' => {
                let Some((&high, rest)) = bytes.get(advance(index, 1)).zip(bytes.get(advance(index, 2)..)) else {
                    return Err(Error::parsing(None, offset + index, ErrorKind::InvalidEncoding));
                };
                let Some(&low) = rest.first() else {
                    return Err(Error::parsing(None, offset + index, ErrorKind::InvalidEncoding));
                };
                let Some(high) = hex(high) else {
                    return Err(Error::parsing(None, offset + index, ErrorKind::InvalidEncoding));
                };
                let Some(low) = hex(low) else {
                    return Err(Error::parsing(None, offset + index, ErrorKind::InvalidEncoding));
                };
                output.push(high * 16 + low);
                index = advance(index, 3);
            }
            byte => {
                output.push(byte);
                index = advance(index, 1);
            }
        }
    }
    add_decoded(decoded_total, output.len(), offset, limits)?;
    String::from_utf8(output)
        .map(Cow::Owned)
        .map_err(|_error| Error::parsing(None, offset, ErrorKind::InvalidUtf8))
}

#[inline]
// Cursor arithmetic is tested directly; mutating it can only make parser loops
// stop progressing and time out the mutation job.
#[cfg_attr(test, mutants::skip)]
fn advance(index: usize, amount: usize) -> usize {
    index
        .checked_add(amount)
        .expect("parser advances an in-bounds cursor by syntax widths bounded by the scanned input")
}

#[inline]
fn unencoded_pair_decoded_len(raw_pair: &str, equals: Option<usize>) -> usize {
    raw_pair.len() - usize::from(equals.is_some())
}

fn add_decoded(total: &mut usize, amount: usize, offset: usize, limits: QueryLimits) -> Result<(), Error> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| Error::parsing(None, offset, ErrorKind::DecodedTooLong))?;
    if *total > limits.max_decoded_length {
        return Err(Error::parsing(None, offset, ErrorKind::DecodedTooLong));
    }
    Ok(())
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_length_overflow_is_reported() {
        let mut total = usize::MAX;
        let error = add_decoded(&mut total, 1, 4, QueryLimits::UNLIMITED).expect_err("length overflow");
        assert_eq!(error.kind(), ErrorKind::DecodedTooLong);
        assert_eq!(error.pair_offset(), Some(4));

        let limits = QueryLimits {
            max_decoded_length: 1,
            ..QueryLimits::UNLIMITED
        };
        let mut total = 0;
        add_decoded(&mut total, 1, 5, limits).expect("exact decoded limit is accepted");
        assert_eq!(total, 1);
    }

    #[test]
    fn scalar_fused_pair_accounting_decodes_keys_and_excludes_equals() {
        assert_eq!(unencoded_pair_decoded_len("a=b", Some(1)), 2);

        let mut parser = Parser::<false>::new("%61=b", QueryLimits::UNLIMITED).expect("query is within limits");
        let pair = parser.next_pair_scalar().expect("encoded key is valid").expect("pair is present");
        assert_eq!(pair.key, "a");
        assert_eq!(pair.value, "b");
    }

    #[test]
    fn cursor_advancement_is_exact() {
        assert_eq!(advance(2, 3), 5);
    }

    #[test]
    fn hexadecimal_digits_accept_both_cases() {
        assert_eq!(hex(b'0'), Some(0));
        assert_eq!(hex(b'a'), Some(10));
        assert_eq!(hex(b'F'), Some(15));
        assert_eq!(hex(b'g'), None);
    }

    #[test]
    fn scalar_pair_scan_finds_structure_and_encoding() {
        let scan = scan_pair(b"plain=value&next=pair");
        assert_eq!(scan.separator, Some(11));
        assert_eq!(scan.equals, Some(5));
        assert!(!scan.key_encoded);
        assert!(!scan.value_encoded);

        let scan = scan_pair(b"encoded%20key=value+text");
        assert_eq!(scan.separator, None);
        assert_eq!(scan.equals, Some(13));
        assert!(scan.key_encoded);
        assert!(scan.value_encoded);

        let scan = scan_pair(b"key=one=two");
        assert_eq!(scan.equals, Some(3));
        assert!(!scan.key_encoded);
        assert!(!scan.value_encoded);
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn scalar_and_wide_pair_parsers_agree() {
        for input in [
            "",
            "a=b",
            "a+b=c%20d",
            "a=%E2%9C%93",
            "&&a=b&&",
            "first=abcdefghijklmnopqrstuvwxyz&tail=%2F",
            "key=one=two",
            "key",
            "bad=%",
            "%GG=value",
            "a=b&c=d",
        ] {
            let mut scalar = Parser::<false>::new(input, QueryLimits::DEFAULT).expect("query is within limits");
            let mut wide = Parser::<true>::new(input, QueryLimits::DEFAULT).expect("query is within limits");
            loop {
                match (scalar.next_pair(), wide.next_pair()) {
                    (Ok(Some(scalar)), Ok(Some(wide))) => {
                        assert_eq!(scalar.key, wide.key, "{input}");
                        assert_eq!(scalar.value, wide.value, "{input}");
                        assert_eq!(scalar.offset, wide.offset, "{input}");
                    }
                    (Ok(None), Ok(None)) => break,
                    (Err(scalar), Err(wide)) => {
                        assert_eq!(scalar, wide, "{input}");
                        break;
                    }
                    (scalar, wide) => panic!("scalar/wide mismatch for {input:?}: {scalar:?} != {wide:?}"),
                }
            }
        }
    }

    #[test]
    fn wide_parser_handles_empty_key_only_and_limited_pairs() {
        let input = "&&abcdefghijklmnopqrstuvwxyz&&key-only";
        let mut parser = Parser::<true>::new(input, QueryLimits::UNLIMITED).expect("query is within limits");
        let pair = parser.next_pair().expect("pair parses").expect("pair exists");
        assert_eq!(pair.key, "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(pair.value, "");
        let pair = parser.next_pair().expect("pair parses").expect("pair exists");
        assert_eq!(pair.key, "key-only");
        assert_eq!(pair.value, "");

        let limits = QueryLimits {
            max_pairs: 0,
            ..QueryLimits::UNLIMITED
        };
        let mut parser = Parser::<true>::new("abcdefghijklmnopqrstuvwxyz=value", limits).expect("query is within limits");
        assert_eq!(
            parser.next_pair().expect_err("pair exceeds the limit").kind(),
            ErrorKind::TooManyPairs
        );
    }

    #[test]
    fn non_ascii_decoding_covers_continuation_inputs_and_errors() {
        let decode = |input, limits| {
            let mut decoded = 0;
            decode_known(input, 10, &mut decoded, limits, true)
        };

        assert_eq!(
            decode("%E2%9C%93+x", QueryLimits::UNLIMITED)
                .expect("encoded UTF-8 parses")
                .as_ref(),
            "✓ x"
        );
        for malformed in ["%E2%", "%E2%A", "%E2%G0", "%E2%0G"] {
            let error = decode(malformed, QueryLimits::UNLIMITED).expect_err("malformed continuation is rejected");
            assert_eq!(error.kind(), ErrorKind::InvalidEncoding);
            assert_eq!(error.pair_offset(), Some(13));
        }
        assert_eq!(
            decode("%FF", QueryLimits::UNLIMITED).expect_err("invalid UTF-8 is rejected").kind(),
            ErrorKind::InvalidUtf8
        );
        assert_eq!(
            decode(
                "%E2%9C%93",
                QueryLimits {
                    max_decoded_length: 0,
                    ..QueryLimits::UNLIMITED
                }
            )
            .expect_err("decoded value exceeds the limit")
            .kind(),
            ErrorKind::DecodedTooLong
        );
    }

    #[test]
    fn pair_and_decoded_limits_accept_the_exact_boundary() {
        let limits = QueryLimits {
            max_pairs: 1,
            max_decoded_length: 2,
            ..QueryLimits::UNLIMITED
        };
        let mut parser = Parser::<false>::new("a=b", limits).expect("query is within limits");
        assert!(parser.next_pair().expect("first pair parses").is_some());
        assert!(parser.next_pair().expect("query ends").is_none());

        let mut parser = Parser::<false>::new("a=b&c=d", limits).expect("query is within limits");
        assert!(parser.next_pair().expect("first pair parses").is_some());
        let error = parser.next_pair().expect_err("second pair exceeds the limit");
        assert_eq!(error.kind(), ErrorKind::TooManyPairs);
        assert_eq!(error.pair_offset(), Some(4));
    }

    #[test]
    fn decoding_reports_exact_escape_offsets() {
        let mut decoded = 0;
        let value = decode_known("x%41", 10, &mut decoded, QueryLimits::UNLIMITED, true).expect("escape decodes");
        assert_eq!(value.as_ref(), "xA");

        for malformed in ["x%", "x%A", "x%AG", "x%GG"] {
            let mut decoded = 0;
            let error = decode_known(malformed, 10, &mut decoded, QueryLimits::UNLIMITED, true).expect_err("malformed escape is rejected");
            assert_eq!(error.kind(), ErrorKind::InvalidEncoding);
            assert_eq!(error.pair_offset(), Some(11));
        }

        let mut parser = Parser::<false>::new("aa=%", QueryLimits::UNLIMITED).expect("query starts");
        let error = parser.next_pair().expect_err("value escape is malformed");
        assert_eq!(error.pair_offset(), Some(3));
    }
}
