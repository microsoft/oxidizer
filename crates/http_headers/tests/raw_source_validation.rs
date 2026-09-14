// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression coverage for raw custom-source field lines.

use http_headers::headers::{
    Accept, AcceptEncoding, AcceptLanguage, AcceptRanges, AccessControlAllowHeaders, AccessControlExposeHeaders,
    AccessControlRequestHeaders, Allow, Authorization, Basic, CacheControl, ContentSecurityPolicy, ETag, IfMatch, IfNoneMatch,
    ReferrerPolicy, SecWebSocketExtensions, SecWebSocketProtocol, Server, SetCookie, UserAgent, Vary,
};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValueRef};

#[derive(Clone, Copy)]
enum RawRepr {
    Single,
    Borrowed,
}

struct RawSource {
    name: &'static FieldName,
    bytes: &'static [u8],
    borrowed: [FieldValueRef<'static>; 1],
    repr: RawRepr,
}

impl RawSource {
    fn new(name: &'static FieldName, bytes: &'static [u8], repr: RawRepr) -> Self {
        Self {
            name,
            bytes,
            borrowed: [FieldValueRef::new(bytes)],
            repr,
        }
    }
}

impl FieldSource for RawSource {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        if name != self.name {
            return None;
        }
        match self.repr {
            RawRepr::Single => Some(FieldLines::single(name, self.bytes)),
            RawRepr::Borrowed => FieldLines::from_borrowed(name, &self.borrowed),
        }
    }
}

fn decode_owned<F: Field>(source: &RawSource) -> Result<(), DecodeError> {
    F::owned(source).map(|_| ())
}

fn decode_view<F: Field>(source: &RawSource) -> Result<(), DecodeError> {
    F::view(source).map(|_| ())
}

#[test]
fn affected_owned_decoders_reject_invalid_raw_field_lines() {
    type Decoder = fn(&RawSource) -> Result<(), DecodeError>;

    let decoders: &[(&FieldName, Decoder)] = &[
        (&FieldName::CacheControl, decode_owned::<CacheControl>),
        (&FieldName::IfMatch, decode_owned::<IfMatch>),
        (&FieldName::IfNoneMatch, decode_owned::<IfNoneMatch>),
        (&FieldName::Accept, decode_owned::<Accept>),
        (&FieldName::AcceptEncoding, decode_owned::<AcceptEncoding>),
        (&FieldName::AcceptLanguage, decode_owned::<AcceptLanguage>),
        (&FieldName::Allow, decode_owned::<Allow>),
        (&FieldName::Vary, decode_owned::<Vary>),
        (&FieldName::AccessControlAllowHeaders, decode_owned::<AccessControlAllowHeaders>),
        (&FieldName::AccessControlExposeHeaders, decode_owned::<AccessControlExposeHeaders>),
        (&FieldName::AccessControlRequestHeaders, decode_owned::<AccessControlRequestHeaders>),
        (&FieldName::ContentSecurityPolicy, decode_owned::<ContentSecurityPolicy>),
        (&FieldName::ReferrerPolicy, decode_owned::<ReferrerPolicy>),
        (&FieldName::AcceptRanges, decode_owned::<AcceptRanges>),
        (&FieldName::SetCookie, decode_owned::<SetCookie>),
        (&FieldName::SecWebSocketExtensions, decode_owned::<SecWebSocketExtensions>),
        (&FieldName::SecWebSocketProtocol, decode_owned::<SecWebSocketProtocol>),
    ];
    let invalid = [
        b"\r".as_slice(),
        b"\n".as_slice(),
        b"\0".as_slice(),
        b"\x1f".as_slice(),
        b"\x7f".as_slice(),
    ];

    for &(name, decode) in decoders {
        for &bytes in &invalid {
            for repr in [RawRepr::Single, RawRepr::Borrowed] {
                let source = RawSource::new(name, bytes, repr);
                decode(&source).expect_err(name.as_str());
            }
        }
    }
}

