// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for [`PathTemplate::parse`].
//!
//! Parsing a `google.api.http` path template is this crate's entire value
//! proposition: a REST-over-gRPC build step parses every annotated method's
//! template once, and a router may re-parse templates when (re)building its
//! route table. Each parse walks the string, splits it into segments, and
//! allocates the resulting [`Segment`](http_path_template::Segment) AST.
//!
//! These benchmarks isolate a single `parse` call across the grammar's
//! branching shapes — literal-only paths, `{variable}` bindings, a `**` rest
//! wildcard, a trailing `:verb`, an extended-grammar intra-segment affix, and
//! the error path.
//!
//! Paired with `hpt_parse.rs`, which covers the same operations under
//! wall-clock (Criterion) measurement. The Callgrind instruction counts here
//! are the authoritative signal for allocation and branch changes, which
//! wall-clock cannot reliably resolve.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
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
    use http_path_template::{Grammar, ParseError, PathTemplate};

    // A path made only of literal segments — no variables, no wildcards.
    const LITERAL_ONLY: &str = "/v1/shelves/books/list";

    // A typical CRUD annotation with two `{variable}` bindings.
    const VARIABLES: &str = "/v1/shelves/{shelf}/books/{book}";

    // A variable whose sub-template ends in a `**` rest wildcard.
    const REST_WILDCARD: &str = "/v1/{name=books/**}";

    // A path with two variables and a trailing custom `:verb`.
    const VERB: &str = "/v1/shelves/{shelf}/books/{book}:archive";

    // An extended-grammar intra-segment affix (`{name}` wrapped in `files/` … `.json`).
    const AFFIX: &str = "/v1/files/{name}.json";

    // An invalid template (`**` is not the final segment) — exercises the error path.
    const INVALID: &str = "/a/**/b";

    #[library_benchmark]
    fn parse_literal_only() -> Result<PathTemplate<'static>, ParseError> {
        PathTemplate::parse(black_box(LITERAL_ONLY), black_box(Grammar::default()))
    }

    #[library_benchmark]
    fn parse_variables() -> Result<PathTemplate<'static>, ParseError> {
        PathTemplate::parse(black_box(VARIABLES), black_box(Grammar::default()))
    }

    #[library_benchmark]
    fn parse_rest_wildcard() -> Result<PathTemplate<'static>, ParseError> {
        PathTemplate::parse(black_box(REST_WILDCARD), black_box(Grammar::default()))
    }

    #[library_benchmark]
    fn parse_verb() -> Result<PathTemplate<'static>, ParseError> {
        PathTemplate::parse(black_box(VERB), black_box(Grammar::default()))
    }

    #[library_benchmark]
    fn parse_affix() -> Result<PathTemplate<'static>, ParseError> {
        PathTemplate::parse(black_box(AFFIX), black_box(Grammar::default().with_segment_affixes()))
    }

    #[library_benchmark]
    fn parse_invalid() -> Result<PathTemplate<'static>, ParseError> {
        PathTemplate::parse(black_box(INVALID), black_box(Grammar::default()))
    }

    library_benchmark_group!(
        name = parse;
        benchmarks =
            parse_literal_only,
            parse_variables,
            parse_rest_wildcard,
            parse_verb,
            parse_affix,
            parse_invalid
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::parse;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = parse
);
