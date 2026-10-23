// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Independent semantic expectations for Location and component construction.

#![cfg(feature = "headers-location")]

use std::hash::{DefaultHasher, Hash, Hasher};

use http_headers::headers::{Location, LocationOwned, UriAuthority, UriReference};
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, SingleValueField};

fn fingerprint(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Eq, PartialEq)]
struct Components<'a> {
    scheme: Option<&'a str>,
    authority: Option<(Option<&'a str>, &'a str, Option<&'a str>)>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn components(uri: UriReference<'_>) -> Components<'_> {
    Components {
        scheme: uri.scheme(),
        authority: uri
            .authority()
            .map(|authority| (authority.userinfo(), authority.host(), authority.port())),
        path: uri.path(),
        query: uri.query(),
        fragment: uri.fragment(),
    }
}

#[test]
fn complete_reference_forms_have_independent_expected_components() {
    let cases = [
        ("", None, None, "", None, None),
        ("/", None, None, "/", None, None),
        ("a", None, None, "a", None, None),
        ("../next", None, None, "../next", None, None),
        ("./a:b", None, None, "./a:b", None, None),
        ("/a:b", None, None, "/a:b", None, None),
        ("a/b:c", None, None, "a/b:c", None, None),
        ("?", None, None, "", Some(""), None),
        ("#", None, None, "", None, Some("")),
        ("?#", None, None, "", Some(""), Some("")),
        ("?q=a/b?c", None, None, "", Some("q=a/b?c"), None),
        ("#a/b?c", None, None, "", None, Some("a/b?c")),
        ("/a?#", None, None, "/a", Some(""), Some("")),
        ("foo:", Some("foo"), None, "", None, None),
        ("foo:/", Some("foo"), None, "/", None, None),
        ("mailto:user@example.com", Some("mailto"), None, "user@example.com", None, None),
        ("urn:a:b:c", Some("urn"), None, "a:b:c", None, None),
        ("//", None, Some((None, "", None)), "", None, None),
        ("///path", None, Some((None, "", None)), "/path", None, None),
        ("//@:", None, Some((Some(""), "", Some(""))), "", None, None),
        ("//host/path", None, Some((None, "host", None)), "/path", None, None),
        ("https://host", Some("https"), Some((None, "host", None)), "", None, None),
        ("https://host:", Some("https"), Some((None, "host", Some(""))), "", None, None),
        (
            "https://host:000443/?#",
            Some("https"),
            Some((None, "host", Some("000443"))),
            "/",
            Some(""),
            Some(""),
        ),
        (
            "HTTPS://USER:secret@EXAMPLE.com:999999999999999999999/a?b#c",
            Some("HTTPS"),
            Some((Some("USER:secret"), "EXAMPLE.com", Some("999999999999999999999"))),
            "/a",
            Some("b"),
            Some("c"),
        ),
        (
            "https://u%40p:p%3Ass@h%6Fst/a%2fb%FF?q=%23%00#%3F",
            Some("https"),
            Some((Some("u%40p:p%3Ass"), "h%6Fst", None)),
            "/a%2fb%FF",
            Some("q=%23%00"),
            Some("%3F"),
        ),
        (
            "//[2001:db8::1]:8443/path",
            None,
            Some((None, "[2001:db8::1]", Some("8443"))),
            "/path",
            None,
            None,
        ),
        (
            "//[::ffff:192.0.2.1]",
            None,
            Some((None, "[::ffff:192.0.2.1]", None)),
            "",
            None,
            None,
        ),
        ("//[vF.host:!$&]:", None, Some((None, "[vF.host:!$&]", Some(""))), "", None, None),
        ("//[V1.future]", None, Some((None, "[V1.future]", None)), "", None, None),
        ("//192.0.2.1", None, Some((None, "192.0.2.1", None)), "", None, None),
        ("//999.0.2.1", None, Some((None, "999.0.2.1", None)), "", None, None),
    ];
    for (wire, scheme, authority, path, query, fragment) in cases {
        let expected = Components {
            scheme,
            authority,
            path,
            query,
            fragment,
        };
        let field = FieldValue::from_str(wire).unwrap();
        let owned = LocationOwned::try_from(field.clone()).unwrap();
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap();
            let mode_owned = <Location as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap();
            assert_eq!(components(view.uri_reference()), expected, "{wire}");
            assert_eq!(components(mode_owned.uri_reference()), expected, "{wire}");
            assert_eq!(view.uri_reference().as_str(), wire);
            assert_eq!(mode_owned.uri_reference(), owned.uri_reference());
            assert_eq!(fingerprint(&view.uri_reference()), fingerprint(&mode_owned.uri_reference()));
            assert!(!view.was_normalized());
            assert!(!mode_owned.was_normalized());
            assert_eq!(view.as_bytes(), wire.as_bytes());
            assert_eq!(view.uri_reference().as_str().as_ptr(), view.as_str().unwrap().as_ptr());
        }
        let uri = owned.uri_reference();
        let rebuilt = LocationOwned::from_components(uri.scheme(), uri.authority(), uri.path(), uri.query(), uri.fragment()).unwrap();
        assert_eq!(rebuilt.as_str().unwrap(), wire);
        assert_eq!(components(rebuilt.uri_reference()), expected, "{wire}");
        assert_eq!(rebuilt, owned);
        assert_eq!(fingerprint(&rebuilt), fingerprint(&owned));
        assert!(rebuilt.into_field_value().is_sensitive());
    }
}

