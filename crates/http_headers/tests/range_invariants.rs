// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field-byte validation and constructor/parser identity for range headers.

#![cfg(feature = "headers-range")]
#![expect(clippy::unwrap_used, reason = "test fixture and round-trip failures provide sufficient context")]

use std::str;

use http_headers::headers::{ByteContentRange, ByteRangeSpec, CompleteLength, ContentRange, ContentRangeOwned, RangeOwned};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, FieldValueRef, InvalidFieldValue, SingleValueField};

struct Source<'a> {
    values: [FieldValueRef<'a>; 1],
    borrowed: bool,
}

impl FieldSource for Source<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        if self.borrowed {
            FieldLines::from_borrowed(name, &self.values)
        } else {
            Some(FieldLines::single(name, self.values[0].as_bytes()))
        }
    }
}

fn assert_forbidden_field_bytes_are_rejected(wire: &[u8]) {
    assert_eq!(FieldValue::from_bytes(wire).unwrap_err(), InvalidFieldValue);
    let strict = <ContentRange as SingleValueField>::decode_view(FieldValueRef::new(wire)).unwrap_err();
    assert_eq!(strict.header(), &FieldName::ContentRange);
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        let direct = <ContentRange as SingleValueField>::decode_view_with(FieldValueRef::new(wire), mode).unwrap_err();
        assert_eq!(direct.header(), &FieldName::ContentRange);
        if mode == DecodeMode::Strict {
            assert_eq!(direct, strict);
        } else {
            assert_eq!(direct.kind(), DecodeErrorKind::InvalidSyntax);
        }
        for borrowed in [false, true] {
            let source = Source {
                values: [FieldValueRef::new(wire)],
                borrowed,
            };
            let view = <ContentRange as Field>::view_with(&source, mode).unwrap_err();
            let owned = <ContentRange as Field>::owned_with(&source, mode).unwrap_err();
            assert_eq!(view.kind(), DecodeErrorKind::InvalidSyntax);
            assert_eq!(view.header(), &FieldName::ContentRange);
            assert_eq!(owned, view);
        }
    }
    let wire = str::from_utf8(wire).unwrap();
    assert_eq!(
        ContentRangeOwned::try_from(wire).unwrap_err().kind(),
        DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        ContentRangeOwned::try_from(wire.to_owned()).unwrap_err().kind(),
        DecodeErrorKind::InvalidSyntax
    );
}

fn assert_content_range_decoding(wire: &str, mode: DecodeMode, expected: ByteContentRange) {
    let field = FieldValue::from_str(wire).unwrap();
    let direct = <ContentRange as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap();
    assert_eq!(direct.byte_range(), Some(expected));
    assert_eq!(direct.extension_payload(), None);
    assert_eq!(direct.as_field_value().as_bytes(), wire.as_bytes());
    let owned = <ContentRange as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap();
    assert_eq!(owned.byte_range(), Some(expected));
    assert_eq!(owned.extension_payload(), None);
    assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
    if mode == DecodeMode::Strict {
        let view = <ContentRange as SingleValueField>::decode_view(field.as_field_value_ref()).unwrap();
        assert_eq!(view.byte_range(), direct.byte_range());
        assert_eq!(<ContentRange as SingleValueField>::decode_owned(field).unwrap(), owned);
    }
    for borrowed in [false, true] {
        let source = Source {
            values: [FieldValueRef::new(wire.as_bytes())],
            borrowed,
        };
        let view = <ContentRange as Field>::view_with(&source, mode).unwrap().unwrap();
        assert_eq!(view.byte_range(), Some(expected));
        assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        assert_eq!(<ContentRange as Field>::owned_with(&source, mode).unwrap().unwrap(), owned);
        if mode == DecodeMode::Strict {
            assert_eq!(
                <ContentRange as Field>::view(&source).unwrap().unwrap().byte_range(),
                Some(expected)
            );
            assert_eq!(<ContentRange as Field>::owned(&source).unwrap().unwrap(), owned);
        }
    }
}

#[test]
fn control_bytes_at_every_content_range_position_are_rejected_by_all_entry_points() {
    for base in [b"bytes 0-1/2".as_slice(), b"bytes 0-1/*", b"bytes */2", b"items opaque"] {
        for byte in (0_u8..=0x1f).filter(|byte| *byte != b'\t').chain([0x7f]) {
            for position in 0..=base.len() {
                let mut wire = base.to_vec();
                wire.insert(position, byte);
                assert_forbidden_field_bytes_are_rejected(&wire);
            }
        }
    }
}

