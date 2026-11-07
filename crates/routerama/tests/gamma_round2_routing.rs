// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Backtracking, verb splitting, and forbidden-byte boundary oracles.

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route, RouteMatch};
use routerama::codegen_helpers::{InvalidPath, scan_segments, with_scanned_path};

fn route(name: &str, method: &str, path: &str) -> Route {
    Route::new(
        name,
        method,
        PathTemplate::parse(path, Grammar::default().with_segment_affixes()).expect("the fixture path is a valid extended route"),
    )
}

#[test]
fn gamma_nov4_failed_deep_literal_restores_the_affix_parent_depth_and_exact_borrowed_captures() {
    for depth in [1, 3, 7] {
        let middle = "/deep".repeat(depth);
        let resolver = RawResolver::new([
            route("literal", "GET", &format!("/root/fixed{middle}/never")),
            route("fallback", "GET", &format!("/root/{{first}}{middle}/{{last}}")),
        ]);
        let request = format!("/root/fixed{middle}/value");
        let matched = resolver.resolve("GET", &request).unwrap();
        assert_eq!(matched.name(), "fallback");
        for (name, value) in [("first", "fixed"), ("last", "value")] {
            let capture = matched.capture(name).unwrap();
            assert_eq!(capture, value);
            assert_eq!(capture.as_ptr(), request.as_ptr().wrapping_add(request.find(value).unwrap()));
        }
        assert!(resolver.resolve("POST", &request).is_none());
    }
}

#[test]
fn gamma_round2_dynamic_capture_diagnostics_preserve_both_sorted_sets() {
    let mut builder = routerama::__rt::DynBuilder::new();
    builder.add(routerama::HttpMethod::GET, "/{z}/{a}", &["wanted_z", "wanted_a"], "Missing", ());
    builder.add(routerama::HttpMethod::GET, "/{a}/{z}", &["a"], "Extra", ());
    let error = builder.finish().unwrap_err().to_string();
    assert!(
        error.contains("whose captures {a, z} do not match its fields {wanted_a, wanted_z}"),
        "{error}"
    );
    assert!(error.contains("whose captures {a, z} do not match its fields {a}"), "{error}");
    assert_eq!(error.matches("\n  - ").count(), 2);
}

#[test]
fn gamma_round2_affix_backtracking_rejects_partial_matches() {
    let resolver = RawResolver::new([
        route("literal", "GET", "/pre-x.txt/literal"),
        route("specific", "GET", "/pre-{id}.txt/specific"),
        route("suffix", "GET", "/{id}.txt/suffix"),
        route("prefix", "GET", "/pre-{id}/prefix"),
        route("single", "GET", "/{id}/single"),
        route("rest", "POST", "/{tail=**}"),
    ]);
    for (path, name, capture) in [
        ("/pre-x.txt/literal", "literal", None),
        ("/pre-x.txt/specific", "specific", Some("x")),
        ("/pre-x.txt/suffix", "suffix", Some("pre-x")),
        ("/pre-x.txt/prefix", "prefix", Some("x.txt")),
        ("/pre-x.txt/single", "single", Some("pre-x.txt")),
    ] {
        let found = resolver.resolve("GET", path).unwrap();
        assert_eq!(found.name(), name);
        assert_eq!(found.capture("id"), capture);
    }
    for path in [
        "/pre-.txt/specific",
        "/pre-x.bad/specific",
        "/bad-x.txt/specific",
        "/pre-.txt/suffix/no",
        "/pre-x.txt/missing",
    ] {
        assert!(resolver.resolve("GET", path).is_none(), "{path}");
        let found = resolver.resolve("POST", path).unwrap();
        assert_eq!(found.name(), "rest");
        assert_eq!(found.capture("tail"), Some(&path[1..]));
    }
    for (first, second, path) in [
        ("/pre-{x}.a/miss", "/pre-{y}.b/target", "/pre-z.a/target"),
        ("/aa-{x}.txt/miss", "/bb-{y}.txt/target", "/aa-z.txt/target"),
        ("/a{x}/miss", "/aa-{y}.txt/target", "/aa-.txt/target"),
    ] {
        let resolver = RawResolver::new([route("first", "GET", first), route("second", "GET", second)]);
        assert!(resolver.resolve("GET", path).is_none(), "{first} / {second} / {path}");
    }
}

#[test]
fn gamma_round2_single_branch_failure_restores_parent_depth_and_rest() {
    let resolver = RawResolver::new([
        route("literal", "GET", "/a/fixed/end"),
        route("single", "GET", "/a/{id}/middle/end"),
        route("rest", "GET", "/a/{tail=**}"),
        route("single2", "GET", "/b/{id}/middle/end"),
        route("rest2", "GET", "/b/{tail=**}"),
    ]);
    for (path, expected) in [
        ("/a/x/middle/end", "single"),
        ("/a/x/no", "rest"),
        ("/a/fixed/no", "rest"),
        ("/a", "rest"),
        ("/b/x/middle/end", "single2"),
        ("/b/x/missing", "rest2"),
        ("/b/x/middle/no", "rest2"),
        ("/b", "rest2"),
    ] {
        let found = resolver.resolve("GET", path).unwrap();
        assert_eq!(found.name(), expected, "{path}");
        if expected.starts_with("rest") {
            assert_eq!(found.capture("tail"), Some(path.get(3..).unwrap_or("")));
        }
    }
}

