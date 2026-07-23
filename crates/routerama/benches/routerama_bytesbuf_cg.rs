// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind comparisons for Routerama `BytesView` vectored access.
//!
//! Paired with `routerama_bytesbuf.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(
    clippy::needless_pass_by_value,
    clippy::panic,
    reason = "prepared values delimit measured ownership and pending in-memory operations violate fixture invariants"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::prelude::*;

    include!("common/bytesbuf_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $count:ident) => {
            #[library_benchmark]
            #[bench::run(prepare_view(SpanCount::$count))]
            fn $name(prepared: PreparedView) -> Observation {
                std::hint::black_box(observe(std::hint::black_box(&prepared.view)))
            }
        };
    }

    benchmark_case!(buf_chunks_vectored_1_span, One);
    benchmark_case!(buf_chunks_vectored_3_spans, Three);
    benchmark_case!(buf_chunks_vectored_8_spans, Eight);
    benchmark_case!(buf_chunks_vectored_9_spans, Nine);
    benchmark_case!(buf_chunks_vectored_32_spans, ThirtyTwo);

    library_benchmark_group!(
        name = buf;
        benchmarks =
            buf_chunks_vectored_1_span,
            buf_chunks_vectored_3_spans,
            buf_chunks_vectored_8_spans,
            buf_chunks_vectored_9_spans,
            buf_chunks_vectored_32_spans
    );
}

#[cfg(target_os = "linux")]
pub use linux::buf;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = buf);
