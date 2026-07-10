// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the routing materialization step on the request hot path.
//!
//! When a request is routed, the resolved [`Uri`] must be turned into an `http::Uri`.
//! `rerender` does this the pre-optimization way (`http::Uri::try_from`, which re-renders the
//! templated path); `reuse` calls [`Uri::to_http_uri`], joining only the base
//! onto the path that was rendered once at build time. The instruction-count delta is the
//! per-attempt saving of the single-materialization optimization, which grows with template
//! size and is paid once per routing attempt (so it multiplies under retry/hedging).
//!
//! Paired with `routing_rerender.rs`, which covers the same operations under wall-clock
//! (Criterion) measurement.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
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
    use templated_uri::{BaseUri, EscapedString, PathAndQueryTemplate as _, Uri, templated};

    #[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
    #[derive(Clone)]
    struct UserPostPath {
        user_id: u32,
        post_id: EscapedString,
    }

    #[templated(
        template = "/orgs/{org}/teams/{team}/projects/{project}/users/{user}/posts/{post}{?sort,order,limit,offset}",
        unredacted
    )]
    #[derive(Clone)]
    struct HeavyPath {
        org: EscapedString,
        team: EscapedString,
        project: EscapedString,
        user: u32,
        post: u32,
        sort: EscapedString,
        order: EscapedString,
        limit: u32,
        offset: u32,
    }

    fn sample_uri() -> (Uri, http::uri::PathAndQuery) {
        let path = UserPostPath {
            user_id: 42,
            post_id: EscapedString::escape(String::from("hello-world")),
        };
        let rendered = http::uri::PathAndQuery::try_from(path.render()).expect("valid path");
        let uri = Uri::default()
            .with_base(BaseUri::from_static("https://api.example.com"))
            .with_path_and_query(path);
        (uri, rendered)
    }

    fn sample_heavy_uri() -> (Uri, http::uri::PathAndQuery) {
        let path = HeavyPath {
            org: EscapedString::escape(String::from("contoso")),
            team: EscapedString::escape(String::from("platform")),
            project: EscapedString::escape(String::from("oxidizer")),
            user: 42,
            post: 1001,
            sort: EscapedString::escape(String::from("created")),
            order: EscapedString::escape(String::from("desc")),
            limit: 50,
            offset: 100,
        };
        let rendered = http::uri::PathAndQuery::try_from(path.render()).expect("valid path");
        let uri = Uri::default()
            .with_base(BaseUri::from_static("https://api.example.com"))
            .with_path_and_query(path);
        (uri, rendered)
    }

    // Pre-optimization: re-render the templated path while materializing.
    #[library_benchmark]
    #[bench::sample(sample_uri())]
    fn rerender(input: (Uri, http::uri::PathAndQuery)) -> http::Uri {
        let (uri, _rendered) = input;
        http::Uri::try_from(black_box(uri)).expect("valid http uri")
    }

    // Optimized: reuse the path rendered once at build time, joining only the base.
    #[library_benchmark]
    #[bench::sample(sample_uri())]
    fn reuse(input: (Uri, http::uri::PathAndQuery)) -> http::Uri {
        let (uri, rendered) = input;
        black_box(&uri).to_http_uri(Some(black_box(&rendered))).expect("valid http uri")
    }

    #[library_benchmark]
    #[bench::sample(sample_heavy_uri())]
    fn rerender_heavy(input: (Uri, http::uri::PathAndQuery)) -> http::Uri {
        let (uri, _rendered) = input;
        http::Uri::try_from(black_box(uri)).expect("valid http uri")
    }

    #[library_benchmark]
    #[bench::sample(sample_heavy_uri())]
    fn reuse_heavy(input: (Uri, http::uri::PathAndQuery)) -> http::Uri {
        let (uri, rendered) = input;
        black_box(&uri).to_http_uri(Some(black_box(&rendered))).expect("valid http uri")
    }

    library_benchmark_group!(
        name = route_materialize;
        benchmarks = rerender, reuse, rerender_heavy, reuse_heavy
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::route_materialize;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = route_materialize
);
