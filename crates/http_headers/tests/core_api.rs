// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public core API integration tests using downstream-defined headers.

#![cfg(feature = "headers-all")]

use std::panic;
use std::sync::LazyLock;

#[cfg(feature = "http")]
use http::HeaderName;
use http_headers::headers::{
    AcceptRanges, AcceptRangesOwned, Authorization, AuthorizationOwned, Basic, BasicCredentials, ContentLength, ContentLengthOwned,
    ContentType, ETag, ETagOwned, IfMatch, IfMatchOwned, IfNoneMatch, IfNoneMatchOwned, IfRange, IfRangeOwned, ReferrerPolicy,
    ReferrerPolicyOwned, ReferrerPolicyValue, SecWebSocketVersion, SecWebSocketVersionOwned, SetCookie, SetCookieOwned, UserAgent,
};
use http_headers::sink::{EncodedValues, FieldSink, FieldSinkExt, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, FieldValueRef, SingleValueField};

use self::common::TestMap;

mod common;

static REQUEST_ID: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-request-id"));
static EMPTY: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-empty"));

impl TestMap {
    fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, name: FieldName, value: FieldValue) {
        self.0.insert(name, vec![value]);
    }

    fn append(&mut self, name: FieldName, value: FieldValue) {
        self.0.entry(name).or_default().push(value);
    }

    fn contains_name(&self, name: &FieldName) -> bool {
        self.0.contains_key(name)
    }

    fn names(&self) -> impl Iterator<Item = &FieldName> {
        self.0.keys()
    }
}

fn content_length_source(values: &[&'static str]) -> TestMap {
    let mut source = TestMap::new();
    for (index, value) in values.iter().copied().enumerate() {
        let value = FieldValue::from_static(value);
        if index == 0 {
            source.insert(FieldName::ContentLength, value);
        } else {
            source.append(FieldName::ContentLength, value);
        }
    }
    source
}

struct BorrowedSource<'a> {
    user_agent: [FieldValueRef<'a>; 1],
}

impl FieldSource for BorrowedSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        if name == &FieldName::UserAgent {
            FieldLines::from_borrowed(name, &self.user_agent)
        } else {
            None
        }
    }
}

#[test]
fn deferred_and_fluent_writes_work_with_downstream_sinks() {
    let mut compatibility = TestMap::new();
    compatibility
        .set_content_length(42)
        .expect("in-memory insertion succeeds")
        .set_content_type(ContentType::json())
        .expect("in-memory insertion succeeds");
    let dynamic_sink: &mut dyn FieldSink = &mut compatibility;
    dynamic_sink.remove_values(&FieldName::ContentLength);

    ContentType::json()
        .insert_into(&mut compatibility)
        .expect("in-memory insertion succeeds");
    compatibility.set_content_length(42).expect("in-memory insertion succeeds");

    let source = BorrowedSource {
        user_agent: [FieldValueRef::new(b"client/1")],
    };
    let view = UserAgent::view(&source).expect("valid user agent").expect("user agent is present");
    view.insert_into(&mut compatibility).expect("borrowed insertion succeeds");
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestId(FieldValue);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequestIdView<'a>(FieldValueRef<'a>);

impl SingleValueField for RequestId {
    type View<'a> = RequestIdView<'a>;
    type Owned = Self;

    fn name() -> &'static FieldName {
        &REQUEST_ID
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        if value.as_bytes().is_empty() {
            Err(DecodeError::new(&REQUEST_ID, DecodeErrorKind::InvalidToken))
        } else {
            Ok(RequestIdView(value))
        }
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        if value.as_bytes().is_empty() {
            Err(DecodeError::new(&REQUEST_ID, DecodeErrorKind::InvalidToken))
        } else {
            Ok(Self(value))
        }
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.0
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EmptyHeader;

impl Field for EmptyHeader {
    type View<'a> = Self;
    type Owned = Self;

    fn name() -> &'static FieldName {
        &EMPTY
    }

    fn view_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Ok(source.contains(Self::name()).then_some(Self))
    }

    fn owned_with<S>(source: &S, mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, mode)
    }

    fn insert<S>(sink: &mut S, _value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::new())
    }
}

#[test]
fn constructed_and_parsed_custom_names_are_one_value() {
    let constructed = FieldName::from_static("x-trace-id");
    let parsed = FieldName::try_from_bytes(b"X-Trace-Id").expect("valid name");

    assert_eq!(constructed, parsed);
    assert_eq!(parsed.as_bytes(), b"x-trace-id");
    assert_eq!(format!("{parsed}"), "x-trace-id");
    assert_eq!(parsed.index(), None);
}

