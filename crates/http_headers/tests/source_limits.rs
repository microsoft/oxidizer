// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression coverage for custom-source decode budgets.

#![cfg(feature = "headers-all")]

use std::iter;

#[cfg(feature = "http")]
use http_headers::headers::UserAgent;
use http_headers::headers::{
    Accept, AcceptRanges, AccessControlAllowCredentials, AccessControlAllowOrigin, AccessControlMaxAge, AccessControlRequestMethod,
    ContentSecurityPolicy, IfMatch, Location, Range, ReferrerPolicy, SetCookie, StrictTransportSecurity,
};
use http_headers::sink::{EncodedValues, FieldSink, InsertError, InsertErrorKind};
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES, MAX_CUSTOM_LIST_ITEMS};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, FieldValueRef};

#[test]
fn published_source_limits_have_stable_values() {
    assert_eq!(MAX_CUSTOM_FIELD_BYTES, 65_536);
    assert_eq!(MAX_CUSTOM_FIELD_LINES, 128);
    assert_eq!(MAX_CUSTOM_LIST_ITEMS, 1_024);
}

fn assert_source_limit<F: Field>(source: &impl FieldSource) {
    assert_decode_error::<F>(source, DecodeErrorKind::SourceLimitExceeded);
}

fn assert_decode_error<F: Field>(source: &impl FieldSource, expected: DecodeErrorKind) {
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(F::view_with(source, mode).map(|_| ()).map_err(|error| error.kind()), Err(expected));
        assert_eq!(F::owned_with(source, mode).map(|_| ()).map_err(|error| error.kind()), Err(expected));
    }
}

struct SingleSource {
    name: &'static FieldName,
    bytes: Vec<u8>,
}

impl FieldSource for SingleSource {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::single(name, &self.bytes))
    }
}

struct ValuesSource {
    name: &'static FieldName,
    values: Vec<FieldValue>,
}

impl FieldSource for ValuesSource {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_slice(name, &self.values)).flatten()
    }
}

struct BorrowedSource<'a>(&'a [FieldValueRef<'a>]);

impl FieldSource for BorrowedSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_borrowed(name, self.0)
    }
}

fn repeated_value(value: &'static str, count: usize) -> Vec<FieldValue> {
    (0..count).map(|_| FieldValue::from_static(value)).collect()
}

fn comma_list(item: &str, count: usize) -> Vec<u8> {
    iter::repeat_n(item, count).collect::<Vec<_>>().join(",").into_bytes()
}

fn range_list(count: usize) -> Vec<u8> {
    let mut value = b"bytes=".to_vec();
    value.extend_from_slice(&comma_list("0-0", count));
    value
}

fn hsts_directives(count: usize) -> Vec<u8> {
    let mut value = b"max-age=1".to_vec();
    for _ in 1..count {
        value.extend_from_slice(b";x");
    }
    value
}

fn padded_value(value: &[u8]) -> Vec<u8> {
    let mut padded = vec![b' '; 65_536];
    padded.extend_from_slice(value);
    padded
}