#[test]
fn relaxed_content_range_delimiters_accept_only_sp_and_htab_without_changing_wire() {
    let known = ByteContentRange::Satisfied {
        first: 0,
        last: 1,
        complete_length: Some(2),
    };
    let unknown = ByteContentRange::Satisfied {
        first: 0,
        last: 1,
        complete_length: None,
    };
    let unsatisfied = ByteContentRange::Unsatisfied { complete_length: 2 };
    for (pattern, expected) in [
        ("bytes 0|-1/2", known),
        ("bytes 0-|1/2", known),
        ("bytes 0-1|/2", known),
        ("bytes 0-1/|2", known),
        ("bytes 0|-|1|/|2", known),
        ("Bytes 0|-|1|/|*", unknown),
        ("bytes *|/|2", unsatisfied),
    ] {
        for whitespace in [" ", "\t", " \t", "\t "] {
            let wire = pattern.replace('|', whitespace);
            assert_content_range_decoding(&wire, DecodeMode::Relaxed, expected);
            let value = FieldValue::from_str(&wire).unwrap();
            <ContentRange as SingleValueField>::decode_view(value.as_field_value_ref()).unwrap_err();
            <ContentRange as SingleValueField>::decode_view_with(value.as_field_value_ref(), DecodeMode::Strict).unwrap_err();
            <ContentRange as SingleValueField>::decode_owned(value.clone()).unwrap_err();
            <ContentRange as SingleValueField>::decode_owned_with(value, DecodeMode::Strict).unwrap_err();
            for borrowed in [false, true] {
                let source = Source {
                    values: [FieldValueRef::new(wire.as_bytes())],
                    borrowed,
                };
                <ContentRange as Field>::view(&source).unwrap_err();
                <ContentRange as Field>::owned(&source).unwrap_err();
            }
        }
    }
    for (wire, expected) in [("bytes 0-1/2", known), ("bytes 0-1/*", unknown), ("bytes */2", unsatisfied)] {
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            assert_content_range_decoding(wire, mode, expected);
        }
    }
}

#[test]
fn relaxed_content_ranges_still_reject_outer_or_missing_component_whitespace() {
    for wire in [
        " bytes 0-1/2",
        "bytes\t0-1/2",
        "bytes  0-1/2",
        "bytes \t0-1/2",
        "bytes 0-1/2 ",
        "bytes 0-1/2\t",
        "bytes 0-1/* ",
        "bytes 0-1/ \t",
        "bytes -1 /2",
        "bytes 0- \t/2",
    ] {
        let field = FieldValue::from_str(wire).unwrap();
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            <ContentRange as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap_err();
            <ContentRange as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap_err();
            let source = Source {
                values: [field.as_field_value_ref()],
                borrowed: false,
            };
            <ContentRange as Field>::view_with(&source, mode).unwrap_err();
            <ContentRange as Field>::owned_with(&source, mode).unwrap_err();
        }
    }
}

fn assert_range_round_trip(value: &RangeOwned) {
    let reparsed = RangeOwned::try_from(value.clone().into_field_value()).unwrap();
    assert_eq!(&reparsed, value);
    assert_eq!(reparsed.is_bytes(), value.is_bytes());
    assert_eq!(reparsed.extension_range_set(), value.extension_range_set());
    #[cfg(feature = "serde")]
    {
        let json = serde_json::to_string(value).unwrap();
        let decoded: RangeOwned = serde_json::from_str(&json).unwrap();
        assert_eq!(&decoded, value);
        assert_eq!(decoded.is_bytes(), value.is_bytes());
        assert_eq!(decoded.as_field_value().as_bytes(), value.as_field_value().as_bytes());
    }
}

fn assert_content_range_round_trip(value: &ContentRangeOwned) {
    let reparsed = ContentRangeOwned::try_from(value.clone().into_field_value()).unwrap();
    assert_eq!(&reparsed, value);
    assert_eq!(reparsed.byte_range(), value.byte_range());
    assert_eq!(reparsed.extension_payload(), value.extension_payload());
    #[cfg(feature = "serde")]
    {
        let json = serde_json::to_string(value).unwrap();
        let decoded: ContentRangeOwned = serde_json::from_str(&json).unwrap();
        assert_eq!(&decoded, value);
        assert_eq!(decoded.byte_range(), value.byte_range());
        assert_eq!(decoded.as_field_value().as_bytes(), value.as_field_value().as_bytes());
    }
}