#[test]
fn every_known_name_indexes_its_own_slot() {
    for (index, name) in FieldName::ALL_KNOWN.iter().enumerate() {
        assert_eq!(name.index(), Some(index));
        assert_eq!(FieldName::try_from_bytes(name.as_str().as_bytes()).expect("a valid name"), *name);
    }
}

#[test]
fn delimited_items_cross_lines_and_preserve_quoted_commas() {
    let stored = [FieldValue::from_static("a, \"b,c\""), FieldValue::from_static("d")];
    let values = FieldLines::from_slice(&REQUEST_ID, &stored).expect("stored lines are present");
    let items: Result<Vec<Vec<u8>>, _> = values.comma_items().map(|item| item.map(<[u8]>::to_vec)).collect();
    assert_eq!(items, Ok(vec![b"a".to_vec(), b"\"b,c\"".to_vec(), b"d".to_vec()]));
}

#[test]
fn values_can_be_reiterated_without_collecting() {
    let stored = [FieldValue::from_static("a"), FieldValue::from_static("b")];
    let values = FieldLines::from_slice(&REQUEST_ID, &stored).expect("stored lines are present");
    assert_eq!(values.repeated().count(), 2);
    assert_eq!(values.repeated().count(), 2);
}

#[test]
fn comma_items_ignore_empty_members() {
    let stored = [FieldValue::from_static(",, a,"), FieldValue::from_static(" , b ,,")];
    let values = FieldLines::from_slice(&REQUEST_ID, &stored).expect("stored lines are present");
    let items: Result<Vec<Vec<u8>>, _> = values.comma_items().map(|item| item.map(<[u8]>::to_vec)).collect();
    assert_eq!(items, Ok(vec![b"a".to_vec(), b"b".to_vec()]));
}

#[test]
fn constructors_distinguish_absence_from_an_empty_field_line() {
    assert!(FieldLines::from_slice(&REQUEST_ID, &[]).is_none());
    let single = FieldLines::single(&REQUEST_ID, b"a");
    assert_eq!(single.len(), 1);
    assert_eq!(single.repeated().count(), 1);

    let empty_line = FieldLines::single(&REQUEST_ID, b"");
    assert_eq!(empty_line.len(), 1);
    assert_eq!(empty_line.repeated().count(), 1);
}

#[test]
fn debug_does_not_expose_field_values() {
    let stored = [FieldValue::from_static("secret")];
    let values = FieldLines::from_slice(&REQUEST_ID, &stored).expect("stored lines are present");
    let debug = format!("{values:?}");
    assert!(!debug.contains("secret"));
    assert!(debug.contains("line_count"));

    let debug = format!("{:?}", values.comma_items());
    assert!(!debug.contains("secret"));
}

#[test]
fn custom_single_value_header_uses_all_map_operations() {
    let mut map = TestMap::new();
    RequestId::insert(&mut map, RequestId(FieldValue::from_static("abc-123"))).expect("header map has capacity");

    assert!(map.contains(&REQUEST_ID));
    assert_eq!(
        RequestId::view(&map).expect("valid request ID").expect("request ID present").0,
        "abc-123"
    );
    assert_eq!(RequestId::owned(&map), Ok(Some(RequestId(FieldValue::from_static("abc-123")))));

    RequestId::remove(&mut map);
    assert!(!map.contains(&REQUEST_ID));
}

#[test]
fn typed_custom_name_finds_an_owned_runtime_name() {
    let mut map = TestMap::new();
    map.insert(
        FieldName::try_from_bytes(b"X-Request-Id").expect("valid runtime name"),
        FieldValue::from_static("from-wire"),
    );

    assert_eq!(
        RequestId::view(&map).expect("valid request ID").expect("request ID present").0,
        "from-wire"
    );
    assert_eq!(map.names().next().expect("stored runtime name").as_str(), "x-request-id");
}

#[test]
fn source_presence_distinguishes_an_empty_field_line_from_absence() {
    let mut map = TestMap::new();
    assert!(map.lines(&FieldName::UserAgent).is_none());

    map.insert(FieldName::UserAgent, FieldValue::from_static(""));
    let values = map.lines(&FieldName::UserAgent).expect("zero-length field line is present");
    assert_eq!(values.len(), 1);
    assert_eq!(values.exactly_one().expect("one field line").as_bytes(), b"");
}

