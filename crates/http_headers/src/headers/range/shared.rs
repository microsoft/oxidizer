// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{DecodeError, DecodeErrorKind, FieldName, validate};

pub(super) fn validate_range_unit_for(bytes: &[u8], name: &'static FieldName) -> Result<(), DecodeError> {
    if validate::token(bytes) {
        Ok(())
    } else {
        Err(DecodeError::new(name, DecodeErrorKind::InvalidToken))
    }
}

#[inline]
pub(super) fn parse_number(bytes: &[u8], name: &'static FieldName) -> Result<u64, DecodeError> {
    validate::decimal_u64(bytes).ok_or_else(|| DecodeError::new(name, DecodeErrorKind::InvalidNumber))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::{accept_ranges, range};

    // These tests exercise private range and unit scanner implementations.

    #[test]
    fn fast_scanners_agree_with_the_general_implementations() {
        const ALPHABET: &[u8] = b"01-, a=\t9";
        const RANGE_ALPHABET: &[u8] = b"01-, ";

        let mut payload = Vec::new();
        for length in 0..=4_u32 {
            for encoded in 0..ALPHABET.len().pow(length) {
                payload.clear();
                let mut encoded = encoded;
                for _position in 0..length {
                    payload.push(ALPHABET[encoded % ALPHABET.len()]);
                    encoded /= ALPHABET.len();
                }

                assert_eq!(
                    range::validate_byte_range_set(&payload, 0).map_err(|error| error.kind()),
                    range::validate_byte_range_set_slow(&payload).map_err(|error| error.kind()),
                    "byte range set {:?}",
                    String::from_utf8_lossy(&payload)
                );

                let fast = accept_ranges::scan_units(&payload).map(|(units, none)| {
                    accept_ranges::validate_none_cardinality(units, none)
                        .map(|()| none)
                        .map_err(|error| error.kind())
                });
                if let Some(fast) = fast {
                    assert_eq!(
                        fast,
                        accept_ranges::validate_units_slow(&payload).map_err(|error| error.kind()),
                        "unit list {:?}",
                        String::from_utf8_lossy(&payload)
                    );
                }

                let mut wire = b"bytes=".to_vec();
                wire.extend_from_slice(&payload);
                for wire in [payload.as_slice(), wire.as_slice()] {
                    let fast = range::parse_range(wire)
                        .map(|value| (value.unit.as_bytes(), value.payload, value.bytes))
                        .map_err(|error| error.kind());
                    let slow = range::parse_range_slow(wire)
                        .map(|value| (value.unit.as_bytes(), value.payload, value.bytes))
                        .map_err(|error| error.kind());
                    assert_eq!(fast, slow, "range {:?}", String::from_utf8_lossy(wire));
                }
            }
        }

        // Longer payloads exercise the multi-item paths of the word scanner,
        // which the short exhaustive sweep above never reaches.
        for length in 5..=7_u32 {
            for encoded in 0..RANGE_ALPHABET.len().pow(length) {
                payload.clear();
                payload.extend_from_slice(b"bytes=");
                let mut encoded = encoded;
                for _position in 0..length {
                    payload.push(RANGE_ALPHABET[encoded % RANGE_ALPHABET.len()]);
                    encoded /= RANGE_ALPHABET.len();
                }

                let fast = range::parse_range(&payload)
                    .map(|value| (value.unit.as_bytes(), value.payload, value.bytes))
                    .map_err(|error| error.kind());
                let slow = range::parse_range_slow(&payload)
                    .map(|value| (value.unit.as_bytes(), value.payload, value.bytes))
                    .map_err(|error| error.kind());
                assert_eq!(fast, slow, "range {:?}", String::from_utf8_lossy(&payload));

                // The accelerated scanner is only ever allowed to settle a set
                // the general implementation also accepts.
                if http_headers_simd::scan_byte_range_set(&payload, 6) {
                    assert!(
                        range::validate_byte_range_set_slow(&payload[6..]).is_ok(),
                        "accelerated scanner accepted {:?}",
                        String::from_utf8_lossy(&payload)
                    );
                }
            }
        }

        for payload in [
            "9999999999999999999-",
            "18446744073709551615-",
            "18446744073709551616-",
            "0-18446744073709551615",
            "0-18446744073709551616",
            "00000000000000000000005-6",
            "-18446744073709551616",
            "0-1, 2-3 ,4-",
        ] {
            assert_eq!(
                range::validate_byte_range_set(payload.as_bytes(), 0).map_err(|error| error.kind()),
                range::validate_byte_range_set_slow(payload.as_bytes()).map_err(|error| error.kind()),
                "byte range set {payload:?}"
            );
        }
    }
}
