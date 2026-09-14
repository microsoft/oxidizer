// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-backed and raw-source decode shapes for negotiation and media-type headers.

use http_headers::DecodeErrorKind;
use http_headers::headers::{Accept, AcceptEncoding, AcceptLanguage, Allow, ContentType, Host, Server, Vary};

#[path = "http_headers_shapes_common.rs"]
mod shapes;

use shapes::Expected;

shapes::define_shapes!(
    "http_headers_negotiation_shapes/parse";
    (
        accept_canonical_browser,
        Accept,
        &["text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"],
        Strict,
        Expected::Valid
    ),
    (accept_short_html, Accept, &["text/html"], Strict, Expected::Valid),
    (accept_common_api, Accept, &["application/json, text/plain;q=0.9, */*;q=0.8"], Strict, Expected::Valid),
    (accept_repeated, Accept, &["text/html", "application/json;q=0.9", "*/*;q=0.1"], Strict, Expected::Valid),
    (
        accept_quoted_comma,
        Accept,
        &["text/html;level=1, application/json;q=0.9;profile=\"a,b\"", "*/*;q=0.1"],
        Strict,
        Expected::Valid
    ),
    (accept_extension_flag, Accept, &["text/html;q=0.8;preview"], Strict, Expected::Valid),
    (accept_relaxed_quality, Accept, &["text/html; q = .12345"], Relaxed, Expected::Valid),
    (
        accept_strict_quality_whitespace,
        Accept,
        &["text/html; q = .12345"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (accept_bad_wildcard, Accept, &["*/json"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (
        accept_late_unterminated_quote,
        Accept,
        &["text/html", "application/json;profile=\"unfinished"],
        Strict,
        Expected::Error(DecodeErrorKind::UnterminatedQuote)
    ),
    (accept_empty, Accept, &[""], Strict, Expected::Valid),
    (accept_absent, Accept, &[], Strict, Expected::Absent),
    (accept_encoding_canonical, AcceptEncoding, &["gzip, deflate, br, zstd"], Strict, Expected::Valid),
    (accept_encoding_short, AcceptEncoding, &["gzip"], Strict, Expected::Valid),
    (
        accept_encoding_weighted,
        AcceptEncoding,
        &["gzip;q=1.0, identity;q=0.5, *;q=0"],
        Strict,
        Expected::Valid
    ),
    (
        accept_encoding_long_preferences,
        AcceptEncoding,
        &["br;q=1.0, zstd;q=0.9, gzip;q=0.8, deflate;q=0.7, identity;q=0.5, *;q=0"],
        Strict,
        Expected::Valid
    ),
    (accept_encoding_repeated, AcceptEncoding, &["gzip", "br;q=0.8", "identity;q=0.5"], Strict, Expected::Valid),
    (accept_encoding_relaxed_quality, AcceptEncoding, &["br; q = .1234"], Relaxed, Expected::Valid),
    (
        accept_encoding_strict_quality_whitespace,
        AcceptEncoding,
        &["br; q = .1234"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        accept_encoding_late_bad_token,
        AcceptEncoding,
        &["gzip", "bad encoding"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (accept_encoding_empty, AcceptEncoding, &[""], Strict, Expected::Valid),
    (accept_encoding_absent, AcceptEncoding, &[], Strict, Expected::Absent),
    (
        accept_language_canonical,
        AcceptLanguage,
        &["en-US,en;q=0.9,fr-FR;q=0.8,fr;q=0.7"],
        Strict,
        Expected::Valid
    ),
    (accept_language_short, AcceptLanguage, &["en"], Strict, Expected::Valid),
    (
        accept_language_long_preferences,
        AcceptLanguage,
        &["zh-Hans-CN, zh-Hans;q=0.9, en-US;q=0.8, en;q=0.7, fr-CH;q=0.6, de;q=0.5, *;q=0.1"],
        Strict,
        Expected::Valid
    ),
    (accept_language_repeated, AcceptLanguage, &["en-US", "fr-CH;q=0.8", "de;q=0.5"], Strict, Expected::Valid),
    (accept_language_wildcard, AcceptLanguage, &["*"], Strict, Expected::Valid),
    (accept_language_relaxed_quality, AcceptLanguage, &["en-US; Q = 1.0000"], Relaxed, Expected::Valid),
    (
        accept_language_strict_quality_whitespace,
        AcceptLanguage,
        &["en-US; Q = 1.0000"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        accept_language_late_bad_subtag,
        AcceptLanguage,
        &["en-US", "en-123456789"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (accept_language_empty, AcceptLanguage, &[""], Strict, Expected::Valid),
    (accept_language_absent, AcceptLanguage, &[], Strict, Expected::Absent),
    (allow_canonical, Allow, &["GET, POST"], Strict, Expected::Valid),
    (allow_short, Allow, &["GET"], Strict, Expected::Valid),
    (
        allow_long_webdav,
        Allow,
        &["OPTIONS, GET, HEAD, POST, PUT, DELETE, TRACE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK"],
        Strict,
        Expected::Valid
    ),
    (allow_repeated, Allow, &["GET, HEAD", "POST", "PATCH, DELETE"], Strict, Expected::Valid),
    (allow_extension, Allow, &["PROPFIND"], Strict, Expected::Valid),
    (allow_ows_empty_members, Allow, &[" , GET,,\tPOST, "], Strict, Expected::Valid),
    (
        allow_late_bad_token,
        Allow,
        &["GET", "BAD METHOD"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (allow_empty, Allow, &[""], Strict, Expected::Valid),
    (allow_absent, Allow, &[], Strict, Expected::Absent),
    (vary_canonical, Vary, &["accept-encoding, origin"], Strict, Expected::Valid),
    (vary_short, Vary, &["accept-encoding"], Strict, Expected::Valid),
    (vary_title_case, Vary, &["Accept-Encoding, Origin"], Strict, Expected::Valid),
    (
        vary_long_selection,
        Vary,
        &["Accept-Encoding, Accept-Language, Origin, User-Agent, X-Requested-With, X-API-Version"],
        Strict,
        Expected::Valid
    ),
    (vary_repeated, Vary, &["accept-encoding", "Origin", "X-API-Version"], Strict, Expected::Valid),
    (vary_wildcard, Vary, &["*"], Strict, Expected::Valid),
    (vary_ows_empty_members, Vary, &[" , Accept-Encoding,,\tOrigin, "], Strict, Expected::Valid),
    (
        vary_late_bad_token,
        Vary,
        &["accept-encoding", "bad field"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidToken)
    ),
    (vary_empty, Vary, &[""], Strict, Expected::Valid),
    (vary_absent, Vary, &[], Strict, Expected::Absent),
    (host_canonical, Host, &["example.com:8443"], Strict, Expected::Valid),
    (host_common_domain, Host, &["api.example.com"], Strict, Expected::Valid),
    (
        host_long_domain,
        Host,
        &["api.customer-a.region-west.service.production.widgets.example.com:8443"],
        Strict,
        Expected::Valid
    ),
    (host_ipv6, Host, &["[2001:db8::1]:443"], Strict, Expected::Valid),
    (host_ipvfuture, Host, &["[v1.fe80::a]:443"], Strict, Expected::Valid),
    (host_percent_encoded, Host, &["api%2Eexample.com:443"], Strict, Expected::Valid),
    (host_empty_port, Host, &["example.com:"], Strict, Expected::Valid),
    (host_relaxed_ascii, Host, &["www.example.com:443"], Relaxed, Expected::Valid),
    (host_bad_port, Host, &["example.com:http"], Strict, Expected::Error(DecodeErrorKind::InvalidNumber)),
    (
        host_repeated,
        Host,
        &["example.com", "example.org"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (host_empty, Host, &[""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (host_absent, Host, &[], Strict, Expected::Absent),
    (server_canonical, Server, &["example/1.0"], Strict, Expected::Valid),
    (server_comment, Server, &["Apache/2.4.58 (Unix)"], Strict, Expected::Valid),
    (
        server_long_products,
        Server,
        &["Apache/2.4.58 (Unix) OpenSSL/3.0.13 example-proxy/2.0 (public synthetic fixture)"],
        Strict,
        Expected::Valid
    ),
    (server_leading_ows, Server, &[" \tnginx/1.25.3 \t"], Strict, Expected::Valid),
    (
        server_repeated,
        Server,
        &["example/1.0", "example-proxy/2.0"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (server_empty, Server, &[""], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (server_ows_only, Server, &[" \t \t"], Strict, Expected::Error(DecodeErrorKind::InvalidSyntax)),
    (server_absent, Server, &[], Strict, Expected::Absent),
    (content_type_canonical, ContentType, &["application/json; charset=utf-8"], Strict, Expected::Valid),
    (content_type_common_json, ContentType, &["application/json"], Strict, Expected::Valid),
    (content_type_case_variant, ContentType, &["Application/JSON; Charset=UTF-8"], Strict, Expected::Valid),
    (content_type_two_parameters, ContentType, &["text/html; charset=utf-8; level=1"], Strict, Expected::Valid),
    (
        content_type_many_parameters,
        ContentType,
        &["multipart/form-data; boundary=------------------------1a2b3c; charset=utf-8; name=\"upload\""],
        Strict,
        Expected::Valid
    ),
    (
        content_type_quoted_boundary,
        ContentType,
        &["multipart/form-data; boundary=\"example boundary, version=1\""],
        Strict,
        Expected::Valid
    ),
    (content_type_empty_slots, ContentType, &["text/plain; ; charset=utf-8;"], Strict, Expected::Valid),
    (content_type_relaxed_slash, ContentType, &["text / html; charset=utf-8"], Relaxed, Expected::Valid),
    (
        content_type_missing_slash,
        ContentType,
        &["application"],
        Strict,
        Expected::Error(DecodeErrorKind::InvalidSyntax)
    ),
    (
        content_type_unterminated_quote,
        ContentType,
        &["text/plain; charset=\"unfinished"],
        Strict,
        Expected::Error(DecodeErrorKind::UnterminatedQuote)
    ),
    (
        content_type_repeated,
        ContentType,
        &["text/plain", "application/json"],
        Strict,
        Expected::Error(DecodeErrorKind::UnexpectedMultipleValues)
    ),
    (content_type_empty, ContentType, &[""], Strict, Expected::Error(DecodeErrorKind::InvalidToken)),
    (content_type_absent, ContentType, &[], Strict, Expected::Absent),
);
