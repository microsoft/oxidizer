// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures raw TCP flush overhead.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::panic,
    clippy::unwrap_used,
    reason = "benchmark fixture failures provide sufficient context"
)]

use std::hint::black_box;
use std::io::{self, IoSlice};
use std::mem::MaybeUninit;
use std::task::{Context, Poll};

use criterion::Criterion;

#[expect(dead_code, unreachable_pub, reason = "the benchmark reuses the production stream core")]
#[path = "../src/stream.rs"]
mod production;

use production::{StreamCore, StreamIo};

const FLUSH_GROUP: &str = "fetch_mio_flush/raw_tcp";

const fn flush_input() -> RawTcpIo {
    RawTcpIo
}

#[metabench::benchmark(POLL_FLUSH, FLUSH_GROUP, "poll_flush")]
#[bench::immediate(flush_input())]
fn poll_flush(io: RawTcpIo) -> RawTcpIo {
    let Poll::Ready(result) = StreamCore::<RawTcpIo>::poll_flush() else {
        panic!("raw TCP flush must complete immediately");
    };
    result.unwrap();
    io
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(FLUSH_GROUP);
    group.bench_function(POLL_FLUSH.benchmark_name(), |bencher| {
        bencher.iter(|| black_box(StreamCore::<RawTcpIo>::poll_flush()));
    });
    group.finish();
}

struct RawTcpIo;

impl StreamIo for RawTcpIo {
    fn read(&mut self, _buffer: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        unreachable!()
    }

    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        unreachable!()
    }

    fn write_vectored(&mut self, _buffers: &[IoSlice<'_>]) -> io::Result<usize> {
        unreachable!()
    }

    fn shutdown(&self) -> io::Result<()> {
        unreachable!()
    }

    fn poll_read_ready(&self, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        unreachable!()
    }

    fn poll_write_ready(&self, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        unreachable!()
    }
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [POLL_FLUSH],
    allocator = std::alloc::System,
);