#[test]
fn simple_authority_boundaries_exclude_query_and_fragment_delimiters() {
    for (wire, port, path, query, fragment) in [
        ("https://host?next=//other:99/a", None, "", Some("next=//other:99/a"), None),
        ("https://host#next://other:99/a", None, "", None, Some("next://other:99/a")),
        (
            "https://host:?next=/a:b#fragment://x:y/z",
            Some(""),
            "",
            Some("next=/a:b"),
            Some("fragment://x:y/z"),
        ),
        (
            "https://host:01234?next=/a:b#fragment",
            Some("01234"),
            "",
            Some("next=/a:b"),
            Some("fragment"),
        ),
        ("https://host:?#", Some(""), "", Some(""), Some("")),
        ("https://host:/?#", Some(""), "/", Some(""), Some("")),
        (
            "https://host/path//to:part?x:/#fragment?y://",
            None,
            "/path//to:part",
            Some("x:/"),
            Some("fragment?y://"),
        ),
        (
            "https://host:9999999999999999999999/path/to:part?next=//other:99#fragment://x",
            Some("9999999999999999999999"),
            "/path/to:part",
            Some("next=//other:99"),
            Some("fragment://x"),
        ),
    ] {
        assert!(http_headers_simd::as_simple_uri_reference(wire.as_bytes()).is_some());
        let expected = Components {
            scheme: Some("https"),
            authority: Some((None, "host", port)),
            path,
            query,
            fragment,
        };
        let field = FieldValue::from_str(wire).unwrap();
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap();
            let owned = <Location as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap();
            assert_eq!(components(view.uri_reference()), expected, "{wire}");
            assert_eq!(components(owned.uri_reference()), expected, "{wire}");
            assert_eq!(view.as_str().unwrap(), wire);
            assert_eq!(owned.as_str().unwrap(), wire);
            assert!(!view.was_normalized());
            assert!(!owned.was_normalized());
        }
    }
}

#[test]
fn simple_authority_boundaries_cover_vector_edges_and_long_ports() {
    let long_port = "9".repeat(4097);
    for length in [0, 1, 14, 15, 16, 17, 30, 31, 32, 33, 62, 63, 64, 65] {
        let host = "h".repeat(length + 1);
        for port in [None, Some(""), Some("000443"), Some(long_port.as_str())] {
            let port_wire = port.map_or_else(String::new, |port| format!(":{port}"));
            for path in [String::new(), String::from("/"), format!("/{}", "p".repeat(length))] {
                let wire = format!("https://{host}{port_wire}{path}?next=//other:99/a#fragment://x:80/z");
                assert!(http_headers_simd::as_simple_uri_reference(wire.as_bytes()).is_some());
                let expected = Components {
                    scheme: Some("https"),
                    authority: Some((None, host.as_str(), port)),
                    path: &path,
                    query: Some("next=//other:99/a"),
                    fragment: Some("fragment://x:80/z"),
                };
                let field = FieldValue::from_str(&wire).unwrap();
                let view = <Location as SingleValueField>::decode_view(field.as_field_value_ref()).unwrap();
                let owned = LocationOwned::try_from(field.clone()).unwrap();
                assert_eq!(components(view.uri_reference()), expected);
                assert_eq!(components(owned.uri_reference()), expected);
                assert_eq!(view.as_str().unwrap(), wire);
                assert_eq!(owned.as_str().unwrap(), wire);
            }
        }
    }
}

