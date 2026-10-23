// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Semantic negotiation coverage independent of Content-Type and HTTP adapters.

#![cfg(feature = "headers-negotiation")]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]
#![expect(clippy::assertions_on_result_states, reason = "invalid-input tables assert rejection")]

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;

use http_headers::headers::{
    Accept, AcceptEncoding, AcceptEncodingEntry, AcceptEncodingOwned, AcceptEntry, AcceptLanguage, AcceptLanguageEntry,
    AcceptLanguageOwned, AcceptOwned, ContentCoding, ContentCodingKind, LanguageRange, MediaRange, MediaRangeKind, NegotiationParameter,
    NegotiationParameterValue, NegotiationToken, Quality, QualityView,
};
use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldSensitivity, FieldValue, FieldValueRef};

struct Source<'a> {
    name: &'static FieldName,
    values: Vec<FieldValueRef<'a>>,
}

impl<'a> Source<'a> {
    fn new(name: &'static FieldName, values: impl IntoIterator<Item = &'a [u8]>) -> Self {
        Self {
            name,
            values: values.into_iter().map(FieldValueRef::new).collect(),
        }
    }
}

impl FieldSource for Source<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (self.name == name).then(|| FieldLines::from_borrowed(name, &self.values)).flatten()
    }
}

struct SingleSource<'a> {
    name: &'static FieldName,
    bytes: &'a [u8],
}

impl FieldSource for SingleSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::single(name, self.bytes))
    }
}

#[derive(Default)]
struct Stored(Vec<FieldValue>);

impl FieldSource for Stored {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_slice(name, &self.0)
    }
}

impl FieldSink for Stored {
    fn set_values(&mut self, _: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.0 = values.into_iter().collect();
        Ok(())
    }

    fn append_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.0.extend(values);
        Ok(())
    }

    fn remove_values(&mut self, _name: &'static FieldName) {
        self.0.clear();
    }
}

fn hash(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn quality(value: &[u8]) -> QualityView<'_> {
    QualityView::parse(value, DecodeMode::Relaxed).unwrap()
}

#[test]
fn quality_expected_values_and_exact_conversion() {
    for (wire, thousandths, canonical) in [
        ("0", 0, "0"),
        ("0.", 0, "0"),
        ("0.000", 0, "0"),
        ("0.001", 1, "0.001"),
        ("0.010", 10, "0.01"),
        ("0.100", 100, "0.1"),
        ("0.501", 501, "0.501"),
        ("1", 1000, "1"),
        ("1.", 1000, "1"),
        ("1.000", 1000, "1"),
    ] {
        let parsed = QualityView::parse(wire.as_bytes(), DecodeMode::Strict).unwrap();
        let compact = Quality::from_thousandths(thousandths).unwrap();
        assert_eq!(parsed.to_quality().unwrap(), compact);
        assert_eq!(Quality::try_from(parsed).unwrap().thousandths(), thousandths);
        assert_eq!(parsed.to_string(), canonical);
        assert_eq!(compact.to_string(), canonical);
        assert_eq!(parsed.is_zero(), thousandths == 0);
        assert_eq!(parsed.is_one(), thousandths == 1000);
    }
    assert_eq!(quality(b".5").to_quality().unwrap().thousandths(), 500);
    assert_eq!(quality(b" \t.50000\t ").to_quality().unwrap().thousandths(), 500);
    assert!(quality(b"0.0001").to_quality().is_err());
    assert_eq!(quality(b"0.0001").to_string(), "0.0001");
    assert!(Quality::from_thousandths(1001).is_err());
    assert!(Quality::from_thousandths(u16::MAX).is_err());
}

#[test]
fn quality_errors_have_distinct_messages_and_no_sources() {
    let invalid = Quality::from_thousandths(1001).unwrap_err();
    assert_eq!(invalid.to_string(), "quality must be an accepted decimal between zero and one");
    assert!(invalid.source().is_none());
    let inexact = quality(b"0.0001").to_quality().unwrap_err();
    assert_eq!(inexact.to_string(), "quality is not an exact number of thousandths");
    assert!(inexact.source().is_none());
}

