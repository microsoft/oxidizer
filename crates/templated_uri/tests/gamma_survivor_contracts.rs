// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! URI joining, redaction, and validation contracts.

use data_privacy::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString, Redactor};
use templated_uri::{BasePath, BaseUri, EscapedString, Origin, PathAndQuery, Uri, templated};

#[test]
fn gamma_round2_owned_base_path_rejects_malformed_http_input() {
    for value in ["/bad\npath/".to_owned(), "relative/".to_owned()] {
        let error = BasePath::try_from(value).unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}

#[templated(template = "/{+raw}", unredacted)]
struct InvalidPath {
    raw: String,
}

#[test]
fn gamma_survivor_component_conversion_and_invalid_template_errors_propagate() {
    BaseUri::try_from_raw_parts("invalid scheme", "example.com", 443, "/").unwrap_err();
    BaseUri::try_from_raw_parts("https", "example.com", 443, "/missing_end").unwrap_err();
    for base in [None, Some(BaseUri::from_static("https://example.com/api/"))] {
        let uri = Uri::from(InvalidPath { raw: "\n".to_owned() });
        let uri = match base {
            Some(base) => uri.with_base(base),
            None => uri,
        };
        http::Uri::try_from(uri).unwrap_err();
    }
}

struct PassThrough;

impl Redactor for PassThrough {
    fn redacts(&self, _: &DataClass) -> bool {
        false
    }

    fn redact(&self, _: &DataClass, value: &str, output: &mut dyn std::fmt::Write) -> std::fmt::Result {
        output.write_str(value)
    }
}

#[test]
fn gamma_survivor_redacted_relative_and_based_paths() {
    let redactor = PassThrough;
    let relative = Uri::from(PathAndQuery::from_static("/users"));
    assert_eq!(relative.to_redacted_string(&redactor), "/users");
    let based = relative.with_base(BaseUri::from_static("https://example.com/api/"));
    assert_eq!(based.to_redacted_string(&redactor), "https://example.com/api/users");
    assert_eq!(based.to_string().declassify_ref(), "https://example.com/api/users");
}

#[test]
fn gamma_survivor_rendered_presence_contract_and_joining() {
    let rendered = http::uri::PathAndQuery::from_static("/users?x=1");
    let uri = Uri::from(PathAndQuery::from_static("/users?x=1"));
    let error = uri.to_http_uri(None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("rendered path must be provided if and only if the URI has a path")
    );
    for base in ["https://example.com", "https://example.com/", "https://example.com/api/"] {
        let based = uri.clone().with_base(BaseUri::from_static(base));
        assert_eq!(based.to_http_uri(&rendered).unwrap(), http::Uri::try_from(based).unwrap());
    }
    for path in ["/", "//", "///", "/users", "//users"] {
        let uri = Uri::from(PathAndQuery::try_from(path).unwrap()).with_base(BaseUri::from_static("https://example.com/api/"));
        let rendered = http::uri::PathAndQuery::try_from(path).unwrap();
        let expected = format!("https://example.com/api/{}", path.trim_start_matches('/'));
        assert_eq!(uri.to_http_uri(&rendered).unwrap().to_string(), expected);
        assert_eq!(http::Uri::try_from(uri).unwrap().to_string(), expected);
    }
}

#[test]
fn gamma_survivor_join_rejects_an_oversized_combined_path() {
    let base_path = format!("/{}/", "a".repeat(40_000));
    let other_path = format!("/{}", "b".repeat(40_000));
    let base = format!("https://example.com{base_path}").parse::<BaseUri>().unwrap();
    let other = other_path.parse::<http::uri::PathAndQuery>().unwrap();
    assert!(base_path.len() + other.as_str().trim_start_matches('/').len() > usize::from(u16::MAX));
    base.build_http_uri(other).unwrap_err();
}

#[test]
fn gamma_survivor_root_join_prefixes_an_asterisk_path() {
    let base = BaseUri::from_static("https://example.com/");
    let path = "*".parse::<http::uri::PathAndQuery>().unwrap();
    assert_eq!(base.build_http_uri(path).unwrap().to_string(), "https://example.com/*");
}

#[test]
fn gamma_survivor_invalid_base_and_origin_inputs() {
    "not a uri".parse::<Origin>().unwrap_err();
    for input in ["relative/", "/no_end", "/query/?x=1", "bad value"] {
        assert!(input.parse::<BasePath>().is_err(), "{input}");
    }
    assert!("/relative".parse::<Origin>().unwrap_err().to_string().contains("missing scheme"));
    for input in ["not a uri", "https://[", "https://example.com/no_end"] {
        assert!(input.parse::<BaseUri>().is_err(), "{input}");
    }
    "not a uri".parse::<Uri>().unwrap_err();
}

#[test]
fn gamma_survivor_escape_scans_after_first_reserved_byte() {
    for (input, expected) in [
        ("a b/c", "a%20b%2Fc"),
        ("{a}é/", "%7Ba%7D%C3%A9%2F"),
        (" / ", "%20%2F%20"),
        ("abc/", "abc%2F"),
    ] {
        assert_eq!(EscapedString::from(input).as_str(), expected);
        assert_eq!(EscapedString::from(input.to_owned()).as_str(), expected);
    }
}

#[cfg(feature = "serde")]
#[test]
fn gamma_survivor_non_string_serde_inputs_fail() {
    serde_json::from_str::<BasePath>("12").unwrap_err();
    serde_json::from_str::<BaseUri>("12").unwrap_err();
    serde_json::from_str::<Origin>("12").unwrap_err();
    serde_json::from_str::<Uri>("12").unwrap_err();
    serde_json::from_str::<PathAndQuery>("12").unwrap_err();
    serde_json::from_str::<EscapedString>("12").unwrap_err();
}

#[test]
fn gamma_survivor_formatting_propagates_sink_failures() {
    struct LimitedSink(usize);

    impl std::fmt::Write for LimitedSink {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0 = self.0.checked_sub(s.len()).ok_or(std::fmt::Error)?;
            Ok(())
        }
    }

    let redactor = PassThrough;
    for text in [
        "https://example.com",
        "https://example.com:8080",
        "https://[::1]",
        "https://example.com:443",
        "http://example.com:80",
    ] {
        let base = text.parse::<BaseUri>().unwrap();
        let origin = text.parse::<Origin>().unwrap();
        for remaining in 0..base.to_string().len() {
            assert!(std::fmt::write(&mut LimitedSink(remaining), format_args!("{base}")).is_err());
        }
        for remaining in 0..origin.to_string().len() {
            assert!(std::fmt::write(&mut LimitedSink(remaining), format_args!("{origin}")).is_err());
        }
    }
    for uri in [
        Uri::from(PathAndQuery::from_static("/users")),
        Uri::from(PathAndQuery::from_static("/users")).with_base(BaseUri::from_static("https://example.com")),
    ] {
        let length = uri.to_redacted_string(&redactor).len();
        for remaining in 0..length {
            let display = std::fmt::from_fn(|f| RedactedDisplay::fmt(&uri, &redactor, f));
            let debug = std::fmt::from_fn(|f| RedactedDebug::fmt(&uri, &redactor, f));
            assert!(std::fmt::write(&mut LimitedSink(remaining), format_args!("{display}")).is_err());
            assert!(std::fmt::write(&mut LimitedSink(remaining), format_args!("{debug}")).is_err());
        }
        let debug = std::fmt::from_fn(|f| RedactedDebug::fmt(&uri, &redactor, f));
        assert_eq!(debug.to_string(), uri.to_redacted_string(&redactor));
    }
}
