// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile coverage for the concise examples shared across public API docs.

#![cfg(feature = "headers-all")]

use std::time::Duration;

use http_headers::headers::*;
use http_headers::*;

#[test]
fn header_type_examples_compile() {
    let authorization = AuthorizationOwned::<Bearer>::bearer("abc.def").unwrap();
    assert_eq!(authorization.token().unwrap(), b"abc.def");

    let cache = CacheControlOwned::try_from("max-age=60").unwrap();
    assert_eq!(cache.max_age(), Some(Duration::from_mins(1)));

    let conditional = IfRangeOwned::try_from("\"revision\"").unwrap();
    assert!(matches!(conditional.value().unwrap(), IfRangeValueView::EntityTag(_)));

    assert_eq!(ContentLengthOwned::new(42).get(), 42);

    let content_type = ContentTypeOwned::try_from("text/html; charset=utf-8").unwrap();
    assert_eq!(content_type.parameter("charset").unwrap(), Some(b"utf-8".as_slice()));

    let origin = AccessControlAllowOriginOwned::try_from("https://example.com").unwrap();
    assert_eq!(origin.origin().unwrap(), Some("https://example.com"));

    assert_eq!(ETagOwned::strong("revision").unwrap().opaque_tag().unwrap(), b"revision");
    assert_eq!(LocationOwned::try_from("/next").unwrap().as_str().unwrap(), "/next");
    assert_eq!(HostOwned::try_from("example.com:443").unwrap().host().unwrap(), "example.com");
    assert!(RangeOwned::try_from("bytes=0-99").unwrap().is_bytes());

    let policy = ReferrerPolicyOwned::new(ReferrerPolicyValue::NoReferrer);
    assert_eq!(policy.preferred().unwrap(), ReferrerPolicyValue::NoReferrer);

    let mut cookies = SetCookieOwned::new();
    cookies.push_str("a=1").unwrap();
    assert_eq!(cookies.len(), 1);

    assert_eq!(UserAgentOwned::try_from("client/1").unwrap().as_bytes(), b"client/1");
    assert_eq!(SecWebSocketVersionOwned::new(13).versions().next().unwrap(), 13);
}

#[test]
fn core_api_examples_compile() {
    let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
    assert_eq!(error.header().as_str(), "content-type");

    #[cfg(feature = "http")]
    {
        let mut map = http::HeaderMap::new();
        map.insert(http::header::USER_AGENT, http::HeaderValue::from_static("client/1"));
        assert!(UserAgent::view(&map).unwrap().is_some());
    }
}