#[test]
fn general_parser_lends_original_encoded_text_and_complete_components() {
    for (wire, expected) in [
        (
            "../a%FF?x=%00#%80",
            Components {
                scheme: None,
                authority: None,
                path: "../a%FF",
                query: Some("x=%00"),
                fragment: Some("%80"),
            },
        ),
        (
            "mailto:a%FF@example.com",
            Components {
                scheme: Some("mailto"),
                authority: None,
                path: "a%FF@example.com",
                query: None,
                fragment: None,
            },
        ),
        (
            "//user%FF@[v1.future]:000443/a%00?%FF#%80",
            Components {
                scheme: None,
                authority: Some((Some("user%FF"), "[v1.future]", Some("000443"))),
                path: "/a%00",
                query: Some("%FF"),
                fragment: Some("%80"),
            },
        ),
    ] {
        assert!(http_headers_simd::as_simple_uri_reference(wire.as_bytes()).is_none());
        let field = FieldValue::from_str(wire).unwrap();
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap();
            let owned = <Location as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap();
            assert_eq!(components(view.uri_reference()), expected);
            assert_eq!(components(owned.uri_reference()), expected);
            assert_eq!(view.as_str().unwrap(), wire);
            assert_eq!(view.as_str().unwrap().as_ptr(), field.as_bytes().as_ptr());
            assert_eq!(view.uri_reference().as_str().as_ptr(), field.as_bytes().as_ptr());
            assert_eq!(owned.uri_reference().as_str().as_ptr(), owned.as_bytes().as_ptr());
            assert!(!view.was_normalized());
            assert!(!owned.was_normalized());
            let rebuilt = LocationOwned::from_components(
                expected.scheme,
                view.uri_reference().authority(),
                expected.path,
                expected.query,
                expected.fragment,
            )
            .unwrap();
            assert_eq!(components(rebuilt.uri_reference()), expected);
            assert_eq!(rebuilt.uri_reference().as_str(), wire);
            assert_eq!(rebuilt, owned);
            assert_eq!(owned.into_field_value().as_bytes(), wire.as_bytes());
        }
    }
}

#[expect(clippy::unwrap_used, reason = "this assertion helper is used only by integration tests")]
fn assert_invalid_location_bytes(bytes: &[u8]) {
    let field = FieldValue::try_from(bytes.to_vec()).unwrap();
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        let view_error = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap_err();
        let owned_error = <Location as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap_err();
        assert_eq!(view_error, owned_error);
        assert_eq!(view_error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(view_error.header(), &FieldName::Location);
        assert_eq!(view_error.value_index(), None);
    }
}

#[test]
fn literal_unicode_is_rejected_in_every_component_and_mode() {
    for unicode in ["\u{80}", "é", "\u{301}", "界", "🦀", "\u{feff}", "\u{10ffff}"] {
        for (prefix, suffix) in [
            ("", ":path"),
            ("//", "@host/path"),
            ("//", "/path"),
            ("//host:", "/path"),
            ("//[v1.", "]/path"),
            ("/", ""),
            ("?q=", ""),
            ("#", ""),
            (r"https:\\", r"\path"),
            (r"https:\\host\", ""),
            (r"/a\?q=", ""),
            (r"/a\#", ""),
        ] {
            let wire = format!("{prefix}{unicode}{suffix}");
            assert_invalid_location_bytes(wire.as_bytes());
        }
    }
}

#[test]
fn invalid_utf8_is_rejected_across_ascii_vector_boundaries() {
    for invalid in [
        b"\x80".as_slice(),
        b"\xff",
        b"\xc0\xaf",
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\xe2\x82",
    ] {
        for padding in [0, 14, 15, 16, 30, 31, 32, 62, 63, 64] {
            for backslash in [false, true] {
                let mut wire = format!("/{}", "a".repeat(padding)).into_bytes();
                if backslash {
                    wire.push(b'\\');
                }
                wire.extend_from_slice(invalid);
                wire.extend_from_slice(b"?q=x#f");
                assert_invalid_location_bytes(&wire);
            }
        }
    }
}

#[test]
fn percent_encoded_octets_stay_opaque_in_strict_and_normalized_references() {
    for encoded in [
        "%00",
        "%7F",
        "%80",
        "%FF",
        "%c0%af",
        "%ED%A0%80",
        "%F4%90%80%80",
        "%C3%A9",
        "%E7%95%8C",
        "%F0%9F%A6%80",
    ] {
        let userinfo = format!("u{encoded}");
        let host = format!("h{encoded}");
        let path = format!("/{encoded}");
        let query = format!("q={encoded}");
        let wire = format!("//{userinfo}@{host}:{path}?{query}#{encoded}");
        let backslash_wire = format!(r"\\{userinfo}@{host}:\{encoded}?{query}#{encoded}");
        let expected = Components {
            scheme: None,
            authority: Some((Some(userinfo.as_str()), host.as_str(), Some(""))),
            path: &path,
            query: Some(&query),
            fragment: Some(encoded),
        };
        for (raw, mode, normalized) in [
            (wire.as_str(), DecodeMode::Strict, false),
            (wire.as_str(), DecodeMode::Relaxed, false),
            (backslash_wire.as_str(), DecodeMode::Relaxed, true),
        ] {
            let field = FieldValue::from_str(raw).unwrap();
            let view = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap();
            let owned = <Location as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap();
            assert_eq!(components(view.uri_reference()), expected);
            assert_eq!(components(owned.uri_reference()), expected);
            assert_eq!(view.uri_reference().as_str(), wire);
            assert_eq!(owned.uri_reference().as_str(), wire);
            assert_eq!(view.as_str().unwrap(), raw);
            assert_eq!(owned.as_str().unwrap(), raw);
            assert_eq!(view.was_normalized(), normalized);
            assert_eq!(owned.was_normalized(), normalized);
            let forwarded = owned.into_field_value();
            assert_eq!(forwarded.as_bytes(), raw.as_bytes());
            assert!(forwarded.is_sensitive());
        }
        let field = FieldValue::from_str(&backslash_wire).unwrap();
        assert_eq!(
            <Location as SingleValueField>::decode_view(field.as_field_value_ref())
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}

#[test]
fn byte_general_parsing_preserves_invalid_syntax_errors() {
    for bytes in [
        b"\xff".as_slice(),
        b"../\xff",
        b"https://host/\xff",
        b"/a\\\xff",
        b"https:\\\\host\\\xc3",
        "https://host/café".as_bytes(),
        "https:\\\\host\\café".as_bytes(),
        b"../%GG",
        b"/a\\%GG",
    ] {
        let field = FieldValue::try_from(bytes.to_vec()).unwrap();
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view_error = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), mode).unwrap_err();
            let owned_error = <Location as SingleValueField>::decode_owned_with(field.clone(), mode).unwrap_err();
            assert_eq!(view_error, owned_error);
            assert_eq!(view_error.kind(), DecodeErrorKind::InvalidSyntax);
            assert_eq!(view_error.header(), &FieldName::Location);
            assert_eq!(view_error.value_index(), None);
        }
    }
}

