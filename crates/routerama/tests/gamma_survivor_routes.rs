// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic matching, capture, and scan regression oracles.

use std::borrow::Cow;
use std::fmt::Write as _;

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route, RouteMatch};
use routerama::codegen_helpers::{scan_path, seg_bytes, split_verb, substr, with_scanned_path};
use routerama::{HttpMethod, ResolveError};

#[routerama::resolver]
#[derive(Debug, PartialEq, Eq)]
enum Typed<'p> {
    #[route(GET, "/s/{raw}/{owned}/{cow}/{number}")]
    Static {
        raw: &'p str,
        owned: String,
        cow: Cow<'p, str>,
        number: u32,
    },
    Dynamic {
        left: String,
        right: u32,
    },
}

#[test]
fn gamma_survivor_capture_coercion_and_dynamic_order() {
    let resolver = Typed::builder().add_dynamic(HttpMethod::GET, "/d/{right}/{left}").build().unwrap();
    assert_eq!(
        resolver.resolve("GET", "/d/%34%32/hello%2Fworld+"),
        Ok(Typed::Dynamic {
            left: "hello/world+".into(),
            right: 42
        })
    );
    assert_eq!(
        resolver.resolve("GET", "/s/a%2Fb/c%2Fd/unchanged/%31%37"),
        Ok(Typed::Static {
            raw: "a%2Fb",
            owned: "c/d".into(),
            cow: Cow::Borrowed("unchanged"),
            number: 17
        })
    );
    assert_eq!(
        resolver.resolve("GET", "/s/raw/plain/caf%C3%A9/0"),
        Ok(Typed::Static {
            raw: "raw",
            owned: "plain".into(),
            cow: Cow::Owned("café".into()),
            number: 0
        })
    );
    for (path, expected) in [
        ("/d/nope/hello", ResolveError::InvalidCapture("right")),
        ("/d/%FF/hello", ResolveError::UndecodableCapture("right")),
        ("/d/3/%G0", ResolveError::UndecodableCapture("left")),
        ("/s/raw/%0/cow/1", ResolveError::UndecodableCapture("owned")),
        ("/s/raw/plain/%FF/1", ResolveError::UndecodableCapture("cow")),
        ("/s/raw/plain/cow/%GG", ResolveError::UndecodableCapture("number")),
        ("/s/raw/plain/cow/nope", ResolveError::InvalidCapture("number")),
    ] {
        assert_eq!(resolver.resolve("GET", path).unwrap_err(), expected, "{path}");
    }
    for path in ["/d/1/x?y", "/d/1/x#y", "/s/raw/plain/cow/1?x"] {
        assert_eq!(resolver.resolve("GET", path).unwrap_err(), ResolveError::InvalidPath(path));
    }
}

#[cfg(test)]
fn route(name: &str, method: &str, path: &str) -> Route {
    Route::new(
        name,
        method,
        PathTemplate::parse(path, Grammar::default().with_segment_affixes()).unwrap(),
    )
}

