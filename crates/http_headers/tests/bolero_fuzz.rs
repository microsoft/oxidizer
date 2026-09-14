// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded Bolero properties for header parsing and encoding.

#![cfg(feature = "headers-all")]

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;
use std::{iter, slice, str};

use http_headers::headers::{
    Accept, AcceptEncoding, AcceptLanguage, AcceptRanges, AccessControlAllowCredentials, AccessControlAllowHeaders,
    AccessControlAllowMethods, AccessControlAllowOrigin, AccessControlExposeHeaders, AccessControlMaxAge, AccessControlRequestHeaders,
    AccessControlRequestMethod, Allow, Authorization, AuthorizationOwned, Basic, BasicCredentials, Bearer, CacheControl, CacheControlOwned,
    ContentLength, ContentLengthOwned, ContentRange, ContentSecurityPolicy, ContentType, ContentTypeOwned, ETag, ETagOwned, Host, IfMatch,
    IfModifiedSince, IfNoneMatch, IfRange, IfUnmodifiedSince, LastModified, Location, LocationOwned, MediaTypeParameterView, Range,
    ReferrerPolicy, SecWebSocketAccept, SecWebSocketExtensions, SecWebSocketKey, SecWebSocketProtocol, SecWebSocketVersion, Server,
    SetCookie, SetCookieOwned, StrictTransportSecurity, UserAgent, UserAgentOwned, Vary, XContentTypeOptions,
};
use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_LIST_ITEMS};
use http_headers::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef};

const BOUNDED_ITERATIONS: usize = 2_048;
const BOUNDED_TEST_TIME: Duration = Duration::from_millis(400);
const MAX_VALUE_LENGTH: usize = 512;
const MAX_FIELD_LINES_U8: u8 = 3;

static COMMA_LIST: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-comma-list"));
static SEMICOLON_LIST: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-semicolon-list"));

#[derive(Default)]
struct TestMap(HashMap<FieldName, Vec<FieldValue>>);

impl TestMap {
    fn new() -> Self {
        Self::default()
    }

    fn append(&mut self, name: FieldName, value: FieldValue) {
        self.0.entry(name).or_default().push(value);
    }

    fn get_all(&self, name: &FieldName) -> &[FieldValue] {
        self.0.get(name).map_or(&[], Vec::as_slice)
    }
}

impl FieldSource for TestMap {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_slice(name, self.get_all(name))
    }
}

impl FieldSink for TestMap {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if values.is_empty() {
            self.0.remove(name);
        } else {
            self.0.insert(name.clone(), values.into_iter().collect());
        }
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.0.entry(name.clone()).or_default().extend(values);
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        self.0.remove(name);
    }
}

macro_rules! bounded {
    () => {
        bolero::check!()
            .with_iterations(BOUNDED_ITERATIONS)
            .with_test_time(BOUNDED_TEST_TIME)
    };
}

#[derive(Debug, Eq, PartialEq)]
enum ParseOutcome {
    Parsed(Vec<Vec<u8>>),
    Rejected(DecodeErrorKind, Option<usize>),
}

#[derive(Debug, Eq, PartialEq)]
enum ContentTypeOutcome {
    Parsed {
        type_: String,
        subtype: String,
        parameters: Vec<(String, Vec<u8>)>,
    },
    Rejected(DecodeErrorKind, Option<usize>),
    Absent,
}

#[derive(Debug, Eq, PartialEq)]
struct CommaItems(Vec<Vec<u8>>);

#[derive(Debug, Eq, PartialEq)]
struct SemicolonItems(Vec<Vec<u8>>);

type DelimitedOutcome = Result<Vec<Vec<u8>>, (DecodeErrorKind, Option<usize>)>;

impl Field for CommaItems {
    type View<'a> = Self;
    type Owned = Self;

    fn name() -> &'static FieldName {
        &COMMA_LIST
    }

    fn view_with<S>(source: &S, _mode: http_headers::DecodeMode) -> Result<Option<Self::View<'_>>, http_headers::DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines
            .comma_items()
            .map(|item| item.map(<[u8]>::to_vec))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
            .map(Some)
    }

    fn owned_with<S>(source: &S, mode: http_headers::DecodeMode) -> Result<Option<Self::Owned>, http_headers::DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, mode)
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), encode_items(&value.0))
    }
}

impl Field for SemicolonItems {
    type View<'a> = Self;
    type Owned = Self;

    fn name() -> &'static FieldName {
        &SEMICOLON_LIST
    }

    fn view_with<S>(source: &S, _mode: http_headers::DecodeMode) -> Result<Option<Self::View<'_>>, http_headers::DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines
            .semicolon_items()
            .map(|item| item.map(<[u8]>::to_vec))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
            .map(Some)
    }

    fn owned_with<S>(source: &S, mode: http_headers::DecodeMode) -> Result<Option<Self::Owned>, http_headers::DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, mode)
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), encode_items(&value.0))
    }
}

