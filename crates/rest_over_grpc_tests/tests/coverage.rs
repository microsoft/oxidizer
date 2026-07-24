// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime correctness tests for the generated routing trie, covering cases the
//! e2e/smoke tests miss: literal-vs-wildcard backtracking, custom verbs, nested
//! field-path bindings, `**` capture values, prefix overlap, and trailing
//! slashes.

use rest_over_grpc::codegen_helpers::RouteMatch;
use rest_over_grpc_tests::coverage::Route;

fn rpc(method: &str, path: &str) -> Option<String> {
    Route::resolve(method, path).map(|m| m.name().to_owned())
}

#[test]
fn literal_beats_wildcard_at_same_depth() {
    // `/v1/system/config` (literal) and `/v1/{tenant}/settings` (wildcard) share
    // depth 1; the literal is more specific for its own path.
    assert_eq!(rpc("GET", "/v1/system/config"), Some("SystemConfig".to_owned()));
}

#[test]
fn backtracks_from_literal_to_wildcard() {
    // `system` matches the literal edge, but there is no `settings` child under
    // it, so resolution must backtrack to the `{tenant}` wildcard edge and bind
    // tenant = "system".
    let matched = Route::resolve("GET", "/v1/system/settings").expect("matches TenantSettings");
    assert_eq!(matched, Route::TenantSettings { tenant: "system" });
}

#[test]
fn wildcard_matches_non_literal_segment() {
    let matched = Route::resolve("GET", "/v1/acme/settings").expect("matches");
    assert_eq!(matched, Route::TenantSettings { tenant: "acme" });
}

#[test]
fn custom_verb_disambiguates_same_path_and_method_family() {
    let get = Route::resolve("GET", "/v1/books/42").expect("get");
    assert_eq!(get, Route::GetBook { book: "42" });

    let archive = Route::resolve("POST", "/v1/books/42:archive").expect("archive");
    assert_eq!(archive, Route::ArchiveBook { book: "42" });
}

#[test]
fn verb_on_wrong_method_does_not_match() {
    // `GetBook` has no verb; a `:archive` request must not resolve to it.
    assert_eq!(rpc("GET", "/v1/books/42:archive"), None);
}

#[test]
fn nested_field_path_binding() {
    // `{item.id}` becomes the `item_id` field on the matched variant.
    let matched = Route::resolve("GET", "/v1/items/99").expect("matches");
    assert_eq!(matched, Route::GetItem { item_id: "99" });
}

#[test]
fn catch_all_captures_the_remainder() {
    let matched = Route::resolve("GET", "/v1/tree/a/b/c.txt").expect("matches");
    assert_eq!(matched, Route::GetTree { path: "a/b/c.txt" });
}

#[test]
fn catch_all_matches_empty_remainder() {
    // `**` matches zero segments: `/v1/tree` binds path = "".
    let matched = Route::resolve("GET", "/v1/tree").expect("matches");
    assert_eq!(matched, Route::GetTree { path: "" });
}

#[test]
fn pattern_var_captures_multi_segment_span() {
    // `{name=shelves/*}` binds `name` to the whole span its interior-literal +
    // wildcard pattern matches (`shelves/<one segment>`), not just the wildcard.
    let matched = Route::resolve("GET", "/v1/search/shelves/42").expect("matches SearchShelf");
    assert_eq!(matched, Route::SearchShelf { name: "shelves/42" });
}

#[test]
fn pattern_var_requires_the_interior_literal() {
    // The pattern's `shelves` literal must be present: a different first segment
    // does not match, and the single wildcard still needs exactly one segment.
    assert_eq!(rpc("GET", "/v1/search/books/42"), None);
    assert_eq!(rpc("GET", "/v1/search/shelves"), None);
    assert_eq!(rpc("GET", "/v1/search/shelves/42/extra"), None);
}

#[test]
fn prefix_and_longer_route_coexist() {
    assert_eq!(rpc("GET", "/v1/x"), Some("GetX".to_owned()));
    assert_eq!(rpc("GET", "/v1/x/y"), Some("GetXY".to_owned()));
}

#[test]
fn trailing_slash_does_not_match_exact_route() {
    // `/v1/x/` has an extra (empty) segment, so it does not match `/v1/x`.
    assert_eq!(rpc("GET", "/v1/x/"), None);
}

#[test]
fn unknown_and_wrong_method_miss() {
    assert_eq!(rpc("GET", "/v1/nope"), None);
    assert_eq!(rpc("DELETE", "/v1/books/42"), None);
    assert_eq!(rpc("GET", "/"), None);
    assert_eq!(rpc("GET", ""), None);
}

#[test]
fn wildcard_does_not_match_empty_segment() {
    // A single-segment wildcard matches exactly one *non-empty* segment, so a
    // trailing slash or a `//` must not bind an empty value (this matches the
    // reference `PathTemplate::match_path`).
    assert_eq!(rpc("GET", "/v1/books/"), None);
    assert_eq!(rpc("GET", "/v1//settings"), None);
    // A normal, non-empty segment still binds.
    let matched = Route::resolve("GET", "/v1/books/42").expect("non-empty still matches");
    assert_eq!(matched, Route::GetBook { book: "42" });
}