#[test]
fn gamma_survivor_runtime_backtracking_affixes_verbs_payloads() {
    let resolver = RawResolver::with_values([
        (route("literal", "GET", "/v/system/config"), 11),
        (route("single", "GET", "/v/{tenant}/settings"), 22),
        (route("affix", "GET", "/v/pre-{id}.json/details"), 33),
        (route("generic", "GET", "/v/{id}/generic"), 44),
        (route("rest", "GET", "/v/{path=**}"), 55),
        (route("verb", "POST", "/v/{name=books/*}:archive"), 66),
        (route("suffix", "GET", "/v/{id}.txt"), 77),
        (route("prefix", "GET", "/v/img-{id}"), 88),
    ]);
    for (method, path, name, value, captures) in [
        ("GET", "/v/system/config", "literal", 11, vec![]),
        ("GET", "/v/system/settings", "single", 22, vec![("tenant", "system")]),
        ("GET", "/v/pre-é.json/details", "affix", 33, vec![("id", "é")]),
        ("GET", "/v/pre-a.json/generic", "generic", 44, vec![("id", "pre-a.json")]),
        ("GET", "/v", "rest", 55, vec![("path", "")]),
        ("GET", "/v/a/b/", "rest", 55, vec![("path", "a/b/")]),
        ("POST", "/v/books/a:b:archive", "verb", 66, vec![("name", "books/a:b")]),
        ("GET", "/v/a.txt", "suffix", 77, vec![("id", "a")]),
        ("GET", "/v/img-b", "prefix", 88, vec![("id", "b")]),
    ] {
        let matched = resolver.resolve(method, path).unwrap();
        assert_eq!(matched.name(), name, "{path}");
        assert_eq!(*matched.value(), value, "{path}");
        assert_eq!(matched.captures().collect::<Vec<_>>(), captures);
        assert_eq!(matched.capture("missing"), None);
        assert!(format!("{matched:?}").contains(name));
    }
    for (method, path) in [
        ("get", "/v/system/config"),
        ("DELETE", "/v/a"),
        ("POST", "/v/books/a"),
        ("POST", "/v/books/a:wrong"),
        ("POST", "/v/books/:archive"),
        ("GET", "/v/a?b"),
        ("GET", "/v/a#b"),
        ("GET", "/outside"),
    ] {
        assert!(resolver.resolve(method, path).is_none(), "{method} {path}");
    }
    assert!(resolver.splits_verbs());
    assert_eq!(
        format!("{resolver:?}"),
        "RawResolver { routes: 8, max_segments: 3, any_verb: true, .. }"
    );
    let mut plain = RawResolver::new([route("plain", "GET", "/{id}")]);
    assert!(!plain.splits_verbs());
    plain.force_verb_split(false);
    assert!(!plain.splits_verbs());
    assert_eq!(plain.resolve("GET", "/a:b").unwrap().capture("id"), Some("a:b"));
    plain.force_verb_split(true);
    plain.force_verb_split(false);
    assert!(plain.splits_verbs());
    assert!(plain.resolve("GET", "/a:b").is_none());
}

#[test]
fn gamma_survivor_scan_ranges_overflow_utf8_and_inline_boundaries() {
    for (path, body, verb) in [
        ("/a:b:go", "/a:b", Some("go")),
        ("/a:b/c", "/a:b/c", None),
        ("/a:", "/a:", None),
        ("/a:b?c", "/a:b?c", None),
        ("/a:b#c", "/a:b#c", None),
        ("abc", "abc", None),
    ] {
        assert_eq!(split_verb(path), (body, verb));
    }
    for count in [0, 1, 15, 16, 17, 31, 32] {
        let body = "/é".repeat(count);
        for capacity in [0, 1, 16, 40] {
            let mut starts = vec![usize::MAX; capacity];
            let mut ends = vec![usize::MAX; capacity + 1];
            let path = scan_path(&body, &mut starts, &mut ends);
            assert!(path.is_valid());
            assert_eq!(path.count(), count);
            for index in 0..count.min(capacity) {
                assert_eq!(path.segment(index), Some("é"));
            }
            assert_eq!(path.segment(count), None);
            assert_eq!(path.segment(count.min(capacity)), None);
            assert_eq!(path.capture(0, count.min(capacity)), None);
            assert_eq!(path.affix(count.min(capacity), 0, 0), None);
            assert_eq!(path.rest(count), Some(""));
            assert_eq!(path.rest(count + 1), None);
            assert_eq!(path.capture(1, 0), None);
        }
        assert_eq!(
            with_scanned_path(&body, 40, |path| path.segment(count.saturating_sub(1)).map(str::to_owned)).unwrap(),
            (count != 0).then(|| "é".to_owned())
        );
    }
    let mut starts = [0; 2];
    let mut ends = [0; 2];
    let scanned = scan_path("/xéz/tail", &mut starts, &mut ends);
    assert_eq!(scanned.capture(0, 1), Some("xéz/tail"));
    assert_eq!(scanned.rest(1), Some("tail"));
    assert_eq!(scanned.affix(0, 1, 1), Some("é"));
    for (prefix, suffix) in [(2, 0), (0, 2), (usize::MAX, 0), (0, usize::MAX), (3, 3)] {
        assert_eq!(scanned.affix(0, prefix, suffix), None);
    }
    assert_eq!(seg_bytes("/xéz", 2, 4), "é".as_bytes());
    assert_eq!(substr("/xéz", 2, 4), "é");
    assert_eq!(format!("{scanned:?}"), "ScannedPath { body: \"/xéz/tail\", count: 2, .. }");
    for index in [1, 15, 16, 17, 31] {
        for forbidden in ['?', '#'] {
            let body = format!("/{}{forbidden}tail", "a".repeat(index));
            assert!(with_scanned_path(&body, 40, |_| ()).is_err());
        }
    }
}