fn encode_items(items: &[Vec<u8>]) -> EncodedValues {
    EncodedValues::from_vec(
        items
            .iter()
            .map(|item| FieldValue::from_bytes(item).expect("delimited items came from valid field-value bytes"))
            .collect(),
    )
}

fn valid_field_byte(byte: u8) -> u8 {
    if byte == b'\t' || byte >= b' ' && byte != 0x7f {
        byte
    } else {
        const STRUCTURAL: &[u8] = b",;\"\\ =/%#";
        STRUCTURAL[usize::from(byte) % STRUCTURAL.len()]
    }
}

fn shaped_field_value(input: &[u8]) -> FieldValue {
    let bytes: Vec<_> = input.iter().copied().take(MAX_VALUE_LENGTH).map(valid_field_byte).collect();
    FieldValue::from_bytes(&bytes).expect("the generator emits only valid field-value bytes")
}

fn shaped_field_values(input: &[u8]) -> Vec<FieldValue> {
    let (selector, input) = input.split_first().map_or((0, &[][..]), |(first, rest)| (*first, rest));
    let count = usize::from(selector % MAX_FIELD_LINES_U8) + 1;
    (0..count)
        .map(|index| {
            let start = input.len() * index / count;
            let end = input.len() * (index + 1) / count;
            shaped_field_value(&input[start..end])
        })
        .collect()
}

fn map_with_values<H: Field>(values: &[FieldValue]) -> TestMap {
    let mut map = TestMap::new();
    for value in values {
        map.append(H::name().clone(), value.clone());
    }
    map
}

fn inserted_bytes<H: Field>(header: H::Owned) -> Vec<Vec<u8>> {
    let mut map = TestMap::new();
    H::insert(&mut map, header).expect("an empty header map has capacity");
    map.get_all(H::name()).iter().map(|value| value.as_bytes().to_vec()).collect()
}

fn borrowed_outcome<H: Field>(map: &TestMap) -> ParseOutcome {
    match H::view(map) {
        Ok(Some(_view)) => ParseOutcome::Parsed(Vec::new()),
        Ok(None) => ParseOutcome::Parsed(Vec::new()),
        Err(error) => ParseOutcome::Rejected(error.kind(), error.value_index()),
    }
}

fn owned_outcome<H: Field>(map: &TestMap) -> ParseOutcome {
    match H::owned(map) {
        Ok(Some(header)) => ParseOutcome::Parsed(inserted_bytes::<H>(header)),
        Ok(None) => ParseOutcome::Parsed(Vec::new()),
        Err(error) => ParseOutcome::Rejected(error.kind(), error.value_index()),
    }
}

fn content_type_projection<'a>(
    type_: Result<&'a str, DecodeError>,
    subtype: Result<&'a str, DecodeError>,
    parameters: impl Iterator<Item = Result<MediaTypeParameterView<'a>, DecodeError>>,
) -> ContentTypeOutcome {
    let type_ = match type_ {
        Ok(type_) => type_,
        Err(error) => return ContentTypeOutcome::Rejected(error.kind(), error.value_index()),
    };
    let subtype = match subtype {
        Ok(subtype) => subtype,
        Err(error) => return ContentTypeOutcome::Rejected(error.kind(), error.value_index()),
    };
    let mut projected_parameters = Vec::new();
    for parameter in parameters {
        match parameter {
            Ok(parameter) => projected_parameters.push((parameter.name().to_owned(), parameter.value().to_vec())),
            Err(error) => return ContentTypeOutcome::Rejected(error.kind(), error.value_index()),
        }
    }
    ContentTypeOutcome::Parsed {
        type_: type_.to_owned(),
        subtype: subtype.to_owned(),
        parameters: projected_parameters,
    }
}

fn content_type_view_outcome(map: &TestMap) -> ContentTypeOutcome {
    match ContentType::view(map) {
        Ok(Some(value)) => content_type_projection(value.type_(), value.subtype(), value.parameters()),
        Ok(None) => ContentTypeOutcome::Absent,
        Err(error) => ContentTypeOutcome::Rejected(error.kind(), error.value_index()),
    }
}

fn content_type_owned_outcome(map: &TestMap) -> ContentTypeOutcome {
    match ContentType::owned(map) {
        Ok(Some(value)) => content_type_projection(value.type_(), value.subtype(), value.parameters()),
        Ok(None) => ContentTypeOutcome::Absent,
        Err(error) => ContentTypeOutcome::Rejected(error.kind(), error.value_index()),
    }
}

