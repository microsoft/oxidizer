// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public-API-only header tests moved from library source modules.

#![cfg(feature = "headers-all")]
//!
//! These tests cover the `http` adapter, so the file is compiled only when the
//! `http` feature is enabled.

#![cfg(feature = "http")]

use std::time::{Duration, UNIX_EPOCH};

use http::header::*;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use http_headers::headers::*;
use http_headers::*;

mod authorization {
    use super::*;

    #[test]
    fn schemes_are_matched_case_insensitively_and_may_be_short() {
        let mut map = HeaderMap::new();
        map.insert("authorization", HeaderValue::from_static("bEaReR  abc.def"));
        let view = Authorization::<Bearer>::view(&map)
            .expect("valid bearer authorization")
            .expect("authorization present");
        assert_eq!(view.token(), b"abc.def");

        for value in ["Basic X", "Basic ", "Bearer ", "Basic", "Bear"] {
            let mut map = HeaderMap::new();
            map.insert(
                "authorization",
                HeaderValue::from_str(value).expect("fixture is a legal field value"),
            );
            let error = Authorization::<Basic>::view(&map).expect_err("truncated authorization values must be rejected");
            assert_eq!(error.kind(), http_headers::DecodeErrorKind::InvalidSyntax, "{value}");
        }
    }

    #[test]
    fn borrowed_view_rejects_malformed_basic_base64() {
        for value in ["Basic YTpi=", "Basic YQ======", "Basic YTp=", "Basic Oh==", "Basic YTp?"] {
            let mut map = HeaderMap::new();
            map.insert(
                "authorization",
                HeaderValue::from_str(value).expect("fixture is a legal field value"),
            );
            let error = Authorization::<Basic>::view(&map).expect_err("malformed Basic base64 must be rejected");
            assert_eq!(error.kind(), http_headers::DecodeErrorKind::InvalidSyntax, "{value}");
        }
    }

    #[test]
    fn borrowed_view_rejects_basic_payload_without_colon() {
        let mut map = HeaderMap::new();
        map.insert("authorization", HeaderValue::from_static("Basic bm8tY29sb24="));
        let error = Authorization::<Basic>::view(&map).expect_err("decoded Basic credentials require a colon");
        assert_eq!(error.kind(), http_headers::DecodeErrorKind::InvalidSyntax);
    }

    #[test]
    fn basic_credentials_debug_is_redacted() {
        let authorization = AuthorizationOwned::<Basic>::basic(b"private-user", b"private-password").expect("valid basic credentials");
        let mut map = HeaderMap::new();
        Authorization::<Basic>::insert(&mut map, authorization).expect("an empty header map has capacity");
        let authorization = Authorization::<Basic>::view(&map)
            .expect("valid basic authorization")
            .expect("authorization present");
        let mut credentials = BasicCredentials::new();
        let credentials = authorization.extract(&mut credentials).expect("valid basic credentials");
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("private-user"));
        assert!(!debug.contains("private-password"));
    }

    #[test]
    fn basic_constructor_matches_rfc_encoding() {
        let authorization = AuthorizationOwned::<Basic>::basic(b"Aladdin", b"open sesame").expect("valid basic credentials");
        assert_eq!(authorization.as_field_value().as_bytes(), b"Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    }

    #[test]
    fn bearer_accepts_padding_and_long_tokens() {
        for token in [
            "YWJj=",
            "YWJjZA==",
            "abcdefghijklmnopqrstuvwxyz0123456789",
            "abcdefghijklmnopqrstuvwxyz012345678=",
            "a",
        ] {
            let mut map = HeaderMap::new();
            map.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}")).expect("fixture is a legal field value"),
            );
            let view = Authorization::<Bearer>::view(&map)
                .expect("valid bearer authorization")
                .expect("authorization present");
            assert_eq!(view.token(), token.as_bytes(), "{token}");
        }
    }

    #[test]
    fn bearer_rejects_malformed_tokens() {
        for token in [
            "",
            "=",
            "==",
            "ab=c",
            "abcdefghijklmnopqrstuvwxyz01234=6",
            "abcdefghijklmnopqrstuvwxyz0123456 ",
            "ab c",
        ] {
            let mut map = HeaderMap::new();
            map.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}")).expect("fixture is a legal field value"),
            );
            let error = Authorization::<Bearer>::view(&map).expect_err("malformed bearer tokens must be rejected");
            assert_eq!(error.kind(), http_headers::DecodeErrorKind::InvalidSyntax);
        }
    }

    #[test]
    fn bearer_is_sensitive_and_borrowed() {
        let authorization = AuthorizationOwned::<Bearer>::bearer("abc.def").expect("valid bearer token");
        assert!(authorization.as_field_value().is_sensitive());
        let mut map = HeaderMap::new();
        Authorization::<Bearer>::insert(&mut map, authorization).expect("an empty header map has capacity");
        let view = Authorization::<Bearer>::view(&map)
            .expect("valid bearer authorization")
            .expect("authorization present");
        assert_eq!(view.token(), b"abc.def");
        assert!(map.get("authorization").is_some_and(HeaderValue::is_sensitive));
    }

    #[test]
    fn basic_extracts_into_reusable_credentials() {
        let authorization = AuthorizationOwned::<Basic>::basic(b"Aladdin", b"open sesame").expect("valid basic credentials");
        let mut map = HeaderMap::new();
        Authorization::<Basic>::insert(&mut map, authorization).expect("an empty header map has capacity");
        let authorization = Authorization::<Basic>::view(&map)
            .expect("valid basic authorization")
            .expect("authorization present");
        let mut credentials = BasicCredentials::new();
        let credentials = authorization.extract(&mut credentials).expect("valid basic credentials");
        assert_eq!(credentials.username(), b"Aladdin");
        assert_eq!(credentials.password(), b"open sesame");
    }
}

mod cache_control {
    use super::*;

    #[test]
    fn parses_multiple_lines_and_preserves_extensions() {
        let mut map = HeaderMap::new();
        map.append("cache-control", HeaderValue::from_static("no-cache, x-mode=\"fast, safe\""));
        map.append("cache-control", HeaderValue::from_static("max-age=30"));
        let view = CacheControl::view(&map)
            .expect("valid cache control")
            .expect("cache control present");
        assert!(view.no_cache());
        assert_eq!(view.max_age(), Some(Duration::from_secs(30)));
        assert!(
            view.directives()
                .any(|directive| { directive.name() == "x-mode" && directive.value() == Some(b"\"fast, safe\"".as_slice()) })
        );
        let owned = CacheControl::owned(&map)
            .expect("valid cache control")
            .expect("cache control present");
        assert!(owned.no_cache());
        assert_eq!(owned.max_age(), Some(Duration::from_secs(30)));
        let mut encoded = HeaderMap::new();
        CacheControl::insert(&mut encoded, owned).expect("an empty header map has capacity");
        assert_eq!(encoded["cache-control"], "no-cache, x-mode=\"fast, safe\", max-age=30");
    }

    #[test]
    fn recipients_ignore_empty_list_elements_but_senders_reject_them() {
        let mut map = HeaderMap::new();
        map.append("cache-control", HeaderValue::from_static(",, no-cache,"));
        map.append("cache-control", HeaderValue::from_static(",,,"));
        let view = CacheControl::view(&map)
            .expect("empty list elements are allowed")
            .expect("cache control present");
        assert!(view.no_cache());

        map.clear();
        map.insert("cache-control", HeaderValue::from_static(",,,"));
        let view = CacheControl::view(&map)
            .expect("empty recipient list is valid")
            .expect("cache control present");
        assert_eq!(view.directives().count(), 0);
        let error = CacheControlOwned::try_from(",,,").expect_err("senders must not generate empty list elements");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    }

    #[test]
    fn rejects_whitespace_around_equals_for_senders_and_recipients() {
        for (value, kind) in [
            ("max-age =30", DecodeErrorKind::InvalidToken),
            ("max-age= 30", DecodeErrorKind::InvalidSyntax),
        ] {
            let mut map = HeaderMap::new();
            map.insert("cache-control", HeaderValue::from_str(value).expect("valid field bytes"));
            let error = CacheControl::view(&map).expect_err("whitespace around equals is not in the grammar");
            assert_eq!(error.kind(), kind);
            let error = CacheControlOwned::try_from(value).expect_err("sender constructor must reject invalid grammar");
            assert_eq!(error.kind(), kind);
        }
    }