#[test]
fn typed_encoding_entries_serialize_zero_one_and_fractional_weights() {
    for (wire, expected) in [
        (b"0".as_slice(), b"gzip;q=0".as_slice()),
        (b"1", b"gzip;q=1"),
        (b"0.001", b"gzip;q=0.001"),
        (b"0.010", b"gzip;q=0.01"),
        (b"0.100", b"gzip;q=0.1"),
        (b".0001000", b"gzip;q=0.0001"),
    ] {
        let entry = AcceptEncodingEntry::new(ContentCoding::parse("gzip").unwrap(), Some(quality(wire))).unwrap();
        let header = AcceptEncodingOwned::from_entries([entry]).unwrap();
        assert_eq!(header.values().next().unwrap().as_bytes(), expected);
        assert_eq!(header.entries().next().unwrap(), entry);
    }
}

#[test]
fn decimal_equality_ordering_and_hashing_are_exact() {
    let equal = [
        quality(b"0.5"),
        quality(b".50"),
        quality(b"0.5000"),
        Quality::from_thousandths(500).unwrap().into(),
    ];
    assert_eq!(equal.into_iter().collect::<HashSet<_>>().len(), 1);
    assert_eq!(equal.into_iter().collect::<BTreeSet<_>>().len(), 1);
    assert!(equal.windows(2).all(|pair| hash(pair[0]) == hash(pair[1])));
    assert_eq!(quality(b".0001000"), quality(b"0.0001"));
    assert_eq!(hash(quality(b".0001000")), hash(quality(b"0.0001")));
    assert_eq!(quality(b"1.00000000"), QualityView::ONE);
    assert_eq!(quality(b".00000000"), QualityView::ZERO);
    let ascending = [
        quality(b"0"),
        quality(b".00000000000000000000001"),
        quality(b".0001"),
        quality(b".00010000001"),
        quality(b".001"),
        quality(b".00100000001"),
        quality(b"0.49999999999999999999999999999999999"),
        quality(b".5"),
        quality(b"0.50000000000000000000000000000000001"),
        quality(b"0.99999999999999999999999999999999999"),
        quality(b"1"),
    ];
    for pair in ascending.windows(2) {
        assert!(pair[0] < pair[1]);
        assert!(pair[1] > pair[0]);
    }
    let mut long = b"0.".to_vec();
    long.extend(iter::repeat_n(b'0', 4096));
    long.push(b'1');
    let mut larger = long.clone();
    *larger.last_mut().unwrap() = b'2';
    assert!(quality(&long) > QualityView::ZERO);
    assert!(quality(&long) < quality(&larger));
    assert!(quality(&long).to_quality().is_err());
    assert_eq!(quality(&long).to_string().as_bytes(), long);
}

#[test]
fn quality_rejection_preserves_strict_and_relaxed_grammars() {
    for invalid in [
        "",
        ".",
        "00",
        "01",
        "2",
        "-0",
        "+0.5",
        "NaN",
        "0.1e0",
        "1.001",
        "1.0000001",
        "\"0.5\"",
        "0..5",
        ".5x",
    ] {
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            assert!(QualityView::parse(invalid.as_bytes(), mode).is_err(), "{invalid:?}");
        }
    }
    for relaxed_only in [".5", "0.0001", "0.5000", "1.0000", " 0.5 "] {
        assert!(QualityView::parse(relaxed_only.as_bytes(), DecodeMode::Strict).is_err());
        assert!(QualityView::parse(relaxed_only.as_bytes(), DecodeMode::Relaxed).is_ok());
    }
}

