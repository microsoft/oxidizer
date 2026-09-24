// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::str;

#[expect(
    clippy::inline_always,
    reason = "preserves measured CORS method projection after sharing the helper"
)]
#[inline(always)]
pub(super) fn common_method(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        b"GET" => Some("GET"),
        b"PUT" => Some("PUT"),
        b"HEAD" => Some("HEAD"),
        b"POST" => Some("POST"),
        b"PATCH" => Some("PATCH"),
        b"TRACE" => Some("TRACE"),
        b"DELETE" => Some("DELETE"),
        b"CONNECT" => Some("CONNECT"),
        b"OPTIONS" => Some("OPTIONS"),
        _ => None,
    }
}

#[inline]
pub(super) fn method_text(bytes: &[u8]) -> &str {
    common_method(bytes).unwrap_or_else(|| str::from_utf8(bytes).expect("validated method tokens contain only ASCII"))
}

#[expect(
    clippy::inline_always,
    reason = "preserves measured CORS field-name projection after sharing the helper"
)]
#[inline(always)]
pub(super) fn common_header_name(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        b"content-type" => Some("content-type"),
        b"authorization" => Some("authorization"),
        b"x-request-id" => Some("x-request-id"),
        b"etag" => Some("etag"),
        b"origin" => Some("origin"),
        b"accept" => Some("accept"),
        b"x-requested-with" => Some("x-requested-with"),
        b"content-length" => Some("content-length"),
        b"cache-control" => Some("cache-control"),
        _ => None,
    }
}

#[inline]
pub(super) fn header_name_text(bytes: &[u8]) -> &str {
    common_header_name(bytes).unwrap_or_else(|| str::from_utf8(bytes).expect("validated field-name tokens contain only ASCII"))
}
