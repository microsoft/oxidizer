// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public parser boundaries from the November 4 afternoon campaign.

#![cfg(feature = "headers-all")]

use std::fmt::Write;

use http_headers::headers::*;
use http_headers::source::FieldLines;
#[cfg(feature = "http")]
use http_headers::source::FieldSource;
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue};

use self::common::TestMap;

mod common;

fn source<H: Field>(values: &[&str]) -> TestMap {
    let mut result = TestMap::default();
    result.0.insert(
        H::name().clone(),
        values
            .iter()
            .map(|value| FieldValue::from_str(value).expect("test fixtures contain only valid HTTP field-value bytes"))
            .collect(),
    );
    result
}

fn rejects<H: Field>(wire: &str) {
    let source = source::<H>(&[wire]);
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert!(H::view_with(&source, mode).is_err(), "{wire:?}/{mode:?}");
        assert!(H::owned_with(&source, mode).is_err(), "{wire:?}/{mode:?}");
    }
}

#[test]
fn afternoon_cache_directive_projection_preserves_quoted_delimiters_and_max_age() {
    for (wire, expected) in [
        ("max-age", vec![("max-age", None)]),
        (
            "max-age=\"123\", x=\"a,b\", no-cache",
            vec![("max-age", Some("\"123\"")), ("x", Some("\"a,b\"")), ("no-cache", None)],
        ),
        (
            "max-age=\"1\\2\", x=\"a\\\"b,c\", no-store",
            vec![("max-age", Some("\"1\\2\"")), ("x", Some("\"a\\\"b,c\"")), ("no-store", None)],
        ),
        (
            "max-age=\"18446744073709551616\", private",
            vec![("max-age", Some("\"18446744073709551616\"")), ("private", None)],
        ),
        ("max-age=\"12x\", public", vec![("max-age", Some("\"12x\"")), ("public", None)]),
        ("max-age=\"\", immutable", vec![("max-age", Some("\"\"")), ("immutable", None)]),
        (
            "max-age=123, no-transform, only-if-cached, must-revalidate, proxy-revalidate",
            vec![
                ("max-age", Some("123")),
                ("no-transform", None),
                ("only-if-cached", None),
                ("must-revalidate", None),
                ("proxy-revalidate", None),
            ],
        ),
    ] {
        let source = source::<CacheControl>(&[wire]);
        let view = CacheControl::view(&source).unwrap().unwrap();
        let owned = CacheControl::owned(&source).unwrap().unwrap();
        let project = |directive: CacheDirectiveView<'_>| {
            (
                directive.name().to_owned(),
                directive.value().map(|value| str::from_utf8(value).unwrap().to_owned()),
            )
        };
        let expected = expected
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.map(str::to_owned)))
            .collect::<Vec<_>>();
        assert_eq!(view.directives().map(project).collect::<Vec<_>>(), expected, "{wire}");
        assert_eq!(owned.directives().map(project).collect::<Vec<_>>(), expected, "{wire}");
        assert_eq!(view.no_cache(), wire.contains("no-cache"));
        assert_eq!(owned.no_cache(), wire.contains("no-cache"));
    }
    for (wire, expected) in [
        ("max-age=\"123\"", Some(123)),
        ("max-age=\"1\\2\"", None),
        ("max-age=\"18446744073709551616\"", None),
        ("max-age=\"18446744073709551620\"", None),
        ("max-age=\"12x\"", None),
        ("max-age=\"\"", None),
        ("x-custom=123", None),
        ("max-ageX=123", None),
    ] {
        let source = source::<CacheControl>(&[wire]);
        assert_eq!(
            CacheControl::view(&source).unwrap().unwrap().max_age().map(|d| d.as_secs()),
            expected
        );
        assert_eq!(
            CacheControl::owned(&source).unwrap().unwrap().max_age().map(|d| d.as_secs()),
            expected
        );
    }
}