#[test]
fn media_ranges_and_codings_have_case_insensitive_semantics() {
    for (wire, kind) in [
        ("*/*", MediaRangeKind::Any),
        ("TEXT/*", MediaRangeKind::TypeWildcard),
        ("Text/HTML", MediaRangeKind::Exact),
        ("te*xt/ht*ml", MediaRangeKind::Exact),
        ("**/*", MediaRangeKind::TypeWildcard),
    ] {
        let range = MediaRange::parse(wire).unwrap();
        assert_eq!(range.kind(), kind);
        assert_eq!(range.to_string(), wire);
    }
    assert_eq!(MediaRange::parse("Text/HTML").unwrap(), MediaRange::parse("text/html").unwrap());
    assert_eq!(
        hash(MediaRange::parse("Text/HTML").unwrap()),
        hash(MediaRange::parse("text/html").unwrap())
    );
    for invalid in ["*/html", "text", "text/html/extra", "text/ html", "/html", "text/"] {
        assert!(MediaRange::parse(invalid).is_err());
    }
    assert!(MediaRange::new(NegotiationToken::new("*").unwrap(), NegotiationToken::new("html").unwrap()).is_err());
    for (wire, kind) in [
        ("*", ContentCodingKind::Wildcard),
        ("IDENTITY", ContentCodingKind::Identity),
        ("GZip", ContentCodingKind::Gzip),
        ("compress", ContentCodingKind::Compress),
        ("deflate", ContentCodingKind::Deflate),
        ("br", ContentCodingKind::Br),
        ("zstd", ContentCodingKind::Zstd),
        ("dcb", ContentCodingKind::Dcb),
        ("dcz", ContentCodingKind::Dcz),
        ("x-private", ContentCodingKind::Extension),
        ("g*zip", ContentCodingKind::Extension),
    ] {
        let coding = ContentCoding::parse(wire).unwrap();
        assert_eq!(coding.kind(), kind);
        assert_eq!(coding.as_str(), wire);
        assert_eq!(coding.to_string(), wire);
        assert!(coding.token().eq_ignore_ascii_case(wire));
    }
    assert_eq!(ContentCoding::parse("GZIP").unwrap(), ContentCoding::parse("gzip").unwrap());
    assert_eq!(
        hash(ContentCoding::parse("GZIP").unwrap()),
        hash(ContentCoding::parse("gzip").unwrap())
    );
    assert!(ContentCoding::parse("g zip").is_err());
    assert!(NegotiationToken::new("").is_err());
    assert!(NegotiationToken::new("ümlaut").is_err());
}

#[test]
fn negotiation_token_order_is_ascii_case_insensitive_and_prefix_aware() {
    for (left, right, expected) in [
        ("GZip", "gzip", Ordering::Equal),
        ("BR", "gzip", Ordering::Less),
        ("gzip", "BR", Ordering::Greater),
        ("A", "aa", Ordering::Less),
        ("aa", "A", Ordering::Greater),
        ("gZiP", "GZIQ", Ordering::Less),
        ("Z", "a", Ordering::Greater),
    ] {
        let left = NegotiationToken::new(left).unwrap();
        let right = NegotiationToken::new(right).unwrap();
        assert_eq!(left.cmp(&right), expected);
        assert_eq!(left.partial_cmp(&right), Some(expected));
        assert_eq!(right.cmp(&left), expected.reverse());
        assert_eq!(left == right, expected == Ordering::Equal);
    }
}

#[test]
fn basic_language_ranges_retain_primary_and_subtags() {
    for (wire, primary, rest) in [
        ("*", None, vec![]),
        ("e", Some("e"), vec![]),
        ("abcdefgh", Some("abcdefgh"), vec![]),
        ("ZH-Hans-CN-12345678", Some("ZH"), vec!["Hans", "CN", "12345678"]),
        ("abcdefgh-1-a", Some("abcdefgh"), vec!["1", "a"]),
    ] {
        let range = LanguageRange::parse(wire).unwrap();
        assert_eq!(range.as_str(), wire);
        assert_eq!(range.to_string(), wire);
        assert_eq!(range.primary().map(NegotiationToken::as_str), primary);
        assert_eq!(range.subtags().map(NegotiationToken::as_str).collect::<Vec<_>>(), rest);
        assert_eq!(range.is_wildcard(), primary.is_none());
    }
    for invalid in [
        "",
        "abcdefghi",
        "1en",
        "en-123456789",
        "en-",
        "-en",
        "en--US",
        "en_US",
        "en-*",
        "*-en",
        "é",
    ] {
        assert!(LanguageRange::parse(invalid).is_err(), "{invalid}");
    }
    let mixed = LanguageRange::parse("eN-uS").unwrap();
    assert!(mixed.eq_ignore_ascii_case("EN-US"));
    assert_eq!(mixed, LanguageRange::parse("en-US").unwrap());
    assert_eq!(hash(mixed), hash(LanguageRange::parse("EN-us").unwrap()));
}