#[test]
fn extension_constructors_reject_every_case_of_the_reserved_bytes_unit() {
    for mask in 0..32 {
        let unit: String = b"bytes"
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                char::from(if mask & (1 << index) == 0 {
                    *byte
                } else {
                    byte.to_ascii_uppercase()
                })
            })
            .collect();
        for payload in ["0-1", "opaque"] {
            let error = RangeOwned::extension(&unit, payload).unwrap_err();
            assert_eq!(error.header(), &FieldName::Range);
            assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        }
        for payload in ["0-1/2", "*/2", "opaque"] {
            let error = ContentRangeOwned::extension(&unit, payload).unwrap_err();
            assert_eq!(error.header(), &FieldName::ContentRange);
            assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        }
        let range_wire = format!("{unit}=0-1");
        let range = RangeOwned::try_from(range_wire.as_str()).unwrap();
        assert_eq!(range.unit().unwrap(), unit);
        assert!(range.is_bytes());
        assert_eq!(
            range.byte_ranges().unwrap().collect::<Vec<_>>(),
            [ByteRangeSpec::FromTo { first: 0, last: 1 }]
        );
        assert_eq!(range.as_field_value().as_bytes(), range_wire.as_bytes());
        assert_range_round_trip(&range);
        let content_wire = format!("{unit} 0-1/2");
        let content = ContentRangeOwned::try_from(content_wire.as_str()).unwrap();
        assert_eq!(content.unit().unwrap(), unit);
        assert_eq!(content.as_field_value().as_bytes(), content_wire.as_bytes());
        assert_eq!(
            content.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: Some(2)
            })
        );
        assert_content_range_round_trip(&content);
    }
}

#[test]
fn byte_specific_constructors_keep_canonical_wire_and_semantics() {
    let range = RangeOwned::bytes([
        ByteRangeSpec::from_to(0, 1).unwrap(),
        ByteRangeSpec::starting_at(3),
        ByteRangeSpec::suffix(2),
    ])
    .unwrap();
    assert_eq!(range.as_field_value().as_bytes(), b"bytes=0-1, 3-, -2");
    assert!(range.is_bytes());
    assert_eq!(range.extension_range_set(), None);
    assert_range_round_trip(&range);
    for (content, wire, expected) in [
        (
            ContentRangeOwned::bytes(0, 1, CompleteLength::Known(2)).unwrap(),
            "bytes 0-1/2",
            ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: Some(2),
            },
        ),
        (
            ContentRangeOwned::bytes_range(0..2, CompleteLength::Unknown).unwrap(),
            "bytes 0-1/*",
            ByteContentRange::Satisfied {
                first: 0,
                last: 1,
                complete_length: None,
            },
        ),
        (
            ContentRangeOwned::unsatisfied_bytes(2).unwrap(),
            "bytes */2",
            ByteContentRange::Unsatisfied { complete_length: 2 },
        ),
    ] {
        assert_eq!(content.as_field_value().as_bytes(), wire.as_bytes());
        assert_eq!(content.byte_range(), Some(expected));
        assert_eq!(content.extension_payload(), None);
        assert_content_range_round_trip(&content);
    }
}

#[test]
fn nonreserved_extension_units_preserve_spelling_and_opaque_semantics() {
    for unit in ["items", "Items", "bytesx", "xbytes", "ByTeS-x"] {
        let range = RangeOwned::extension(unit, "opaque").unwrap();
        assert_eq!(range.unit().unwrap(), unit);
        assert!(!range.is_bytes());
        assert!(range.byte_ranges().is_none());
        assert_eq!(range.extension_range_set(), Some(b"opaque".as_slice()));
        assert_eq!(range.as_field_value().as_bytes(), format!("{unit}=opaque").as_bytes());
        assert_range_round_trip(&range);
        let content = ContentRangeOwned::extension(unit, "\topaque / payload\t").unwrap();
        assert_eq!(content.unit().unwrap(), unit);
        assert_eq!(content.byte_range(), None);
        assert_eq!(content.extension_payload(), Some(b"\topaque / payload\t".as_slice()));
        assert_eq!(
            content.as_field_value().as_bytes(),
            format!("{unit} \topaque / payload\t").as_bytes()
        );
        assert_content_range_round_trip(&content);
    }
}