#[test]
fn afternoon_relaxed_dates_validate_each_component_before_normalizing() {
    let canonical = source::<LastModified>(&["Sun, 06 Nov 1994 08:49:37 GMT"]);
    let expected = std::time::UNIX_EPOCH + std::time::Duration::from_secs(784_111_777);
    assert_eq!(LastModified::owned(&canonical).unwrap().unwrap().date(), expected);
    for wire in ["Sun, 6 Nov 1994 8:49:37 UTC", "\tSun, 6 Nov 1994 8:49:37 UTC \t"] {
        let source = source::<LastModified>(&[wire]);
        assert_eq!(
            LastModified::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap().date(),
            expected
        );
    }
    for wire in [
        "Bad, 6 Nov 1994 8:49:37 UTC",
        "Sun, 6 Bad 1994 8:49:37 UTC",
        "Sun, 6 Nov 1994 8:49:37 BAD",
        "Sun, 6 Nov 1969 8:49:37 UTC",
        "Sun, 6 Nov 10000 8:49:37 UTC",
        "Sun, 6 Nov 1994 24:49:37 UTC",
        "Sun, 6 Nov 1994 8:60:37 UTC",
        "Sun, 6 Nov 1994 8:49:60 UTC",
        "Sun, 6 Nov 1994 8:49:37:1 UTC",
        "Sun, 6 Nov 1994 8:49:37 UTC extra",
        "Sun, 6 Nov 1994 :49:37 UTC",
        "Sun, 6 Nov 1994 8::37 UTC",
        "Sun, 6 Nov 1994 8:49: UTC",
    ] {
        rejects::<LastModified>(wire);
    }
}