#[test]
fn gamma_survivor_deep_runtime_route_and_many_captures() {
    for count in [16, 17, 20] {
        let mut template = String::new();
        let mut request = String::new();
        for i in 0..count {
            write!(template, "/{{v{i}}}").unwrap();
            write!(request, "/value{i}").unwrap();
        }
        let resolver = RawResolver::with_values([(route("deep", "GET", &template), 91)]);
        let matched = resolver.resolve("GET", &request).unwrap();
        assert_eq!(*matched.value(), 91);
        assert_eq!(matched.captures().count(), count);
        for i in 0..count {
            assert_eq!(matched.capture(&format!("v{i}")), Some(format!("value{i}").as_str()));
        }
        assert!(resolver.resolve("GET", "/short").is_none());
        assert!(resolver.resolve("POST", &request).is_none());
        let longer = format!("{request}/extra");
        assert!(resolver.resolve("GET", &longer).is_none());
    }
}

#[test]
fn gamma_survivor_dynamic_builder_accumulates_exact_causes() {
    let error = Typed::builder().add_dynamic(HttpMethod::GET, "/d/{right}").build().unwrap_err();
    assert!(error.to_string().contains("{left, right}"));
    assert!(error.to_string().contains("{right}"));
    assert!(std::error::Error::source(&error).is_none());
    let error = Typed::builder()
        .add_dynamic(HttpMethod::GET, "/{broken")
        .add_dynamic(HttpMethod::GET, "broken")
        .build()
        .unwrap_err();
    assert_eq!(error.causes().count(), 2);
    let causes = error
        .causes()
        .map(|error| error.downcast_ref::<http_path_template::ParseError>().unwrap())
        .collect::<Vec<_>>();
    assert!(causes[0].is_unbalanced_braces());
    assert!(causes[1].is_missing_leading_slash());
    let error = Typed::builder().build().unwrap_err();
    assert!(error.to_string().contains("add_dynamic"));
}