#[test]
fn gamma_round2_scan_forbidden_bytes_at_every_vector_and_tail_position() {
    for length in [15, 16, 17, 31, 32, 33, 47, 48, 49] {
        for index in 0..length {
            for byte in [b'#', b'?', b'$', b'"', 0] {
                let mut data = vec![b'a'; length];
                data[index] = byte;
                let body = String::from_utf8(data).unwrap();
                assert_eq!(
                    with_scanned_path(&body, 4, |_| ()).is_err(),
                    matches!(byte, b'#' | b'?'),
                    "{length}/{index}/{byte}"
                );
            }
        }
    }
}

#[test]
fn gamma_round2_segment_offsets_respect_unequal_output_capacities() {
    for (starts_len, ends_len) in [(0, 3), (3, 0), (1, 3), (3, 1), (3, 3)] {
        let mut starts = vec![usize::MAX; starts_len];
        let mut ends = vec![usize::MAX; ends_len];
        assert_eq!(scan_segments("/a/b/c", &mut starts, &mut ends), 3);
        let written = starts_len.min(ends_len);
        assert_eq!(&starts[..written], &[1, 3, 5][..written]);
        assert_eq!(&ends[..written], &[2, 4, 6][..written]);
        assert!(starts[written..].iter().all(|&value| value == usize::MAX));
        assert!(ends[written..].iter().all(|&value| value == usize::MAX));
    }
}

#[test]
fn gamma_round2_heap_offset_path_keeps_custom_verb_and_plain_colons_distinct() {
    let prefix = "/a".repeat(18);
    let verb_path = format!("{prefix}/{{name}}:go");
    let plain_path = format!("{prefix}/{{name}}");
    let resolver = RawResolver::new([route("verb", "POST", &verb_path), route("plain", "GET", &plain_path)]);
    let request = format!("{prefix}/v:go");
    let matched = resolver.resolve("POST", &request).unwrap();
    assert_eq!(matched.name(), "verb");
    assert_eq!(matched.capture("name"), Some("v"));
    assert!(resolver.resolve("GET", &format!("{prefix}/v:go")).is_none());
    let plain = RawResolver::new([route("plain", "GET", &plain_path)]);
    let request = format!("{prefix}/v:go");
    assert_eq!(plain.resolve("GET", &request).unwrap().capture("name"), Some("v:go"));
}

#[test]
fn gamma_round2_scanned_callbacks_observe_only_the_route_bounded_offset_prefix() {
    for maximum in [1, 20] {
        let longest = "/z".repeat(maximum);
        let resolver = RawResolver::new([route("rest", "GET", "/{tail=**}"), route("longest", "GET", &longest)]);
        let segments = (0..maximum + 3).map(|index| format!("s{index}")).collect::<Vec<_>>();
        let request = format!("/{}", segments.join("/"));
        let result = resolver
            .resolve_scanned_checked("GET", &request, |_, (), scanned| {
                assert_eq!(scanned.count(), segments.len());
                for (index, expected) in segments[..maximum].iter().enumerate() {
                    assert_eq!(scanned.segment(index), Some(expected.as_str()));
                }

                assert_eq!(scanned.segment(maximum), None);
                assert_eq!(scanned.capture(0, maximum), None);
                assert_eq!(scanned.rest(0), Some(&request[1..]));
                scanned.count()
            })
            .expect("all generated segments are valid");
        assert_eq!(result, Some(segments.len()));
        assert_eq!(
            resolver.resolve_scanned_checked("GET", "/bad?path", |_, (), _| ()),
            Err(InvalidPath)
        );
    }
}

#[test]
fn gamma_round2_failed_literal_restores_affix_and_affix_precedes_single_without_rest() {
    let resolver = RawResolver::new([route("literal", "GET", "/r/da-v/no"), route("affix", "GET", "/r/da-{id}/yes")]);
    let matched = resolver.resolve("GET", "/r/da-v/yes").unwrap();
    assert_eq!(matched.name(), "affix");
    assert_eq!(matched.capture("id"), Some("v"));
    let resolver = RawResolver::new([route("affix", "GET", "/r/pre-{id}/yes"), route("single", "GET", "/r/{id}/yes")]);
    let matched = resolver.resolve("GET", "/r/pre-v/yes").unwrap();
    assert_eq!(matched.name(), "affix");
    assert_eq!(matched.capture("id"), Some("v"));
}

#[test]
fn gamma_round2_segment_offsets_cover_every_adjacent_delimiter_in_vector_tails() {
    for length in 1..=65 {
        for marker in 0..length {
            let mut bytes = vec![b'a'; length];
            bytes[marker] = b'/';
            if marker + 1 < length {
                bytes[marker + 1] = b'/';
            }
            let body = String::from_utf8(bytes).unwrap();
            let rest = body.strip_prefix('/').unwrap_or(&body);
            let expected = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split('/')
                    .map(|part| {
                        let start = part.as_ptr().addr() - body.as_ptr().addr();
                        (start, start + part.len())
                    })
                    .collect::<Vec<_>>()
            };
            for capacity in [0, 1, 3, length + 1] {
                let mut starts = vec![usize::MAX; capacity];
                let mut ends = vec![usize::MAX; capacity];
                assert_eq!(scan_segments(&body, &mut starts, &mut ends), expected.len());
                for (index, &(start, end)) in expected.iter().take(capacity).enumerate() {
                    assert_eq!((starts[index], ends[index]), (start, end), "{body:?} at {index}");
                }
                assert!(starts[expected.len().min(capacity)..].iter().all(|&value| value == usize::MAX));
                assert!(ends[expected.len().min(capacity)..].iter().all(|&value| value == usize::MAX));
            }
        }
    }
}