fn assert_parser_consistency<H: Field>(values: &[FieldValue]) {
    let map = map_with_values::<H>(values);
    let borrowed = borrowed_outcome::<H>(&map);
    let owned = owned_outcome::<H>(&map);
    if has_independent_owned_decoder::<H>() {
        let owned_status = match &owned {
            ParseOutcome::Parsed(_) => ParseOutcome::Parsed(Vec::new()),
            ParseOutcome::Rejected(kind, index) => ParseOutcome::Rejected(*kind, *index),
        };
        assert_eq!(
            borrowed,
            owned_status,
            "{} borrowed and owned decoders disagree",
            std::any::type_name::<H>()
        );
    }

    if let ParseOutcome::Parsed(encoded) = owned {
        let round_trip_values: Vec<_> = encoded
            .iter()
            .map(|bytes| FieldValue::from_bytes(bytes).expect("a header must encode valid FieldValue bytes"))
            .collect();
        let round_trip = map_with_values::<H>(&round_trip_values);
        assert_eq!(owned_outcome::<H>(&round_trip), ParseOutcome::Parsed(encoded));
    }
}

fn has_independent_owned_decoder<H: Field>() -> bool {
    let id = TypeId::of::<H>();
    [
        TypeId::of::<IfMatch>(),
        TypeId::of::<IfNoneMatch>(),
        TypeId::of::<AcceptRanges>(),
        TypeId::of::<AccessControlAllowOrigin>(),
        TypeId::of::<AccessControlAllowHeaders>(),
        TypeId::of::<AccessControlAllowMethods>(),
        TypeId::of::<AccessControlExposeHeaders>(),
        TypeId::of::<AccessControlRequestHeaders>(),
        TypeId::of::<AccessControlRequestMethod>(),
        TypeId::of::<SecWebSocketVersion>(),
        TypeId::of::<ReferrerPolicy>(),
    ]
    .contains(&id)
}

fn assert_duplicate_singleton_rejected<H: Field>(value: FieldValue) {
    let values = [value.clone(), value];
    assert_eq!(
        owned_outcome::<H>(&map_with_values::<H>(&values)),
        ParseOutcome::Rejected(DecodeErrorKind::UnexpectedMultipleValues, None)
    );
}

fn assert_absence_differs_from_empty<H: Field>() {
    assert_eq!(owned_outcome::<H>(&TestMap::new()), ParseOutcome::Parsed(Vec::new()));
    let empty = [FieldValue::from_static("")];
    assert!(
        matches!(owned_outcome::<H>(&map_with_values::<H>(&empty)), ParseOutcome::Rejected(_, _)),
        "a present empty {} value must not be treated as absence",
        H::name()
    );
}

fn assert_constructed_round_trip<H>(header: H::Owned)
where
    H: Field,
{
    let mut map = TestMap::new();
    H::insert(&mut map, header).expect("an empty header map has capacity");
    let expected: Vec<_> = map.get_all(H::name()).iter().map(|value| value.as_bytes().to_vec()).collect();
    assert_eq!(borrowed_outcome::<H>(&map), ParseOutcome::Parsed(Vec::new()));
    assert_eq!(owned_outcome::<H>(&map), ParseOutcome::Parsed(expected.clone()));

    let values: Vec<_> = expected
        .iter()
        .map(|bytes| FieldValue::from_bytes(bytes).expect("typed encoding must produce FieldValue bytes"))
        .collect();
    assert_eq!(owned_outcome::<H>(&map_with_values::<H>(&values)), ParseOutcome::Parsed(expected));
}

fn trim_ows(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|byte| !matches!(byte, b' ' | b'\t')).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

fn push_reference_item(output: &mut Vec<Vec<u8>>, item: &[u8]) -> Result<(), (DecodeErrorKind, Option<usize>)> {
    if output.len() == MAX_CUSTOM_LIST_ITEMS {
        return Err((DecodeErrorKind::InvalidSyntax, None));
    }
    output.push(item.to_vec());
    Ok(())
}

fn reference_delimited(values: &[FieldValue], delimiter: u8) -> DelimitedOutcome {
    let mut output = Vec::new();
    for (value_index, value) in values.iter().enumerate() {
        let bytes = value.as_bytes();
        let mut start = 0;
        let mut quoted = false;
        let mut escaped = false;
        for (position, byte) in bytes.iter().copied().enumerate() {
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            } else if !quoted && byte == delimiter {
                let item = trim_ows(&bytes[start..position]);
                if delimiter != b',' || !item.is_empty() {
                    push_reference_item(&mut output, item)?;
                }
                start = position + 1;
            }
        }
        if quoted || escaped {
            return Err((DecodeErrorKind::UnterminatedQuote, Some(value_index)));
        }
        let item = trim_ows(&bytes[start..]);
        if delimiter != b',' || !item.is_empty() {
            push_reference_item(&mut output, item)?;
        }
    }
    Ok(output)
}