#[test]
fn gamma_survivor_capture_views_bounds_debug_and_missing_errors() {
    use routerama::__rt::{Captures, DynBuilder, owned, parse};
    let resolver = RawResolver::new([route("pair", "GET", "/{left}/{right}")]);
    resolver
        .resolve_scanned_checked("GET", "/first/42", |leaf, (), path| {
            let captures = Captures::new(leaf, path, &[1, 0]);
            assert_eq!(captures.get(0), Some("42"));
            assert_eq!(captures.get(1), Some("first"));
            assert_eq!(captures.get(2), None);
            assert_eq!(format!("{captures:?}"), "Captures { count: 2, .. }");
            assert_eq!(owned(&captures, 1, "left").unwrap(), "first");
            assert_eq!(parse::<u32>(&captures, 0, "right").unwrap(), 42);
            assert_eq!(owned(&captures, 2, "missing").unwrap_err(), ResolveError::MissingCapture("missing"));
            assert_eq!(
                parse::<u32>(&captures, 2, "missing").unwrap_err(),
                ResolveError::MissingCapture("missing")
            );
            let bad_order = Captures::new(leaf, path, &[3]);
            assert_eq!(bad_order.get(0), None);
        })
        .unwrap()
        .unwrap();
    let mut builder = DynBuilder::new();
    builder.add(HttpMethod::GET, "/", &[], "Home", ());
    builder.add(HttpMethod::GET, "/{bad", &[], "Bad", ());
    assert_eq!(format!("{builder:?}"), "DynBuilder { routes: 1, errors: 1, .. }");
    let mut dotted = DynBuilder::new();
    dotted.add(HttpMethod::GET, "/{book.id}", &["book.id"], "Nested", ());
    let resolver = dotted.finish().unwrap();
    let matched = resolver.resolve("GET", "/value").unwrap();
    assert_eq!(matched.capture("book.id"), Some("value"));
    assert_eq!(
        format!("{matched:?}"),
        "RawMatch { name: \"Nested\", captures: [(\"book.id\", \"value\")], .. }"
    );
}

#[test]
fn gamma_survivor_affix_fallback_precedence_and_empty_capture() {
    let resolver = RawResolver::new([
        route("literal-dead-end", "GET", "/img-a.json/missing"),
        route("specific", "GET", "/img-{id}.json/details"),
        route("suffix", "GET", "/{id}.json/tail"),
        route("prefix", "GET", "/img-{id}/tail"),
        route("single", "GET", "/{id}/tail"),
        route("rest", "GET", "/{path=**}"),
    ]);
    for (path, name, capture) in [
        ("/img-a.json/details", "specific", "a"),
        ("/img-a.json/tail", "suffix", "img-a"),
        ("/img-a/tail", "prefix", "a"),
        ("/a.json/tail", "suffix", "a"),
        ("/plain/tail", "single", "plain"),
        ("/img-.json/tail", "suffix", "img-"),
        ("/.json/tail", "single", ".json"),
        ("/img-/tail", "single", "img-"),
    ] {
        let matched = resolver.resolve("GET", path).unwrap();
        assert_eq!(matched.name(), name, "{path}");
        assert_eq!(matched.capture("id"), Some(capture), "{path}");
    }
    let matched = resolver.resolve("GET", "/other/unmatched").unwrap();
    assert_eq!(matched.name(), "rest");
    assert_eq!(matched.capture("path"), Some("other/unmatched"));
    let mut starts = [0; 3];
    let mut ends = [0; 3];
    let scanned = scan_path("/x", &mut starts, &mut ends);
    assert_eq!(scanned.segment(1), None);
    assert_eq!(scanned.capture(0, 1), None);
    assert_eq!(scanned.affix(1, 0, 0), None);
    assert_eq!(scanned.affix(0, 1, 0), Some(""));
    assert_eq!(scanned.rest(2), None);
    for capacity in [0, 1] {
        let mut starts = [0; 3];
        let mut ends = vec![0; capacity];
        let scanned = scan_path("/a/b/c", &mut starts, &mut ends);
        assert_eq!(scanned.count(), 3);
        assert_eq!(scanned.segment(capacity), None);
    }
    with_scanned_path(&"/x".repeat(20), 17, |scanned| {
        assert_eq!(scanned.segment(16), Some("x"));
        assert_eq!(scanned.segment(17), None);
    })
    .unwrap();
}

