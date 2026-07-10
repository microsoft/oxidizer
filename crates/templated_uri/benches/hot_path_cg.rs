// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the per-request URI hot path in `templated_uri`.
//!
//! An HTTP client builds a URI for every outgoing request; these benchmarks
//! isolate the per-call steps for a dynamic REST path — escaping a variable,
//! rendering a templated path, validating it into an `http` `PathAndQuery`,
//! assembling a `Uri`, and materializing the full `http::Uri` (base + path).
//!
//! Paired with `hot_path.rs`, which covers the same operations under wall-clock
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

    // A realistic REST path: a numeric id plus an escaped string segment.
    #[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
    #[derive(Clone)]
    struct UserPostPath {
        user_id: u32,
        post_id: EscapedString,
    }

    // A dynamic value carrying reserved characters, so `escape` must percent-encode.
    const RAW_VALUE: &str = "my post title/with?reserved=chars";

    // A dynamic value that is already URI-clean, exercising the no-encoding fast path.
    const CLEAN_VALUE: &str = "already-clean_value.42~ok";

    fn sample_path() -> UserPostPath {
        UserPostPath {
            user_id: 42,
            // Owned value, mirroring a real request where the segment was just escaped.
            post_id: EscapedString::escape(String::from("hello-world")),
        }
    }

    fn sample_base() -> BaseUri {
        BaseUri::from_static("https://api.example.com")
    }

    // Percent-encode a dynamic string value containing reserved characters.
    #[library_benchmark]
    fn escape() -> EscapedString {
        EscapedString::escape(black_box(RAW_VALUE))
    }

    // Escape a value that needs no encoding: exercises the allocation-free scan.
    #[library_benchmark]
    fn escape_clean() -> EscapedString {
        EscapedString::escape(black_box(CLEAN_VALUE))
    }

    // Render a templated path into a `String`.
    #[library_benchmark]
    #[bench::sample(sample_path())]
    fn render(path: UserPostPath) -> UserPostPath {
        black_box(black_box(&path).render());
        path
    }

    // Render + validate a templated path into an `http` `PathAndQuery`.
    #[library_benchmark]
    #[bench::sample(sample_path())]
    fn to_path_and_query(path: UserPostPath) -> UserPostPath {
        black_box(black_box(&path).to_path_and_query().expect("valid path-and-query"));
        path
    }

    // End-to-end per-call construction: a reused base plus a freshly built path.
    #[library_benchmark]
    #[bench::sample((sample_path(), sample_base()))]
    fn build_uri(input: (UserPostPath, BaseUri)) -> Uri {
        let (path, base) = input;
        Uri::default().with_base(black_box(base)).with_path_and_query(black_box(path))
    }

    // Full per-request materialization: build the `Uri` (reused base + fresh templated
    // path) and convert it into a validated `http::Uri`, exactly as an HTTP client does
    // for every outgoing request. This is the real hot path end to end.
    #[library_benchmark]
    #[bench::sample((sample_path(), sample_base()))]
    fn materialize(input: (UserPostPath, BaseUri)) -> http::Uri {
        let (path, base) = input;
        let uri = Uri::default().with_base(black_box(base)).with_path_and_query(black_box(path));
        http::Uri::try_from(uri).expect("valid http uri")
    }

    library_benchmark_group!(
        name = hot_path;
        benchmarks = escape, escape_clean, render, to_path_and_query, build_uri, materialize
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::hot_path;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = hot_path
);
