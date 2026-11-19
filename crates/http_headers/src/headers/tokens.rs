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
    // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "The fallback decodes the same validated ASCII method bytes; every recognized literal has exactly the same text")]
    let common = common_method(bytes);
    common.unwrap_or_else(|| str::from_utf8(bytes).expect("validated method tokens contain only ASCII"))
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
    // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "The fallback decodes the same validated ASCII field-name bytes; every recognized literal has exactly the same text")]
    let common = common_header_name(bytes);
    common.unwrap_or_else(|| str::from_utf8(bytes).expect("validated field-name tokens contain only ASCII"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod gamma_round2_tests {
    #[test]
    fn gamma_round2_known_tokens_remain_recognized_before_text_fallback() {
        for method in ["GET", "PUT", "HEAD", "POST", "PATCH", "TRACE", "DELETE", "CONNECT", "OPTIONS"] {
            assert_eq!(super::common_method(method.as_bytes()), Some(method));
        }
        for name in [
            "content-type",
            "authorization",
            "x-request-id",
            "etag",
            "origin",
            "accept",
            "x-requested-with",
            "content-length",
            "cache-control",
        ] {
            assert_eq!(super::common_header_name(name.as_bytes()), Some(name));
        }
        assert_eq!(super::common_method(b"CUSTOM"), None);
        assert_eq!(super::common_header_name(b"x-custom"), None);
    }
}