#[test]
fn gamma_survivor_configuration_display_propagates_writer_failure() {
    use std::fmt;
    struct Reject(usize);
    impl fmt::Write for Reject {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            if self.0 == 0 {
                return Err(fmt::Error);
            }
            self.0 -= 1;
            Ok(())
        }
    }
    let error = Typed::builder().build().unwrap_err();
    for remaining in 0..3 {
        assert!(fmt::write(&mut Reject(remaining), format_args!("{error}")).is_err());
    }
}
#[routerama::resolver]
#[derive(Debug, PartialEq, Eq)]
enum Static<'p> {
    #[route(get, "/")]
    Root,
    #[route(GET, "/fixed")]
    Fixed,
    #[route(GET, "/files/{path=**}")]
    Rest { path: &'p str },
    #[route(GET, "/span/{book.id=shelves/*}")]
    Span { book_id: &'p str },
    #[route(POST, "/span/{book.id=shelves/*}:archive")]
    Verb { book_id: &'p str },
    #[route(GET, "/img-{type}.json/details")]
    Specific { _f_type: &'p str },
    #[route(GET, "/{id}.json/tail")]
    Suffix { id: &'p str },
    #[route(GET, "/img-{id}/tail")]
    Prefix { id: &'p str },
    #[route(GET, "/{id}/tail")]
    Single { id: &'p str },
}

#[test]
fn gamma_survivor_static_generated_route_variants_capture_and_backtracking() {
    let resolver = Static::resolver();
    for (method, path, expected) in [
        ("GET", "/", Static::Root),
        ("GET", "/fixed", Static::Fixed),
        ("GET", "/files", Static::Rest { path: "" }),
        ("GET", "/files/a/b", Static::Rest { path: "a/b" }),
        ("GET", "/span/shelves/7", Static::Span { book_id: "shelves/7" }),
        ("POST", "/span/shelves/7:archive", Static::Verb { book_id: "shelves/7" }),
        ("GET", "/img-a.json/details", Static::Specific { _f_type: "a" }),
        ("GET", "/img-a.json/tail", Static::Suffix { id: "img-a" }),
        ("GET", "/img-a/tail", Static::Prefix { id: "a" }),
        ("GET", "/plain/tail", Static::Single { id: "plain" }),
        ("GET", "/.json/tail", Static::Single { id: ".json" }),
    ] {
        assert_eq!(resolver.resolve(method, path), Ok(expected), "{method} {path}");
    }
    for (method, path) in [
        ("get", "/"),
        ("POST", "/fixed"),
        ("GET", "/fixed/"),
        ("GET", "/span/books/7"),
        ("GET", "/span/shelves/"),
        ("POST", "/span/shelves/7"),
        ("GET", "/files/a:wrong"),
        ("GET", "/img-.json/details"),
        ("GET", "//tail"),
    ] {
        assert_eq!(resolver.resolve(method, path), Err(ResolveError::NotFound(path)), "{method} {path}");
    }
}

#[routerama::resolver]
#[derive(Debug, PartialEq, Eq)]
enum Units {
    #[route(GET, "/")]
    Root,
}

#[routerama::resolver]
#[derive(Debug, PartialEq, Eq)]
enum DynamicOnly {
    Home,
    Value { x: String },
}

#[test]
fn gamma_survivor_unit_and_dynamic_only_resolvers() {
    let unit = Units::resolver();
    assert_eq!(unit.resolve("GET", "/"), Ok(Units::Root));
    assert_eq!(unit.resolve("POST", "/"), Err(ResolveError::NotFound("/")));
    assert_eq!(unit.resolve("GET", "/extra"), Err(ResolveError::NotFound("/extra")));
    let dynamic = DynamicOnly::builder()
        .add_home(HttpMethod::GET, "/")
        .add_value(HttpMethod::POST, "/value/{x}")
        .build()
        .unwrap();
    assert_eq!(dynamic.resolve("GET", "/"), Ok(DynamicOnly::Home));
    assert_eq!(dynamic.resolve("POST", "/value/a%2Fb"), Ok(DynamicOnly::Value { x: "a/b".into() }));
    assert_eq!(dynamic.resolve("GET", "/value/a"), Err(ResolveError::NotFound("/value/a")));
    assert_eq!(dynamic.resolve("POST", "/value/%GG"), Err(ResolveError::UndecodableCapture("x")));
}