#[test]
fn byte_general_parsing_retains_normalized_encoded_components() {
    let wire = r"https:\\user:p%FFss@[2001:db8::1]:\a%2Fb?%FF#%80";
    let field = FieldValue::from_str(wire).unwrap();
    let view = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), DecodeMode::Relaxed).unwrap();
    let owned = <Location as SingleValueField>::decode_owned_with(field.clone(), DecodeMode::Relaxed).unwrap();
    let expected = Components {
        scheme: Some("https"),
        authority: Some((Some("user:p%FFss"), "[2001:db8::1]", Some(""))),
        path: "/a%2Fb",
        query: Some("%FF"),
        fragment: Some("%80"),
    };
    assert_eq!(components(view.uri_reference()), expected);
    assert_eq!(components(owned.uri_reference()), expected);
    assert_eq!(
        view.uri_reference().as_str(),
        concat!("https://", "user:p%FFss@", "[2001:db8::1]:/a%2Fb?%FF#%80")
    );
    assert_eq!(owned.uri_reference().as_str(), view.uri_reference().as_str());
    assert_eq!(view.as_str().unwrap(), wire);
    assert_eq!(owned.as_str().unwrap(), wire);
    assert!(view.was_normalized());
    assert!(owned.was_normalized());
    let forwarded = owned.into_field_value();
    assert_eq!(forwarded.as_bytes(), wire.as_bytes());
    assert!(forwarded.is_sensitive());
}

#[test]
fn owned_ascii_projection_retains_encoded_octets_across_vector_boundaries() {
    let encoded = "%00%7f%80%FF";
    let tail = "?x=%FE#%FD";
    let fixed_length = 1 + encoded.len() + tail.len();
    for length in [fixed_length, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257] {
        let path = format!("/{}{encoded}", "a".repeat(length - fixed_length));
        let wire = format!("{path}{tail}");
        let owned = LocationOwned::try_from(wire.as_str()).unwrap();
        let expected = Components {
            scheme: None,
            authority: None,
            path: &path,
            query: Some("x=%FE"),
            fragment: Some("%FD"),
        };
        for _ in 0..2 {
            assert_eq!(components(owned.uri_reference()), expected);
            assert_eq!(owned.uri_reference().as_str(), wire);
            assert_eq!(owned.uri_reference().as_str().as_ptr(), owned.as_bytes().as_ptr());
            assert_eq!(owned.as_str().unwrap(), wire);
            assert!(!owned.was_normalized());
        }
        let clone = owned.clone();
        drop(owned);
        assert_eq!(components(clone.uri_reference()), expected);
        let field = clone.into_field_value();
        assert_eq!(field.as_bytes(), wire.as_bytes());
        assert!(field.is_sensitive());
    }
    for wire in ["", "/", "?", "#", "?#", "//", "//@:", "/%FF", "?%FF", "#%80"] {
        let owned = LocationOwned::try_from(wire).unwrap();
        assert_eq!(owned.uri_reference().as_str(), wire);
        assert_eq!(owned.as_str().unwrap(), wire);
    }
}

