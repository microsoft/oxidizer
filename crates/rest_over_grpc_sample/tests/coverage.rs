// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime correctness tests for the generated routing trie, covering cases the
//! e2e/smoke tests miss: literal-vs-wildcard backtracking, custom verbs, nested
//! field-path bindings, `**` capture values, prefix overlap, and trailing
//! slashes.

use rest_over_grpc_sample::coverage::resolve;

fn rpc(method: &str, path: &str) -> Option<String> {
    resolve(method, path).map(|m| m.rpc().to_owned())
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
    let matched = resolve("GET", "/v1/system/settings").expect("matches TenantSettings");
    assert_eq!(matched.rpc(), "TenantSettings");
    assert_eq!(matched.get(&["tenant"]), Some("system"));
}

#[test]
fn wildcard_matches_non_literal_segment() {
    let matched = resolve("GET", "/v1/acme/settings").expect("matches");
    assert_eq!(matched.rpc(), "TenantSettings");
    assert_eq!(matched.get(&["tenant"]), Some("acme"));
}

#[test]
fn custom_verb_disambiguates_same_path_and_method_family() {
    let get = resolve("GET", "/v1/books/42").expect("get");
    assert_eq!(get.rpc(), "GetBook");
    assert_eq!(get.get(&["book"]), Some("42"));

    let archive = resolve("POST", "/v1/books/42:archive").expect("archive");
    assert_eq!(archive.rpc(), "ArchiveBook");
    assert_eq!(archive.get(&["book"]), Some("42"));
}

#[test]
fn verb_on_wrong_method_does_not_match() {
    // `GetBook` has no verb; a `:archive` request must not resolve to it.
    assert_eq!(rpc("GET", "/v1/books/42:archive"), None);
}

#[test]
fn nested_field_path_binding() {
    let matched = resolve("GET", "/v1/items/99").expect("matches");
    assert_eq!(matched.rpc(), "GetItem");
    assert_eq!(matched.get(&["item", "id"]), Some("99"));
    assert_eq!(matched.get(&["item"]), None);
}

#[test]
fn catch_all_captures_the_remainder() {
    let matched = resolve("GET", "/v1/tree/a/b/c.txt").expect("matches");
    assert_eq!(matched.rpc(), "GetTree");
    assert_eq!(matched.get(&["path"]), Some("a/b/c.txt"));
}

#[test]
fn catch_all_matches_empty_remainder() {
    // `**` matches zero segments: `/v1/tree` binds path = "".
    let matched = resolve("GET", "/v1/tree").expect("matches");
    assert_eq!(matched.rpc(), "GetTree");
    assert_eq!(matched.get(&["path"]), Some(""));
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
    let matched = resolve("GET", "/v1/books/42").expect("non-empty still matches");
    assert_eq!(matched.get(&["book"]), Some("42"));
}
