// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures registration on the primary driver owner thread.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(clippy::unwrap_used, reason = "benchmark fixture failures provide sufficient context")]

use std::hint::black_box;
use std::task::Waker;

use arty_io_core::{DriverOptions, DriverRole, IoContext};
use arty_mio::{MioContext, MioDriver, MioOptions, Registered};
use criterion::{BatchSize, Criterion};
use mio::Interest;
use mio::net::UdpSocket;
use thread_aware::ThreadBuilder;
use thread_aware_core::ThreadAware;

const GROUP: &str = "arty_mio/registration";

struct RegistrationState {
    io: MioContext,
    _driver: MioDriver,
    source: Option<UdpSocket>,
}

fn registration_state() -> RegistrationState {
    let mut io = MioContext::context(MioOptions::default());
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    io.relocate(None, &thread);
    let driver = io
        .driver(DriverOptions::new(thread, Waker::noop().clone(), DriverRole::Primary))
        .unwrap();
    let source = UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    RegistrationState {
        io,
        _driver: driver,
        source: Some(source),
    }
}

#[metabench::benchmark(REGISTER, GROUP, "register")]
#[bench::primary_owner(Box::new(registration_state()))]
fn register<S: std::borrow::BorrowMut<RegistrationState>>(mut owned_input: S) -> (Registered<UdpSocket>, S) {
    let state = owned_input.borrow_mut();
    let source = state.source.take().unwrap();
    let registration = state.io.register(source, Interest::READABLE).unwrap();
    (registration, owned_input)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(GROUP);
    group.bench_function(REGISTER.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(registration_state, |state| register(black_box(state)).0, BatchSize::PerIteration);
    });
    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [REGISTER],);