fn check_accept_entries<'a>(mut entries: impl Iterator<Item = AcceptEntry<'a>>) {
    let first = entries.next().unwrap();
    assert_eq!(first.range().type_().as_str(), "TEXT");
    assert_eq!(first.range().subtype().as_str(), "HTML");
    assert_eq!(first.range().kind(), MediaRangeKind::Exact);
    assert_eq!(first.quality().to_quality().unwrap().thousandths(), 500);
    assert_eq!(first.explicit_quality(), Some(quality(b".5")));
    let parameters = first.parameters().collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0].name(), NegotiationToken::new("level").unwrap());
    let value = parameters[0].value().unwrap();
    assert!(value.is_quoted());
    assert_eq!(value.raw_bytes(), b"\"a,b;c\\\"\\\\\xff\"");
    assert_eq!(value.decoded_bytes().collect::<Vec<_>>(), b"a,b;c\"\\\xff");
    assert_eq!(value.to_decoded_bytes(), b"a,b;c\"\\\xff");
    assert_eq!(parameters[1].value().unwrap().raw_bytes(), b"two");
    assert!(!parameters[1].value().unwrap().is_quoted());
    let extensions = first.extensions().collect::<Vec<_>>();
    assert_eq!(extensions.len(), 2);
    assert_eq!(extensions[0].name().as_str(), "Flag");
    assert_eq!(extensions[0].value(), None);
    assert_eq!(extensions[1].name().as_str(), "empty");
    assert_eq!(extensions[1].value().unwrap().to_decoded_bytes(), b"");
    assert!(extensions[1].value().unwrap().is_quoted());
    let wildcard = entries.next().unwrap();
    assert_eq!(wildcard.range().kind(), MediaRangeKind::TypeWildcard);
    assert_eq!(wildcard.quality(), QualityView::ZERO);
    assert_eq!(wildcard.parameters().count(), 0);
    assert_eq!(wildcard.extensions().count(), 0);
    let any = entries.next().unwrap();
    assert_eq!(any.range().kind(), MediaRangeKind::Any);
    assert_eq!(any.quality(), QualityView::ONE);
    assert_eq!(any.explicit_quality(), None);
    let repeated = entries.next().unwrap();
    assert_eq!(repeated.range().type_().as_str(), "text");
    assert_eq!(repeated.quality(), QualityView::ONE);
    assert_eq!(repeated.explicit_quality(), Some(QualityView::ONE));
    assert_eq!(repeated.parameters().count(), 0);
    assert!(entries.next().is_none());
}

#[test]
fn accept_parameters_extensions_quoting_order_and_wire_are_independent() {
    let lines = [
        b" , TEXT/HTML;Level=\"a,b;c\\\"\\\\\xff\";level=two;q=0.5;Flag;empty=\"\", text/*;q=0.000 , */*, ".as_slice(),
        b"text/html;q=1",
    ];
    let source = Source::new(&FieldName::Accept, lines);
    let view = Accept::view(&source).unwrap().unwrap();
    let owned = Accept::owned(&source).unwrap().unwrap();
    for _ in 0..8 {
        check_accept_entries(view.entries());
        check_accept_entries(owned.entries());
    }
    assert_eq!(view.values().map(FieldValueRef::as_bytes).collect::<Vec<_>>(), lines);
    assert_eq!(owned.values().map(FieldValueRef::as_bytes).collect::<Vec<_>>(), lines);
    assert_eq!(view.items().collect::<Vec<_>>(), owned.items().collect::<Vec<_>>());
    let mut sink = Stored::default();
    Accept::insert(&mut sink, owned).unwrap();
    assert_eq!(sink.0.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(), lines);
    check_accept_entries(Accept::view(&sink).unwrap().unwrap().entries());
}

