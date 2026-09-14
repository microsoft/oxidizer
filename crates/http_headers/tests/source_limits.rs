// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression coverage for custom-source decode budgets.

#![expect(
    clippy::assertions_on_result_states,
    reason = "these tests assert rejection without needing the successful value"
)]

use std::iter;

#[cfg(feature = "http")]
use http_headers::headers::UserAgent;
use http_headers::headers::{
    Accept, AcceptRanges, AccessControlAllowCredentials, AccessControlAllowOrigin, AccessControlMaxAge, AccessControlRequestMethod,
    ContentSecurityPolicy, IfMatch, Location, Range, ReferrerPolicy, SetCookie, StrictTransportSecurity,
};
use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES, MAX_CUSTOM_LIST_ITEMS};
use http_headers::{FieldName, FieldValue};

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
    let mut padded = vec![b' '; MAX_CUSTOM_FIELD_BYTES];
    padded.extend_from_slice(value);
    padded
}

#[test]
fn custom_source_total_byte_limit_accepts_boundary_and_rejects_overflow() {
    let half = MAX_CUSTOM_FIELD_BYTES / 2;
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
    assert_eq!(
        SetCookie::owned(&over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        SetCookie::view(&over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );

    let list_over_limit = ValuesSource {
        name: &FieldName::AcceptRanges,
        values: repeated_value("bytes", MAX_CUSTOM_FIELD_LINES + 1),
    };
    assert_eq!(
        AcceptRanges::view(&list_over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );

    let mut negotiation_boundary_bytes = b"text/plain".to_vec();
    negotiation_boundary_bytes.resize(MAX_CUSTOM_FIELD_BYTES, b' ');
    let negotiation_boundary = SingleSource {
        name: &FieldName::Accept,
        bytes: negotiation_boundary_bytes,
    };
    assert!(Accept::view(&negotiation_boundary).expect("byte boundary is accepted").is_some());
    assert!(Accept::owned(&negotiation_boundary).expect("byte boundary is accepted").is_some());

    let negotiation_over_limit = SingleSource {
        name: &FieldName::Accept,
        bytes: vec![b'a'; MAX_CUSTOM_FIELD_BYTES + 1],
    };
    assert!(Accept::view(&negotiation_over_limit).is_err());
    assert!(Accept::owned(&negotiation_over_limit).is_err());
}

#[test]
fn custom_source_line_limit_accepts_boundary_and_rejects_overflow() {
    let boundary = ValuesSource {
        name: &FieldName::SetCookie,
        values: repeated_value("a=1", MAX_CUSTOM_FIELD_LINES),
    };
    let decoded = SetCookie::owned(&boundary)
        .expect("boundary is accepted")
        .expect("field is present");
    assert_eq!(decoded.len(), MAX_CUSTOM_FIELD_LINES);
    assert_eq!(
        SetCookie::view(&boundary)
            .expect("boundary view is accepted")
            .expect("field is present")
            .len(),
        MAX_CUSTOM_FIELD_LINES
    );

    let over_limit = ValuesSource {
        name: &FieldName::SetCookie,
        values: repeated_value("a=1", MAX_CUSTOM_FIELD_LINES + 1),
    };
    assert_eq!(
        SetCookie::owned(&over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        SetCookie::view(&over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );

    let negotiation_boundary = ValuesSource {
        name: &FieldName::Accept,
        values: repeated_value("text/plain", MAX_CUSTOM_FIELD_LINES),
    };
    assert!(Accept::view(&negotiation_boundary).expect("line boundary is accepted").is_some());
    assert!(Accept::owned(&negotiation_boundary).expect("line boundary is accepted").is_some());

    let negotiation_over_limit = ValuesSource {
        name: &FieldName::Accept,
        values: repeated_value("text/plain", MAX_CUSTOM_FIELD_LINES + 1),
    };
    assert!(Accept::view(&negotiation_over_limit).is_err());
    assert!(Accept::owned(&negotiation_over_limit).is_err());
}

#[test]
fn custom_source_list_item_limit_accepts_boundary_and_rejects_overflow() {
    let boundary = SingleSource {
        name: &FieldName::ReferrerPolicy,
        bytes: comma_list("origin", MAX_CUSTOM_LIST_ITEMS),
    };
    let decoded = ReferrerPolicy::owned(&boundary)
        .expect("boundary is accepted")
        .expect("field is present");
    assert_eq!(decoded.tokens().count(), MAX_CUSTOM_LIST_ITEMS);
    assert_eq!(
        ReferrerPolicy::view(&boundary)
            .expect("boundary view is accepted")
            .expect("field is present")
            .tokens()
            .count(),
        MAX_CUSTOM_LIST_ITEMS
    );

    let over_limit = SingleSource {
        name: &FieldName::ReferrerPolicy,
        bytes: comma_list("origin", MAX_CUSTOM_LIST_ITEMS + 1),
    };
    assert_eq!(
        ReferrerPolicy::owned(&over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        ReferrerPolicy::view(&over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );

    let over_limit_before_final_item = SingleSource {
        name: &FieldName::ReferrerPolicy,
        bytes: comma_list("origin", MAX_CUSTOM_LIST_ITEMS + 2),
    };
    assert!(ReferrerPolicy::view(&over_limit_before_final_item).is_err());

    let negotiation_boundary = SingleSource {
        name: &FieldName::Accept,
        bytes: comma_list("text/plain", MAX_CUSTOM_LIST_ITEMS),
    };
    assert!(Accept::view(&negotiation_boundary).expect("list boundary is accepted").is_some());
    assert!(Accept::owned(&negotiation_boundary).expect("list boundary is accepted").is_some());

    let negotiation_over_limit = SingleSource {
        name: &FieldName::Accept,
        bytes: comma_list("text/plain", MAX_CUSTOM_LIST_ITEMS + 1),
    };
    assert!(Accept::view(&negotiation_over_limit).is_err());
    assert!(Accept::owned(&negotiation_over_limit).is_err());

    let boundary = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\"", MAX_CUSTOM_LIST_ITEMS),
    };
    assert!(IfMatch::owned(&boundary).expect("boundary is accepted").is_some());

    let over_limit = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\"", MAX_CUSTOM_LIST_ITEMS + 1),
    };
    assert!(IfMatch::owned(&over_limit).is_err());

    let boundary = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\\\"", MAX_CUSTOM_LIST_ITEMS),
    };
    assert!(IfMatch::owned(&boundary).expect("backslash boundary is accepted").is_some());

    let over_limit = SingleSource {
        name: &FieldName::IfMatch,
        bytes: comma_list("\"tag\\\"", MAX_CUSTOM_LIST_ITEMS + 1),
    };
    assert!(IfMatch::owned(&over_limit).is_err());
}

#[test]
fn singleton_list_item_limit_accepts_boundary_and_rejects_overflow() {
    let range_boundary = SingleSource {
        name: &FieldName::Range,
        bytes: range_list(MAX_CUSTOM_LIST_ITEMS),
    };
    assert!(Range::view(&range_boundary).expect("Range boundary is accepted").is_some());
    assert!(Range::owned(&range_boundary).expect("Range boundary is accepted").is_some());

    let range_over_limit = SingleSource {
        name: &FieldName::Range,
        bytes: range_list(MAX_CUSTOM_LIST_ITEMS + 1),
    };
    assert_eq!(
        Range::view(&range_over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        Range::owned(&range_over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );

    let hsts_boundary = SingleSource {
        name: &FieldName::StrictTransportSecurity,
        bytes: hsts_directives(MAX_CUSTOM_LIST_ITEMS),
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
        bytes: hsts_directives(MAX_CUSTOM_LIST_ITEMS + 1),
    };
    assert_eq!(
        StrictTransportSecurity::view(&hsts_over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        StrictTransportSecurity::owned(&hsts_over_limit).unwrap_err().kind(),
        http_headers::DecodeErrorKind::InvalidSyntax
    );
}

#[test]
fn bounded_opaque_single_value_does_not_treat_delimiters_as_list_items() {
    let mut bytes = b"https://example.com/".to_vec();
    bytes.extend(iter::repeat_n(b',', MAX_CUSTOM_LIST_ITEMS + 1));
    bytes.extend(iter::repeat_n(b';', MAX_CUSTOM_LIST_ITEMS + 1));
    let source = SingleSource {
        name: &FieldName::Location,
        bytes,
    };

    assert!(Location::owned(&source).expect("valid URI decodes").is_some());
}

#[test]
fn handwritten_direct_decoders_preflight_custom_source_budgets() {
    let credentials = SingleSource {
        name: &FieldName::AccessControlAllowCredentials,
        bytes: padded_value(b"true"),
    };
    assert!(AccessControlAllowCredentials::view(&credentials).is_err());
    assert!(AccessControlAllowCredentials::owned(&credentials).is_err());

    let max_age = SingleSource {
        name: &FieldName::AccessControlMaxAge,
        bytes: padded_value(b"1"),
    };
    assert!(AccessControlMaxAge::view(&max_age).is_err());
    assert!(AccessControlMaxAge::owned(&max_age).is_err());

    let request_method = SingleSource {
        name: &FieldName::AccessControlRequestMethod,
        bytes: padded_value(b"GET"),
    };
    assert!(AccessControlRequestMethod::view(&request_method).is_err());
    assert!(AccessControlRequestMethod::owned(&request_method).is_err());

    let origin = SingleSource {
        name: &FieldName::AccessControlAllowOrigin,
        bytes: padded_value(b"https://example.com"),
    };
    assert!(AccessControlAllowOrigin::view(&origin).is_err());
    assert!(AccessControlAllowOrigin::owned(&origin).is_err());

    let policy = SingleSource {
        name: &FieldName::ContentSecurityPolicy,
        bytes: vec![b'a'; MAX_CUSTOM_FIELD_BYTES + 1],
    };
    assert!(ContentSecurityPolicy::view(&policy).is_err());
    assert!(ContentSecurityPolicy::owned(&policy).is_err());

    let recognized = ValuesSource {
        name: &FieldName::ReferrerPolicy,
        values: repeated_value("origin", MAX_CUSTOM_FIELD_LINES + 1),
    };
    assert!(ReferrerPolicy::view(&recognized).is_err());
    assert!(ReferrerPolicy::owned(&recognized).is_err());
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
        if self
            .values
            .len()
            .checked_add(values.len())
            .is_none_or(|length| length > MAX_CUSTOM_FIELD_LINES)
        {
            return Err(InsertError);
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
        values: repeated_value("a=1", MAX_CUSTOM_FIELD_LINES + 1),
    };
    let original_len = sink.values.len();

    assert_eq!(
        sink.append_encoded(&FieldName::SetCookie, FieldValue::from_static("b=2")),
        Err(InsertError)
    );
    assert_eq!(sink.values.len(), original_len);
}

#[cfg(feature = "http")]
#[test]
fn validated_http_map_values_are_not_subject_to_custom_source_limits() {
    let mut map = http::HeaderMap::new();
    map.insert(
        http::header::USER_AGENT,
        http::HeaderValue::from_bytes(&vec![b'a'; MAX_CUSTOM_FIELD_BYTES + 1]).expect("valid long field value"),
    );
    assert!(UserAgent::owned(&map).expect("http value decodes").is_some());

    for _ in 0..=MAX_CUSTOM_FIELD_LINES {
        map.append(http::header::SET_COOKIE, http::HeaderValue::from_static("a=1"));
    }
    let cookies = SetCookie::owned(&map).expect("http lines decode").expect("cookies are present");
    assert_eq!(cookies.len(), MAX_CUSTOM_FIELD_LINES + 1);

    map.insert(
        http::header::REFERRER_POLICY,
        http::HeaderValue::from_bytes(&comma_list("origin", MAX_CUSTOM_LIST_ITEMS + 1)).expect("valid long list"),
    );
    let policies = ReferrerPolicy::owned(&map).expect("http list decodes").expect("policy is present");
    assert_eq!(policies.tokens().count(), MAX_CUSTOM_LIST_ITEMS + 1);
}