#[test]
fn afternoon_websocket_shortcuts_preserve_names_parameters_and_near_misses() {
    for wire in [
        "permessage-deflate",
        "permessage-deflate; client_max_window_bits",
        "permessage-deflate; server_no_context_takeover; client_no_context_takeover",
        "permessage-deflate; x=\"one\\two\"; y=\"a\\b\"",
        "custom; x=1, second; y=two",
    ] {
        let source = source::<SecWebSocketExtensions>(&[wire]);
        let view = SecWebSocketExtensions::view(&source).unwrap().unwrap();
        let owned = SecWebSocketExtensions::owned(&source).unwrap().unwrap();
        for extensions in [
            view.extensions().collect::<Result<Vec<_>, _>>().unwrap(),
            owned.extensions().collect::<Result<Vec<_>, _>>().unwrap(),
        ] {
            let expected = wire.split(", ").collect::<Vec<_>>();
            assert_eq!(extensions.len(), expected.len());
            for (extension, expected) in extensions.iter().zip(expected) {
                let mut parts = expected.split("; ");
                assert_eq!(extension.name(), parts.next().unwrap());
                let parameters = extension.parameters().collect::<Result<Vec<_>, _>>().unwrap();
                let expected = parts.collect::<Vec<_>>();
                assert_eq!(parameters.len(), expected.len());
                for (parameter, expected) in parameters.iter().zip(expected) {
                    let (name, value) = expected
                        .split_once('=')
                        .map_or((expected, None), |(name, value)| (name, Some(value.as_bytes())));
                    assert_eq!(parameter.name(), name);
                    assert_eq!(parameter.value(), value);
                    assert_eq!(parameter.is_quoted(), value.is_some_and(|value| value.starts_with(b"\"")));
                }
            }
        }
        assert_eq!(owned.to_string(), wire);
    }
    for wire in [
        "permessage-deflate; =1",
        "permessage-deflate; x=",
        "permessage-deflate; x=\"unterminated",
        "permessage-deflate; x=\"a\"junk",
        "permessage-deflate; x=\"one,two\"",
        "permessage-deflate; x=\"a\\\"b\"",
        "permessage-deflate; server_no_context_takeover client_no_context_takeover",
    ] {
        rejects::<SecWebSocketExtensions>(wire);
    }
    for wire in [
        "graphql-ws",
        "chat, superchat",
        "graphql-transport-ws",
        "graphql-transport-ws, graphql-ws",
    ] {
        let source = source::<SecWebSocketProtocol>(&[wire]);
        let expected = wire.split(", ").collect::<Vec<_>>();
        assert_eq!(
            SecWebSocketProtocol::view(&source)
                .unwrap()
                .unwrap()
                .protocols()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
        assert_eq!(
            SecWebSocketProtocol::owned(&source)
                .unwrap()
                .unwrap()
                .protocols()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
    }
}

#[test]
fn afternoon_content_type_metadata_handles_inline_overflow_and_empty_construction() {
    for count in [0, 1, 2, 3, 4, 8, 17] {
        let parameters = (0..count).fold(String::new(), |mut wire, i| {
            write!(wire, "; p{i}=v{i}").unwrap();
            wire
        });
        let wire = format!("application/x-custom{parameters}");
        let source = source::<ContentType>(&[&wire]);
        let view = ContentType::view(&source).unwrap().unwrap();
        let owned = ContentType::owned(&source).unwrap().unwrap();
        for parameters in [view.parameters(), owned.parameters()] {
            let parameters = parameters.collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(parameters.len(), count);
            for (index, parameter) in parameters.iter().enumerate() {
                assert_eq!(parameter.name(), format!("p{index}"));
                assert_eq!(parameter.value(), format!("v{index}").as_bytes());
            }
        }
        assert_eq!(view.type_().unwrap(), "application");
        assert_eq!(view.subtype().unwrap(), "x-custom");
        assert_eq!(owned.type_().unwrap(), "application");
        assert_eq!(owned.subtype().unwrap(), "x-custom");
    }
}

#[test]
fn afternoon_invalid_range_delimiters_are_rejected_after_each_boundary() {
    for (wire, expected) in [
        ("bytes=1-2-3", DecodeErrorKind::InvalidSyntax),
        ("bytes=-1-2", DecodeErrorKind::InvalidSyntax),
        ("bytes=1--2", DecodeErrorKind::InvalidSyntax),
        ("bytes=1-2,3-4-5", DecodeErrorKind::InvalidSyntax),
    ] {
        let source = source::<Range>(&[wire]);
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let expected = if wire == "bytes=-1-2" && mode == DecodeMode::Strict {
                DecodeErrorKind::InvalidNumber
            } else {
                expected
            };
            assert_eq!(Range::view_with(&source, mode).unwrap_err().kind(), expected, "{wire}");
            assert_eq!(Range::owned_with(&source, mode).unwrap_err().kind(), expected, "{wire}");
        }
    }
    for wire in ["bytes 1-2/3/4", "bytes 1-2//3", "bytes 1--2/3", "bytes 1-2-/3", "bytes */3/4"] {
        let source = source::<ContentRange>(&[wire]);
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let expected = if wire == "bytes */3/4" && mode == DecodeMode::Strict {
                DecodeErrorKind::InvalidNumber
            } else {
                DecodeErrorKind::InvalidSyntax
            };
            assert_eq!(ContentRange::view_with(&source, mode).unwrap_err().kind(), expected, "{wire}");
            assert_eq!(ContentRange::owned_with(&source, mode).unwrap_err().kind(), expected, "{wire}");
        }
    }
}

#[test]
fn afternoon_cors_origins_reject_numeric_and_authority_boundary_violations() {
    for wire in [
        "https://",
        "https://host:00001",
        "https://host:000001",
        "https://host:65536",
        "https://host:1:2",
        "https://256.1.1.1",
        "https://1.2.3.0001",
        "https://1.2.3.01",
        "https://[::1]:00001",
        "https://[::1]extra",
        "https://[1111:2222:3333:4444:5555:6666:7777:88888]",
        "https://user@host",
    ] {
        rejects::<AccessControlAllowOrigin>(wire);
    }
}

#[test]
fn afternoon_hsts_builder_capacity_is_not_the_serialized_length() {
    let value = "a".repeat(64);
    let owned = StrictTransportSecurityOwned::builder(std::time::Duration::from_secs(7))
        .extension_value("custom", &value)
        .build()
        .unwrap();
    let mut sink = TestMap::default();
    StrictTransportSecurity::insert(&mut sink, owned).unwrap();
    assert_eq!(
        sink.get_all(StrictTransportSecurity::name())
            .iter()
            .map(FieldValue::as_bytes)
            .collect::<Vec<_>>(),
        [format!("max-age=7; custom={value}").as_bytes()]
    );
}

