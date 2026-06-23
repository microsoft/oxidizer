// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for `BytesBuf` write fast paths in the `bytesbuf` package.
//!
//! Paired with `buf.rs`, which covers the same operations under wall-clock measurement. The
//! `put_slice` and `put_num_le` benchmarks pair with the Criterion `BytesBuf/put_slice` and
//! `BytesBuf/put_num_le` benchmarks (the Callgrind variants isolate a single write).
//!
//! Each setup function reserves the destination capacity outside the measured region so the
//! timed body exercises only the fused write into the first unfilled slice, never allocation.

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

    use bytesbuf::BytesBuf;
    use bytesbuf::mem::testing::TransparentMemory;
    use gungraun::{library_benchmark, library_benchmark_group};

    // A single block large enough that small writes always fit in the first unfilled slice.
    const RESERVE_BYTES: usize = 4096;
    const WRITE: &[u8] = &[0xAB; 16];

    fn reserved() -> BytesBuf {
        TransparentMemory::new().reserve(RESERVE_BYTES)
    }

    // Fused single-slice write of a small slice.
    #[library_benchmark]
    #[bench::single_span(reserved())]
    fn bytes_buf_put_slice(mut buf: BytesBuf) -> BytesBuf {
        buf.put_slice(black_box(WRITE));
        buf
    }

    // Fused single-slice numeric write.
    #[library_benchmark]
    #[bench::single_span(reserved())]
    fn bytes_buf_put_num_le(mut buf: BytesBuf) -> BytesBuf {
        buf.put_num_le(black_box(0x1234_5678_u32));
        buf
    }

    library_benchmark_group!(
        name = bytes_buf;
        benchmarks = bytes_buf_put_slice, bytes_buf_put_num_le
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::bytes_buf;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = bytes_buf
);
