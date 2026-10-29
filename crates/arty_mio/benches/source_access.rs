// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures direct source access through a registration.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(clippy::unwrap_used, reason = "benchmark fixture failures provide sufficient context")]

use std::hint::black_box;
use std::io::{self, IoSlice};
use std::task::Waker;

use arty_io_core::{DriverOptions, DriverRole, IoContext};
use arty_mio::{MioContext, MioDriver, MioOptions, Registered};
use criterion::Criterion;
use mio::event::Source;
use mio::{Interest, Registry, Token};
use thread_aware::ThreadBuilder;
use thread_aware_core::ThreadAware;

const READ_GROUP: &str = "arty_mio/source_access/read";
const WRITE_GROUP: &str = "arty_mio/source_access/write";
const VECTORED_WRITE_GROUP: &str = "arty_mio/source_access/write_vectored";
const BUFFER: [u8; 64] = [42; 64];

struct SourceAccessState {
    registration: Registered<BenchSource>,
    _driver: MioDriver,
    destination: [u8; BUFFER.len()],
}

#[derive(Default)]
struct BenchSource {
    bytes: usize,
}

impl BenchSource {
    fn read(&mut self, destination: &mut [u8]) -> usize {
        destination.copy_from_slice(black_box(&BUFFER));
        self.bytes = destination.len();
        self.bytes
    }

    fn write(&mut self, source: &[u8]) -> usize {
        self.bytes = black_box(source).len();
        self.bytes
    }

    fn write_vectored(&mut self, sources: &[IoSlice<'_>]) -> usize {
        self.bytes = black_box(sources).iter().map(|source| source.len()).sum();
        self.bytes
    }
}

impl Source for BenchSource {
    fn register(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        Ok(())
    }

    fn reregister(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        Ok(())
    }

    fn deregister(&mut self, _registry: &Registry) -> io::Result<()> {
        Ok(())
    }
}

fn context() -> (MioContext, MioDriver) {
    let mut io = MioContext::context(MioOptions::default());
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    io.relocate(None, &thread);
    let driver = io
        .driver(DriverOptions::new(thread, Waker::noop().clone(), DriverRole::Primary))
        .unwrap();
    (io, driver)
}

fn source_access_state() -> SourceAccessState {
    let (io, driver) = context();
    let registration = io.register(BenchSource::default(), Interest::READABLE).unwrap();
    SourceAccessState {
        registration,
        _driver: driver,
        destination: [0; BUFFER.len()],
    }
}

#[metabench::benchmark(READ_DIRECT, READ_GROUP, "direct")]
#[bench::source(Box::new(source_access_state()))]
fn read_direct<S: std::borrow::BorrowMut<SourceAccessState>>(mut owned_input: S) -> (usize, S) {
    let state = owned_input.borrow_mut();
    let result = state.registration.source_mut().unwrap().read(black_box(&mut state.destination));
    (result, owned_input)
}

#[metabench::benchmark(WRITE_DIRECT, WRITE_GROUP, "direct")]
#[bench::source(Box::new(source_access_state()))]
fn write_direct<S: std::borrow::BorrowMut<SourceAccessState>>(mut owned_input: S) -> (usize, S) {
    let state = owned_input.borrow_mut();
    let result = state.registration.source_mut().unwrap().write(black_box(&BUFFER));
    (result, owned_input)
}

#[metabench::benchmark(VECTORED_WRITE_DIRECT, VECTORED_WRITE_GROUP, "direct")]
#[bench::source(Box::new(source_access_state()))]
fn write_vectored_direct<S: std::borrow::BorrowMut<SourceAccessState>>(mut owned_input: S) -> (usize, S) {
    let state = owned_input.borrow_mut();
    let buffers = [IoSlice::new(&BUFFER[..16]), IoSlice::new(&BUFFER[16..])];
    let result = state.registration.source_mut().unwrap().write_vectored(black_box(&buffers));
    (result, owned_input)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut read = criterion.benchmark_group(READ_GROUP);
    let mut direct = source_access_state();
    read.bench_function(READ_DIRECT.benchmark_name(), |bencher| {
        bencher.iter(|| black_box(read_direct(black_box(&mut direct)).0));
    });
    read.finish();

    let mut write = criterion.benchmark_group(WRITE_GROUP);
    let mut direct = source_access_state();
    write.bench_function(WRITE_DIRECT.benchmark_name(), |bencher| {
        bencher.iter(|| black_box(write_direct(black_box(&mut direct)).0));
    });
    write.finish();

    let mut vectored = criterion.benchmark_group(VECTORED_WRITE_GROUP);
    let mut direct = source_access_state();
    vectored.bench_function(VECTORED_WRITE_DIRECT.benchmark_name(), |bencher| {
        bencher.iter(|| black_box(write_vectored_direct(black_box(&mut direct)).0));
    });
    vectored.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [READ_DIRECT, WRITE_DIRECT, VECTORED_WRITE_DIRECT],
    allocator = std::alloc::System,
);