#[test]
fn custom_source_total_byte_limit_accepts_boundary_and_rejects_overflow() {
    let half = 65_536 / 2;
    let boundary = ValuesSource {
        name: &FieldName::SetCookie,
        values: vec![
            FieldValue::try_from(vec![b'a'; half]).expect("valid field value"),
            FieldValue::try_from(vec![b'b'; half]).expect("valid field value"),
        ],
    };
    let decoded = SetCookie::owned(&boundary)
        .expect("aggregate boundary is accepted")
        .expect("field is present");
    assert_eq!(decoded.len(), 2);
    assert_eq!(
        SetCookie::view(&boundary)
            .expect("aggregate boundary view is accepted")
            .expect("field is present")
            .len(),
        2
    );

    let over_limit = ValuesSource {
        name: &FieldName::SetCookie,
        values: vec![
            FieldValue::try_from(vec![b'a'; half]).expect("valid field value"),
            FieldValue::try_from(vec![b'b'; half + 1]).expect("valid field value"),
        ],
    };
    assert_source_limit::<SetCookie>(&over_limit);

    let list_over_limit = ValuesSource {
        name: &FieldName::AcceptRanges,
        values: repeated_value("bytes", 129),
    };
    assert_source_limit::<AcceptRanges>(&list_over_limit);

    let mut negotiation_boundary_bytes = b"text/plain".to_vec();
    negotiation_boundary_bytes.resize(65_536, b' ');
    let negotiation_boundary = SingleSource {
        name: &FieldName::Accept,
        bytes: negotiation_boundary_bytes,
    };
    assert!(Accept::view(&negotiation_boundary).expect("byte boundary is accepted").is_some());
    assert!(Accept::owned(&negotiation_boundary).expect("byte boundary is accepted").is_some());

    let negotiation_over_limit = SingleSource {
        name: &FieldName::Accept,
        bytes: vec![b'a'; 65_537],
    };
    assert_source_limit::<Accept>(&negotiation_over_limit);
}

#[test]
fn custom_source_line_limit_accepts_boundary_and_rejects_overflow() {
    let boundary = ValuesSource {
        name: &FieldName::SetCookie,
        values: repeated_value("a=1", 128),
    };
    let decoded = SetCookie::owned(&boundary)
        .expect("boundary is accepted")
        .expect("field is present");
    assert_eq!(decoded.len(), 128);
    assert_eq!(
        SetCookie::view(&boundary)
            .expect("boundary view is accepted")
            .expect("field is present")
            .len(),
        128
    );

    let over_limit = ValuesSource {
        name: &FieldName::SetCookie,
        values: repeated_value("a=1", 129),
    };
    assert_source_limit::<SetCookie>(&over_limit);

    let negotiation_boundary = ValuesSource {
        name: &FieldName::Accept,
        values: repeated_value("text/plain", 128),
    };
    assert!(Accept::view(&negotiation_boundary).expect("line boundary is accepted").is_some());
    assert!(Accept::owned(&negotiation_boundary).expect("line boundary is accepted").is_some());

    let negotiation_over_limit = ValuesSource {
        name: &FieldName::Accept,
        values: repeated_value("text/plain", 129),
    };
    assert_source_limit::<Accept>(&negotiation_over_limit);
}

#[test]
fn custom_source_list_item_limit_accepts_boundary_and_rejects_overflow() {
    let boundary = SingleSource {
        name: &FieldName::ReferrerPolicy,
        bytes: comma_list("origin", 1_024),
    };
    let decoded = ReferrerPolicy::owned(&boundary)
        .expect("boundary is accepted")
        .expect("field is present");
    assert_eq!(decoded.tokens().count(), 1_024);
    assert_eq!(
        ReferrerPolicy::view(&boundary)
            .expect("boundary view is accepted")
            .expect("field is present")
            .tokens()
            .count(),
        1_024
    );

    let over_limit = SingleSource {
        name: &FieldName::ReferrerPolicy,
        bytes: comma_list("origin", 1_025),
    };
    assert_source_limit::<ReferrerPolicy>(&over_limit);

    let over_limit_before_final_item = SingleSource {
        name: &FieldName::ReferrerPolicy,
        bytes: comma_list("origin", 1_026),
    };
    assert_source_limit::<ReferrerPolicy>(&over_limit_before_final_item);

    let negotiation_boundary = SingleSource {
        name: &FieldName::Accept,
        bytes: comma_list("text/plain", 1_024),
    };
    assert!(Accept::view(&negotiation_boundary).expect("list boundary is accepted").is_some());
    assert!(Accept::owned(&negotiation_boundary).expect("list boundary is accepted").is_some());

    let negotiation_over_limit = SingleSource {
        name: &FieldName::Accept,
        bytes: comma_list("text/plain", 1_025),
    };
    assert_source_limit::<Accept>(&negotiation_over_limit);

    let boundary = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\"", 1_024),
    };
    assert!(IfMatch::owned(&boundary).expect("boundary is accepted").is_some());

    let over_limit = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\"", 1_025),
    };
    assert_source_limit::<IfMatch>(&over_limit);

    let boundary = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\\\"", 1_024),
    };
    assert!(IfMatch::owned(&boundary).expect("backslash boundary is accepted").is_some());

    let over_limit = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\\\"", 1_025),
    };
    assert_source_limit::<IfMatch>(&over_limit);
}