#[test]
fn encoding_and_language_entries_preserve_duplicates_default_and_explicit_quality() {
    let source = Source::new(
        &FieldName::AcceptEncoding,
        [b" , GZIP, gzip;q=1,, identity;q=0, *;q=0.5".as_slice(), b"x-private"],
    );
    let view = AcceptEncoding::view(&source).unwrap().unwrap();
    let owned = AcceptEncoding::owned(&source).unwrap().unwrap();
    let entries = view.entries().collect::<Vec<_>>();
    assert_eq!(entries, owned.entries().collect::<Vec<_>>());
    assert_eq!(
        entries.iter().map(|entry| entry.coding().as_str()).collect::<Vec<_>>(),
        ["GZIP", "gzip", "identity", "*", "x-private"]
    );
    assert_eq!(entries[0].quality(), QualityView::ONE);
    assert_eq!(entries[0].explicit_quality(), None);
    assert_eq!(entries[1].explicit_quality(), Some(QualityView::ONE));
    assert_ne!(entries[0], entries[1]);
    assert!(entries[2].quality().is_zero());
    assert_eq!(entries[2].coding().kind(), ContentCodingKind::Identity);
    assert_eq!(entries[3].coding().kind(), ContentCodingKind::Wildcard);
    assert_eq!(entries[4].coding().kind(), ContentCodingKind::Extension);
    let source = Source::new(
        &FieldName::AcceptLanguage,
        [b"en-US, EN-us;q=1, *;q=0".as_slice(), b"zh-Hans-CN;q=0.125"],
    );
    let view = AcceptLanguage::view(&source).unwrap().unwrap();
    let owned = AcceptLanguage::owned(&source).unwrap().unwrap();
    let entries = view.entries().collect::<Vec<_>>();
    assert_eq!(entries, owned.entries().collect::<Vec<_>>());
    assert_eq!(entries[0].range(), entries[1].range());
    assert_eq!(entries[0].explicit_quality(), None);
    assert_eq!(entries[1].explicit_quality(), Some(QualityView::ONE));
    assert!(entries[2].range().is_wildcard());
    assert!(entries[2].quality().is_zero());
    assert_eq!(entries[3].quality().to_quality().unwrap().thousandths(), 125);
}

#[test]
fn relaxed_precision_is_shared_across_all_three_headers() {
    let source = Source::new(&FieldName::Accept, [b"text/plain;p=x; Q \t= .0001000 ;flag".as_slice()]);
    assert!(Accept::view(&source).is_err());
    let view = Accept::view_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    let owned = Accept::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    assert_eq!(view.entries().next().unwrap().quality(), quality(b".0001"));
    assert_eq!(owned.entries().next().unwrap().quality(), quality(b".0001"));
    assert_eq!(view.entries().next().unwrap().parameters().next().unwrap().name().as_str(), "p");
    assert_eq!(view.entries().next().unwrap().extensions().next().unwrap().name().as_str(), "flag");
    let source = Source::new(&FieldName::AcceptEncoding, [b"br; Q \t= .0001000".as_slice()]);
    assert!(AcceptEncoding::view(&source).is_err());
    let view = AcceptEncoding::view_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    let owned = AcceptEncoding::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    assert_eq!(view.entries().next().unwrap().quality(), quality(b".0001"));
    assert_eq!(view.entries().collect::<Vec<_>>(), owned.entries().collect::<Vec<_>>());
    let source = Source::new(&FieldName::AcceptLanguage, [b"en; Q \t= .0001000".as_slice()]);
    assert!(AcceptLanguage::view(&source).is_err());
    let view = AcceptLanguage::view_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    let owned = AcceptLanguage::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    assert_eq!(view.entries().next().unwrap().quality(), quality(b".0001"));
    assert_eq!(view.entries().collect::<Vec<_>>(), owned.entries().collect::<Vec<_>>());
}