#[test]
fn owned_ascii_projection_keeps_normalized_backing_separate() {
    let wire = r"/a\%FF?x=%80#%00";
    let field = FieldValue::from_str(wire).unwrap();
    let owned = <Location as SingleValueField>::decode_owned_with(field, DecodeMode::Relaxed).unwrap();
    let expected = Components {
        scheme: None,
        authority: None,
        path: "/a/%FF",
        query: Some("x=%80"),
        fragment: Some("%00"),
    };
    assert_eq!(components(owned.uri_reference()), expected);
    assert_eq!(owned.uri_reference().as_str(), "/a/%FF?x=%80#%00");
    assert_eq!(owned.as_str().unwrap(), wire);
    assert!(owned.was_normalized());
    let clone = owned.clone();
    drop(owned);
    assert_eq!(components(clone.uri_reference()), expected);
    assert_eq!(clone.as_str().unwrap(), wire);
    let forwarded = clone.into_field_value();
    assert_eq!(forwarded.as_bytes(), wire.as_bytes());
    assert!(forwarded.is_sensitive());
}

#[test]
fn relaxed_backslashes_retain_wire_and_normalized_component_boundaries() {
    for (wire, semantic, host, path, query, fragment) in [
        (r"/a\b", "/a/b", None, "/a/b", None, None),
        (r"\\host\a", "//host/a", Some("host"), "/a", None, None),
        (r"https:\\host\a", "https://host/a", Some("host"), "/a", None, None),
        (
            r"https://host\@other/x",
            "https://host/@other/x",
            Some("host"),
            "/@other/x",
            None,
            None,
        ),
        (
            r"https://host\?q=\#f=\",
            "https://host/?q=/#f=/",
            Some("host"),
            "/",
            Some("q=/"),
            Some("f=/"),
        ),
        (r"\?#\", "/?#/", None, "/", Some(""), Some("/")),
    ] {
        let field = FieldValue::from_str(wire).unwrap();
        assert_eq!(
            <Location as SingleValueField>::decode_view(field.as_field_value_ref())
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        let view = <Location as SingleValueField>::decode_view_with(field.as_field_value_ref(), DecodeMode::Relaxed).unwrap();
        let owned = <Location as SingleValueField>::decode_owned_with(field.clone(), DecodeMode::Relaxed).unwrap();
        assert!(view.was_normalized());
        assert!(owned.was_normalized());
        assert_eq!(view.as_str().unwrap(), wire);
        assert_eq!(owned.as_bytes(), wire.as_bytes());
        assert_eq!(fingerprint(&owned), fingerprint(&field));
        assert_eq!(fingerprint(&view), fingerprint(&wire));
        let expected = LocationOwned::try_from(semantic).unwrap();
        for uri in [view.uri_reference(), owned.uri_reference()] {
            assert_eq!(uri.as_str(), semantic);
            assert_eq!(uri.authority().map(UriAuthority::host), host);
            assert_eq!(uri.path(), path);
            assert_eq!(uri.query(), query);
            assert_eq!(uri.fragment(), fragment);
            assert_eq!(uri, expected.uri_reference());
            assert_eq!(fingerprint(&uri), fingerprint(&expected.uri_reference()));
        }
        assert_ne!(owned, expected);
        let cloned = view.clone();
        let wire_borrow = view.as_str().unwrap();
        drop(view);
        assert_eq!(wire_borrow, wire);
        assert_eq!(cloned.uri_reference().as_str(), semantic);
        let moved = vec![cloned].pop().unwrap();
        assert_eq!(moved.uri_reference().as_str(), semantic);
        let cloned_owned = owned.clone();
        drop(owned);
        assert_eq!(cloned_owned.uri_reference().as_str(), semantic);
        let encoded = cloned_owned.into_field_value();
        assert_eq!(encoded.as_bytes(), wire.as_bytes());
        assert!(encoded.is_sensitive());
    }
}

#[test]
fn validated_construction_checks_component_and_contextual_grammar() {
    let authority = UriAuthority::new(Some("user:p%40ss"), "[::1]", Some("")).unwrap();
    let constructed = LocationOwned::from_components(Some("https"), Some(authority), "/p%2Fq", Some("a?b/c"), Some("")).unwrap();
    assert_eq!(constructed.as_str().unwrap(), "https://user:p%40ss@[::1]:/p%2Fq?a?b/c#");
    assert_eq!(
        constructed.uri_reference().authority(),
        Some(UriAuthority::new(Some("user:p%40ss"), "[::1]", Some("")).unwrap())
    );
    for (scheme, authority, path, query, fragment) in [
        (Some(""), None, "", None, None),
        (Some("1http"), None, "", None, None),
        (Some("ht%74p"), None, "", None, None),
        (Some("ht tp"), None, "", None, None),
        (None, Some(authority), "relative", None, None),
        (Some("s"), Some(authority), "relative", None, None),
        (None, None, "//host", None, None),
        (Some("s"), None, "//host", None, None),
        (None, None, "s:opaque", None, None),
        (None, None, "a%2Fb:c", None, None),
        (None, None, "/bad?path", None, None),
        (None, None, "/bad#path", None, None),
        (None, None, "/bad%2", None, None),
        (None, None, "/bad space", None, None),
        (None, None, "/café", None, None),
        (None, None, "", Some("bad#query"), None),
        (None, None, "", Some("bad%xx"), None),
        (None, None, "", None, Some("bad#fragment")),
        (None, None, "", None, Some("bad\\fragment")),
        (None, None, "", None, Some("\n")),
    ] {
        let error = LocationOwned::from_components(scheme, authority, path, query, fragment).unwrap_err();
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(error.header(), &FieldName::Location);
        assert_eq!(error.value_index(), None);
    }
    let colon = LocationOwned::from_components(None, None, "a%3Ab/c:d", None, None).unwrap();
    assert_eq!(colon.uri_reference().path(), "a%3Ab/c:d");
    let authority_path = LocationOwned::from_components(None, Some(authority), "//p", None, None).unwrap();
    assert_eq!(authority_path.uri_reference().path(), "//p");
}

#[test]
fn authority_construction_preserves_full_generic_host_and_port_grammar() {
    for host in [
        "",
        "host",
        "999.0.0.1",
        "h%ffst",
        "!$&'()*+,;=",
        "[::]",
        "[::ffff:192.0.2.1]",
        "[v1.a:b]",
        "[VF.!$&'()*+,;=:_~-]",
    ] {
        for userinfo in [None, Some(""), Some("user:p%40ss")] {
            for port in [None, Some(""), Some("00080"), Some("9999999999999999999999999999999999")] {
                let authority = UriAuthority::new(userinfo, host, port).unwrap();
                assert_eq!(authority.userinfo(), userinfo);
                assert_eq!(authority.host(), host);
                assert_eq!(authority.port(), port);
                let built = LocationOwned::from_components(None, Some(authority), "", None, None).unwrap();
                let parsed = LocationOwned::try_from(built.as_str().unwrap()).unwrap();
                assert_eq!(parsed.uri_reference().authority(), Some(authority));
                assert_eq!(fingerprint(&parsed.uri_reference().authority().unwrap()), fingerprint(&authority));
            }
        }
    }
    for (userinfo, host, port) in [
        (Some("a@b"), "host", None),
        (Some("a%2"), "host", None),
        (Some("a/b"), "host", None),
        (None, "[v.a]", None),
        (None, "[v1.]", None),
        (None, "[vG.a]", None),
        (None, "[v1.a%20]", None),
        (None, "[::gg]", None),
        (None, "[::1", None),
        (None, "::1", None),
        (None, "[fe80::1%25eth0]", None),
        (None, "h%zzst", None),
        (None, "host/path", None),
        (None, "host?query", None),
        (None, "host#fragment", None),
        (None, "user@host", None),
        (None, "münich.example", None),
        (None, "host:443", None),
        (None, "host", Some("a")),
        (None, "host", Some("-1")),
        (None, "host", Some(" 80")),
        (None, "host", Some("%38%30")),
    ] {
        assert_eq!(
            UriAuthority::new(userinfo, host, port).unwrap_err().kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}

#[test]
fn component_validators_match_the_full_parser_without_delimiter_reinterpretation() {
    let candidates = (0_u8..=127)
        .map(|byte| char::from(byte).to_string())
        .chain(["", "%00", "%2f", "%FF", "%", "%0", "%GG", "é"].into_iter().map(str::to_owned));
    for candidate in candidates {
        let path = format!("/a{candidate}");
        let wire = format!("https://host{path}");
        let expected =
            fluent_uri::Uri::parse(&wire).is_ok_and(|uri| uri.path().as_str() == path && uri.query().is_none() && uri.fragment().is_none());
        let authority = UriAuthority::new(None, "host", None).unwrap();
        assert_eq!(
            LocationOwned::from_components(Some("https"), Some(authority), &path, None, None).is_ok(),
            expected,
            "path {candidate:?}"
        );

        let wire = format!("https://host/?{candidate}");
        let expected = fluent_uri::Uri::parse(&wire)
            .is_ok_and(|uri| uri.query().is_some_and(|query| query.as_str() == candidate) && uri.fragment().is_none());
        assert_eq!(
            LocationOwned::from_components(Some("https"), Some(authority), "/", Some(&candidate), None).is_ok(),
            expected,
            "query {candidate:?}"
        );

        let wire = format!("https://host/#{candidate}");
        let expected = fluent_uri::Uri::parse(&wire).is_ok_and(|uri| uri.fragment().is_some_and(|fragment| fragment.as_str() == candidate));
        assert_eq!(
            LocationOwned::from_components(Some("https"), Some(authority), "/", None, Some(&candidate)).is_ok(),
            expected,
            "fragment {candidate:?}"
        );

        let wire = format!("//{candidate}@host");
        let expected = fluent_uri::Uri::parse(&wire).is_ok_and(|uri| {
            uri.authority().is_some_and(|authority| {
                authority.userinfo().is_some_and(|userinfo| userinfo.as_str() == candidate) && authority.host().as_str() == "host"
            }) && uri.path().as_str().is_empty()
                && uri.query().is_none()
                && uri.fragment().is_none()
        });
        assert_eq!(
            UriAuthority::new(Some(&candidate), "host", None).is_ok(),
            expected,
            "userinfo {candidate:?}"
        );

        let wire = format!("//{candidate}");
        let expected = fluent_uri::Uri::parse(&wire).is_ok_and(|uri| {
            uri.authority().is_some_and(|authority| {
                authority.userinfo().is_none() && authority.host().as_str() == candidate && authority.port().is_none()
            }) && uri.path().as_str().is_empty()
                && uri.query().is_none()
                && uri.fragment().is_none()
        });
        assert_eq!(UriAuthority::new(None, &candidate, None).is_ok(), expected, "host {candidate:?}");

        let scheme = format!("s{candidate}");
        let wire = format!("{scheme}:");
        let expected = fluent_uri::Uri::parse(&wire)
            .is_ok_and(|uri| uri.scheme().is_some_and(|parsed| parsed.as_str() == scheme) && uri.path().as_str().is_empty());
        assert_eq!(
            LocationOwned::from_components(Some(&scheme), None, "", None, None).is_ok(),
            expected,
            "scheme {candidate:?}"
        );

        let host = format!("[v1.{candidate}]");
        let wire = format!("//{host}");
        let expected = fluent_uri::Uri::parse(&wire).is_ok_and(|uri| {
            uri.authority()
                .is_some_and(|authority| authority.host().as_str() == host && authority.port().is_none())
        });
        assert_eq!(UriAuthority::new(None, &host, None).is_ok(), expected, "IPvFuture {candidate:?}");

        let wire = format!("//host:{candidate}");
        let expected = fluent_uri::Uri::parse(&wire).is_ok_and(|uri| {
            uri.authority()
                .is_some_and(|authority| authority.port() == Some(candidate.as_str()))
                && uri.path().as_str().is_empty()
                && uri.query().is_none()
                && uri.fragment().is_none()
        });
        assert_eq!(
            UriAuthority::new(None, "host", Some(&candidate)).is_ok(),
            expected,
            "port {candidate:?}"
        );
    }
}

#[test]
fn sensitive_debug_and_exact_spelling_equality() {
    let field = FieldValue::from_static("https://private:secret@host/private?secret#private");
    let view = <Location as SingleValueField>::decode_view(field.as_field_value_ref()).unwrap();
    let owned = LocationOwned::try_from(field.clone()).unwrap();
    for debug in [
        format!("{view:?}"),
        format!("{owned:?}"),
        format!("{:?}", view.uri_reference()),
        format!("{:?}", view.uri_reference().authority().unwrap()),
    ] {
        assert!(debug.contains("redacted"));
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("private"));
    }
    assert_eq!(fingerprint(&view), fingerprint(&view.clone()));
    assert_eq!(owned, owned.clone());
    assert_eq!(fingerprint(&owned), fingerprint(&field));
    assert_eq!(fingerprint(&view), fingerprint(&view.as_str().unwrap()));
    let other_case = LocationOwned::try_from("HTTPS://private:secret@host/private?secret#private").unwrap();
    assert_ne!(owned.uri_reference(), other_case.uri_reference());
    let no_port = UriAuthority::new(None, "", None).unwrap();
    assert_ne!(no_port, UriAuthority::new(None, "", Some("")).unwrap());
    assert_ne!(no_port, UriAuthority::new(Some(""), "", None).unwrap());
}

#[test]
fn borrowed_equality_and_hashing_preserve_exact_wire_spelling() {
    for (left_wire, right_wire, equal) in [("/a", "/a", true), ("/a", "/A", false), ("/%61", "/a", false)] {
        let left_field = FieldValue::from_str(left_wire).unwrap();
        let right_field = FieldValue::from_str(right_wire).unwrap();
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let left = <Location as SingleValueField>::decode_view_with(left_field.as_field_value_ref(), mode).unwrap();
            let right = <Location as SingleValueField>::decode_view_with(right_field.as_field_value_ref(), mode).unwrap();
            assert_eq!(left == right, equal);
            assert_eq!(fingerprint(&left), fingerprint(&left_wire));
            assert_eq!(fingerprint(&right), fingerprint(&right_wire));
        }
    }
}

struct Source<'a>(&'a [FieldValue]);

impl FieldSource for Source<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == &FieldName::Location)
            .then(|| FieldLines::from_slice(name, self.0))
            .flatten()
    }
}