#[test]
fn afternoon_delimiters_only_treat_backslash_as_an_escape_inside_quotes() {
    let values = [
        FieldValue::from_static("first\\,second,\"third\\\",fourth\",last"),
        FieldValue::from_static("next"),
    ];
    let lines = FieldLines::from_slice(&FieldName::Vary, &values).unwrap();
    assert_eq!(
        lines.comma_items().collect::<Result<Vec<_>, _>>().unwrap(),
        [b"first\\".as_slice(), b"second", b"\"third\\\",fourth\"", b"last", b"next"]
    );
    let values = [FieldValue::from_static("first\\;second;\"third\\\";fourth\";last")];
    let lines = FieldLines::from_slice(&FieldName::CacheControl, &values).unwrap();
    assert_eq!(
        lines.semicolon_items().collect::<Result<Vec<_>, _>>().unwrap(),
        [b"first\\".as_slice(), b"second", b"\"third\\\";fourth\"", b"last"]
    );
}

#[cfg(feature = "http")]
#[test]
fn afternoon_trusted_http_source_does_not_apply_custom_list_budget() {
    let wire = std::iter::repeat_n("x", 1_025).collect::<Vec<_>>().join(",");
    let mut map = http::HeaderMap::new();
    map.insert(http::header::VARY, http::HeaderValue::from_str(&wire).unwrap());
    let lines = FieldSource::lines(&map, &FieldName::Vary).unwrap();
    let expected = vec![b"x".as_slice(); 1_025];
    assert_eq!(lines.comma_items().collect::<Result<Vec<_>, _>>().unwrap(), expected);
}

#[test]
fn afternoon_location_scheme_search_does_not_match_an_earlier_scheme_character() {
    for scheme in ["x", "x-custom", "example+x"] {
        let wire = format!("{scheme}://host:123/path?query#fragment");
        let source = source::<Location>(&[&wire]);
        let view = Location::view(&source).unwrap().unwrap();
        let owned = Location::owned(&source).unwrap().unwrap();
        for uri in [view.uri_reference(), owned.uri_reference()] {
            assert_eq!(uri.scheme(), Some(scheme));
            assert_eq!(uri.path(), "/path");
            assert_eq!(uri.query(), Some("query"));
            assert_eq!(uri.fragment(), Some("fragment"));
            let authority = uri.authority().unwrap();
            assert_eq!(authority.host(), "host");
            assert_eq!(authority.port(), Some("123"));
        }
    }
}

#[test]
fn afternoon_parameter_limits_do_not_accept_a_double_increment() {
    for count in [512, 513, 1024] {
        let wire = std::iter::repeat_n("x", count).collect::<Vec<_>>().join(",");
        let extensions = source::<SecWebSocketExtensions>(&[&wire]);
        for extensions in [
            SecWebSocketExtensions::view(&extensions)
                .unwrap()
                .unwrap()
                .extensions()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            SecWebSocketExtensions::owned(&extensions)
                .unwrap()
                .unwrap()
                .extensions()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        ] {
            assert_eq!(extensions.len(), count);
            assert!(extensions.iter().all(|extension| extension.name() == "x"));
        }
        let wire = format!("bytes={}", std::iter::repeat_n("1-2", count).collect::<Vec<_>>().join(","));
        let source = source::<Range>(&[&wire]);
        for ranges in [
            Range::view(&source).unwrap().unwrap().byte_ranges().unwrap().collect::<Vec<_>>(),
            Range::owned(&source).unwrap().unwrap().byte_ranges().unwrap().collect::<Vec<_>>(),
        ] {
            assert_eq!(ranges, vec![ByteRangeSpec::FromTo { first: 1, last: 2 }; count]);
        }
    }
    let wire = std::iter::repeat_n("x", 1025).collect::<Vec<_>>().join(",");
    let source = source::<SecWebSocketExtensions>(&[&wire]);
    assert_eq!(
        SecWebSocketExtensions::view(&source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
}