#[test]
fn custom_source_decodes_references_into_external_storage() {
    let wire = b"client/1";
    let source = BorrowedSource {
        user_agent: [FieldValueRef::new(wire)],
    };

    assert_eq!(
        UserAgent::view(&source)
            .expect("valid user agent")
            .expect("user agent present")
            .as_bytes(),
        b"client/1"
    );
}

#[test]
fn duplicate_single_value_is_an_error() {
    let mut map = TestMap::new();
    map.append((*REQUEST_ID).clone(), FieldValue::from_static("first"));
    map.append((*REQUEST_ID).clone(), FieldValue::from_static("second"));

    let error = RequestId::view(&map).expect_err("duplicates must fail");
    assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
}

#[test]
fn empty_encoding_removes_a_stale_value() {
    let mut map = TestMap::new();
    map.insert(EMPTY.clone(), FieldValue::from_static("stale"));
    EmptyHeader::insert(&mut map, EmptyHeader).expect("header map has capacity");
    assert!(!map.contains_name(&EMPTY));
}

#[test]
fn insert_replaces_existing_single_and_repeated_values() {
    let mut map = TestMap::new();
    map.insert((*REQUEST_ID).clone(), FieldValue::from_static("old"));
    RequestId::insert(&mut map, RequestId(FieldValue::from_static("new"))).expect("header map has capacity");
    assert_eq!(map.get_all(&REQUEST_ID), [FieldValue::from_static("new")]);

    map.append(FieldName::SetCookie, FieldValue::from_static("stale=first"));
    map.append(FieldName::SetCookie, FieldValue::from_static("stale=second"));
    let mut replacement = SetCookieOwned::new();
    replacement.push_str("fresh=first").expect("valid Set-Cookie value");
    replacement.push_str("fresh=second").expect("valid Set-Cookie value");
    SetCookie::insert(&mut map, replacement).expect("header map has capacity");
    assert_eq!(
        map.get_all(&FieldName::SetCookie)
            .iter()
            .map(FieldValue::as_bytes)
            .collect::<Vec<_>>(),
        [b"fresh=first".as_slice(), b"fresh=second".as_slice()]
    );
}

#[test]
fn built_in_headers_expose_their_well_known_name() {
    fn assert_known<H: Field>(expected: &'static FieldName) {
        assert_eq!(H::name(), expected);
        assert_eq!(H::name().as_str(), expected.as_str());
        assert!(
            H::name().index().expect("a built-in header is well known") < FieldName::COUNT,
            "a well-known name addresses a slot directly"
        );
    }

    assert_known::<AcceptRanges>(&FieldName::AcceptRanges);
    assert_known::<Authorization<Basic>>(&FieldName::Authorization);
    assert_known::<ETag>(&FieldName::Etag);
    assert_known::<IfMatch>(&FieldName::IfMatch);
    assert_known::<IfNoneMatch>(&FieldName::IfNoneMatch);
    assert_known::<IfRange>(&FieldName::IfRange);
    assert_known::<ReferrerPolicy>(&FieldName::ReferrerPolicy);
    assert_known::<SecWebSocketVersion>(&FieldName::SecWebSocketVersion);
    assert_known::<SetCookie>(&FieldName::SetCookie);

    // A downstream header has no dense index, so it is resolved by name.
    assert_eq!(<RequestId as Field>::name().index(), None);
    assert_eq!(<RequestId as Field>::name().as_str(), "x-request-id");
}