#[test]
fn source_boundaries_and_error_behavior_are_preserved() {
    assert!(Location::view(&Source(&[])).unwrap().is_none());
    for wire in ["bad%xx", "https://host/a b", "a:b#c#d", r"bad\%xx", "//[::gg]", "//host:a"] {
        let values = [FieldValue::from_str(wire).unwrap()];
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view_error = Location::view_with(&Source(&values), mode).unwrap_err();
            let owned_error = Location::owned_with(&Source(&values), mode).unwrap_err();
            assert_eq!(view_error, owned_error);
            assert_eq!(view_error.kind(), DecodeErrorKind::InvalidSyntax);
            assert_eq!(view_error.header(), &FieldName::Location);
            assert_eq!(view_error.value_index(), None);
        }
    }
    let duplicates = [FieldValue::from_static("%xx"), FieldValue::from_static("/valid")];
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        let error = Location::view_with(&Source(&duplicates), mode).unwrap_err();
        assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
        assert_eq!(error.value_index(), None);
        assert_eq!(error, Location::owned_with(&Source(&duplicates), mode).unwrap_err());
    }
    let mut wire = String::from("/");
    wire.extend(std::iter::repeat_n('a', MAX_CUSTOM_FIELD_BYTES - 1));
    let boundary = [FieldValue::from_str(&wire).unwrap()];
    let source = Source(&boundary);
    assert_eq!(Location::view(&source).unwrap().unwrap().uri_reference().path(), wire);
    wire.push('a');
    let over = [FieldValue::from_str(&wire).unwrap()];
    assert_eq!(
        Location::view(&Source(&over)).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    assert_eq!(
        Location::owned(&Source(&over)).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    assert_eq!(LocationOwned::try_from(wire.as_str()).unwrap().uri_reference().path(), wire);
    assert_eq!(
        LocationOwned::from_components(None, None, &wire, None, None)
            .unwrap()
            .uri_reference()
            .path(),
        wire
    );
}

struct RawSource<'a>(&'a [u8]);