#[test]
fn typed_constructors_validate_cross_component_rules_and_retain_exact_values() {
    let range = MediaRange::parse("Text/HTML").unwrap();
    let parameter = NegotiationParameter::new(
        NegotiationToken::new("Level").unwrap(),
        Some(NegotiationParameterValue::parse(b"\"a\\\"b,\xff\"").unwrap()),
    );
    let flag = NegotiationParameter::new(NegotiationToken::new("preview").unwrap(), None);
    let reserved = NegotiationParameter::new(
        NegotiationToken::new("Q").unwrap(),
        Some(NegotiationParameterValue::parse(b"1").unwrap()),
    );
    assert!(AcceptEntry::new(range, &[flag], None, &[]).is_err());
    assert!(AcceptEntry::new(range, &[reserved], None, &[]).is_err());
    assert!(AcceptEntry::new(range, &[], Some(QualityView::ONE), &[reserved]).is_err());
    assert!(AcceptEntry::new(range, &[], None, &[flag]).is_err());
    let parameters = [parameter, parameter];
    let extensions = [flag];
    let entry = AcceptEntry::new(range, &parameters, Some(quality(b".0001000")), &extensions).unwrap();
    let owned = AcceptOwned::from_entries([entry]).unwrap();
    assert_eq!(
        owned.values().next().unwrap().as_bytes(),
        b"Text/HTML;Level=\"a\\\"b,\xff\";Level=\"a\\\"b,\xff\";q=0.0001;preview"
    );
    let decoded = owned.entries().next().unwrap();
    assert_eq!(decoded.range(), range);
    assert_eq!(decoded.quality(), quality(b".0001"));
    assert_eq!(decoded.parameters().collect::<Vec<_>>(), parameters);
    assert_eq!(decoded.extensions().collect::<Vec<_>>(), extensions);

    let entry = AcceptEncodingEntry::new(ContentCoding::parse("GZIP").unwrap(), Some(quality(b".0001000"))).unwrap();
    let header = AcceptEncodingOwned::from_entries([entry, entry]).unwrap();
    assert_eq!(header.values().next().unwrap().as_bytes(), b"GZIP;q=0.0001, GZIP;q=0.0001");
    assert_eq!(header.entries().collect::<Vec<_>>(), [entry, entry]);
    let entry = AcceptLanguageEntry::new(
        LanguageRange::parse("en-US").unwrap(),
        Some(Quality::from_thousandths(900).unwrap().into()),
    )
    .unwrap();
    let header = AcceptLanguageOwned::from_entries([entry]).unwrap();
    assert_eq!(header.values().next().unwrap().as_bytes(), b"en-US;q=0.9");
    assert_eq!(header.entries().next().unwrap(), entry);
    for invalid in [b"".as_slice(), b"a b", b"\"unclosed", b"\"line\n\"", b"\"trailing\\\"", b"\"a\"b\""] {
        assert!(NegotiationParameterValue::parse(invalid).is_err(), "{invalid:?}");
    }
    assert_eq!(NegotiationParameterValue::parse(b"\"\\\xff\"").unwrap().to_decoded_bytes(), [0xff]);
}