#[test]
fn public_header_constructors_cover_success_and_rejection_paths() {
    let basic = AuthorizationOwned::<Basic>::basic(b"user", b"password").expect("valid credentials");
    assert_eq!(basic.encoded_credentials().expect("stored value is valid"), b"dXNlcjpwYXNzd29yZA==");

    ETagOwned::try_from_wire("\"revision\"").expect("quoted wire tag is valid");
    assert_eq!(
        ETagOwned::try_from_wire("revision").expect_err("wire tags require quotes").kind(),
        DecodeErrorKind::InvalidSyntax
    );

    let tag = ETagOwned::strong("revision").expect("valid tag");
    IfMatchOwned::from_tags([tag.clone()]).expect("nonempty tag list is valid");
    assert_eq!(
        IfMatchOwned::from_tags(Vec::<ETagOwned>::new())
            .expect_err("an empty tag list is invalid")
            .kind(),
        DecodeErrorKind::MissingValue
    );
    IfNoneMatchOwned::from_tags([tag.clone()]).expect("nonempty tag list is valid");
    assert_eq!(
        IfNoneMatchOwned::from_tags(Vec::<ETagOwned>::new())
            .expect_err("an empty tag list is invalid")
            .kind(),
        DecodeErrorKind::MissingValue
    );

    IfRangeOwned::entity_tag(tag).expect("strong entity tag is valid for If-Range");
    assert_eq!(
        IfRangeOwned::entity_tag(ETagOwned::weak("revision").expect("valid weak tag"))
            .expect_err("If-Range requires a strong tag")
            .kind(),
        DecodeErrorKind::InvalidSyntax
    );

    AcceptRangesOwned::from_units(["bytes", "items"]).expect("token range units are valid");
    assert_eq!(
        AcceptRangesOwned::from_units(["none", "bytes"])
            .expect_err("none cannot be combined with another unit")
            .kind(),
        DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        AcceptRangesOwned::from_units(Vec::<String>::new())
            .expect_err("the unit list cannot be empty")
            .kind(),
        DecodeErrorKind::MissingValue
    );

    let policies = ReferrerPolicyOwned::new(ReferrerPolicyValue::NoReferrer).with_fallback(ReferrerPolicyValue::StrictOrigin);
    assert_eq!(
        policies.preferred().expect("fallback list is valid"),
        ReferrerPolicyValue::StrictOrigin
    );

    let versions = SecWebSocketVersionOwned::new(13).with_version(8);
    assert_eq!(versions.versions().collect::<Vec<_>>(), [8, 13]);
}

#[test]
fn credentials_release_excess_capacity() {
    let mut map = TestMap::new();
    Authorization::<Basic>::insert(
        &mut map,
        AuthorizationOwned::<Basic>::basic([b'a'; 64], b"").expect("valid credentials"),
    )
    .expect("header map has capacity");
    let authorization = Authorization::<Basic>::view(&map)
        .expect("valid credentials")
        .expect("authorization present");
    let mut credentials = BasicCredentials::with_retain_limit(8);
    authorization.extract(&mut credentials).expect("valid credentials");
    assert!(credentials.capacity() >= 64);
    credentials.clear();
    assert!(credentials.capacity() <= 8);
    assert!(credentials.username().is_empty());
    assert!(credentials.password().is_empty());
}

#[test]
fn credentials_can_release_or_retain_their_allocation() {
    let mut map = TestMap::new();
    Authorization::<Basic>::insert(
        &mut map,
        AuthorizationOwned::<Basic>::basic(b"user", b"password").expect("valid credentials"),
    )
    .expect("header map has capacity");
    let authorization = Authorization::<Basic>::view(&map)
        .expect("valid credentials")
        .expect("authorization present");

    let mut released = BasicCredentials::with_retain_limit(0);
    authorization.extract(&mut released).expect("valid credentials");
    released.clear();
    assert_eq!(released.capacity(), 0);

    let mut retained = BasicCredentials::with_retain_limit(usize::MAX);
    authorization.extract(&mut retained).expect("valid credentials");
    let capacity = retained.capacity();
    retained.clear();
    assert_eq!(retained.capacity(), capacity);
}

/// Covers the one storage limit only the adapted `http::HeaderMap` imposes.
#[cfg(feature = "http")]
#[test]
fn insert_reports_full_map_without_replacing_values() {
    use http::{HeaderMap, HeaderValue};

    let old = HeaderValue::from_static("old");
    let mut map = HeaderMap::new();
    map.insert(REQUEST_ID.as_str(), old.clone());
    for index in 0_u64.. {
        let name = HeaderName::try_from(format!("x-fill-{index}")).expect("generated header name is valid");
        if map.try_insert(name, HeaderValue::from_static("fill")).is_err() {
            break;
        }
    }

    let result = RequestId::insert(&mut map, RequestId(FieldValue::from_static("new")));
    assert!(result.is_err());
    assert_eq!(map.get(REQUEST_ID.as_str()), Some(&old));

    let append_result = map.append_encoded(&REQUEST_ID, FieldValue::from_static("additional"));
    assert!(append_result.is_err());
    assert_eq!(map.get(REQUEST_ID.as_str()), Some(&old));
}