#[test]
fn raw_and_validated_list_sources_share_the_aggregate_item_budget() {
    let first = comma_list("origin", 512);
    for count in [512, 513] {
        let second = comma_list("origin", count);
        let values = [FieldValueRef::new(&first), FieldValueRef::new(&second)];
        let raw = BorrowedSource(&values);
        let stored = ValuesSource {
            name: &FieldName::ReferrerPolicy,
            values: vec![FieldValue::from_bytes(&first).unwrap(), FieldValue::from_bytes(&second).unwrap()],
        };
        if count == 512 {
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                assert_eq!(ReferrerPolicy::view_with(&raw, mode).unwrap().unwrap().tokens().count(), 1_024);
                assert_eq!(ReferrerPolicy::owned_with(&raw, mode).unwrap().unwrap().tokens().count(), 1_024);
                assert_eq!(ReferrerPolicy::view_with(&stored, mode).unwrap().unwrap().tokens().count(), 1_024);
                assert_eq!(ReferrerPolicy::owned_with(&stored, mode).unwrap().unwrap().tokens().count(), 1_024);
            }
        } else {
            assert_source_limit::<ReferrerPolicy>(&raw);
            assert_source_limit::<ReferrerPolicy>(&stored);
        }
    }
}

#[test]
fn singleton_list_item_limit_accepts_boundary_and_rejects_overflow() {
    let range_boundary = SingleSource {
        name: &FieldName::Range,
        bytes: range_list(1_024),
    };
    assert!(Range::view(&range_boundary).expect("Range boundary is accepted").is_some());
    assert!(Range::owned(&range_boundary).expect("Range boundary is accepted").is_some());

    let range_over_limit = SingleSource {
        name: &FieldName::Range,
        bytes: range_list(1_025),
    };
    assert_source_limit::<Range>(&range_over_limit);

    let hsts_boundary = SingleSource {
        name: &FieldName::StrictTransportSecurity,
        bytes: hsts_directives(1_024),
    };
    assert!(
        StrictTransportSecurity::view(&hsts_boundary)
            .expect("HSTS boundary is accepted")
            .is_some()
    );
    assert!(
        StrictTransportSecurity::owned(&hsts_boundary)
            .expect("HSTS boundary is accepted")
            .is_some()
    );

    let hsts_over_limit = SingleSource {
        name: &FieldName::StrictTransportSecurity,
        bytes: hsts_directives(1_025),
    };
    assert_source_limit::<StrictTransportSecurity>(&hsts_over_limit);
}

#[test]
fn bounded_opaque_single_value_does_not_treat_delimiters_as_list_items() {
    let mut bytes = b"https://example.com/".to_vec();
    bytes.extend(iter::repeat_n(b',', 1_025));
    bytes.extend(iter::repeat_n(b';', 1_025));
    let source = SingleSource {
        name: &FieldName::Location,
        bytes,
    };

    assert!(Location::owned(&source).expect("valid URI decodes").is_some());
}