#[test]
fn absence_empty_lists_and_constructor_budgets_are_distinct() {
    for name in [&FieldName::Accept, &FieldName::AcceptEncoding, &FieldName::AcceptLanguage] {
        let absent = Source::new(name, []);
        assert!(Accept::view(&absent).unwrap().is_none());
        assert!(AcceptEncoding::view(&absent).unwrap().is_none());
        assert!(AcceptLanguage::view(&absent).unwrap().is_none());
    }
    assert_eq!(AcceptOwned::from_entries([]).unwrap().entries().count(), 0);
    assert_eq!(AcceptEncodingOwned::from_entries([]).unwrap().entries().count(), 0);
    assert_eq!(AcceptLanguageOwned::from_entries([]).unwrap().entries().count(), 0);
    assert_eq!(AcceptOwned::try_from(" , , ").unwrap().entries().count(), 0);
    assert_eq!(AcceptEncodingOwned::try_from(" , , ").unwrap().entries().count(), 0);
    assert_eq!(AcceptLanguageOwned::try_from(" , , ").unwrap().entries().count(), 0);
    let entry = AcceptEncodingEntry::new(ContentCoding::parse("br").unwrap(), None).unwrap();
    assert_eq!(
        AcceptEncodingOwned::from_entries(iter::repeat_n(entry, 1_024))
            .unwrap()
            .entries()
            .count(),
        1_024
    );
    assert_eq!(
        AcceptEncodingOwned::from_entries(iter::repeat_n(entry, 1_025)).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    let maximum = "x".repeat(65_536);
    let maximum_coding = ContentCoding::parse(&maximum).unwrap();
    let maximum_entry = AcceptEncodingEntry::new(maximum_coding, None).unwrap();
    assert_eq!(
        AcceptEncodingOwned::from_entries([maximum_entry])
            .unwrap()
            .values()
            .next()
            .unwrap()
            .as_bytes()
            .len(),
        65_536
    );
    assert_eq!(
        AcceptEncodingEntry::new(maximum_coding, Some(QualityView::ONE)).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    assert_eq!(
        AcceptEncodingOwned::from_entries([maximum_entry, entry]).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    assert!(
        AcceptEntry::new(
            MediaRange::new(NegotiationToken::new(&maximum).unwrap(), NegotiationToken::new("x").unwrap()).unwrap(),
            &[],
            None,
            &[],
        )
        .is_err()
    );
}

fn rejects_in_all_positions<F: Field>(valid: &'static [u8], invalid: &'static [u8], kind: DecodeErrorKind) {
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        for position in 0..3 {
            let mut lines = [valid; 3];
            lines[position] = invalid;
            let source = Source::new(F::name(), lines);
            let owned = F::owned_with(&source, mode).err().unwrap();
            let view = F::view_with(&source, mode).err().unwrap();
            assert_eq!(owned.kind(), kind);
            assert_eq!(view.kind(), kind);
            assert_eq!(owned.value_index(), None);
            assert_eq!(view.value_index(), None);
            let joined = lines.join(&b',');
            let source = SingleSource {
                name: F::name(),
                bytes: &joined,
            };
            let owned = F::owned_with(&source, mode).err().unwrap();
            let view = F::view_with(&source, mode).err().unwrap();
            assert_eq!(owned.kind(), kind);
            assert_eq!(view.kind(), kind);
            assert_eq!(owned.value_index(), None);
            assert_eq!(view.value_index(), None);
        }
    }
}

#[test]
fn streaming_projection_handles_many_members_and_late_quoted_parameters() {
    let mut wire = iter::repeat_n("text/plain;p=x;q=0.125", 12).collect::<Vec<_>>().join(", ");
    wire.push_str(", application/json;p=\"a;b,c\";q=0.75;flag");
    let source = SingleSource {
        name: &FieldName::Accept,
        bytes: wire.as_bytes(),
    };
    let view = Accept::view(&source).unwrap().unwrap();
    let owned = Accept::owned(&source).unwrap().unwrap();
    for entries in [view.entries().collect::<Vec<_>>(), owned.entries().collect::<Vec<_>>()] {
        assert_eq!(entries.len(), 13);
        for entry in &entries[..12] {
            assert_eq!(entry.quality().to_quality().unwrap().thousandths(), 125);
            assert_eq!(entry.parameters().next().unwrap().value().unwrap().raw_bytes(), b"x");
        }
        let last = entries[12];
        assert_eq!(last.range().subtype().as_str(), "json");
        assert_eq!(last.quality().to_quality().unwrap().thousandths(), 750);
        assert_eq!(last.parameters().next().unwrap().value().unwrap().to_decoded_bytes(), b"a;b,c");
        assert_eq!(last.extensions().next().unwrap().name().as_str(), "flag");
    }
}

#[test]
fn invalid_members_and_source_budgets_never_become_partial_typed_lists() {
    rejects_in_all_positions::<Accept>(b"text/html", b"*/plain", DecodeErrorKind::InvalidSyntax);
    rejects_in_all_positions::<Accept>(b"text/html", b"text/html;q=0.5;Q=0.4", DecodeErrorKind::InvalidSyntax);
    rejects_in_all_positions::<Accept>(b"text/html", b"text/html;x=\"bad", DecodeErrorKind::UnterminatedQuote);
    rejects_in_all_positions::<Accept>(b"text/html", b"text/html;x", DecodeErrorKind::InvalidSyntax);
    rejects_in_all_positions::<AcceptEncoding>(b"gzip", b"bad coding", DecodeErrorKind::InvalidToken);
    rejects_in_all_positions::<AcceptEncoding>(b"gzip", b"gzip;q=1;x=y", DecodeErrorKind::InvalidSyntax);
    rejects_in_all_positions::<AcceptLanguage>(b"en", b"en-*", DecodeErrorKind::InvalidToken);
    rejects_in_all_positions::<AcceptLanguage>(b"en", b"en;q=1.0001", DecodeErrorKind::InvalidSyntax);
    let entries = iter::repeat_n("en", 1_024).collect::<Vec<_>>().join(",");
    let source = Source::new(&FieldName::AcceptLanguage, [entries.as_bytes()]);
    assert_eq!(AcceptLanguage::view(&source).unwrap().unwrap().entries().count(), 1_024);
    let too_many = format!("{entries},en");
    let source = Source::new(&FieldName::AcceptLanguage, [too_many.as_bytes()]);
    assert!(AcceptLanguage::view(&source).is_err());
    assert!(AcceptLanguage::owned(&source).is_err());
    let too_large = vec![b'a'; 65_537];
    let source = Source::new(&FieldName::AcceptEncoding, [too_large.as_slice()]);
    assert!(AcceptEncoding::view(&source).is_err());
    assert!(AcceptEncoding::owned(&source).is_err());
    let source = Source::new(&FieldName::Accept, iter::repeat_n(b"*/*".as_slice(), 129));
    assert!(Accept::view(&source).is_err());
    assert!(Accept::owned(&source).is_err());
}

#[test]
fn raw_owned_slice_forwarding_retains_sensitivity_and_spelling() {
    let source = Stored(vec![
        FieldValue::from_static("GZIP;q=0.500").with_sensitivity(FieldSensitivity::Sensitive),
        FieldValue::from_static("identity;q=0"),
    ]);
    let view = AcceptEncoding::view(&source).unwrap().unwrap();
    assert_eq!(view.entries().next().unwrap().quality(), quality(b".5"));
    assert!(view.values().next().unwrap().is_sensitive());
    let owned = AcceptEncoding::owned(&source).unwrap().unwrap();
    let mut sink = Stored::default();
    AcceptEncoding::insert(&mut sink, owned).unwrap();
    assert_eq!(sink.0, source.0);
    assert!(sink.0[0].is_sensitive());
    assert!(!sink.0[1].is_sensitive());
}

#[cfg(feature = "http")]
#[test]
fn http_sources_have_the_same_semantics_and_preserve_wire_forwarding() {
    use http::{HeaderMap, HeaderValue, header};

    let mut map = HeaderMap::new();
    let mut first = HeaderValue::from_static("GZIP;q=0.500, *;q=0");
    first.set_sensitive(true);
    map.append(header::ACCEPT_ENCODING, first);
    map.append(header::ACCEPT_ENCODING, HeaderValue::from_static("identity;q=1"));
    let view = AcceptEncoding::view(&map).unwrap().unwrap();
    let owned = AcceptEncoding::owned(&map).unwrap().unwrap();
    assert_eq!(view.entries().collect::<Vec<_>>(), owned.entries().collect::<Vec<_>>());
    assert_eq!(view.entries().next().unwrap().quality(), quality(b".5"));
    let mut forwarded = HeaderMap::new();
    AcceptEncoding::insert(&mut forwarded, owned).unwrap();
    assert_eq!(map, forwarded);
    assert!(forwarded[header::ACCEPT_ENCODING].is_sensitive());
    map.insert(header::ACCEPT, HeaderValue::from_static("text/plain;p=\"a,b\";q=0.125;flag"));
    let accept = Accept::view(&map).unwrap().unwrap();
    let entry = accept.entries().next().unwrap();
    assert_eq!(entry.parameters().next().unwrap().value().unwrap().to_decoded_bytes(), b"a,b");
    assert_eq!(entry.extensions().next().unwrap().value(), None);
    map.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("fr-CH;q=0.75, *;q=0"));
    let language = AcceptLanguage::view(&map).unwrap().unwrap();
    assert_eq!(language.entries().next().unwrap().range().primary().unwrap().as_str(), "fr");
    assert_eq!(
        language.entries().next().unwrap().quality().to_quality().unwrap().thousandths(),
        750
    );
    let mut invalid = HeaderMap::new();
    invalid.append(header::ACCEPT, HeaderValue::from_static("text/plain"));
    invalid.append(header::ACCEPT, HeaderValue::from_static("text/html;p=\"unterminated"));
    let view_error = Accept::view(&invalid).unwrap_err();
    let owned_error = Accept::owned(&invalid).unwrap_err();
    assert_eq!(view_error.kind(), DecodeErrorKind::UnterminatedQuote);
    assert_eq!(owned_error.kind(), DecodeErrorKind::UnterminatedQuote);
    assert_eq!(view_error.value_index(), None);
    assert_eq!(owned_error.value_index(), Some(1));
}
