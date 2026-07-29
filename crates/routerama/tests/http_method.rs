// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP method public API tests.

use routerama::HttpMethod;

#[test]
fn standard_and_custom_methods_expose_their_tokens() {
    assert_eq!(HttpMethod::GET.as_str(), "GET");
    assert_eq!(HttpMethod::PUT.as_str(), "PUT");
    assert_eq!(HttpMethod::POST.to_string(), "POST");
    assert_eq!(HttpMethod::DELETE.as_ref() as &str, "DELETE");
    assert_eq!(HttpMethod::PATCH.as_str(), "PATCH");
    assert_eq!(HttpMethod::HEAD.as_str(), "HEAD");
    assert_eq!(HttpMethod::OPTIONS.as_str(), "OPTIONS");
    assert_eq!(HttpMethod::CONNECT.as_str(), "CONNECT");
    assert_eq!(HttpMethod::TRACE.as_str(), "TRACE");
    assert_eq!(String::from(HttpMethod::GET), "GET");

    let custom = HttpMethod::custom("M-SEARCH").expect("M-SEARCH is a valid token");
    assert_eq!(custom.as_str(), "M-SEARCH");
    assert_eq!(String::from(custom), "M-SEARCH");
}

#[test]
fn custom_methods_reject_invalid_tokens() {
    for invalid in ["", "BAD METHOD", "METHOD\n"] {
        let error = HttpMethod::custom(invalid).expect_err("invalid HTTP method tokens must be rejected");
        assert_eq!(error.invalid_http_method_value(), Some(invalid));
        assert!(error.causes().next().is_none());
    }

    let error = "BAD METHOD".parse::<HttpMethod>().expect_err("spaces are not allowed");
    assert_eq!(error.to_string(), "`BAD METHOD` is not a valid RFC 9110 HTTP method token");
}

#[test]
fn validated_tokens_and_strings_convert_to_methods() {
    for (token, expected) in [
        ("GET", HttpMethod::GET),
        ("PUT", HttpMethod::PUT),
        ("POST", HttpMethod::POST),
        ("DELETE", HttpMethod::DELETE),
        ("PATCH", HttpMethod::PATCH),
        ("HEAD", HttpMethod::HEAD),
        ("OPTIONS", HttpMethod::OPTIONS),
        ("CONNECT", HttpMethod::CONNECT),
        ("TRACE", HttpMethod::TRACE),
    ] {
        assert_eq!(token.parse::<HttpMethod>().expect("standard method is valid"), expected);
        assert_eq!(HttpMethod::custom(token).expect("standard method is valid"), expected);
    }
    assert_eq!(HttpMethod::try_from("OPTIONS").expect("OPTIONS is valid"), HttpMethod::OPTIONS);
    assert_eq!(HttpMethod::try_from("GET".to_owned()).expect("owned GET is valid"), HttpMethod::GET);
    assert_eq!(
        HttpMethod::try_from("M-SEARCH".to_owned()).expect("M-SEARCH is valid"),
        HttpMethod::custom("M-SEARCH").expect("M-SEARCH is valid")
    );
    let error = HttpMethod::try_from("BAD METHOD").expect_err("spaces are invalid");
    assert_eq!(error.invalid_http_method_value(), Some("BAD METHOD"));
    let error = HttpMethod::try_from("BAD METHOD".to_owned()).expect_err("owned spaces are invalid");
    assert_eq!(error.invalid_http_method_value(), Some("BAD METHOD"));
}