fn delimited_outcome<H: Field>(values: &[FieldValue]) -> DelimitedOutcome
where
    for<'a> H::View<'a>: IntoDelimitedItems,
{
    let map = map_with_values::<H>(values);
    match H::view(&map) {
        Ok(Some(items)) => Ok(items.into_items()),
        Ok(None) => Ok(Vec::new()),
        Err(error) => Err((error.kind(), error.value_index())),
    }
}

trait IntoDelimitedItems {
    fn into_items(self) -> Vec<Vec<u8>>;
}

impl IntoDelimitedItems for CommaItems {
    fn into_items(self) -> Vec<Vec<u8>> {
        self.0
    }
}

impl IntoDelimitedItems for SemicolonItems {
    fn into_items(self) -> Vec<Vec<u8>> {
        self.0
    }
}

fn token_from(input: &[u8], salt: u8) -> String {
    const TOKEN: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&'*+-.^_`|~";
    let mut output = String::new();
    for byte in input.iter().copied().take(24) {
        output.push(char::from(TOKEN[usize::from(byte.wrapping_add(salt)) % TOKEN.len()]));
    }
    if output.is_empty() {
        output.push(char::from(TOKEN[usize::from(salt) % TOKEN.len()]));
    }
    output
}

fn token68_from(input: &[u8]) -> String {
    const TOKEN68: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~+/";
    let mut output = String::new();
    for byte in input.iter().copied().take(24) {
        output.push(char::from(TOKEN68[usize::from(byte) % TOKEN68.len()]));
    }
    if output.is_empty() {
        output.push('A');
    }
    output.extend(iter::repeat_n('=', usize::from(input.first().copied().unwrap_or(0) % 3)));
    output
}

fn opaque_from(input: &[u8]) -> String {
    input
        .iter()
        .copied()
        .take(32)
        .map(|byte| {
            let printable = b'!' + byte % (b'~' - b'!' + 1);
            char::from(if printable == b'"' { b'#' } else { printable })
        })
        .collect()
}

fn quoted_text_from(input: &[u8]) -> String {
    const QUOTED: &[u8] = b"abcXYZ019 ,;=";
    input
        .iter()
        .copied()
        .take(24)
        .map(|byte| char::from(QUOTED[usize::from(byte) % QUOTED.len()]))
        .collect()
}

fn bounded_bytes(input: &[u8], start: usize) -> Vec<u8> {
    input.iter().copied().skip(start).take(64).collect()
}

#[test]
fn arbitrary_header_values_do_not_panic() {
    bounded!().for_each(|input: &[u8]| {
        let values = shaped_field_values(input);
        assert_parser_consistency::<Authorization<Bearer>>(&values);
        assert_parser_consistency::<Authorization<Basic>>(&values);
        assert_parser_consistency::<CacheControl>(&values);
        assert_parser_consistency::<ContentLength>(&values);
        assert_parser_consistency::<ContentType>(&values);
        assert_parser_consistency::<ETag>(&values);
        assert_parser_consistency::<Location>(&values);
        assert_parser_consistency::<SetCookie>(&values);
        assert_parser_consistency::<UserAgent>(&values);

        let basic_map = map_with_values::<Authorization<Basic>>(&values);
        let mut credentials = BasicCredentials::new();
        if let Ok(Some(authorization)) = Authorization::<Basic>::view(&basic_map) {
            let _ = authorization.extract(&mut credentials);
        }
    });
}

#[test]
fn expanded_header_values_do_not_panic() {
    bounded!().for_each(|input: &[u8]| {
        let values = shaped_field_values(input);
        assert_parser_consistency::<Host>(&values);
        assert_parser_consistency::<Accept>(&values);
        assert_parser_consistency::<AcceptEncoding>(&values);
        assert_parser_consistency::<AcceptLanguage>(&values);
        assert_parser_consistency::<Allow>(&values);
        assert_parser_consistency::<Vary>(&values);
        assert_parser_consistency::<Server>(&values);
        assert_parser_consistency::<IfMatch>(&values);
        assert_parser_consistency::<IfNoneMatch>(&values);
        assert_parser_consistency::<IfModifiedSince>(&values);
        assert_parser_consistency::<IfUnmodifiedSince>(&values);
        assert_parser_consistency::<LastModified>(&values);
        assert_parser_consistency::<IfRange>(&values);
        assert_parser_consistency::<Range>(&values);
        assert_parser_consistency::<ContentRange>(&values);
        assert_parser_consistency::<AcceptRanges>(&values);
        assert_parser_consistency::<AccessControlAllowOrigin>(&values);
        assert_parser_consistency::<AccessControlAllowCredentials>(&values);
        assert_parser_consistency::<AccessControlAllowHeaders>(&values);
        assert_parser_consistency::<AccessControlAllowMethods>(&values);
        assert_parser_consistency::<AccessControlExposeHeaders>(&values);
        assert_parser_consistency::<AccessControlMaxAge>(&values);
        assert_parser_consistency::<AccessControlRequestHeaders>(&values);
        assert_parser_consistency::<AccessControlRequestMethod>(&values);
        assert_parser_consistency::<SecWebSocketKey>(&values);
        assert_parser_consistency::<SecWebSocketAccept>(&values);
        assert_parser_consistency::<SecWebSocketVersion>(&values);
        assert_parser_consistency::<SecWebSocketProtocol>(&values);
        assert_parser_consistency::<SecWebSocketExtensions>(&values);
        assert_parser_consistency::<StrictTransportSecurity>(&values);
        assert_parser_consistency::<ContentSecurityPolicy>(&values);
        assert_parser_consistency::<XContentTypeOptions>(&values);
        assert_parser_consistency::<ReferrerPolicy>(&values);
    });
}

