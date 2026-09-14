// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coverage for public API conformance guarantees.

use std::fmt::{self, Write as _};
use std::time::Duration;

use http_headers::headers::*;
use http_headers::sink::{EncodedValues, FieldSink, InsertError, ValueRefsEncoder};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeError, FieldName, FieldSensitivity, FieldValue, FieldValueRef};

macro_rules! assert_field_value_conversion {
    ($owned:ty, $wire:literal) => {{
        let from = FieldValue::from(<$owned>::try_from($wire).expect("valid fixture"));
        let into: FieldValue = <$owned>::try_from($wire).expect("valid fixture").into();
        let shim = <$owned>::try_from($wire).expect("valid fixture").into_field_value();
        assert_eq!(from, $wire);
        assert_eq!(into, $wire);
        assert_eq!(shim, $wire);
    }};
}

struct RejectWriter;

impl fmt::Write for RejectWriter {
    fn write_str(&mut self, _value: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn all_21_owned_single_value_types_support_standard_conversion() {
    assert_field_value_conversion!(StrictTransportSecurityOwned, "max-age=60");
    assert_field_value_conversion!(IfRangeOwned, "\"revision\"");
    assert_field_value_conversion!(XContentTypeOptionsOwned, "nosniff");
    assert_field_value_conversion!(IfModifiedSinceOwned, "Sat, 29 Oct 1994 19:43:31 GMT");
    assert_field_value_conversion!(IfUnmodifiedSinceOwned, "Sat, 29 Oct 1994 19:43:31 GMT");
    assert_field_value_conversion!(LastModifiedOwned, "Sat, 29 Oct 1994 19:43:31 GMT");
    assert_field_value_conversion!(RangeOwned, "bytes=0-9");
    assert_field_value_conversion!(ContentRangeOwned, "bytes 0-9/10");
    assert_field_value_conversion!(LocationOwned, "https://example.com/people");
    assert_field_value_conversion!(ContentTypeOwned, "application/json");
    let bearer = AuthorizationOwned::<Bearer>::bearer("abc.def").expect("valid bearer credentials");
    let bearer_from = FieldValue::from(bearer.clone());
    let bearer_into: FieldValue = bearer.clone().into();
    let bearer_shim = bearer.into_field_value();
    for value in [&bearer_from, &bearer_into, &bearer_shim] {
        assert_eq!(value.as_bytes(), b"Bearer abc.def");
        assert!(value.is_sensitive());
    }

    let basic = AuthorizationOwned::<Basic>::basic(b"user", b"pass").expect("valid basic credentials");
    let basic_from = FieldValue::from(basic.clone());
    let basic_into: FieldValue = basic.clone().into();
    let basic_shim = basic.into_field_value();
    for value in [&basic_from, &basic_into, &basic_shim] {
        assert_eq!(value.as_bytes(), b"Basic dXNlcjpwYXNz");
        assert!(value.is_sensitive());
    }
    assert_field_value_conversion!(ETagOwned, "\"revision\"");
    assert_field_value_conversion!(UserAgentOwned, "client/1");
    assert_field_value_conversion!(HostOwned, "example.com");
    assert_field_value_conversion!(ServerOwned, "example/1");
    assert_field_value_conversion!(SecWebSocketKeyOwned, "dGhlIHNhbXBsZSBub25jZQ==");
    assert_field_value_conversion!(SecWebSocketAcceptOwned, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    assert_field_value_conversion!(AccessControlAllowOriginOwned, "https://example.com");
    assert_field_value_conversion!(AccessControlAllowCredentialsOwned, "true");
    assert_field_value_conversion!(AccessControlMaxAgeOwned, "600");
    assert_field_value_conversion!(AccessControlRequestMethodOwned, "DELETE");
}

#[test]
fn collection_displays_propagate_formatter_failures() {
    let ranges = AcceptRangesOwned::from_units(["bytes"]).expect("valid range unit");
    assert!(write!(&mut RejectWriter, "{ranges}").is_err());

    let methods = AccessControlAllowMethodsOwned::from_methods(["GET"]).expect("valid method");
    assert!(write!(&mut RejectWriter, "{methods}").is_err());
}

#[test]
fn all_11_ascii_owned_types_display_canonical_values() {
    assert_eq!(AllowOwned::try_from("GET, HEAD").expect("valid methods").to_string(), "GET, HEAD");
    assert_eq!(
        VaryOwned::try_from("accept, origin").expect("valid field names").to_string(),
        "accept, origin"
    );
    assert_eq!(
        AcceptRangesOwned::from_units(["bytes", "items"])
            .expect("valid range units")
            .to_string(),
        "bytes, items"
    );
    assert_eq!(
        AccessControlAllowHeadersOwned::from_header_names(["content-type", "x-trace-id"])
            .expect("valid header names")
            .to_string(),
        "content-type, x-trace-id"
    );
    assert_eq!(
        AccessControlAllowMethodsOwned::from_methods(["GET", "POST"])
            .expect("valid methods")
            .to_string(),
        "GET, POST"
    );
    assert_eq!(
        AccessControlExposeHeadersOwned::from_header_names(["etag", "x-trace-id"])
            .expect("valid header names")
            .to_string(),
        "etag, x-trace-id"
    );
    assert_eq!(
        AccessControlRequestHeadersOwned::from_header_names(["content-type", "authorization"])
            .expect("valid header names")
            .to_string(),
        "content-type, authorization"
    );
    assert_eq!(
        ReferrerPolicyOwned::new(ReferrerPolicyValue::NoReferrer)
            .with_fallback(ReferrerPolicyValue::StrictOrigin)
            .to_string(),
        "no-referrer, strict-origin"
    );
    assert_eq!(
        SecWebSocketExtensionsOwned::builder()
            .extension("permessage-deflate")
            .parameter_flag("client_max_window_bits")
            .build()
            .expect("nonempty extension list")
            .to_string(),
        "permessage-deflate; client_max_window_bits"
    );
    assert_eq!(
        SecWebSocketProtocolOwned::new("chat")
            .expect("valid protocol")
            .with_protocol("superchat")
            .expect("valid protocol")
            .to_string(),
        "chat, superchat"
    );
    assert_eq!(SecWebSocketVersionOwned::new(13).with_version(8).to_string(), "8, 13");
}

struct EmptySource;

impl FieldSource for EmptySource {
    fn lines(&self, _name: &'static FieldName) -> Option<FieldLines<'_>> {
        None
    }
}

#[derive(Default)]
struct Sink;

impl FieldSource for Sink {
    fn lines(&self, _name: &'static FieldName) -> Option<FieldLines<'_>> {
        None
    }
}

impl FieldSink for Sink {
    fn set_values(&mut self, _name: &'static FieldName, _values: EncodedValues) -> Result<(), InsertError> {
        Ok(())
    }

    fn append_values(&mut self, _name: &'static FieldName, _values: EncodedValues) -> Result<(), InsertError> {
        Ok(())
    }

    fn remove_values(&mut self, _name: &'static FieldName) {}
}

fn assert_debug<T: std::fmt::Debug>() {}

macro_rules! assert_inherent_operations {
    ($(($header:ty, $owned:ty)),+ $(,)?) => {
        $(
            assert_debug::<$header>();
            assert!(
                <$header>::view(&EmptySource)
                    .expect("absence is valid")
                    .is_none()
            );
            assert!(
                <$header>::owned(&EmptySource)
                    .expect("absence is valid")
                    .is_none()
            );
            let _insert: fn(&mut Sink, $owned) -> Result<(), InsertError> =
                <$header>::insert::<Sink>;
            <$header>::remove(&mut Sink);
        )+
    };
}

#[test]
fn all_42_built_in_instantiations_expose_inherent_operations() {
    assert_inherent_operations!(
        (CacheControl, CacheControlOwned),
        (Accept, AcceptOwned),
        (AcceptEncoding, AcceptEncodingOwned),
        (AcceptLanguage, AcceptLanguageOwned),
        (Allow, AllowOwned),
        (Host, HostOwned),
        (Server, ServerOwned),
        (Vary, VaryOwned),
        (AcceptRanges, AcceptRangesOwned),
        (ContentRange, ContentRangeOwned),
        (Range, RangeOwned),
        (ETag, ETagOwned),
        (Location, LocationOwned),
        (UserAgent, UserAgentOwned),
        (ContentType, ContentTypeOwned),
        (IfMatch, IfMatchOwned),
        (IfNoneMatch, IfNoneMatchOwned),
        (IfModifiedSince, IfModifiedSinceOwned),
        (IfUnmodifiedSince, IfUnmodifiedSinceOwned),
        (IfRange, IfRangeOwned),
        (LastModified, LastModifiedOwned),
        (AccessControlAllowCredentials, AccessControlAllowCredentialsOwned),
        (AccessControlAllowHeaders, AccessControlAllowHeadersOwned),
        (AccessControlAllowMethods, AccessControlAllowMethodsOwned),
        (AccessControlAllowOrigin, AccessControlAllowOriginOwned),
        (AccessControlExposeHeaders, AccessControlExposeHeadersOwned),
        (AccessControlMaxAge, AccessControlMaxAgeOwned),
        (AccessControlRequestHeaders, AccessControlRequestHeadersOwned),
        (AccessControlRequestMethod, AccessControlRequestMethodOwned),
        (ContentSecurityPolicy, ContentSecurityPolicyOwned),
        (ReferrerPolicy, ReferrerPolicyOwned),
        (StrictTransportSecurity, StrictTransportSecurityOwned),
        (XContentTypeOptions, XContentTypeOptionsOwned),
        (SecWebSocketAccept, SecWebSocketAcceptOwned),
        (SecWebSocketExtensions, SecWebSocketExtensionsOwned),
        (SecWebSocketKey, SecWebSocketKeyOwned),
        (SecWebSocketProtocol, SecWebSocketProtocolOwned),
        (SecWebSocketVersion, SecWebSocketVersionOwned),
        (Authorization<Basic>, AuthorizationOwned<Basic>),
        (Authorization<Bearer>, AuthorizationOwned<Bearer>),
        (SetCookie, SetCookieOwned),
        (ContentLength, ContentLengthOwned),
    );
}

fn generic_owned<H: http_headers::Field>(source: &EmptySource) -> Result<Option<H::Owned>, DecodeError> {
    H::owned(source)
}

#[test]
fn generic_field_behavior_is_retained() {
    assert!(generic_owned::<UserAgent>(&EmptySource).expect("absence is valid").is_none());
}

#[test]
fn borrowed_byte_input_constructors_accept_owned_values() {
    assert_eq!(
        FieldName::try_from_bytes(Vec::from(&b"Accept"[..])).expect("valid name"),
        FieldName::Accept
    );
    assert_eq!(
        FieldValue::from_bytes(Vec::from(&b"gzip"[..])).expect("valid bytes").as_bytes(),
        b"gzip"
    );
    assert_eq!(
        AuthorizationOwned::<Basic>::basic(Vec::from(&b"user"[..]), Vec::from(&b"pass"[..]))
            .expect("valid credentials")
            .encoded_credentials(),
        Ok(b"dXNlcjpwYXNz".as_slice())
    );
    assert_eq!(
        ContentSecurityPolicyOwned::from_bytes(Vec::from(&b"img-src *"[..]))
            .expect("valid policy")
            .policies()
            .next(),
        Some(b"img-src *".as_slice())
    );
}

#[test]
fn borrowed_text_input_constructors_accept_owned_values() {
    assert_eq!(
        FieldValue::from_str(String::from("gzip")).expect("valid string").as_bytes(),
        b"gzip"
    );
    assert_eq!(
        FieldValue::from(ContentTypeOwned::new(String::from("application"), String::from("json")).expect("valid media type"),).as_bytes(),
        b"application/json"
    );
    assert_eq!(
        AuthorizationOwned::<Bearer>::bearer(String::from("abc.def"))
            .expect("valid token")
            .token(),
        Ok(b"abc.def".as_slice())
    );
    assert_eq!(
        ContentSecurityPolicyOwned::new(String::from("default-src 'self'"))
            .expect("valid policy")
            .policies()
            .next(),
        Some(b"default-src 'self'".as_slice())
    );
    assert_eq!(
        RangeOwned::extension(String::from("items"), String::from("1-5"))
            .expect("valid extension")
            .as_field_value()
            .as_bytes(),
        b"items=1-5"
    );
    assert_eq!(
        ContentRangeOwned::extension(String::from("items"), String::from("0-9/100"))
            .expect("valid extension")
            .as_field_value()
            .as_bytes(),
        b"items 0-9/100"
    );
    assert_eq!(
        AccessControlAllowOriginOwned::from_origin(String::from("https://example.com"))
            .expect("valid origin")
            .origin(),
        Ok(Some("https://example.com"))
    );
    assert!(!ETagOwned::strong(String::from("revision")).expect("valid tag").is_weak());
    assert!(ETagOwned::weak(String::from("revision")).expect("valid tag").is_weak());
    assert!(
        ETagOwned::try_from_wire(String::from("W/\"revision\""))
            .expect("valid wire tag")
            .is_weak()
    );
    assert_eq!(
        SecWebSocketProtocolOwned::new(String::from("chat"))
            .expect("valid protocol")
            .selected(),
        Ok("chat")
    );
    assert_eq!(
        HostOwned::new(String::from("example.com")).expect("valid host").as_str(),
        Ok("example.com")
    );
    assert_eq!(
        HostOwned::with_port(String::from("example.com"), 443).expect("valid host").as_str(),
        Ok("example.com:443")
    );
}

#[test]
fn semantic_arguments_and_build_time_validation_preserve_output() {
    let known = ContentRangeOwned::bytes(0, 9, CompleteLength::from(10)).expect("valid byte range");
    assert_eq!(known.as_field_value().as_bytes(), b"bytes 0-9/10");
    let unknown = ContentRangeOwned::bytes(0, 9, CompleteLength::Unknown).expect("valid byte range");
    assert_eq!(unknown.as_field_value().as_bytes(), b"bytes 0-9/*");

    let cache = CacheControlOwned::builder()
        .extension("stale-if-error", ExtensionValue::Value("30"))
        .extension_flag("immutable-extension")
        .build()
        .expect("valid cache extensions");
    let cache_directives = cache
        .directives()
        .map(|directive| directive.as_bytes().to_vec())
        .collect::<Vec<_>>();
    assert_eq!(
        cache_directives,
        [b"stale-if-error=30".as_slice(), b"immutable-extension"].map(<[u8]>::to_vec)
    );
    CacheControlOwned::builder()
        .extension_value("bad name", "bad value")
        .build()
        .expect_err("malformed cache extension must fail at build");
    assert_eq!(
        Sink.set_encoded(
            &FieldName::CacheControl,
            CacheControlOwned::builder().extension_value("bad name", "bad value"),
        ),
        Err(InsertError)
    );

    Sink.set_encoded(
        &FieldName::SetCookie,
        ValueRefsEncoder::new([FieldValueRef::new(b"a=1")]).with_sensitivity(FieldSensitivity::Sensitive),
    )
    .expect("sensitive value references encode");
    SetCookie::insert(&mut Sink, SetCookieOwned::new()).expect("an empty cookie collection removes the field");

    let hsts = StrictTransportSecurityOwned::builder(Duration::from_mins(1))
        .extension("report", ExtensionValue::Value("audit"))
        .extension_flag("future")
        .build()
        .expect("valid HSTS extensions");
    assert_eq!(hsts.as_field_value().as_bytes(), b"max-age=60; report=audit; future");
    StrictTransportSecurityOwned::builder(Duration::from_mins(1))
        .extension_flag("preload")
        .build()
        .expect_err("reserved HSTS extension must fail at build");

    let websocket = SecWebSocketExtensionsOwned::builder()
        .extension("permessage-deflate")
        .parameter("server_max_window_bits", ExtensionValue::Value("15"))
        .quoted_parameter("mode", "fast")
        .build()
        .expect("valid WebSocket extension");
    assert_eq!(
        websocket.to_string(),
        "permessage-deflate; server_max_window_bits=15; mode=\"fast\""
    );
    SecWebSocketExtensionsOwned::builder()
        .parameter_flag("orphan")
        .build()
        .expect_err("orphan parameter must fail at build");

    CacheControlOwned::builder()
        .extension_flag("x=y")
        .build()
        .expect_err("flag names containing equals must not become valued extensions");
    StrictTransportSecurityOwned::builder(Duration::from_mins(1))
        .extension_flag("x=y")
        .build()
        .expect_err("flag names containing equals must not become valued extensions");

    assert!(!FieldSensitivity::NonSensitive.is_sensitive());
    assert!(FieldSensitivity::Sensitive.is_sensitive());
}