#[test]
fn content_length_formats_parses_and_inserts() {
    let value = ContentLengthOwned::new(42);
    assert_eq!(value.get(), 42);
    assert_eq!(value.to_string(), "42");
    assert_eq!(" \t42 ".parse::<ContentLengthOwned>(), Ok(value));
    assert_eq!(
        "42x".parse::<ContentLengthOwned>().expect_err("trailing data must fail").kind(),
        DecodeErrorKind::InvalidNumber
    );

    let mut source = TestMap::new();
    ContentLength::insert(&mut source, value).expect("in-memory insertion succeeds");
    assert_eq!(ContentLength::view(&source), Ok(Some(value)));
    assert_eq!(ContentLength::owned(&source), Ok(Some(value)));
    source.remove_values(&FieldName::ContentLength);
    assert_eq!(ContentLength::view(&source), Ok(None));
}

#[test]
fn content_length_decodes_single_repeated_and_missing_lines() {
    assert_eq!(
        ContentLength::view(&content_length_source(&[" 7\t"])),
        Ok(Some(ContentLengthOwned::new(7)))
    );
    assert_eq!(
        ContentLength::view(&content_length_source(&["7, 7", " 7 ", "7,7", "7"])),
        Ok(Some(ContentLengthOwned::new(7)))
    );

    for values in [&["7,8"][..], &["7", "8"], &[","]] {
        ContentLength::view(&content_length_source(values)).expect_err("inconsistent or empty content lengths are invalid");
    }

    assert_eq!(
        ContentLength::view(&content_length_source(&[""]))
            .expect_err("empty value is invalid")
            .kind(),
        DecodeErrorKind::InvalidNumber
    );
}

#[cfg(feature = "http")]
#[test]
fn publicly_constructible_names_convert_without_panicking() {
    let names = FieldName::ALL_KNOWN
        .iter()
        .cloned()
        .chain([
            FieldName::from_static("x-trace-id"),
            FieldName::try_from_bytes(b"X-Vendor-Field").expect("valid name"),
            FieldName::try_from_bytes(vec![b'a'; (1 << 16) - 1]).expect("valid name"),
            FieldName::from(&HeaderName::from_lowercase(b"x\"y").expect("http admits `\"`")),
        ])
        .collect::<Vec<_>>();

    for name in names {
        let converted = name.try_to_http_header_name().expect("every name converts");
        assert_eq!(converted.as_str(), name.as_str());
        assert_eq!(HeaderName::from(&name), converted);
        let round_trip = FieldName::from(&converted);
        assert_eq!(round_trip, name);
        assert_eq!(round_trip.index(), name.index());
    }
}

#[cfg(feature = "http")]
#[test]
fn http_names_holding_a_quote_convert_without_panicking() {
    let quoted = HeaderName::from_lowercase(b"x\"y").expect("http admits `\"`");
    let converted = FieldName::from(&quoted);
    assert_eq!(converted.as_str(), "x\"y");
    assert_eq!(converted.index(), None);
    assert_eq!(FieldName::from(quoted.clone()), converted);

    assert_eq!(HeaderName::from(&converted), quoted);
    assert_eq!(converted.try_to_http_header_name().expect("round trips"), quoted);

    FieldName::try_from_bytes(b"x\"y").expect_err("this crate's own parser rejects `\"`");
}

#[test]
fn field_names_respect_the_http_length_limit() {
    const MAX: usize = (1 << 16) - 1;

    FieldName::try_from_bytes(vec![b'a'; MAX + 1]).expect_err("an over-long name is rejected");
    panic::catch_unwind(|| FieldName::from_static(String::from_utf8(vec![b'a'; MAX + 1]).expect("ASCII").leak()))
        .expect_err("an over-long static name panics");

    let longest = FieldName::try_from_bytes(vec![b'a'; MAX]).expect("the longest name is valid");
    assert_eq!(longest.as_str().len(), MAX);

    #[cfg(feature = "http")]
    {
        let converted = HeaderName::from(&longest);
        assert_eq!(converted.as_str().len(), MAX);
        assert_eq!(FieldName::from(&converted), longest);
    }
}

#[test]
fn field_name_recognition_only_uses_same_length_candidates() {
    for name in FieldName::ALL_KNOWN {
        let mut shorter = name.as_str().as_bytes().to_vec();
        let _removed = shorter.pop();
        let mut longer = name.as_str().as_bytes().to_vec();
        longer.push(b'x');

        for altered in [shorter, longer] {
            let parsed = FieldName::try_from_bytes(&altered).expect("still a token");
            assert_ne!(parsed, *name);
        }
    }

    assert_eq!(FieldName::try_from_bytes(b"a").expect("valid").index(), None);
    let over_long = vec![b'a'; 64];
    assert_eq!(FieldName::try_from_bytes(&over_long).expect("valid").index(), None);
}