#[test]
fn raw_borrowed_sources_obey_aggregate_byte_and_line_boundaries() {
    let first = vec![b'a'; 32_768];
    let mut second = vec![b'b'; 32_768];
    {
        let values = [FieldValueRef::new(&first), FieldValueRef::new(&second)];
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            assert_eq!(SetCookie::view_with(&BorrowedSource(&values), mode).unwrap().unwrap().len(), 2);
            assert_eq!(SetCookie::owned_with(&BorrowedSource(&values), mode).unwrap().unwrap().len(), 2);
        }
    }
    second.push(b'b');
    let values = [FieldValueRef::new(&first), FieldValueRef::new(&second)];
    assert_source_limit::<SetCookie>(&BorrowedSource(&values));

    let mut values = vec![FieldValueRef::new(b"a=1"); 128];
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(SetCookie::view_with(&BorrowedSource(&values), mode).unwrap().unwrap().len(), 128);
        assert_eq!(SetCookie::owned_with(&BorrowedSource(&values), mode).unwrap().unwrap().len(), 128);
    }
    values.push(FieldValueRef::new(b"a=1"));
    assert_source_limit::<SetCookie>(&BorrowedSource(&values));
}

#[test]
fn admission_limits_do_not_reclassify_invalid_field_bytes() {
    let oversized = vec![b'a'; 65_537];
    for invalid in [b"\r".as_slice(), b"\n", b"\0", b"\x1f", b"\x7f"] {
        let values = [FieldValueRef::new(invalid)];
        assert_decode_error::<SetCookie>(&BorrowedSource(&values), DecodeErrorKind::InvalidSyntax);
        assert_decode_error::<AcceptRanges>(&BorrowedSource(&values), DecodeErrorKind::InvalidSyntax);

        let values = [FieldValueRef::new(invalid), FieldValueRef::new(&oversized)];
        assert_decode_error::<SetCookie>(&BorrowedSource(&values), DecodeErrorKind::InvalidSyntax);
        assert_decode_error::<AcceptRanges>(&BorrowedSource(&values), DecodeErrorKind::InvalidSyntax);

        let values = [FieldValueRef::new(&oversized), FieldValueRef::new(invalid)];
        assert_source_limit::<SetCookie>(&BorrowedSource(&values));
        assert_source_limit::<AcceptRanges>(&BorrowedSource(&values));
    }

    let values = vec![FieldValueRef::new(b"\r"); 129];
    assert_source_limit::<SetCookie>(&BorrowedSource(&values));
    assert_source_limit::<AcceptRanges>(&BorrowedSource(&values));

    let mut source = SingleSource {
        name: &FieldName::SetCookie,
        bytes: vec![b'a'; 65_536],
    };
    source.bytes[0] = b'\r';
    assert_decode_error::<SetCookie>(&source, DecodeErrorKind::InvalidSyntax);
    source.bytes.push(b'a');
    assert_source_limit::<SetCookie>(&source);

    source.name = &FieldName::AcceptRanges;
    assert_source_limit::<AcceptRanges>(&source);
    source.bytes.truncate(65_536);
    assert_decode_error::<AcceptRanges>(&source, DecodeErrorKind::InvalidSyntax);
}

#[test]
fn delimited_iteration_distinguishes_item_admission_from_quote_errors() {
    for (tail, kind, index) in [
        ("tail", DecodeErrorKind::SourceLimitExceeded, None),
        ("\"unterminated", DecodeErrorKind::UnterminatedQuote, Some(0)),
    ] {
        let mut bytes = comma_list("origin", 1_024);
        bytes.push(b',');
        bytes.extend_from_slice(tail.as_bytes());
        let lines = FieldLines::single(&FieldName::ReferrerPolicy, &bytes);
        let mut items = lines.comma_items();
        for _ in 0..1_024 {
            assert_eq!(items.next(), Some(Ok(b"origin".as_slice())));
        }
        let error = items.next().unwrap().unwrap_err();
        assert_eq!(error.kind(), kind);
        assert_eq!(error.header(), &FieldName::ReferrerPolicy);
        assert_eq!(error.value_index(), index);
        assert_eq!(items.next(), None);
        assert_eq!(items.next(), None);
    }
}