#[test]
fn comma_and_quoted_delimiters_match_reference() {
    bounded!().for_each(|input: &[u8]| {
        let values = shaped_field_values(input);
        let comma = reference_delimited(&values, b',');
        assert_eq!(delimited_outcome::<CommaItems>(&values), comma);

        let semicolon = reference_delimited(&values, b';');
        assert_eq!(delimited_outcome::<SemicolonItems>(&values), semicolon);
    });
}

#[test]
fn content_type_cached_and_uncached_agree() {
    bounded!().for_each(|input: &[u8]| {
        let value = shaped_field_value(input);
        let map = map_with_values::<ContentType>(slice::from_ref(&value));
        let first_view = content_type_view_outcome(&map);
        let repeated_view = content_type_view_outcome(&map);
        let first_owned = content_type_owned_outcome(&map);
        let repeated_owned = content_type_owned_outcome(&map);

        if let ContentTypeOutcome::Parsed { type_, subtype, .. } = &first_view
            && let Ok(wire) = str::from_utf8(value.as_bytes())
            && let Ok(reference) = wire.parse::<headers::Mime>()
        {
            let reference_subtype = reference.suffix().map_or_else(
                || reference.subtype().as_str().to_owned(),
                |suffix| format!("{}+{}", reference.subtype(), suffix),
            );
            assert!(type_.eq_ignore_ascii_case(reference.type_().as_str()));
            assert!(subtype.eq_ignore_ascii_case(&reference_subtype));
        }
        assert_ne!(first_view, ContentTypeOutcome::Absent);
        assert_eq!(repeated_view, first_view);
        assert_eq!(first_owned, first_view);
        assert_eq!(repeated_owned, first_view);
    });
}

#[test]
fn scalar_and_simd_agree() {
    bounded!().with_type::<(Vec<u8>, Vec<u8>)>().for_each(|(left, right)| {
        let left = &left[..left.len().min(MAX_VALUE_LENGTH)];
        let right = &right[..right.len().min(MAX_VALUE_LENGTH)];
        let token = !left.is_empty()
            && left
                .iter()
                .copied()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
        let token68_data_end = left.iter().position(|byte| *byte == b'=').unwrap_or(left.len());
        let token68 = token68_data_end != 0
            && left[..token68_data_end]
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(byte))
            && left[token68_data_end..].iter().all(|byte| *byte == b'=');
        let field_value = left.iter().copied().all(|byte| byte == b'\t' || byte >= b' ' && byte != 0x7f);
        let equal = left.len() == right.len() && left.iter().zip(right).all(|(left, right)| left.eq_ignore_ascii_case(right));
        let interesting = left.iter().position(|byte| b",;\"\\ \t".contains(byte));

        assert_eq!(http_headers_simd::is_token(left), token);
        assert_eq!(http_headers_simd::is_token68(left), token68);
        assert_eq!(http_headers_simd::is_field_value(left), field_value);
        assert_eq!(http_headers_simd::eq_ignore_ascii_case(left, right), equal);
        assert_eq!(http_headers_simd::find_interesting(left), interesting);
    });
}

