// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Smoke tests for the large build-time generated router used by the
//! `grs_router_vs_matchit` benchmark, confirming it resolves representative routes
//! (so the benchmark measures real hits, not accidental misses).

use rest_over_grpc::codegen_helpers::RouteMatch;
use rest_over_grpc_tests::bench_router::Route;

#[test]
fn resolves_shallow_and_deep_routes() {
    assert_eq!(
        Route::resolve("GET", "/v1/users/octocat").map(|m| m.name().to_owned()),
        Some("GetUser".to_owned())
    );
    assert_eq!(
        Route::resolve("GET", "/v1/repos/rust-lang/cargo/issues/1347/comments/7").map(|m| m.name().to_owned()),
        Some("GetIssueComment".to_owned())
    );
    assert_eq!(
        Route::resolve("PUT", "/v1/repos/rust-lang/cargo/pulls/42/merge").map(|m| m.name().to_owned()),
        Some("MergePull".to_owned())
    );
}

#[test]
fn resolves_catch_all_route() {
    let matched = Route::resolve("GET", "/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs").expect("catch-all matches");
    assert_eq!(matched.name(), "GetContents");
}

#[test]
fn method_disambiguates_same_path() {
    assert_eq!(
        Route::resolve("GET", "/v1/users").map(|m| m.name().to_owned()),
        Some("ListUsers".to_owned())
    );
    assert_eq!(
        Route::resolve("POST", "/v1/users").map(|m| m.name().to_owned()),
        Some("CreateUser".to_owned())
    );
}

#[test]
fn misses_resolve_to_none() {
    assert!(Route::resolve("GET", "/v1/unknown").is_none());
    assert!(Route::resolve("GET", "/v1/repos/rust-lang").is_none());
    assert!(Route::resolve("DELETE", "/v1/users").is_none());
}