#[test]
fn handwritten_direct_decoders_preflight_custom_source_budgets() {
    let credentials = SingleSource {
        name: &FieldName::AccessControlAllowCredentials,
        bytes: padded_value(b"true"),
    };
    assert_source_limit::<AccessControlAllowCredentials>(&credentials);

    let max_age = SingleSource {
        name: &FieldName::AccessControlMaxAge,
        bytes: padded_value(b"1"),
    };
    assert_source_limit::<AccessControlMaxAge>(&max_age);

    let request_method = SingleSource {
        name: &FieldName::AccessControlRequestMethod,
        bytes: padded_value(b"GET"),
    };
    assert_source_limit::<AccessControlRequestMethod>(&request_method);

    let origin = SingleSource {
        name: &FieldName::AccessControlAllowOrigin,
        bytes: padded_value(b"https://example.com"),
    };
    assert_source_limit::<AccessControlAllowOrigin>(&origin);

    let policy = SingleSource {
        name: &FieldName::ContentSecurityPolicy,
        bytes: vec![b'a'; 65_537],
    };
    assert_source_limit::<ContentSecurityPolicy>(&policy);

    let recognized = ValuesSource {
        name: &FieldName::ReferrerPolicy,
        values: repeated_value("origin", 129),
    };
    assert_source_limit::<ReferrerPolicy>(&recognized);
}

struct LimitedSink {
    values: Vec<FieldValue>,
}

impl FieldSource for LimitedSink {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == &FieldName::SetCookie)
            .then(|| FieldLines::from_slice(name, &self.values))
            .flatten()
    }
}

impl FieldSink for LimitedSink {
    fn set_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.values = values.into_iter().collect();
        Ok(())
    }

    fn append_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if self.values.len().checked_add(values.len()).is_none_or(|length| length > 128) {
            return Err(InsertError::new(InsertErrorKind::CapacityExceeded));
        }
        self.values.extend(values);
        Ok(())
    }

    fn remove_values(&mut self, _name: &'static FieldName) {
        self.values.clear();
    }
}

#[test]
fn appending_to_an_over_limit_custom_sink_returns_insert_error() {
    let mut sink = LimitedSink {
        values: repeated_value("a=1", 129),
    };
    let original_len = sink.values.len();

    assert_eq!(
        sink.append_encoded(&FieldName::SetCookie, FieldValue::from_static("b=2")),
        Err(InsertError::new(InsertErrorKind::CapacityExceeded))
    );
    assert_eq!(sink.values.len(), original_len);
}

#[cfg(feature = "http")]
#[test]
fn validated_http_map_values_are_not_subject_to_custom_source_limits() {
    let mut map = http::HeaderMap::new();
    map.insert(
        http::header::USER_AGENT,
        http::HeaderValue::from_bytes(&vec![b'a'; 65_537]).expect("valid long field value"),
    );
    for _ in 0..129 {
        map.append(http::header::SET_COOKIE, http::HeaderValue::from_static("a=1"));
    }
    map.insert(
        http::header::REFERRER_POLICY,
        http::HeaderValue::from_bytes(&comma_list("origin", 1_025)).expect("valid long list"),
    );
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(UserAgent::view_with(&map, mode).unwrap().unwrap().as_bytes().len(), 65_537);
        assert_eq!(UserAgent::owned_with(&map, mode).unwrap().unwrap().as_bytes().len(), 65_537);
        assert_eq!(SetCookie::view_with(&map, mode).unwrap().unwrap().len(), 129);
        assert_eq!(SetCookie::owned_with(&map, mode).unwrap().unwrap().len(), 129);
        assert_eq!(ReferrerPolicy::view_with(&map, mode).unwrap().unwrap().tokens().count(), 1_025);
        assert_eq!(ReferrerPolicy::owned_with(&map, mode).unwrap().unwrap().tokens().count(), 1_025);
    }
}
