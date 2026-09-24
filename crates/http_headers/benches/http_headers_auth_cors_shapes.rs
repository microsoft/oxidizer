// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-backed and raw-source decode shapes for authorization, CORS and content length.

use http_headers::DecodeErrorKind;
use http_headers::headers::{
    AccessControlAllowCredentials, AccessControlAllowHeaders, AccessControlAllowMethods, AccessControlAllowOrigin,
    AccessControlExposeHeaders, AccessControlMaxAge, AccessControlRequestHeaders, AccessControlRequestMethod, Authorization, Basic, Bearer,
    ContentLength,
};

#[path = "http_headers_shapes_common.rs"]
mod shapes;

use shapes::Expected;

shapes::define_shapes!(
    "http_headers_auth_cors_shapes/parse";
    (authorization_basic_short, Authorization<Basic>, &["Basic dTpw"], Strict, Expected::Valid),
    (
        authorization_basic_existing_fixture,
        Authorization<Basic>,
        &["Basic YWxhZGRpbjpvcGVuc2VzYW1l"],
        Strict,
        Expected::Valid
    ),
    (authorization_basic_padded, Authorization<Basic>, &["Basic dXNlcjpwYXNzd29yZA=="], Strict, Expected::Valid),
    (
        authorization_basic_service,
        Authorization<Basic>,
        &["Basic Z2F0ZXdheS1zZXJ2aWNlLWFjY291bnQ6c3ludGhldGljLXBhc3N3b3JkLXdpdGgtNjQtY2hhcmFjdGVycy0wMTIzNDU2Nzg5LWFiY2RlZmdoaWprbG1ub3A="],
        Strict,
        Expected::Valid
    ),
    (
        authorization_basic_long_username,
        Authorization<Basic>,
        &["Basic bG9uZy1zZXJ2aWNlLWFjY291bnQtbmFtZS1mb3ItYXV0aGVudGljYXRpb24tcHJveHktdGVzdGluZzpzaG9ydA=="],
        Strict,
        Expected::Valid
    ),
    (authorization_basic_binary_password, Authorization<Basic>, &["Basic dXNlcjoA/w=="], Strict, Expected::Valid),
    (authorization_basic_mixed_case_spaces, Authorization<Basic>, &["bAsIc   dTpw"], Strict, Expected::Valid),
    (authorization_basic_relaxed, Authorization<Basic>, &["bAsIc   dTpw"], Relaxed, Expected::Valid),
    (authorization_basic_absent, Authorization<Basic>, &[], Strict, Expected::Absent),
    (
        authorization_basic_noncanonical_pad_bits,
        Authorization<Basic>,
        &["Basic Oh=="],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        authorization_basic_missing_colon,
        Authorization<Basic>,
        &["Basic bm8tY29sb24="],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        authorization_basic_repeated,
        Authorization<Basic>,
        &["Basic dTpw", "Basic dTpw"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (authorization_bearer_short, Authorization<Bearer>, &["Bearer abc.def"], Strict, Expected::Valid),
    (authorization_bearer_token_eight, Authorization<Bearer>, &["Bearer test1234"], Strict, Expected::Valid),
    (authorization_bearer_token_nine, Authorization<Bearer>, &["Bearer test12345"], Strict, Expected::Valid),
    (
        authorization_bearer_token_31,
        Authorization<Bearer>,
        &["Bearer abcdefghijklmnopqrstuvwxyz01234"],
        Strict,
        Expected::Valid
    ),
    (
        authorization_bearer_token_32,
        Authorization<Bearer>,
        &["Bearer abcdefghijklmnopqrstuvwxyz012345"],
        Strict,
        Expected::Valid
    ),
    (
        authorization_bearer_token_33,
        Authorization<Bearer>,
        &["Bearer abcdefghijklmnopqrstuvwxyz0123456"],
        Strict,
        Expected::Valid
    ),
    (
        authorization_bearer_synthetic_jwt_shape,
        Authorization<Bearer>,
        &["Bearer syntheticHeader0123456789.syntheticPayloadForGatewayServiceAccountWithScopesReadWriteAndExpiry0123456789.syntheticSignatureAbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-"],
        Strict,
        Expected::Valid
    ),
    (authorization_bearer_padding, Authorization<Bearer>, &["Bearer YWJjZA=="], Strict, Expected::Valid),
    (authorization_bearer_mixed_case_spaces, Authorization<Bearer>, &["bEaReR   abc.def"], Strict, Expected::Valid),
    (authorization_bearer_absent, Authorization<Bearer>, &[], Strict, Expected::Absent),
    (
        authorization_bearer_interior_padding,
        Authorization<Bearer>,
        &["Bearer ab=c"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        authorization_bearer_invalid_relaxed,
        Authorization<Bearer>,
        &["Bearer ab=c"],
        Relaxed,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        authorization_bearer_repeated,
        Authorization<Bearer>,
        &["Bearer abc.def", "Bearer abc.def"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (access_control_allow_credentials_true, AccessControlAllowCredentials, &["true"], Strict, Expected::Valid),
    (access_control_allow_credentials_leading_ows, AccessControlAllowCredentials, &[" true"], Strict, Expected::Valid),
    (access_control_allow_credentials_trailing_ows, AccessControlAllowCredentials, &["true\t"], Strict, Expected::Valid),
    (access_control_allow_credentials_framed, AccessControlAllowCredentials, &[" \ttrue\t "], Strict, Expected::Valid),
    (
        access_control_allow_credentials_wide_ows,
        AccessControlAllowCredentials,
        &["                true                "],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_credentials_absent, AccessControlAllowCredentials, &[], Strict, Expected::Absent),
    (
        access_control_allow_credentials_empty,
        AccessControlAllowCredentials,
        &[""],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_allow_credentials_case_relaxed,
        AccessControlAllowCredentials,
        &["TRUE"],
        Relaxed,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_allow_credentials_suffix,
        AccessControlAllowCredentials,
        &[" truee "],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_allow_credentials_repeated,
        AccessControlAllowCredentials,
        &["true", "true"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (
        access_control_allow_headers_common_pair,
        AccessControlAllowHeaders,
        &["content-type, x-request-id"],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_headers_custom, AccessControlAllowHeaders, &["x-custom-header"], Strict, Expected::Valid),
    (
        access_control_allow_headers_mixed_case,
        AccessControlAllowHeaders,
        &["Content-Type, X-Correlation-Id"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_allow_headers_large,
        AccessControlAllowHeaders,
        &["authorization, content-type, x-request-id, x-correlation-id, x-client-version, x-tenant-id, x-trace-id, x-idempotency-key, x-api-version, x-requested-with, traceparent, tracestate"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_allow_headers_repeated,
        AccessControlAllowHeaders,
        &["content-type, x-request-id", "authorization", "x-custom-header, content-type"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_allow_headers_sixteen_lines,
        AccessControlAllowHeaders,
        &["x-00", "x-01", "x-02", "x-03", "x-04", "x-05", "x-06", "x-07", "x-08", "x-09", "x-10", "x-11", "x-12", "x-13", "x-14", "x-15"],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_headers_empty, AccessControlAllowHeaders, &[""], Strict, Expected::Valid),
    (
        access_control_allow_headers_empty_slots,
        AccessControlAllowHeaders,
        &[" ,\t, content-type,, X-Trace-Id, "],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_headers_wildcard, AccessControlAllowHeaders, &["*"], Strict, Expected::Valid),
    (
        access_control_allow_headers_late_error,
        AccessControlAllowHeaders,
        &["content-type", "x-request-id", "bad:name"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (access_control_allow_headers_absent, AccessControlAllowHeaders, &[], Strict, Expected::Absent),
    (access_control_allow_methods_pair, AccessControlAllowMethods, &["GET, POST"], Strict, Expected::Valid),
    (
        access_control_allow_methods_crud,
        AccessControlAllowMethods,
        &["GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_allow_methods_extensions,
        AccessControlAllowMethods,
        &["PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_allow_methods_repeated,
        AccessControlAllowMethods,
        &["GET, X-PURGE,,", "PATCH, GET", "POST"],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_methods_empty, AccessControlAllowMethods, &[""], Strict, Expected::Valid),
    (
        access_control_allow_methods_whitespace,
        AccessControlAllowMethods,
        &["\t GET ,\tPOST, , PATCH \t"],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_methods_wildcard, AccessControlAllowMethods, &["*"], Strict, Expected::Valid),
    (access_control_allow_methods_absent, AccessControlAllowMethods, &[], Strict, Expected::Absent),
    (
        access_control_allow_methods_late_error,
        AccessControlAllowMethods,
        &["GET, POST", "PATCH", "bad method"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (access_control_allow_methods_relaxed, AccessControlAllowMethods, &["get, X-PURGE"], Relaxed, Expected::Valid),
    (access_control_allow_origin_https, AccessControlAllowOrigin, &["https://example.com"], Strict, Expected::Valid),
    (access_control_allow_origin_http, AccessControlAllowOrigin, &["http://example.com"], Strict, Expected::Valid),
    (access_control_allow_origin_wildcard, AccessControlAllowOrigin, &["*"], Strict, Expected::Valid),
    (access_control_allow_origin_null, AccessControlAllowOrigin, &["null"], Strict, Expected::Valid),
    (access_control_allow_origin_port, AccessControlAllowOrigin, &["https://api.example.com:8443"], Strict, Expected::Valid),
    (
        access_control_allow_origin_long_domain,
        AccessControlAllowOrigin,
        &["https://service-authentication.production.westus2.customer-tenant-0123456789.internal.example.com"],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_origin_ipv4, AccessControlAllowOrigin, &["http://127.0.0.1"], Strict, Expected::Valid),
    (access_control_allow_origin_ipv6_port, AccessControlAllowOrigin, &["https://[2001:db8::1]:8443"], Strict, Expected::Valid),
    (
        access_control_allow_origin_ipv6_uncompressed,
        AccessControlAllowOrigin,
        &["https://[2001:db8:1:2:3:4:5:6]"],
        Strict,
        Expected::Valid
    ),
    (access_control_allow_origin_wss, AccessControlAllowOrigin, &["wss://example.com"], Strict, Expected::Valid),
    (access_control_allow_origin_whitespace, AccessControlAllowOrigin, &[" \thttps://example.com\t "], Strict, Expected::Valid),
    (access_control_allow_origin_absent, AccessControlAllowOrigin, &[], Strict, Expected::Absent),
    (
        access_control_allow_origin_uppercase_host,
        AccessControlAllowOrigin,
        &["https://Example.com"],
        Relaxed,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_allow_origin_default_port,
        AccessControlAllowOrigin,
        &["https://example.com:443"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_allow_origin_ipv6_leading_zero,
        AccessControlAllowOrigin,
        &["https://[2001:0db8::1]"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_allow_origin_repeated,
        AccessControlAllowOrigin,
        &["https://example.com", "https://example.com"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (access_control_expose_headers_pair, AccessControlExposeHeaders, &["etag, x-request-id"], Strict, Expected::Valid),
    (
        access_control_expose_headers_response,
        AccessControlExposeHeaders,
        &["content-length, content-range, etag"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_expose_headers_mixed_case,
        AccessControlExposeHeaders,
        &["X-Request-Id, Content-Length"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_expose_headers_large,
        AccessControlExposeHeaders,
        &["etag, content-length, content-range, x-request-id, x-correlation-id, x-ratelimit-limit, x-ratelimit-remaining, x-ratelimit-reset, retry-after, server-timing, x-api-version, x-total-count"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_expose_headers_repeated,
        AccessControlExposeHeaders,
        &["etag, x-request-id", "X-Trace-Id", "etag, content-length"],
        Strict,
        Expected::Valid
    ),
    (access_control_expose_headers_empty, AccessControlExposeHeaders, &[""], Strict, Expected::Valid),
    (access_control_expose_headers_wildcard, AccessControlExposeHeaders, &["*"], Strict, Expected::Valid),
    (
        access_control_expose_headers_empty_slots,
        AccessControlExposeHeaders,
        &["\t, etag,, X-Request-Id,\t"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_expose_headers_late_error,
        AccessControlExposeHeaders,
        &["etag", "x-request-id", "x-bad:name"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (access_control_expose_headers_absent, AccessControlExposeHeaders, &[], Strict, Expected::Absent),
    (access_control_max_age_600, AccessControlMaxAge, &["600"], Strict, Expected::Valid),
    (access_control_max_age_zero, AccessControlMaxAge, &["0"], Strict, Expected::Valid),
    (access_control_max_age_whitespace, AccessControlMaxAge, &[" \t00600\t "], Strict, Expected::Valid),
    (access_control_max_age_nineteen_digits, AccessControlMaxAge, &["9999999999999999999"], Strict, Expected::Valid),
    (access_control_max_age_maximum, AccessControlMaxAge, &["18446744073709551615"], Strict, Expected::Valid),
    (access_control_max_age_many_zeroes, AccessControlMaxAge, &["00000000000000000000000000000600"], Strict, Expected::Valid),
    (access_control_max_age_absent, AccessControlMaxAge, &[], Strict, Expected::Absent),
    (
        access_control_max_age_overflow,
        AccessControlMaxAge,
        &["18446744073709551616"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidNumber)
    ),
    (access_control_max_age_invalid_relaxed, AccessControlMaxAge, &["-1"], Relaxed, Expected::Error(DecodeErrorKind::InvalidNumber)),
    (
        access_control_max_age_repeated,
        AccessControlMaxAge,
        &["600", "600"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (access_control_request_headers_pair, AccessControlRequestHeaders, &["content-type, x-request-id"], Strict, Expected::Valid),
    (access_control_request_headers_single, AccessControlRequestHeaders, &["content-type"], Strict, Expected::Valid),
    (
        access_control_request_headers_mixed_case,
        AccessControlRequestHeaders,
        &["X-Trace-Id, Content-Type"],
        Strict,
        Expected::Valid
    ),
    (access_control_request_headers_custom, AccessControlRequestHeaders, &["x-idempotency-key"], Strict, Expected::Valid),
    (
        access_control_request_headers_large,
        AccessControlRequestHeaders,
        &["authorization, content-type, traceparent, tracestate, x-api-version, x-client-version, x-correlation-id, x-idempotency-key, x-request-id, x-tenant-id, x-trace-id"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_request_headers_repeated,
        AccessControlRequestHeaders,
        &["X-Trace-Id, content-type", "x-trace-id", "authorization"],
        Strict,
        Expected::Valid
    ),
    (
        access_control_request_headers_empty_then_member,
        AccessControlRequestHeaders,
        &[" , ", "content-type", ""],
        Strict,
        Expected::Valid
    ),
    (access_control_request_headers_absent, AccessControlRequestHeaders, &[], Strict, Expected::Absent),
    (
        access_control_request_headers_empty_repeated,
        AccessControlRequestHeaders,
        &[" , ", "\t,,", ""],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        access_control_request_headers_late_error,
        AccessControlRequestHeaders,
        &["content-type", "x-trace-id", "bad:name"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (access_control_request_method_post, AccessControlRequestMethod, &["POST"], Strict, Expected::Valid),
    (access_control_request_method_get, AccessControlRequestMethod, &["GET"], Strict, Expected::Valid),
    (access_control_request_method_patch, AccessControlRequestMethod, &["PATCH"], Strict, Expected::Valid),
    (access_control_request_method_options, AccessControlRequestMethod, &["OPTIONS"], Strict, Expected::Valid),
    (access_control_request_method_extension, AccessControlRequestMethod, &["PROPFIND"], Strict, Expected::Valid),
    (
        access_control_request_method_long_extension,
        AccessControlRequestMethod,
        &["X-REBUILD-SEARCH-INDEX-FOR-TENANT"],
        Strict,
        Expected::Valid
    ),
    (access_control_request_method_lowercase, AccessControlRequestMethod, &["post"], Strict, Expected::Valid),
    (access_control_request_method_framed_registered, AccessControlRequestMethod, &[" \tPOST\t "], Strict, Expected::Valid),
    (access_control_request_method_absent, AccessControlRequestMethod, &[], Strict, Expected::Absent),
    (
        access_control_request_method_invalid_relaxed,
        AccessControlRequestMethod,
        &["GET, POST"],
        Relaxed,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (
        access_control_request_method_repeated,
        AccessControlRequestMethod,
        &["POST", "POST"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (content_length_348, ContentLength, &["348"], Strict, Expected::Valid),
    (content_length_zero, ContentLength, &["0"], Strict, Expected::Valid),
    (content_length_megabyte, ContentLength, &["1048576"], Strict, Expected::Valid),
    (content_length_nineteen_digits, ContentLength, &["9999999999999999999"], Strict, Expected::Valid),
    (content_length_maximum, ContentLength, &["18446744073709551615"], Strict, Expected::Valid),
    (content_length_many_zeroes, ContentLength, &["00000000000000000000000000000348"], Strict, Expected::Valid),
    (content_length_whitespace, ContentLength, &[" \t348\t "], Strict, Expected::Valid),
    (content_length_joined_equal, ContentLength, &["348, 348"], Strict, Expected::Valid),
    (
        content_length_repeated_numeric_equivalence,
        ContentLength,
        &["000348, 348", " 348 ", "348,348", "00348"],
        Strict,
        Expected::Valid
    ),
    (content_length_absent, ContentLength, &[], Strict, Expected::Absent),
    (
        content_length_overflow,
        ContentLength,
        &["18446744073709551616"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidNumber)
    ),
    (
        content_length_repeated_conflict,
        ContentLength,
        &["348", "348", "349"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        content_length_late_malformed,
        ContentLength,
        &["348", "348", "348x"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidNumber)
    ),
);