    #[test]
    fn accepts_obs_text_and_quoted_delta_seconds() {
        let mut map = HeaderMap::new();
        map.insert(
            "cache-control",
            HeaderValue::from_bytes(b"max-age=\"30\", x-note=\"\xff\"").expect("valid field value"),
        );
        let view = CacheControl::view(&map)
            .expect("valid cache control")
            .expect("cache control present");
        assert_eq!(view.max_age(), Some(Duration::from_secs(30)));
        let extension = view
            .directives()
            .find(|directive| directive.name() == "x-note")
            .expect("extension present");
        assert_eq!(extension.value(), Some(b"\"\xff\"".as_slice()));
        assert_eq!(
            extension.value_str().expect_err("obs-text is not UTF-8").kind(),
            DecodeErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn summary_uses_first_valid_max_age() {
        let mut map = HeaderMap::new();
        map.insert(
            "cache-control",
            HeaderValue::from_static("max-age=invalid, max-age=\"45\", max-age=90"),
        );
        let view = CacheControl::view(&map)
            .expect("valid cache control")
            .expect("cache control present");
        assert_eq!(view.max_age(), Some(Duration::from_secs(45)));
    }

    #[test]
    fn builder_rejects_empty_and_keeps_extensions() {
        let error = CacheControlOwned::builder().build().expect_err("empty cache control must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        let header = CacheControlOwned::builder()
            .private()
            .max_age(Duration::from_mins(5))
            .extension_value("stale-while-revalidate", "30")
            .build()
            .expect("nonempty cache control");
        assert_eq!(header.max_age(), Some(Duration::from_mins(5)));
        assert!(
            header
                .directives()
                .any(|directive| { directive.name() == "stale-while-revalidate" && directive.value() == Some(b"30".as_slice()) })
        );
    }
}

mod conditional {
    use super::*;

    #[test]
    fn dates_preserve_valid_obsolete_wire_forms_and_construct_imf_fixdate() {
        let obsolete = "Sunday, 06-Nov-94 08:49:37 GMT";
        let header = LastModifiedOwned::try_from(obsolete).expect("valid HTTP-date");
        assert_eq!(header.as_field_value(), obsolete);
        let mut encoded = HeaderMap::new();
        LastModified::insert(&mut encoded, header).expect("an empty header map has capacity");
        assert_eq!(
            encoded.get_all(LAST_MODIFIED).iter().map(HeaderValue::as_bytes).collect::<Vec<_>>(),
            vec![obsolete.as_bytes()]
        );

        let date = UNIX_EPOCH + Duration::from_secs(784_111_777);
        let canonical = IfModifiedSinceOwned::new(date).expect("representable date");
        assert_eq!(canonical.as_field_value(), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(canonical.date(), date);

        let last_modified = LastModifiedOwned::new(date).expect("representable date");
        assert_eq!(last_modified.date(), date);
        let if_unmodified = IfUnmodifiedSinceOwned::new(date).expect("representable date");
        assert_eq!(if_unmodified.date(), date);
    }

    #[test]
    fn parses_wildcards_and_tag_lists_across_field_lines() {
        let wildcard = IfMatchOwned::try_from("*").expect("valid wildcard");
        assert!(wildcard.is_wildcard());
        assert_eq!(wildcard.tags().count(), 0);

        let first = ETagOwned::try_from("\"first\"").expect("valid entity tag");
        let second = ETagOwned::try_from("W/\"second\"").expect("valid entity tag");
        let if_match = IfMatchOwned::from_tags([first.clone(), second.clone()]).expect("borrowed array of tags");
        assert_eq!(if_match.tags().count(), 2);
        let if_none_match = IfNoneMatchOwned::from_tags(vec![first, second]).expect("owned vector of tags");
        assert_eq!(if_none_match.tags().count(), 2);

        let mut map = HeaderMap::new();
        map.append("if-match", HeaderValue::from_static(",,,"));
        map.append("if-match", HeaderValue::from_static("\"one\", W/\"two\""));
        map.append("if-match", HeaderValue::from_static("\"comma,slash\\\""));
        let view = IfMatch::view(&map).expect("valid field").expect("field present");
        let tags: Vec<_> = view.tags().map(|tag| (tag.opaque_tag().to_vec(), tag.is_weak())).collect();
        assert_eq!(
            tags,
            vec![
                (b"one".to_vec(), false),
                (b"two".to_vec(), true),
                (b"comma,slash\\".to_vec(), false),
            ]
        );
        let owned = IfMatch::owned(&map).expect("valid field").expect("field present");
        assert_eq!(owned.tags().count(), 3);
        let mut encoded = HeaderMap::new();
        IfMatch::insert(&mut encoded, owned).expect("an empty header map has capacity");
        assert_eq!(encoded.get_all(IF_MATCH).iter().count(), 3);
    }

    #[test]
    fn decodes_owned_tag_lists_from_one_and_many_field_lines() {
        let mut single = HeaderMap::new();
        single.append("if-match", HeaderValue::from_static("\"one,two\", W/\"three\""));
        let owned = IfMatch::owned(&single).expect("valid field").expect("field present");
        assert!(!owned.is_wildcard());
        let tags: Vec<_> = owned.tags().map(|tag| (tag.opaque_tag().to_vec(), tag.is_weak())).collect();
        assert_eq!(tags, vec![(b"one,two".to_vec(), false), (b"three".to_vec(), true)]);

        let mut many = HeaderMap::new();
        many.append("if-none-match", HeaderValue::from_static("\"one\""));
        many.append("if-none-match", HeaderValue::from_static("W/\"two\""));
        let owned = IfNoneMatch::owned(&many).expect("valid field").expect("field present");
        assert_eq!(owned.tags().count(), 2);

        let mut broken = HeaderMap::new();
        broken.append("if-match", HeaderValue::from_static("\"one\""));
        broken.append("if-match", HeaderValue::from_static("nonsense"));
        let error = IfMatch::owned(&broken).expect_err("the second field line is invalid");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        let mut wildcard_conflict = HeaderMap::new();
        wildcard_conflict.append("if-match", HeaderValue::from_static("*"));
        wildcard_conflict.append("if-match", HeaderValue::from_static("\"one\""));
        let error = IfMatch::owned(&wildcard_conflict).expect_err("a wildcard never shares a field with a tag");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        assert!(
            IfMatch::owned(&HeaderMap::new())
                .expect("an absent field is not an error")
                .is_none()
        );
    }

    #[test]
    fn borrowed_and_owned_if_range_views_agree() {
        let mut map = HeaderMap::new();
        map.insert("if-range", HeaderValue::from_static("\"revision\""));
        let view = IfRange::view(&map).expect("valid field").expect("field present");
        let borrowed = view.value();
        let owned = IfRange::owned(&map).expect("valid field").expect("field present");
        assert_eq!(owned.value(), Ok(borrowed));
    }

    #[test]
    fn rejects_mixed_wildcards_malformed_tags_and_missing_lists() {
        for wire in ["*, \"tag\"", "\"unterminated", "w/\"lowercase\"", "\"bad tag\""] {
            let error = IfNoneMatchOwned::try_from(wire).expect_err("invalid tag list");
            assert!(matches!(
                error.kind(),
                DecodeErrorKind::InvalidSyntax | DecodeErrorKind::MissingValue
            ));
        }
        let error = IfMatchOwned::try_from(",,,").expect_err("empty list must fail");
        assert_eq!(error.kind(), DecodeErrorKind::MissingValue);

        let mut map = HeaderMap::new();
        map.append("if-none-match", HeaderValue::from_static("*"));
        map.append("if-none-match", HeaderValue::from_static("*"));
        let error = IfNoneMatch::view(&map).expect_err("duplicate wildcard must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    }

    #[test]
    fn if_range_distinguishes_strong_tags_and_dates() {
        let tag = IfRangeOwned::try_from("\"revision\"").expect("strong tag");
        let value = tag.value().expect("consistent stored range");
        assert!(matches!(value, IfRangeValueView::EntityTag(_)));
        if let IfRangeValueView::EntityTag(tag) = value {
            assert_eq!(tag.opaque_tag(), b"revision");
            assert!(!tag.is_weak());
        }

        let date = IfRangeOwned::try_from("Sun, 06 Nov 1994 08:49:37 GMT").expect("date");
        assert!(matches!(date.value().expect("consistent value"), IfRangeValueView::Date(_)));
        let error = IfRangeOwned::try_from("W/\"weak\"").expect_err("weak If-Range must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    }
}

mod content_type {
    use super::*;

    #[test]
    fn rejects_invalid_parameter_syntax() {
        let error = ContentTypeOwned::try_from("text/plain; charset").expect_err("missing parameter value must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        let error = ContentTypeOwned::try_from("text/plain; q=\"unterminated").expect_err("unterminated quote must fail");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        for (value, kind) in [
            ("text/plain; charset =utf-8", DecodeErrorKind::InvalidSyntax),
            ("text/plain; charset= utf-8", DecodeErrorKind::InvalidToken),
        ] {
            let error = ContentTypeOwned::try_from(value).expect_err("whitespace around equals must fail");
            assert_eq!(error.kind(), kind, "{value}");
        }
    }

    #[test]
    fn exposes_borrowed_components_and_parameters() {
        let content_type = ContentTypeOwned::try_from("text/html; charset=utf-8; level=\"1\"").expect("valid media type");
        assert_eq!(content_type.type_(), Ok("text"));
        assert_eq!(content_type.subtype(), Ok("html"));
        assert_eq!(content_type.parameter("charset"), Ok(Some(b"utf-8".as_slice())));
        assert_eq!(content_type.parameter("level"), Ok(Some(b"\"1\"".as_slice())));
        assert_eq!(content_type.parameter("missing"), Ok(None));
    }

    #[test]
    fn accepts_empty_parameter_slots() {
        for value in ["text/plain;", "text/plain; ; charset=utf-8;"] {
            let content_type = ContentTypeOwned::try_from(value).expect("empty slots are valid");
            let parameters: Result<Vec<_>, _> = content_type.parameters().collect();
            let parameters = parameters.expect("stored media type remains valid");
            if value.contains("charset") {
                assert_eq!(parameters.len(), 1);
                assert_eq!(parameters[0].name(), "charset");
            } else {
                assert!(parameters.is_empty());
            }
        }
    }
}

mod cors {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::*;

    #[test]
    fn header_name_lists_borrow_preserve_case_and_duplicates() {
        let mut map = HeaderMap::new();
        map.append(ACCESS_CONTROL_REQUEST_HEADERS, HeaderValue::from_static("X-Trace, content-type"));
        map.append(ACCESS_CONTROL_REQUEST_HEADERS, HeaderValue::from_static("x-trace"));
        let raw = map.get(ACCESS_CONTROL_REQUEST_HEADERS).expect("inserted value");
        let view = AccessControlRequestHeaders::view(&map)
            .expect("valid header-name list")
            .expect("present header-name list");
        let names: Vec<&str> = view.iter().map(CorsHeaderNameView::as_str).collect();
        assert_eq!(names, ["X-Trace", "content-type", "x-trace"]);
        assert_eq!(view.len(), 3);
        assert!(!view.is_empty());
        assert_eq!(view.field_values().count(), 2);
        assert!(format!("{view:?}").contains("header_name_count"));
        assert_eq!(names[0].as_ptr(), raw.as_bytes().as_ptr());
        assert!(view.iter().next().expect("first name").eq_ignore_ascii_case("x-trace"));
        assert_eq!(
            view.iter().next().expect("first name").to_header_name().expect("validated name"),
            "x-trace"
        );
        assert_eq!(
            view.iter()
                .next()
                .expect("first name")
                .to_http_header_name()
                .expect("validated HTTP name"),
            "x-trace"
        );

        map.clear();
        map.insert(ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("X-Trace"));
        let exposed = AccessControlExposeHeaders::view(&map)
            .expect("valid exposed-header list")
            .expect("present exposed-header list");
        assert_eq!(exposed.len(), 1);
        assert!(!exposed.is_empty());
        assert_eq!(exposed.field_values().count(), 1);
        assert!(!exposed.contains_wildcard());
        assert!(format!("{exposed:?}").contains("header_name_count"));
        assert_eq!(exposed.iter().map(CorsHeaderNameView::as_str).collect::<Vec<_>>(), ["X-Trace"]);
        map.insert(ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("*"));
        let wildcard = AccessControlExposeHeaders::view(&map)
            .expect("valid wildcard exposed-header list")
            .expect("present wildcard exposed-header list");
        assert!(wildcard.is_wildcard());

        map.clear();
        map.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("*"));
        let allowed = AccessControlAllowHeaders::view(&map)
            .expect("valid allowed-header list")
            .expect("present allowed-header list");
        assert_eq!(allowed.len(), 1);
        assert!(!allowed.is_empty());
        assert_eq!(allowed.field_values().count(), 1);
        assert!(allowed.contains_wildcard());
        assert!(allowed.is_wildcard());
        assert!(format!("{allowed:?}").contains("header_name_count"));
    }

    #[test]
    fn list_constructors_validate_tokens_and_keep_wildcard_syntactic() {
        let methods = AccessControlAllowMethodsOwned::from_methods([Method::GET.as_str(), "X-CUSTOM", "GET"]).expect("valid methods");
        assert_eq!(
            methods.iter().map(CorsMethodView::as_str).collect::<Vec<_>>(),
            ["GET", "X-CUSTOM", "GET"]
        );
        AccessControlAllowMethodsOwned::from_methods(["GET", "not a method"]).expect_err("methods must be tokens");

        let headers = AccessControlExposeHeadersOwned::from_header_names([http::header::CONTENT_TYPE.as_str(), "X-Extension"])
            .expect("valid field names");
        assert_eq!(
            headers.iter().map(CorsHeaderNameView::as_str).collect::<Vec<_>>(),
            ["content-type", "X-Extension"]
        );
        assert_eq!(headers.len(), 2);
        assert!(!headers.is_empty());
        assert_eq!(headers.field_values().count(), 1);
        assert_eq!(headers.clone().into_field_values().len(), 1);
        assert!(format!("{headers:?}").contains("header_name_count"));
        let expose_adopted =
            AccessControlExposeHeadersOwned::try_from(FieldValue::from_static("x-trace")).expect("single exposed field line");
        assert_eq!(expose_adopted.len(), 1);
        let expose_lines =
            AccessControlExposeHeadersOwned::try_from(vec![FieldValue::from_static("x-trace"), FieldValue::from_static("content-type")])
                .expect("repeated exposed field lines");
        assert_eq!(expose_lines.len(), 2);
        let expose_wildcard = AccessControlExposeHeadersOwned::wildcard();
        assert!(expose_wildcard.contains_wildcard());
        assert!(expose_wildcard.is_wildcard());
        assert!(AccessControlExposeHeadersOwned::empty().is_empty());
        AccessControlExposeHeadersOwned::from_header_names(["bad name"]).expect_err("field names must be tokens");

        let wildcard = AccessControlAllowHeadersOwned::wildcard();
        assert!(wildcard.is_wildcard());
        let mixed = AccessControlAllowHeadersOwned::from_header_names(["*", "authorization"]).expect("wildcard is also a field-name token");
        assert!(mixed.contains_wildcard());
        assert!(!mixed.is_wildcard());
    }

    #[test]
    fn list_constructors_execute_external_generic_shapes() {
        let single = AccessControlAllowHeadersOwned::from_header_names(["content-type"]).expect("single borrowed field name");
        assert_eq!(single, single.clone());
        assert_eq!(single.len(), 1);
        assert!(!single.is_empty());
        assert_eq!(single.field_values().count(), 1);
        assert_eq!(single.clone().into_field_values().len(), 1);
        assert!(format!("{single:?}").contains("header_name_count"));
        assert!(AccessControlAllowHeadersOwned::empty().is_empty());
        let allow_lines =
            AccessControlAllowHeadersOwned::try_from(vec![FieldValue::from_static("content-type"), FieldValue::from_static("x-trace")])
                .expect("repeated allowed field lines");
        assert_eq!(allow_lines.len(), 2);
        let adopted = AccessControlAllowHeadersOwned::try_from(FieldValue::from_static("content-type")).expect("single adopted field line");
        assert_eq!(adopted, single);
        assert_eq!(single.iter().next().expect("field name").as_str(), "content-type");

        let three = AccessControlAllowHeadersOwned::from_header_names(["content-type", "authorization", "x-trace-id"])
            .expect("three borrowed field names");
        assert_eq!(three.len(), 3);
        let mut first_hash = DefaultHasher::new();
        single.hash(&mut first_hash);
        let mut second_hash = DefaultHasher::new();
        single.clone().hash(&mut second_hash);
        assert_eq!(first_hash.finish(), second_hash.finish());

        let owned = AccessControlRequestHeadersOwned::from_header_names(vec![String::from("content-type"), String::from("x-trace-id")])
            .expect("owned field names");
        assert_eq!(owned.len(), 2);
        assert!(!owned.is_empty());
        assert_eq!(owned.field_values().count(), 1);
        assert_eq!(owned.clone().into_field_values().len(), 1);
        assert!(format!("{owned:?}").contains("header_name_count"));
        let request_adopted =
            AccessControlRequestHeadersOwned::try_from(FieldValue::from_static("content-type")).expect("single request field line");
        assert_eq!(request_adopted.len(), 1);
        AccessControlRequestHeadersOwned::from_header_names(Vec::<String>::new()).expect_err("request header list must not be empty");
        AccessControlRequestHeadersOwned::from_header_names(vec![String::from("bad name")]).expect_err("owned invalid field name");
        AccessControlRequestHeadersOwned::try_from(vec![FieldValue::from_static("")])
            .expect_err("adopted request-header lists must contain an item");

        let method = AccessControlAllowMethodsOwned::from_methods(["GET"]).expect("single borrowed method");
        assert_eq!(method.iter().next().expect("method").as_str(), "GET");
        let standard_methods =
            AccessControlAllowMethodsOwned::from_methods(["GET", "PUT", "HEAD", "POST", "PATCH", "TRACE", "DELETE", "CONNECT", "OPTIONS"])
                .expect("standard methods");
        assert_eq!(standard_methods.len(), 9);
        let common_names = AccessControlExposeHeadersOwned::from_header_names([
            "accept",
            "authorization",
            "cache-control",
            "content-length",
            "content-type",
            "etag",
            "origin",
            "x-requested-with",
        ])
        .expect("common field names");
        assert_eq!(common_names.len(), 8);
        let owned_methods =
            AccessControlAllowMethodsOwned::from_methods(vec![String::from("GET"), String::from("X-CUSTOM")]).expect("owned methods");
        assert_eq!(owned_methods.len(), 2);
        AccessControlAllowMethodsOwned::from_methods(vec![String::from("bad method")]).expect_err("owned invalid method");
    }

    #[test]
    fn malformed_list_items_report_the_field_line() {
        let mut map = HeaderMap::new();
        map.append(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET"));
        map.append(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("bad method"));
        let error = AccessControlAllowMethods::view(&map).expect_err("invalid method token must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
        assert_eq!(error.value_index(), Some(1));

        map.clear();
        map.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("\"x-header\""));
        let error = AccessControlAllowHeaders::view(&map).expect_err("quoted field name must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
    }

    #[test]
    fn singleton_cors_headers_reject_multiple_field_lines() {
        let mut map = HeaderMap::new();
        for (name, value) in [
            (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (ACCESS_CONTROL_ALLOW_CREDENTIALS, "true"),
            (ACCESS_CONTROL_MAX_AGE, "60"),
            (ACCESS_CONTROL_REQUEST_METHOD, "PATCH"),
        ] {
            map.clear();
            map.append(name.clone(), HeaderValue::from_str(value).expect("static test value"));
            map.append(name.clone(), HeaderValue::from_str(value).expect("static test value"));
            let error = if name == ACCESS_CONTROL_ALLOW_ORIGIN {
                AccessControlAllowOrigin::view(&map).expect_err("duplicate singleton must fail")
            } else if name == ACCESS_CONTROL_ALLOW_CREDENTIALS {
                AccessControlAllowCredentials::view(&map).expect_err("duplicate singleton must fail")
            } else if name == ACCESS_CONTROL_MAX_AGE {
                AccessControlMaxAge::view(&map).expect_err("duplicate singleton must fail")
            } else {
                AccessControlRequestMethod::view(&map).expect_err("duplicate singleton must fail")
            };
            assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
        }
    }

    #[test]
    fn request_method_borrows_and_preserves_extensions() {
        let mut map = HeaderMap::new();
        map.insert(ACCESS_CONTROL_REQUEST_METHOD, HeaderValue::from_static("X-REINDEX"));
        let raw = map.get(ACCESS_CONTROL_REQUEST_METHOD).expect("inserted value");
        let view = AccessControlRequestMethod::view(&map)
            .expect("valid request method")
            .expect("present request method");
        assert_eq!(view.method().as_str(), "X-REINDEX");
        assert_eq!(view.method().as_bytes().as_ptr(), raw.as_bytes().as_ptr());
        assert_eq!(view.method().to_method().expect("validated method").as_str(), "X-REINDEX");

        for invalid in ["", "GET, POST", "bad method", "\"PATCH\""] {
            AccessControlRequestMethodOwned::try_from(invalid).expect_err("request method must be one token");
        }
    }

    #[test]
    fn method_lists_preserve_lines_extensions_empty_members_and_duplicates() {
        let mut map = HeaderMap::new();
        map.append(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, X-PURGE,,"));
        map.append(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("PATCH, GET"));
        let view = AccessControlAllowMethods::view(&map)
            .expect("valid method list")
            .expect("present method list");
        let methods: Vec<&str> = view.iter().map(CorsMethodView::as_str).collect();
        assert_eq!(methods, ["GET", "X-PURGE", "PATCH", "GET"]);
        assert_eq!(view.field_values().count(), 2);

        let owned = AccessControlAllowMethods::owned(&map)
            .expect("valid method list")
            .expect("present method list");
        let mut encoded = HeaderMap::new();
        AccessControlAllowMethods::insert(&mut encoded, owned).expect("an empty header map has capacity");
        assert_eq!(
            encoded
                .get_all(ACCESS_CONTROL_ALLOW_METHODS)
                .iter()
                .map(HeaderValue::as_bytes)
                .collect::<Vec<_>>(),
            [b"GET, X-PURGE,,".as_slice(), b"PATCH, GET".as_slice(),]
        );
    }

    #[test]
    fn max_age_handles_boundaries_overflow_and_duplicate_lines() {
        let maximum = AccessControlMaxAgeOwned::try_from(u64::MAX.to_string()).expect("u64 maximum is valid");
        assert_eq!(maximum.seconds(), u64::MAX);
        assert_eq!(maximum.duration(), Duration::from_secs(u64::MAX));

        for invalid in ["", "-1", "+1", "1.0", "18446744073709551616"] {
            let error = AccessControlMaxAgeOwned::try_from(invalid).expect_err("invalid delta-seconds must fail");
            assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
        }

        let padded = AccessControlMaxAgeOwned::try_from(" 00060 ").expect("OWS and leading zeroes are valid");
        assert_eq!(padded.seconds(), 60);
        assert_eq!(padded, AccessControlMaxAgeOwned::new(60));
        assert_eq!(padded.into_field_value(), "60");
    }

    #[test]
    fn allow_origin_enforces_serialized_origin_grammar() {
        for invalid in [
            "",
            "NULL",
            "HTTPS://example.com",
            "https://Example.com",
            "https://example.com/path",
            "https://user@example.com",
            "https://example.com:123456",
            "https://example.com:65536",
            "https://example.com:00443",
            "https://example.com:443",
            "https://01.2.3.4",
            "https://256.2.3.4",
            "https://[2001:0db8::1]",
            "https://[2001:db8:0:0:0:0::1]",
            "https://[2001:db8:0:0:0:0:0:1]",
            "https://[2001:db8::192.0.2.1]",
        ] {
            assert!(
                AccessControlAllowOriginOwned::try_from(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
        for valid in [
            "http://127.0.0.1",
            "https://example.com",
            "wss://a-b.0:8080",
            "https://[2001:db8::1]",
            "https://[2001::1:0:0:1:1]",
        ] {
            assert!(
                AccessControlAllowOriginOwned::from_origin(valid).is_ok(),
                "{valid:?} must be accepted"
            );
        }
    }

    #[test]
    fn response_lists_allow_empty_but_request_headers_require_a_member() {
        let mut map = HeaderMap::new();
        map.insert(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static(""));
        assert!(
            AccessControlAllowMethods::view(&map)
                .expect("empty #method is valid")
                .expect("present header")
                .is_empty()
        );

        map.clear();
        map.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static(",,,"));
        assert!(
            AccessControlAllowHeaders::view(&map)
                .expect("empty #field-name is valid")
                .expect("present header")
                .is_empty()
        );

        map.clear();
        map.insert(ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static(""));
        assert!(
            AccessControlExposeHeaders::view(&map)
                .expect("empty #field-name is valid")
                .expect("present header")
                .is_empty()
        );

        map.clear();
        map.insert(ACCESS_CONTROL_REQUEST_HEADERS, HeaderValue::from_static(",,,"));
        let error = AccessControlRequestHeaders::view(&map).expect_err("1#field-name requires a member");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    }

    #[test]
    fn owned_list_round_trip_retains_all_field_lines() {
        let original = vec![FieldValue::from_static("X-A, x-a"), FieldValue::from_static("X-B,, X-C")];
        let header = AccessControlExposeHeadersOwned::from_field_values(original.clone()).expect("valid field lines");
        assert_eq!(header.clone().into_field_values(), original);

        let mut map = HeaderMap::new();
        AccessControlExposeHeaders::insert(&mut map, header).expect("empty map has insertion capacity");
        let round_trip: Vec<FieldValue> = map.get_all(ACCESS_CONTROL_EXPOSE_HEADERS).iter().map(FieldValue::from).collect();
        assert_eq!(round_trip, original);
    }

    #[test]
    fn credentials_are_case_sensitive_and_policy_independent() {
        assert_eq!(
            AccessControlAllowCredentialsOwned::try_from(" \ttrue\t ")
                .expect("OWS-framed true is valid")
                .into_field_value(),
            "true"
        );
        for invalid in ["", "True", "TRUE", "false", "true, true"] {
            AccessControlAllowCredentialsOwned::try_from(invalid).expect_err("credentials value must be exactly true");
        }

        let origin = AccessControlAllowOriginOwned::wildcard();
        let credentials = AccessControlAllowCredentialsOwned::allow();
        assert!(origin.is_wildcard());
        assert_eq!(credentials.into_field_value(), "true");
    }

    #[test]
    fn allow_origin_borrows_and_classifies_values() {
        let mut map = HeaderMap::new();
        map.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("https://api.example:8443"));
        let raw = map.get(ACCESS_CONTROL_ALLOW_ORIGIN).expect("inserted value");
        let view = AccessControlAllowOrigin::view(&map).expect("valid origin").expect("present origin");
        let origin = view.origin().expect("serialized origin");
        assert_eq!(origin, "https://api.example:8443");
        assert_eq!(origin.as_ptr(), raw.as_bytes().as_ptr());
        assert!(!view.is_wildcard());
        assert!(!view.is_null());

        assert!(AccessControlAllowOriginOwned::wildcard().is_wildcard());
        assert!(AccessControlAllowOriginOwned::null().is_null());
        AccessControlAllowOriginOwned::from_origin("custom+v1://example").expect_err("unsupported schemes have opaque origins");
    }
}

mod negotiation {
    use super::*;

    #[test]
    fn encoding_and_language_accept_unknown_values() {
        let encodings = AcceptEncodingOwned::try_from("gzip, br;q=0.8, x-custom").expect("valid content codings");
        assert_eq!(encodings.items().count(), 3);

        let languages = AcceptLanguageOwned::try_from("en-US, fr;q=0.7, x-private").expect("valid language ranges");
        assert_eq!(languages.items().count(), 3);
        let error = AcceptLanguageOwned::try_from("toolongprimarytag").expect_err("primary subtag is limited to eight letters");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
        let error = AcceptEncodingOwned::try_from("gzip;level=1").expect_err("only a quality parameter is allowed");
        assert_eq!(error.header().as_str(), "accept-encoding");
    }

    #[test]
    fn server_is_opaque_nonempty_and_singleton() {
        let server = ServerOwned::try_from("example/1.0 (edge)").expect("valid opaque server value");
        assert_eq!(server.as_bytes(), b"example/1.0 (edge)");
        let error = ServerOwned::try_from(" \t").expect_err("blank Server value must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);

        let mut map = HeaderMap::new();
        map.append("server", HeaderValue::from_static("one"));
        map.append("server", HeaderValue::from_static("two"));
        let error = Server::view(&map).expect_err("Server is a singleton");
        assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
    }

    #[test]
    fn host_rejects_userinfo_bad_ports_and_duplicates() {
        let error = HostOwned::try_from("user@example.com").expect_err("userinfo is forbidden");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
        let error = HostOwned::try_from("example.com:http").expect_err("port must be decimal");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);

        let mut map = HeaderMap::new();
        map.append("host", HeaderValue::from_static("example.com"));
        map.append("host", HeaderValue::from_static("example.org"));
        let error = Host::view(&map).expect_err("Host is a singleton");
        assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
    }

    #[test]
    fn host_supports_names_ports_and_ip_literals() {
        let host = HostOwned::with_port("example.com", 8443).expect("valid host and port");
        assert_eq!(host.host(), Ok("example.com"));
        assert_eq!(host.port(), Ok(Some("8443")));

        let ipv6 = HostOwned::try_from("[2001:db8::1]:443").expect("valid IPv6 host");
        assert_eq!(ipv6.host(), Ok("[2001:db8::1]"));
        assert_eq!(ipv6.port(), Ok(Some("443")));

        let future = HostOwned::try_from("[v1.fe80]:80").expect("valid IPvFuture host");
        assert_eq!(future.host(), Ok("[v1.fe80]"));

        // The owned form rebuilds its offsets from the borrowed slices.
        let mut map = HeaderMap::new();
        map.insert(http::header::HOST, HeaderValue::from_static("example.com:8443"));
        let owned = Host::owned(&map).expect("valid authority").expect("host present");
        assert_eq!(owned.host(), Ok("example.com"));
        assert_eq!(owned.port(), Ok(Some("8443")));

        map.insert(http::header::HOST, HeaderValue::from_static("[2001:db8::1]"));
        let bare = Host::owned(&map).expect("valid authority").expect("host present");
        assert_eq!(bare.host(), Ok("[2001:db8::1]"));
        assert_eq!(bare.port(), Ok(None));

        // An authority may end at its colon, which is the case that separates
        // a derived port offset from a stored one.
        for (wire, port) in [("example.com:", Some("")), ("example.com", None), ("[2001:db8::1]:", Some(""))] {
            map.insert(http::header::HOST, HeaderValue::from_static(wire));
            let owned = Host::owned(&map).expect("valid authority").expect("host present");
            assert_eq!(owned.port(), Ok(port), "{wire}");
            assert_eq!(
                owned.port(),
                Ok(Host::view(&map).expect("valid authority").expect("host present").port()),
                "{wire}"
            );
        }
    }

    #[test]
    fn accept_preserves_quoted_commas_and_extensions() {
        let mut map = HeaderMap::new();
        map.append(
            "accept",
            HeaderValue::from_static("text/html;level=1, application/json;q=0.9;profile=\"a,b\""),
        );
        map.append("accept", HeaderValue::from_static("*/*;q=0.1"));
        let view = Accept::view(&map).expect("valid Accept").expect("Accept present");
        let items: Vec<_> = view.items().collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[1], b"application/json;q=0.9;profile=\"a,b\"");

        let owned = Accept::owned(&map).expect("valid Accept").expect("Accept present");
        let mut encoded = HeaderMap::new();
        Accept::insert(&mut encoded, owned).expect("an empty map has capacity");
        assert_eq!(encoded.get_all("accept").iter().count(), 2);
    }

    #[test]
    fn allow_and_vary_preserve_raw_tokens() {
        let allow = AllowOwned::try_from("GET, PATCH, X-CUSTOM").expect("valid method list");
        assert_eq!(
            allow.items().collect::<Vec<_>>(),
            vec![b"GET".as_slice(), b"PATCH".as_slice(), b"X-CUSTOM".as_slice()]
        );

        let empty = AllowOwned::try_from("").expect("an empty Allow value is valid");
        assert_eq!(empty.items().count(), 0);

        let vary = VaryOwned::try_from("accept-encoding, x-tenant, *").expect("valid field-name list");
        assert_eq!(vary.items().count(), 3);

        let error = VaryOwned::try_from("\"unterminated").expect_err("quoted strings are not field names");
        assert_eq!(error.header().as_str(), "vary");
    }

    #[test]
    fn accept_rejects_invalid_wildcards_and_quality() {
        for value in ["*/json", "text/html;q=1.1", "text/html;q =0.5", "text/html;q= 0.5", "text"] {
            let error = AcceptOwned::try_from(value).expect_err("invalid Accept grammar");
            assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax, "{value}");
        }
    }

    #[test]
    fn relaxed_mode_accepts_only_documented_quality_deviations() {
        fn assert_relaxed<H: Field>(name: &'static HeaderName, value: &'static str) {
            let mut map = HeaderMap::new();
            map.insert(name, HeaderValue::from_static(value));
            assert!(H::view(&map).err().is_some(), "{name}: {value}");
            assert!(
                H::owned_with(&map, DecodeMode::Relaxed).expect("relaxed quality syntax").is_some(),
                "{name}: {value}"
            );
        }

        assert_relaxed::<Accept>(&ACCEPT, "text/html; q = .12345");
        assert_relaxed::<AcceptEncoding>(&ACCEPT_ENCODING, "gzip; q=.5");
        assert_relaxed::<AcceptLanguage>(&ACCEPT_LANGUAGE, "en-US; Q = 1.0000");
    }

    #[test]
    fn relaxed_mode_accepts_documented_interoperability_deviations() {
        fn assert_relaxed<H: Field>(name: &'static HeaderName, value: HeaderValue) {
            let mut map = HeaderMap::new();
            map.insert(name, value);
            assert!(H::view(&map).err().is_some(), "{name}");
            assert!(
                H::view_with(&map, DecodeMode::Relaxed)
                    .expect("relaxed borrowed decoding succeeds")
                    .is_some()
            );
            assert!(
                H::owned_with(&map, DecodeMode::Relaxed)
                    .expect("relaxed owned decoding succeeds")
                    .is_some()
            );
        }

        assert_relaxed::<ETag>(&ETAG, HeaderValue::from_static("w/\"revision\""));
        assert_relaxed::<IfMatch>(&IF_MATCH, HeaderValue::from_static("w/\"revision\""));
        assert_relaxed::<ContentType>(&CONTENT_TYPE, HeaderValue::from_static("text / html"));
        assert_relaxed::<Range>(&RANGE, HeaderValue::from_static("bytes = 0 - 499"));
        assert_relaxed::<ContentRange>(&CONTENT_RANGE, HeaderValue::from_static("bytes 0 - 499 / 1234"));
        assert_relaxed::<LastModified>(&LAST_MODIFIED, HeaderValue::from_static("Sun, 6 Nov 1994 8:49:37 UTC"));
        assert_relaxed::<Host>(&HOST, HeaderValue::from_bytes(b"caf\xc3\xa9.example").expect("valid field bytes"));
        assert_relaxed::<Location>(&LOCATION, HeaderValue::from_static("/a\\b\\c"));
    }

    #[test]
    fn relaxed_mode_preserves_semantics_and_original_wire_values() {
        let mut map = HeaderMap::new();
        map.insert(ETAG, HeaderValue::from_static("w/\"revision\""));
        let etag = ETag::owned_with(&map, DecodeMode::Relaxed)
            .expect("relaxed ETag decoding succeeds")
            .expect("ETag is present");
        assert!(etag.is_weak());
        assert_eq!(etag.opaque_tag().expect("opaque tag metadata is valid"), b"revision");

        map.clear();
        map.insert(IF_MATCH, HeaderValue::from_static("w/\"revision\""));
        let if_match = IfMatch::view_with(&map, DecodeMode::Relaxed)
            .expect("relaxed If-Match decoding succeeds")
            .expect("If-Match is present");
        let tag = if_match.tags().next().expect("one conditional tag");
        assert!(tag.is_weak());
        assert_eq!(tag.opaque_tag(), b"revision");

        map.clear();
        map.insert(RANGE, HeaderValue::from_static("bytes = 0 - 499"));
        let range = Range::view_with(&map, DecodeMode::Relaxed)
            .expect("relaxed Range decoding succeeds")
            .expect("Range is present");
        assert_eq!(
            range.byte_ranges().expect("byte ranges are available").collect::<Vec<_>>(),
            vec![ByteRangeSpec::FromTo { first: 0, last: 499 }]
        );

        map.clear();
        map.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0 - 499 / 1234"));
        let content_range = ContentRange::view_with(&map, DecodeMode::Relaxed)
            .expect("relaxed Content-Range decoding succeeds")
            .expect("Content-Range is present");
        assert_eq!(
            content_range.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 499,
                complete_length: Some(1234),
            })
        );

        map.clear();
        map.insert(LAST_MODIFIED, HeaderValue::from_static("Sun, 6 Nov 1994 8:49:37 UTC"));
        let modified = LastModified::view_with(&map, DecodeMode::Relaxed)
            .expect("relaxed Last-Modified decoding succeeds")
            .expect("Last-Modified is present");
        assert_eq!(modified.date(), UNIX_EPOCH + Duration::from_secs(784_111_777));

        map.clear();
        map.insert(
            HOST,
            HeaderValue::from_bytes(b"caf\xc3\xa9.example:443").expect("valid field bytes"),
        );
        let host = Host::view_with(&map, DecodeMode::Relaxed)
            .expect("relaxed Host decoding succeeds")
            .expect("Host is present");
        assert_eq!(host.host().as_bytes(), b"caf\xc3\xa9.example");
        assert_eq!(host.port(), Some("443"));

        map.clear();
        map.insert(LOCATION, HeaderValue::from_static("/a\\b\\c"));
        let location = Location::view_with(&map, DecodeMode::Relaxed)
            .expect("relaxed Location decoding succeeds")
            .expect("Location is present");
        assert_eq!(location.as_str().expect("Location remains valid UTF-8"), "/a\\b\\c");
    }

    #[test]
    fn relaxed_mode_preserves_structural_validation() {
        for value in ["*/json", "text/html;q=1.01", "text/html;q=-0.5"] {
            let mut map = HeaderMap::new();
            map.insert(ACCEPT, HeaderValue::from_static(value));
            Accept::view_with(&map, DecodeMode::Relaxed).expect_err("relaxed mode must preserve structural validation");
        }

        let mut map = HeaderMap::new();
        map.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en_US;q=.5"));
        AcceptLanguage::view_with(&map, DecodeMode::Relaxed).expect_err("underscores are not language-range separators");

        map.insert(HOST, HeaderValue::from_static("user@example.com"));
        Host::view_with(&map, DecodeMode::Relaxed).expect_err("relaxed host decoding must still reject user-info");

        map.insert(ETAG, HeaderValue::from_static("\"space inside\""));
        ETag::view_with(&map, DecodeMode::Relaxed).expect_err("relaxed ETag decoding must retain the etagc grammar");

        map.insert(RANGE, HeaderValue::from_static("bytes = +1 - 2"));
        Range::view_with(&map, DecodeMode::Relaxed).expect_err("relaxed ranges must retain digit-only positions");

        map.insert(CONTENT_RANGE, HeaderValue::from_static("bytes  0 - 1 / 2"));
        ContentRange::view_with(&map, DecodeMode::Relaxed).expect_err("relaxed Content-Range keeps its unit separator strict");

        map.insert(
            LAST_MODIFIED,
            HeaderValue::from_bytes(b"Sun,\t6 Nov 1994 8:49:37 UTC").expect("horizontal tabs are legal field bytes"),
        );
        LastModified::view_with(&map, DecodeMode::Relaxed).expect_err("relaxed dates keep internal separators strict");
    }

    #[test]
    fn source_exposes_relaxed_decoding() {
        let mut map = HeaderMap::new();
        map.insert(ACCEPT_ENCODING, HeaderValue::from_static("br; q = .75"));

        AcceptEncoding::view(&map).expect_err("ordinary facade decoding remains strict");
        assert!(
            AcceptEncoding::view_with(&map, DecodeMode::Relaxed)
                .expect("relaxed quality syntax")
                .is_some()
        );
    }
}

mod range {
    use super::*;

    #[test]
    fn parses_rfc_byte_range_examples() {
        let range = RangeOwned::try_from("bytes=0-499, 500-999, -500, 9500-").expect("valid byte ranges");
        let specs: Vec<_> = range.byte_ranges().expect("byte unit").collect();
        assert_eq!(
            specs,
            vec![
                ByteRangeSpec::FromTo { first: 0, last: 499 },
                ByteRangeSpec::FromTo { first: 500, last: 999 },
                ByteRangeSpec::Suffix { length: 500 },
                ByteRangeSpec::From { first: 9500 },
            ]
        );
        assert_eq!(range.as_field_value(), "bytes=0-499, 500-999, -500, 9500-");
    }

    #[test]
    fn accept_ranges_supports_lists_extensions_and_none() {
        let mut map = HeaderMap::new();
        map.append("accept-ranges", HeaderValue::from_static("bytes"));
        map.append("accept-ranges", HeaderValue::from_static("example"));
        let view = AcceptRanges::view(&map).expect("valid units").expect("field present");
        assert_eq!(view.units().collect::<Vec<_>>(), vec!["bytes", "example"]);
        assert!(!view.is_none());
        let owned = AcceptRanges::owned(&map).expect("valid units").expect("field present");
        assert_eq!(owned.units().collect::<Vec<_>>(), vec!["bytes", "example"]);
        let mut encoded = HeaderMap::new();
        AcceptRanges::insert(&mut encoded, owned).expect("an empty header map has capacity");
        assert_eq!(encoded[ACCEPT_RANGES], "bytes, example");

        assert!(AcceptRangesOwned::none().is_none());
        let error = AcceptRangesOwned::try_from("none, bytes").expect_err("none is exclusive");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    }

    #[test]
    fn borrowed_and_owned_range_views_agree() {
        let mut map = HeaderMap::new();
        map.insert("range", HeaderValue::from_static("bytes=0-99, -10"));
        let view = Range::view(&map).expect("valid range").expect("field present");
        let borrowed: Vec<_> = view.byte_ranges().expect("byte unit").collect();
        let owned = Range::owned(&map).expect("valid range").expect("field present");
        let owned_specs: Vec<_> = owned.byte_ranges().expect("byte unit").collect();
        assert_eq!(borrowed, owned_specs);
        assert_eq!(owned.as_field_value(), "bytes=0-99, -10");
    }

    #[test]
    fn range_constructor_round_trips_and_preserves_extensions() {
        let range = RangeOwned::bytes([
            ByteRangeSpec::from_range(0..=99).expect("ordered"),
            ByteRangeSpec::starting_at(200),
            ByteRangeSpec::suffix(50),
        ])
        .expect("valid set");
        assert_eq!(range.as_field_value(), "bytes=0-99, 200-, -50");

        let extension = RangeOwned::extension("example-unit", "opaque=payload").expect("valid extension range");
        assert_eq!(extension.unit(), Ok("example-unit"));
        assert!(extension.byte_ranges().is_none());
        assert_eq!(extension.extension_range_set(), Some(b"opaque=payload".as_slice()));
        assert_eq!(extension.as_field_value(), "example-unit=opaque=payload");
    }

    #[test]
    fn rejects_malformed_inverted_overflowing_and_duplicate_ranges() {
        for (wire, kind) in [
            ("bytes=", DecodeErrorKind::MissingValue),
            ("bytes=10-9", DecodeErrorKind::InvalidSyntax),
            ("bytes=1-2-3", DecodeErrorKind::InvalidSyntax),
            ("bytes=-", DecodeErrorKind::InvalidNumber),
            ("bytes=18446744073709551616-", DecodeErrorKind::InvalidNumber),
            ("bytes =0-1", DecodeErrorKind::InvalidToken),
        ] {
            let error = RangeOwned::try_from(wire).expect_err("invalid byte range");
            assert_eq!(error.kind(), kind, "{wire}");
        }
        let error = RangeOwned::try_from("bytes=18446744073709551616-").expect_err("overflow must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);

        let mut map = HeaderMap::new();
        map.append("range", HeaderValue::from_static("bytes=0-1"));
        map.append("range", HeaderValue::from_static("bytes=2-3"));
        let error = Range::view(&map).expect_err("Range is a singleton field");
        assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
    }

    #[test]
    fn parses_satisfied_unknown_and_unsatisfied_content_ranges() {
        let satisfied = ContentRangeOwned::try_from("bytes 0-499/1234").expect("satisfied");
        assert_eq!(
            satisfied.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 499,
                complete_length: Some(1234),
            })
        );

        let unknown = ContentRangeOwned::try_from("bytes 0-499/*").expect("unknown length");
        assert_eq!(
            unknown.byte_range(),
            Some(ByteContentRange::Satisfied {
                first: 0,
                last: 499,
                complete_length: None,
            })
        );

        let unsatisfied = ContentRangeOwned::try_from("bytes */1234").expect("unsatisfied");
        assert_eq!(
            unsatisfied.byte_range(),
            Some(ByteContentRange::Unsatisfied { complete_length: 1234 })
        );
        assert_eq!(
            ContentRangeOwned::unsatisfied_bytes(1234)
                .expect("valid unsatisfied range")
                .as_field_value(),
            "bytes */1234"
        );

        let extension = ContentRangeOwned::extension("example", "opaque response").expect("valid extension");
        assert_eq!(extension.unit(), Ok("example"));
        assert_eq!(extension.byte_range(), None);
        assert_eq!(extension.extension_payload(), Some(b"opaque response".as_slice()));
        assert_eq!(extension.as_field_value(), "example opaque response");
    }

    #[test]
    fn rejects_invalid_and_overflowing_content_ranges() {
        for (wire, kind) in [
            ("bytes 500-499/1234", DecodeErrorKind::InvalidSyntax),
            ("bytes 0-499/499", DecodeErrorKind::InvalidSyntax),
            ("bytes */*", DecodeErrorKind::InvalidNumber),
            ("bytes 0-1/18446744073709551616", DecodeErrorKind::InvalidNumber),
            ("bytes 0-1-2/3", DecodeErrorKind::InvalidSyntax),
        ] {
            let error = ContentRangeOwned::try_from(wire).expect_err("invalid content range");
            assert_eq!(error.kind(), kind, "{wire}");
        }
        let error = ContentRangeOwned::try_from("bytes */18446744073709551616").expect_err("overflow must fail");
        assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
    }
}

mod security {
    use super::*;

    #[test]
    fn referrer_policy_parses_fallback_lists_and_selects_last() {
        let mut map = HeaderMap::new();
        map.append("referrer-policy", HeaderValue::from_static("no-referrer-when-downgrade, origin"));
        map.append("referrer-policy", HeaderValue::from_static("strict-origin-when-cross-origin"));
        let view = ReferrerPolicy::view(&map).expect("valid policy list").expect("policy present");
        let policies: Result<Vec<_>, _> = view.policies().collect();
        assert_eq!(
            policies,
            Ok(vec![
                ReferrerPolicyValue::NoReferrerWhenDowngrade,
                ReferrerPolicyValue::Origin,
                ReferrerPolicyValue::StrictOriginWhenCrossOrigin,
            ])
        );
        assert_eq!(view.preferred(), Ok(ReferrerPolicyValue::StrictOriginWhenCrossOrigin));
        assert_eq!(
            ReferrerPolicy::owned(&map)
                .expect("valid policy list")
                .expect("policy present")
                .preferred(),
            Ok(ReferrerPolicyValue::StrictOriginWhenCrossOrigin)
        );
    }

    #[test]
    fn hsts_is_a_singleton_header() {
        let mut map = HeaderMap::new();
        map.append("strict-transport-security", HeaderValue::from_static("max-age=60"));
        map.append("strict-transport-security", HeaderValue::from_static("max-age=120"));
        StrictTransportSecurity::view(&map).expect_err("HSTS is a singleton");
    }

    #[test]
    fn nosniff_is_exact_and_singleton() {
        assert_eq!(XContentTypeOptionsOwned::nosniff().as_field_value().as_bytes(), b"nosniff");
        XContentTypeOptionsOwned::try_from("nosniff").expect("nosniff is valid");
        XContentTypeOptionsOwned::try_from("NoSniff").expect_err("value is case-sensitive");
        XContentTypeOptionsOwned::try_from("nosniff ").expect_err("trailing space must fail");
        let mut map = HeaderMap::new();
        map.append("x-content-type-options", HeaderValue::from_static("nosniff"));
        map.append("x-content-type-options", HeaderValue::from_static("nosniff"));
        XContentTypeOptions::view(&map).expect_err("X-Content-Type-Options is a singleton");
    }

    #[test]
    fn hsts_preserves_wire_and_enforces_known_directives() {
        let wire = "MAX-AGE=60 ; includeSubDomains ; x-vendor=\"a;b\"";
        let hsts = StrictTransportSecurityOwned::try_from(wire).expect("valid HSTS");
        assert_eq!(hsts.as_field_value().as_bytes(), wire.as_bytes());
        assert_eq!(hsts.max_age(), Duration::from_mins(1));
        assert!(hsts.include_subdomains());
        assert!(!hsts.preload());
        StrictTransportSecurityOwned::try_from("includeSubDomains").expect_err("max-age is required");
        StrictTransportSecurityOwned::try_from("max-age=1; max-age=2").expect_err("duplicate max-age must fail");
        let quoted = StrictTransportSecurityOwned::try_from("max-age=\"10\"").expect("quoted decimal is valid");
        assert_eq!(quoted.max_age(), Duration::from_secs(10));
        let escaped = StrictTransportSecurityOwned::try_from("max-age=\"\\10\"").expect("quoted pairs unescape");
        assert_eq!(escaped.max_age(), Duration::from_secs(10));
        assert_eq!(
            StrictTransportSecurityOwned::try_from("max-age=\"ten\"")
                .expect_err("unescaped value must contain only digits")
                .kind(),
            http_headers::DecodeErrorKind::InvalidNumber
        );
        StrictTransportSecurityOwned::try_from("max-age =10").expect_err("whitespace before equals must fail");
        StrictTransportSecurityOwned::try_from("max-age= 10").expect_err("whitespace after equals must fail");
        StrictTransportSecurityOwned::try_from("max-age=10; preload=yes").expect_err("preload does not take a value");
        StrictTransportSecurityOwned::try_from("max-age=18446744073709551616").expect_err("overflow must fail");
        StrictTransportSecurityOwned::try_from("max-age=10; includeSubDomains; includeSubDomains")
            .expect_err("duplicate includeSubDomains must fail");
    }

    #[test]
    fn csp_is_opaque_safe_policy_text_and_preserves_multiple_lines() {
        let csp = ContentSecurityPolicyOwned::new("default-src 'self'")
            .and_then(|policy| policy.with_policy("frame-ancestors 'none'"))
            .expect("valid policies");
        let policies: Vec<_> = csp.policies().collect();
        assert_eq!(
            policies,
            vec![b"default-src 'self'".as_slice(), b"frame-ancestors 'none'".as_slice()]
        );
        ContentSecurityPolicyOwned::new("default-src 'self'\nscript-src *").expect_err("newlines are forbidden");
    }

    #[test]
    fn csp_exposes_non_utf8_policy_bytes_fallibly() {
        let policy =
            ContentSecurityPolicyOwned::from_bytes([b'd', b'e', b'f', b'a', b'u', b'l', b't', 0xff]).expect("obs-text is field-value safe");
        assert_eq!(policy.policies().next(), Some(&b"default\xff"[..]));
        assert!(policy.policy_strs().next().is_some_and(|policy| policy.is_err()));
    }

    #[test]
    fn hsts_builder_and_accessors_cover_common_directives() {
        let hsts = StrictTransportSecurityOwned::builder(Duration::from_hours(8760))
            .include_subdomains()
            .preload()
            .extension_value("x-rollout", "\"stable\"")
            .build()
            .expect("valid HSTS construction");
        assert_eq!(hsts.max_age(), Duration::from_hours(8760));
        assert!(hsts.include_subdomains());
        assert!(hsts.preload());
        assert_eq!(
            hsts.as_field_value().as_bytes(),
            b"max-age=31536000; includeSubDomains; preload; x-rollout=\"stable\""
        );
        let directives: Result<Vec<_>, _> = hsts.directives().collect();
        let directives = directives.expect("valid stored directives");
        assert_eq!(directives.len(), 4);
        assert_eq!(directives[3].name(), "x-rollout");
        assert_eq!(directives[3].value(), Some(b"\"stable\"".as_slice()));
    }

    #[test]
    fn referrer_policy_preserves_unknown_extension_tokens() {
        let mut map = HeaderMap::new();
        map.insert("referrer-policy", HeaderValue::from_static("strict-origin, future-policy"));
        let view = ReferrerPolicy::view(&map).expect("valid token list").expect("policy present");
        let tokens: Result<Vec<_>, _> = view.tokens().collect();
        let tokens = tokens.expect("valid policy tokens");
        assert_eq!(tokens[0].policy(), Some(ReferrerPolicyValue::StrictOrigin));
        assert_eq!(tokens[1].as_str(), "future-policy");
        assert_eq!(tokens[1].policy(), None);
        assert_eq!(view.preferred(), Ok(ReferrerPolicyValue::StrictOrigin));
        map.insert("referrer-policy", HeaderValue::from_static("not a token"));
        ReferrerPolicy::view(&map).expect_err("invalid policy tokens must fail");
    }
}

mod websocket {
    use super::*;

    #[test]
    fn validates_accept_canonical_base64_and_length() {
        let digest = [
            0xb3, 0x7a, 0x4f, 0x2c, 0xc0, 0x62, 0x4f, 0x16, 0x90, 0xf6, 0x46, 0x06, 0xcf, 0x38, 0x59, 0x45, 0xb2, 0xbe, 0xc4, 0xea,
        ];
        let accept = SecWebSocketAcceptOwned::from_digest(digest).expect("a digest has a valid encoding");
        assert_eq!(accept.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        SecWebSocketAcceptOwned::try_from("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=").expect("canonical accept value");
        SecWebSocketAcceptOwned::try_from("s3pPLMBiTxaQ9kYGzzhZRbK+xOp=").expect_err("noncanonical tail bits must fail");
    }

    #[test]
    fn version_lists_cross_field_lines_and_reject_noncanonical_values() {
        let mut map = HeaderMap::new();
        map.append("sec-websocket-version", HeaderValue::from_static("13, 8"));
        map.append("sec-websocket-version", HeaderValue::from_static("7"));
        let view = SecWebSocketVersion::view(&map)
            .expect("valid version list")
            .expect("version present");
        assert_eq!(view.versions().collect::<Vec<_>>(), vec![7, 8, 13]);
        view.requested().expect_err("multiple versions are not one request version");
        map.insert("sec-websocket-version", HeaderValue::from_static("013"));
        SecWebSocketVersion::view(&map).expect_err("leading zero is noncanonical");
        map.insert("sec-websocket-version", HeaderValue::from_static("256"));
        SecWebSocketVersion::view(&map).expect_err("version exceeds u8");
        assert_eq!(
            SecWebSocketVersionOwned::try_from("13").and_then(|version| version.requested()),
            Ok(13)
        );
    }

    #[test]
    fn extension_builder_emits_quoted_token_values() {
        let extensions = SecWebSocketExtensionsOwned::builder()
            .extension("permessage-deflate")
            .parameter_flag("client_no_context_takeover")
            .quoted_parameter("mode", "fast")
            .extension("x-test")
            .build()
            .expect("valid extension construction");
        let wire = extensions.extensions().next().expect("first extension").expect("valid extension");
        let mode = wire.parameters().nth(1).expect("mode parameter").expect("valid mode parameter");
        assert_eq!(mode.value(), Some(b"\"fast\"".as_slice()));
    }

    #[test]
    fn protocol_lists_are_tokens_and_case_sensitive() {
        let protocols = SecWebSocketProtocolOwned::new("chat")
            .and_then(|protocols| protocols.with_protocol("superchat"))
            .expect("valid protocol tokens");
        let collected: Result<Vec<_>, _> = protocols.protocols().collect();
        assert_eq!(collected, Ok(vec!["chat", "superchat"]));
        protocols.selected().expect_err("multiple protocols are not one selected protocol");
        assert_eq!(
            SecWebSocketProtocolOwned::try_from("chat").and_then(|protocol| { protocol.selected().map(std::string::ToString::to_string) }),
            Ok(String::from("chat"))
        );
        SecWebSocketProtocolOwned::new("not a token").expect_err("protocol must be a token");
    }

    #[test]
    fn derives_accept_from_client_key() {
        let key = SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==").expect("valid key");
        let accept = SecWebSocketAcceptOwned::from_key(&key).expect("valid derived response");
        assert_eq!(accept.encoded(), b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn key_and_accept_are_singleton_headers() {
        let mut map = HeaderMap::new();
        map.append("sec-websocket-key", HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="));
        map.append("sec-websocket-key", HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="));
        SecWebSocketKey::view(&map).expect_err("WebSocket key is a singleton");
    }

    #[test]
    fn validates_key_canonical_base64_and_length() {
        let key = SecWebSocketKeyOwned::from_nonce(*b"the sample nonce").expect("a fixed nonce has a valid encoding");
        assert_eq!(key.encoded(), b"dGhlIHNhbXBsZSBub25jZQ==");
        SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ==").expect("canonical key");
        SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZR==").expect_err("noncanonical tail bits must fail");
        SecWebSocketKeyOwned::try_from("dGhlIHNhbXBsZSBub25jZQ=").expect_err("incorrect padding must fail");
    }

    #[test]
    fn list_headers_preserve_field_lines_when_owned() {
        let mut map = HeaderMap::new();
        map.append(
            "sec-websocket-extensions",
            HeaderValue::from_static("permessage-deflate; client_max_window_bits"),
        );
        map.append("sec-websocket-extensions", HeaderValue::from_static("x-vendor; mode=\"fast\""));
        let owned = SecWebSocketExtensions::owned(&map)
            .expect("valid extensions")
            .expect("extensions present");
        let mut output = HeaderMap::new();
        SecWebSocketExtensions::insert(&mut output, owned).expect("an empty header map has capacity");
        let encoded: Vec<_> = output
            .get_all("sec-websocket-extensions")
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect();
        assert_eq!(
            encoded,
            vec![
                b"permessage-deflate; client_max_window_bits".to_vec(),
                b"x-vendor; mode=\"fast\"".to_vec(),
            ]
        );
    }

    #[test]
    fn rejects_malformed_extension_parameters() {
        SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode=\"open").expect_err("unterminated quote must fail");
        SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode=\"not token\"").expect_err("decoded quoted value must be a token");
        SecWebSocketExtensionsOwned::try_from("permessage-deflate; =value").expect_err("parameter name is required");
        SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode=").expect_err("parameter value is required");
        SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode =fast").expect_err("whitespace before equals must fail");
        SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode= fast").expect_err("whitespace after equals must fail");
    }

    #[test]
    fn parses_quoted_extension_parameters_and_lists() {
        let mut map = HeaderMap::new();
        map.insert(
            "sec-websocket-extensions",
            HeaderValue::from_static("permessage-deflate; mode=\"fa\\st\"; client_max_window_bits, x-test"),
        );
        let view = SecWebSocketExtensions::view(&map)
            .expect("valid extension list")
            .expect("extensions present");
        let mut extensions = view.extensions();
        let first = extensions.next().expect("first extension").expect("valid first extension");
        assert_eq!(first.name(), "permessage-deflate");
        let parameters: Result<Vec<_>, _> = first.parameters().collect();
        let parameters = parameters.expect("valid parameters");
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name(), "mode");
        assert_eq!(parameters[0].value(), Some(b"\"fa\\st\"".as_slice()));
        assert!(parameters[0].is_quoted());
        assert_eq!(parameters[1].name(), "client_max_window_bits");
        assert_eq!(parameters[1].value(), None);
        assert_eq!(
            extensions.next().expect("second extension").expect("valid second extension").name(),
            "x-test"
        );
    }

    #[test]
    fn borrowed_views_preserve_original_wire() {
        let mut map = HeaderMap::new();
        map.insert("sec-websocket-key", HeaderValue::from_static("dGhlIHNhbXBsZSBub25jZQ=="));
        let view = SecWebSocketKey::view(&map).expect("valid key").expect("key present");
        assert_eq!(view.as_field_value().as_bytes(), view.encoded());
        assert_eq!(
            SecWebSocketKey::owned(&map).expect("valid key").expect("key present").encoded(),
            b"dGhlIHNhbXBsZSBub25jZQ=="
        );
    }
}

mod reusable_credentials {
    use super::*;

    #[test]
    fn debug_does_not_expose_credentials() {
        let authorization = AuthorizationOwned::<Basic>::basic(b"user", b"secret").expect("valid credentials");
        let mut map = HeaderMap::new();
        Authorization::<Basic>::insert(&mut map, authorization).expect("an empty header map has capacity");
        let authorization = Authorization::<Basic>::view(&map)
            .expect("valid basic authorization")
            .expect("authorization present");
        let mut credentials = BasicCredentials::new();
        authorization.extract(&mut credentials).expect("valid basic credentials");
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("sensitive"));
    }
}

mod security_additional {
    use super::*;

    #[test]
    fn referrer_policy_fast_path_agrees_with_the_general_parser() {
        let inputs = [
            "no-referrer",
            "no-referrer-when-downgrade",
            "origin",
            "origin-when-cross-origin",
            "same-origin",
            "strict-origin",
            "strict-origin-when-cross-origin",
            "unsafe-url",
            " strict-origin ",
            "strict-origin, future-policy",
            "future-policy",
            "no-referrer,",
            ",no-referrer",
            "",
            "not a token",
            "NO-REFERRER",
        ];
        for input in inputs {
            let mut map = HeaderMap::new();
            map.insert("referrer-policy", HeaderValue::from_static(input));
            let borrowed = ReferrerPolicy::view(&map).map(|view| {
                view.map(|view| {
                    view.tokens()
                        .map(|token| token.map(|token| token.as_str().to_owned()))
                        .collect::<Vec<_>>()
                })
            });
            let owned = ReferrerPolicyOwned::try_from(input).map(|policy| {
                policy
                    .tokens()
                    .map(|token| token.map(|token| token.as_str().to_owned()))
                    .collect::<Vec<_>>()
            });
            assert_eq!(borrowed.is_ok(), owned.is_ok(), "acceptance disagreed for {input:?}");
            if let (Ok(Some(borrowed)), Ok(owned)) = (borrowed, owned) {
                assert_eq!(borrowed, owned, "tokens disagreed for {input:?}");
            }
        }
    }
}

mod websocket_additional {
    use super::*;

    #[test]
    fn list_fast_paths_agree_with_general_delimiter_handling() {
        let mut map = HeaderMap::new();
        map.append("sec-websocket-version", HeaderValue::from_static(" 13 ,, 8\t"));
        let view = SecWebSocketVersion::view(&map)
            .expect("optional whitespace and empty members are tolerated")
            .expect("version present");
        assert_eq!(view.versions().collect::<Vec<_>>(), vec![8, 13]);

        map.insert("sec-websocket-version", HeaderValue::from_static(","));
        let error = SecWebSocketVersion::view(&map).expect_err("a line of only delimiters holds no member");
        assert_eq!(error.kind(), DecodeErrorKind::MissingValue);

        let mut map = HeaderMap::new();
        map.append("sec-websocket-protocol", HeaderValue::from_static("chat"));
        map.append("sec-websocket-protocol", HeaderValue::from_static("\"open"));
        let error = SecWebSocketProtocol::view(&map).expect_err("an unterminated quote is rejected");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        assert_eq!(error.value_index(), Some(1));
    }
}