#[test]
fn affected_borrowed_decoders_reject_invalid_raw_field_lines() {
    type Decoder = fn(&RawSource) -> Result<(), DecodeError>;

    let decoders: &[(&FieldName, Decoder)] = &[
        (&FieldName::UserAgent, decode_view::<UserAgent>),
        (&FieldName::Server, decode_view::<Server>),
        (&FieldName::SetCookie, decode_view::<SetCookie>),
        (&FieldName::ContentSecurityPolicy, decode_view::<ContentSecurityPolicy>),
    ];

    for bytes in [
        b"\r".as_slice(),
        b"\n".as_slice(),
        b"\0".as_slice(),
        b"\x1f".as_slice(),
        b"\x7f".as_slice(),
    ] {
        for &(name, decode) in decoders {
            for repr in [RawRepr::Single, RawRepr::Borrowed] {
                let source = RawSource::new(name, bytes, repr);
                assert_eq!(decode(&source).expect_err(name.as_str()).kind(), DecodeErrorKind::InvalidSyntax);
            }
        }
    }
}

#[test]
fn entity_tags_reject_del_from_raw_sources() {
    for bytes in [b"\"\x7f\"".as_slice(), b"\"abc\x7fdefgh\""] {
        for repr in [RawRepr::Single, RawRepr::Borrowed] {
            let source = RawSource::new(&FieldName::Etag, bytes, repr);
            assert_eq!(
                decode_view::<ETag>(&source).expect_err("DEL is not etagc").kind(),
                DecodeErrorKind::InvalidSyntax
            );
            assert_eq!(
                decode_owned::<ETag>(&source).expect_err("DEL is not etagc").kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }
}

#[test]
fn delimited_iteration_reports_preflight_errors_once() {
    let mut items = FieldLines::single(&FieldName::Vary, b"valid,\ninvalid").comma_items();
    assert_eq!(
        items.next().expect("preflight error").expect_err("invalid raw bytes").kind(),
        DecodeErrorKind::InvalidSyntax
    );
    assert!(items.next().is_none());
}

#[test]
fn sensitive_borrowed_headers_override_source_classification() {
    let authorization_source = RawSource::new(&FieldName::Authorization, b"Basic dXNlcjpwYXNz", RawRepr::Borrowed);
    let authorization = Authorization::<Basic>::view(&authorization_source)
        .expect("valid authorization")
        .expect("authorization is present");
    let authorization_value = authorization.as_field_value();
    assert!(authorization_value.is_sensitive());
    assert!(authorization_value.try_to_field_value().unwrap().is_sensitive());

    let cookie_source = RawSource::new(&FieldName::SetCookie, b"session=secret", RawRepr::Borrowed);
    let cookies = SetCookie::view(&cookie_source)
        .expect("valid Set-Cookie")
        .expect("Set-Cookie is present");
    let cookie_value = cookies.iter().next().unwrap();
    assert!(cookie_value.is_sensitive());
    assert!(cookie_value.try_to_field_value().unwrap().is_sensitive());

    #[cfg(feature = "http")]
    {
        let authorization_value = http::HeaderValue::try_from(authorization_value).unwrap();
        let cookie_value = http::HeaderValue::try_from(cookie_value).unwrap();
        assert!(authorization_value.is_sensitive());
        assert!(cookie_value.is_sensitive());
    }
}

#[cfg(feature = "http")]
#[test]
fn sensitive_borrowed_headers_override_http_map_classification() {
    let mut map = http::HeaderMap::new();
    map.insert(http::header::AUTHORIZATION, http::HeaderValue::from_static("Basic dXNlcjpwYXNz"));
    map.insert(http::header::SET_COOKIE, http::HeaderValue::from_static("session=secret"));

    let authorization = Authorization::<Basic>::view(&map).unwrap().unwrap().as_field_value();
    let cookie = SetCookie::view(&map).unwrap().unwrap().iter().next().unwrap();
    for (value, secret) in [(authorization, "dXNlcjpwYXNz"), (cookie, "session=secret")] {
        assert!(value.is_sensitive());
        let owned = value.try_to_field_value().unwrap();
        assert!(owned.is_sensitive());
        assert!(!format!("{owned:?}").contains(secret));
        assert!(http::HeaderValue::try_from(value).unwrap().is_sensitive());
    }
}