#[test]
fn typed_round_trips() {
    bounded!().for_each(|input: &[u8]| {
        let number = input
            .iter()
            .copied()
            .take(8)
            .fold(0_u64, |value, byte| value.rotate_left(8) ^ u64::from(byte));

        let content_length = ContentLengthOwned::new(number);
        assert_eq!(content_length.get(), number);
        assert_constructed_round_trip::<ContentLength>(content_length);

        let opaque = opaque_from(input);
        let etag = if input.first().is_some_and(|byte| byte & 1 == 1) {
            ETagOwned::weak(&opaque).expect("generated opaque tags are valid")
        } else {
            ETagOwned::strong(&opaque).expect("generated opaque tags are valid")
        };
        assert_eq!(etag.opaque_tag(), Ok(opaque.as_bytes()));
        assert_constructed_round_trip::<ETag>(etag);

        let bearer_token = token68_from(input);
        let bearer = AuthorizationOwned::<Bearer>::bearer(&bearer_token).expect("generated token68 is valid");
        assert_eq!(bearer.token(), Ok(bearer_token.as_bytes()));
        assert_constructed_round_trip::<Authorization<Bearer>>(bearer);

        let mut username = bounded_bytes(input, 0);
        username.retain(|byte| *byte != b':');
        let password = bounded_bytes(input, 64);
        let basic = AuthorizationOwned::<Basic>::basic(&username, &password).expect("bounded generated credentials fit");
        let mut map = TestMap::new();
        Authorization::<Basic>::insert(&mut map, basic).expect("an empty header map has capacity");
        let authorization = Authorization::<Basic>::view(&map)
            .expect("constructed Basic authorization must decode")
            .expect("constructed Basic authorization must be present");
        let mut credentials = BasicCredentials::new();
        let credentials = authorization
            .extract(&mut credentials)
            .expect("constructed Basic credentials must extract");
        assert_eq!(credentials.username(), username);
        assert_eq!(credentials.password(), password);
        let basic = Authorization::<Basic>::owned(&map)
            .expect("constructed Basic authorization must decode")
            .expect("constructed Basic authorization must be present");
        assert_constructed_round_trip::<Authorization<Basic>>(basic);

        let type_ = token_from(input, 17);
        let subtype = token_from(input, 93);
        let parameter_text = quoted_text_from(input);
        let parameter_wire = format!("\"{parameter_text}\"");
        let content_type =
            ContentTypeOwned::try_from(format!("{type_}/{subtype}; x-note={parameter_wire}")).expect("generated media type is valid");
        assert_eq!(content_type.type_(), Ok(type_.as_str()));
        assert_eq!(content_type.subtype(), Ok(subtype.as_str()));
        assert_eq!(content_type.parameter("X-NOTE"), Ok(Some(parameter_wire.as_bytes())));
        assert_constructed_round_trip::<ContentType>(content_type);

        let cache_control = CacheControlOwned::builder()
            .no_cache()
            .max_age(Duration::from_secs(number))
            .extension_value("x-note", &parameter_wire)
            .build()
            .expect("the builder contains directives");
        assert!(cache_control.no_cache());
        assert_eq!(cache_control.max_age(), Some(Duration::from_secs(number)));
        assert!(
            cache_control
                .directives()
                .any(|directive| { directive.name() == "x-note" && directive.value() == Some(parameter_wire.as_bytes()) })
        );
        assert_constructed_round_trip::<CacheControl>(cache_control);

        let low_byte = number.to_le_bytes()[0];
        let location_wire = format!("/resource/{number:016x}?q={low_byte:02x}#section");
        let location = LocationOwned::try_from(location_wire.clone()).expect("generated URI is valid");
        assert_eq!(location.as_str(), Ok(location_wire.as_str()));
        assert_constructed_round_trip::<Location>(location);

        let first_cookie = format!("a={number:016x}; Path=/");
        let second_cookie = format!("b={low_byte:02x}; HttpOnly");
        let mut cookies = SetCookieOwned::new();
        cookies.push_str(&first_cookie).expect("generated cookie is nonempty");
        cookies.push_str(&second_cookie).expect("generated cookie is nonempty");
        assert_eq!(
            cookies.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(),
            vec![first_cookie.as_bytes(), second_cookie.as_bytes()]
        );
        assert_constructed_round_trip::<SetCookie>(cookies);

        let user_agent_wire = format!("bolero-agent/{number:016x}");
        let user_agent = UserAgentOwned::try_from(user_agent_wire.clone()).expect("generated user agent is valid");
        assert_eq!(user_agent.as_bytes(), user_agent_wire.as_bytes());
        assert_constructed_round_trip::<UserAgent>(user_agent);
    });
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one corpus-style test keeps the deterministic protocol seed matrix together"
)]
fn protocol_regression_seeds() {
    let bearer_map = map_with_values::<Authorization<Bearer>>(&[FieldValue::from_static("Bearer mF_9.B5f-4.1JqM")]);
    let bearer = Authorization::<Bearer>::view(&bearer_map)
        .expect("RFC bearer example is valid")
        .expect("authorization is present");
    assert_eq!(bearer.token(), b"mF_9.B5f-4.1JqM");

    let basic_map = map_with_values::<Authorization<Basic>>(&[FieldValue::from_static("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==")]);
    let authorization = Authorization::<Basic>::view(&basic_map)
        .expect("RFC Basic example is valid")
        .expect("authorization is present");
    let mut credentials = BasicCredentials::new();
    let basic = authorization.extract(&mut credentials).expect("RFC Basic credentials extract");
    assert_eq!(basic.username(), b"Aladdin");
    assert_eq!(basic.password(), b"open sesame");

    let cache_map = map_with_values::<CacheControl>(&[FieldValue::from_static("no-cache, private, max-age=60, x-note=\"a,b\"")]);
    let cache = CacheControl::view(&cache_map)
        .expect("representative Cache-Control is valid")
        .expect("cache-control is present");
    assert!(cache.no_cache());
    assert_eq!(cache.max_age(), Some(Duration::from_mins(1)));
    assert!(
        cache
            .directives()
            .any(|directive| { directive.name() == "x-note" && directive.value() == Some(b"\"a,b\"".as_slice()) })
    );

    let length_map = map_with_values::<ContentLength>(&[FieldValue::from_static("42, 42")]);
    assert_eq!(ContentLength::view(&length_map), Ok(Some(ContentLengthOwned::new(42))));

    let content_type_map = map_with_values::<ContentType>(&[FieldValue::from_static("text/html; charset=utf-8; boundary=\"a,b\"")]);
    let content_type = ContentType::view(&content_type_map)
        .expect("representative Content-Type is valid")
        .expect("content-type is present");
    assert_eq!(content_type.type_(), Ok("text"));
    assert_eq!(content_type.subtype(), Ok("html"));
    assert_eq!(content_type.parameter("BOUNDARY"), Ok(Some(b"\"a,b\"".as_slice())));

    let strong_map = map_with_values::<ETag>(&[FieldValue::from_static("\"xyzzy\"")]);
    let strong = ETag::view(&strong_map)
        .expect("strong entity tag is valid")
        .expect("etag is present");
    assert!(!strong.is_weak());
    assert_eq!(strong.opaque_tag(), b"xyzzy");
    let weak_map = map_with_values::<ETag>(&[FieldValue::from_static("W/\"xyzzy\"")]);
    let weak = ETag::view(&weak_map).expect("weak entity tag is valid").expect("etag is present");
    assert!(weak.is_weak());
    assert!(strong.weak_eq(weak));

    let location_wire = "https://example.com/a%20b?q=x#frag";
    let location_map = map_with_values::<Location>(&[FieldValue::from_static(location_wire)]);
    let location = Location::view(&location_map)
        .expect("representative Location is valid")
        .expect("location is present");
    assert_eq!(location.as_str(), Ok(location_wire));

    let cookie_values = [
        FieldValue::from_static("session=abc; Path=/; Secure; HttpOnly"),
        FieldValue::from_static("theme=light; SameSite=Lax"),
    ];
    let cookie_map = map_with_values::<SetCookie>(&cookie_values);
    let cookies = SetCookie::view(&cookie_map)
        .expect("representative cookies are valid")
        .expect("set-cookie is present");
    assert_eq!(cookies.len(), 2);
    assert_eq!(
        cookies.iter().map(FieldValueRef::as_bytes).collect::<Vec<_>>(),
        cookie_values.iter().map(FieldValue::as_bytes).collect::<Vec<_>>()
    );

    let user_agent_wire = "ExampleBrowser/1.0 (compatible; TestBot/2.0)";
    let user_agent_map = map_with_values::<UserAgent>(&[FieldValue::from_static(user_agent_wire)]);
    let user_agent = UserAgent::view(&user_agent_map)
        .expect("representative User-Agent is valid")
        .expect("user-agent is present");
    assert_eq!(user_agent.as_str(), Ok(user_agent_wire));

    let values = [
        FieldValue::from_static("a, \"b,c\", d"),
        FieldValue::from_static("x=\"escaped\\\" comma, stays\"; y=z"),
    ];
    assert_eq!(
        reference_delimited(&values, b','),
        Ok(vec![
            b"a".to_vec(),
            b"\"b,c\"".to_vec(),
            b"d".to_vec(),
            b"x=\"escaped\\\" comma, stays\"; y=z".to_vec(),
        ])
    );
    assert_eq!(delimited_outcome::<CommaItems>(&values), reference_delimited(&values, b','));

    let malformed = [FieldValue::from_static("a, \"unterminated")];
    assert_eq!(
        delimited_outcome::<CommaItems>(&malformed),
        Err((DecodeErrorKind::UnterminatedQuote, Some(0)))
    );

    let item_limit_before_quote = [
        FieldValue::from_bytes(vec![b';'; MAX_CUSTOM_LIST_ITEMS - 1]).unwrap(),
        FieldValue::from_static(";\""),
    ];
    assert_eq!(
        reference_delimited(&item_limit_before_quote, b';'),
        Err((DecodeErrorKind::InvalidSyntax, None))
    );
    assert_eq!(
        delimited_outcome::<SemicolonItems>(&item_limit_before_quote),
        Err((DecodeErrorKind::InvalidSyntax, None))
    );

    let all_empty = [FieldValue::from_static(",, ,\t,,")];
    assert_eq!(delimited_outcome::<CommaItems>(&all_empty), Ok(Vec::new()));
    assert_eq!(
        owned_outcome::<CacheControl>(&map_with_values::<CacheControl>(&all_empty)),
        ParseOutcome::Parsed(Vec::new())
    );

    let obs_text = FieldValue::from_bytes([0x80]).expect("obs-text is a valid field value");
    let obs_user_agent_map = map_with_values::<UserAgent>(slice::from_ref(&obs_text));
    let obs_user_agent = UserAgent::view(&obs_user_agent_map)
        .expect("obs-text User-Agent bytes are a valid field value")
        .expect("user-agent is present");
    assert_eq!(
        obs_user_agent.as_str().map_err(|error| error.kind()),
        Err(DecodeErrorKind::InvalidUtf8)
    );
    assert_eq!(
        owned_outcome::<Location>(&map_with_values::<Location>(slice::from_ref(&obs_text))),
        ParseOutcome::Rejected(DecodeErrorKind::InvalidSyntax, None)
    );

    let obs_etag = FieldValue::from_bytes([b'"', 0x80, b'"']).expect("obs-text entity tag is valid");
    let obs_etag_map = map_with_values::<ETag>(&[obs_etag]);
    assert_eq!(
        ETag::view(&obs_etag_map)
            .expect("obs-text entity tag is valid")
            .expect("etag is present")
            .opaque_tag(),
        &[0x80]
    );

    let obs_content_type = FieldValue::from_bytes(b"text/plain; note=\"\x80\"").expect("obs-text quoted parameters are valid field values");
    let obs_content_type_map = map_with_values::<ContentType>(&[obs_content_type]);
    let content_type = ContentType::view(&obs_content_type_map)
        .expect("obs-text quoted parameter is valid")
        .expect("content-type is present");
    assert_eq!(content_type.parameter("note"), Ok(Some(b"\"\x80\"".as_slice())));

    let obs_cache_control = FieldValue::from_bytes(b"x-note=\"\x80\"").expect("obs-text quoted directive is a valid field value");
    assert_eq!(
        owned_outcome::<CacheControl>(&map_with_values::<CacheControl>(&[obs_cache_control])),
        ParseOutcome::Parsed(vec![b"x-note=\"\x80\"".to_vec()])
    );

    assert_duplicate_singleton_rejected::<Authorization<Bearer>>(FieldValue::from_static("Bearer abc"));
    assert_duplicate_singleton_rejected::<Authorization<Basic>>(FieldValue::from_static("Basic dXNlcjpwYXNz"));
    assert_duplicate_singleton_rejected::<ContentType>(FieldValue::from_static("text/plain"));
    assert_duplicate_singleton_rejected::<ETag>(FieldValue::from_static("\"tag\""));
    assert_duplicate_singleton_rejected::<Location>(FieldValue::from_static("/target"));
    assert_duplicate_singleton_rejected::<UserAgent>(FieldValue::from_static("agent/1"));

    let conflicting_lengths = [FieldValue::from_static("31"), FieldValue::from_static("32")];
    assert_eq!(
        owned_outcome::<ContentLength>(&map_with_values::<ContentLength>(&conflicting_lengths)),
        ParseOutcome::Rejected(DecodeErrorKind::InvalidSyntax, Some(1))
    );

    assert_absence_differs_from_empty::<Authorization<Bearer>>();
    assert_absence_differs_from_empty::<Authorization<Basic>>();
    assert_absence_differs_from_empty::<ContentLength>();
    assert_absence_differs_from_empty::<ContentType>();
    assert_absence_differs_from_empty::<ETag>();
    assert_absence_differs_from_empty::<SetCookie>();
    assert_absence_differs_from_empty::<UserAgent>();

    assert_eq!(
        owned_outcome::<CacheControl>(&map_with_values::<CacheControl>(&[FieldValue::from_static(""),])),
        ParseOutcome::Parsed(Vec::new())
    );
    assert_eq!(
        owned_outcome::<Location>(&map_with_values::<Location>(&[FieldValue::from_static("",)])),
        ParseOutcome::Parsed(vec![Vec::new()])
    );

    for length in [31, 32, 33, MAX_VALUE_LENGTH] {
        let value = FieldValue::from_bytes(vec![b'a'; length]).expect("ASCII field value is valid");
        assert_parser_consistency::<UserAgent>(slice::from_ref(&value));
        assert_parser_consistency::<SetCookie>(slice::from_ref(&value));
        assert_eq!(http_headers_simd::find_interesting(value.as_bytes()), None);
    }
}