impl FieldSource for RawSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == &FieldName::Location).then(|| FieldLines::single(name, self.0))
    }
}

#[test]
fn raw_source_has_the_same_semantics_and_preserves_error_preflight() {
    let source = RawSource(br"\\user:secret@host:0123\path?x=%FF#");
    let view = Location::view_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    let owned = Location::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    assert_eq!(
        components(view.uri_reference()),
        Components {
            scheme: None,
            authority: Some((Some("user:secret"), "host", Some("0123"))),
            path: "/path",
            query: Some("x=%FF"),
            fragment: Some(""),
        }
    );
    assert_eq!(owned.uri_reference(), view.uri_reference());
    assert_eq!(owned.as_bytes(), source.0);
    assert!(owned.into_field_value().is_sensitive());

    for bytes in [b"/line\nbreak".as_slice(), b"/line\rbreak".as_slice(), &[0xff]] {
        let source = RawSource(bytes);
        let view_error = Location::view_with(&source, DecodeMode::Relaxed).unwrap_err();
        let owned_error = Location::owned_with(&source, DecodeMode::Relaxed).unwrap_err();
        assert_eq!(view_error, owned_error);
        assert_eq!(view_error.header(), &FieldName::Location);
        assert_eq!(view_error.kind(), DecodeErrorKind::InvalidSyntax);
    }
}

#[cfg(feature = "http")]
#[test]
fn http_forwarding_retains_original_sensitive_wire() {
    let mut headers = http::HeaderMap::new();
    headers.insert("location", http::HeaderValue::from_static(r"https:\\private\path?secret#fragment"));
    let view = Location::view_with(&headers, DecodeMode::Relaxed).unwrap().unwrap();
    let owned = Location::owned_with(&headers, DecodeMode::Relaxed).unwrap().unwrap();
    assert_eq!(view.uri_reference(), owned.uri_reference());
    let mut borrowed_sink = http::HeaderMap::new();
    view.insert_into(&mut borrowed_sink).unwrap();
    assert_eq!(borrowed_sink["location"].as_bytes(), headers["location"].as_bytes());
    assert!(borrowed_sink["location"].is_sensitive());
    let mut owned_sink = http::HeaderMap::new();
    Location::insert(&mut owned_sink, owned).unwrap();
    assert_eq!(owned_sink["location"], borrowed_sink["location"]);
    assert!(owned_sink["location"].is_sensitive());
}
