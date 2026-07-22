// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for [`EscapedString`] construction.
//!
//! An HTTP client escapes every dynamic string URI component for each outgoing request.
//! `EscapedString::escape` owns the escaped bytes, so today it heap-allocates for every
//! value. These benchmarks isolate that construction across the value lengths that matter
//! for a small-string optimization (short values that could live inline, <= 24 bytes on
//! 64-bit, versus longer values that always spill to the heap), plus a per-request group
//! that builds a templated struct from several short escaped fields.
//!
//! Paired with `escaped_string.rs`, which covers the same operations under wall-clock
//! (Criterion) measurement. The Callgrind instruction counts here are the authoritative
//! signal for allocation changes, which wall-clock cannot reliably resolve.

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
    use templated_uri::{EscapedString, templated};

    // A short, already-clean value: no encoding, short enough to live inline once
    // `EscapedString` uses a small-string-optimized backing store.
    const SHORT_CLEAN: &str = "hello-world";

    // A short value that must be percent-encoded; its escaped form is still short enough to
    // live inline.
    const SHORT_ENCODED_INPUT: &str = "a b/c?d"; // -> "a%20b%2Fc%3Fd" (13 bytes)

    // Exactly 24 bytes (clean): the largest value that fits inline on 64-bit.
    const BOUNDARY_INLINE: &str = "abcdefghijklmnopqrstuvwx";

    // 25 bytes (clean): one byte over the inline capacity, so it always spills to the heap.
    const BOUNDARY_HEAP: &str = "abcdefghijklmnopqrstuvwxy";

    // A long clean value that always lives on the heap.
    const LONG_CLEAN: &str = "abcdefghijklmnopqrstuvwxyz0123456789-._~ABCDEFG";

    // A long value needing encoding whose escaped form always lives on the heap.
    const LONG_ENCODED_INPUT: &str = "my post title/with?reserved=chars&and=more stuff";

    // A realistic templated path with several short, escaped string components.
    #[templated(template = "/orgs/{org}/users/{user}/posts/{post}", unredacted)]
    #[derive(Clone)]
    struct RequestPath {
        org: EscapedString,
        user: EscapedString,
        post: EscapedString,
    }

    #[library_benchmark]
    fn short_clean() -> EscapedString {
        EscapedString::escape(black_box(SHORT_CLEAN))
    }

    #[library_benchmark]
    fn short_encoded() -> EscapedString {
        EscapedString::escape(black_box(SHORT_ENCODED_INPUT))
    }

    #[library_benchmark]
    fn boundary_inline_24() -> EscapedString {
        EscapedString::escape(black_box(BOUNDARY_INLINE))
    }

    #[library_benchmark]
    fn boundary_heap_25() -> EscapedString {
        EscapedString::escape(black_box(BOUNDARY_HEAP))
    }

    #[library_benchmark]
    fn long_clean() -> EscapedString {
        EscapedString::escape(black_box(LONG_CLEAN))
    }

    #[library_benchmark]
    fn long_encoded() -> EscapedString {
        EscapedString::escape(black_box(LONG_ENCODED_INPUT))
    }

    // Build a templated struct from several short escaped components, as a client does for
    // every outgoing request: the per-request string allocations are incurred here.
    #[library_benchmark]
    fn request_construct() -> RequestPath {
        RequestPath {
            org: EscapedString::escape(black_box("contoso")),
            user: EscapedString::escape(black_box("Will_E_Coyote")),
            post: EscapedString::escape(black_box("hello-world")),
        }
    }

    library_benchmark_group!(
        name = escaped_string;
        benchmarks =
            short_clean,
            short_encoded,
            boundary_inline_24,
            boundary_heap_25,
            long_clean,
            long_encoded,
            request_construct
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::escaped_string;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = escaped_string
);
