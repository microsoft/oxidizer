// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `BytesView` read fast paths in the `bytesbuf` package.
//!
//! Paired with `view.rs`, which covers the same operations under wall-clock measurement.
//! The `slice_*` benchmarks pair with the Criterion `BytesView/slice_*` benchmarks; `get_byte`
//! and `get_num_le` pair with the per-element `BytesView/get_byte_drain` and
//! `BytesView/get_num_le_drain` benchmarks (the Callgrind variants isolate a single call).
//!
//! Each setup function builds the view outside the measured region and the benchmark returns its
//! state so the view's drop (and the `BlockRef` refcount traffic it triggers) is not counted.

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
    use std::iter;
    use std::num::NonZero;

    use bytesbuf::BytesView;
    use bytesbuf::mem::BlockSize;
    use bytesbuf::mem::testing::FixedBlockMemory;
    use gungraun::{library_benchmark, library_benchmark_group};
    use new_zealand::nz;

    // "HTTP request sized" single span, matching the Criterion benchmarks in `view.rs`.
    const TEST_SPAN_SIZE: NonZero<BlockSize> = nz!(12345);
    const TEST_DATA: &[u8] = &[88_u8; TEST_SPAN_SIZE.get() as usize];

    fn single_span() -> BytesView {
        let memory = FixedBlockMemory::new(TEST_SPAN_SIZE);
        BytesView::copied_from_slice(TEST_DATA, &memory)
    }

    fn ten_spans() -> BytesView {
        BytesView::from_views(iter::repeat_n(single_span(), 10))
    }

    // Range fully within the only span: the single-span fast path.
    #[library_benchmark]
    #[bench::single_span(single_span())]
    fn bytes_view_slice_near(view: BytesView) -> (BytesView, BytesView) {
        let sub = view.range(black_box(0..10));
        (view, sub)
    }

    // Range near the end of the only span: still the single-span fast path.
    #[library_benchmark]
    #[bench::single_span(single_span())]
    fn bytes_view_slice_far(view: BytesView) -> (BytesView, BytesView) {
        let sub = view.range(black_box(12_300..12_310));
        (view, sub)
    }

    // Range landing in the last of ten spans: the general cross-span path (fast path not taken).
    #[library_benchmark]
    #[bench::ten_spans(ten_spans())]
    fn bytes_view_slice_very_far(view: BytesView) -> (BytesView, BytesView) {
        let sub = view.range(black_box(123_000..123_010));
        (view, sub)
    }

    // Fused single-span read: front byte plus in-span advance in one span lookup.
    #[library_benchmark]
    #[bench::single_span(single_span())]
    fn bytes_view_get_byte(mut view: BytesView) -> (BytesView, u8) {
        let byte = view.get_byte();
        (view, black_box(byte))
    }

    // Fused single-span numeric read: the value never straddles a span boundary.
    #[library_benchmark]
    #[bench::single_span(single_span())]
    fn bytes_view_get_num_le(mut view: BytesView) -> (BytesView, u32) {
        let value = view.get_num_le::<u32>();
        (view, black_box(value))
    }

    library_benchmark_group!(
        name = bytes_view;
        benchmarks =
            bytes_view_slice_near, bytes_view_slice_far, bytes_view_slice_very_far,
            bytes_view_get_byte, bytes_view_get_num_le
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::bytes_view;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = bytes_view
);
