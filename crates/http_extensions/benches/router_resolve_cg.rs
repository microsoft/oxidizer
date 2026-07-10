// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for [`Router::resolve_request_uri`], the per-request routing step.
//!
//! For every outgoing request (and again per retry/hedge attempt) the router picks the
//! [`BaseUri`] and materializes the request's `http::Uri`. This benchmark isolates that step
//! on a request built through [`HttpRequestBuilder`] (so the build-time path rendering is
//! cached and reused) for both a root and a prefixed base path.
//!
//! Paired with `router_resolve.rs`, which covers the same operation under wall-clock
//! (Criterion) measurement.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    clippy::unwrap_used,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::{library_benchmark, library_benchmark_group};
    use http_extensions::routing::{Router, RouterContext};
    use http_extensions::{HttpRequest, HttpRequestBuilder};
    use templated_uri::BaseUri;

    fn setup(base: &'static str) -> (Router, HttpRequest) {
        let router = Router::fixed(BaseUri::from_static(base));
        let request = HttpRequestBuilder::new_fake()
            .get("/users/42/posts/hello-world?active=true")
            .build()
            .unwrap();
        (router, request)
    }

    // Fixed endpoint, root base path (common case): the reused rendered path is returned
    // without a re-validation scan.
    #[library_benchmark]
    #[bench::root(setup("https://api.example.com"))]
    fn resolve_root(input: (Router, HttpRequest)) -> HttpRequest {
        let (router, mut request) = input;
        router.resolve_request_uri(RouterContext::new(), black_box(&mut request)).unwrap();
        request
    }

    // Fixed endpoint, non-root base path: the join concatenates and re-validates.
    #[library_benchmark]
    #[bench::prefixed(setup("https://api.example.com/v1/"))]
    fn resolve_prefixed(input: (Router, HttpRequest)) -> HttpRequest {
        let (router, mut request) = input;
        router.resolve_request_uri(RouterContext::new(), black_box(&mut request)).unwrap();
        request
    }

    library_benchmark_group!(
        name = router_resolve;
        benchmarks = resolve_root, resolve_prefixed
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::router_resolve;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = router_resolve
);
