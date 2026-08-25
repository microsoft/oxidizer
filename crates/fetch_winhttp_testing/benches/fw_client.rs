// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Client-lifecycle cost of the `fetch_winhttp` transport: the work that happens once per client
//! rather than once per request, plus the cost of a request that never reaches a server.
//!
//! Only the benchmark thread is measured, so WinHTTP's own worker threads and the fixture's runtime
//! threads are excluded from the processor-time and allocation figures.

#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![cfg_attr(
    not(windows),
    allow(
        unused_crate_dependencies,
        reason = "the transport under test only exists on Windows, so the benchmark body is compiled away"
    )
)]

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    #[cfg(windows)]
    windows::entrypoint(c);

    #[cfg(not(windows))]
    {
        _ = c;
    }
}

#[cfg(windows)]
mod windows {
    use std::hint::black_box;
    use std::net::{Ipv4Addr, TcpListener};

    use all_the_time::Session as TimeSession;
    use alloc_tracker::Session as AllocSession;
    use benchmarking::time_sample;
    use criterion::measurement::WallTime;
    use criterion::{BenchmarkGroup, Criterion};
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_testing::{ResponsePlan, TestServer, client, client_builder};
    use futures::executor::block_on;
    use http::Version;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        let allocs = AllocSession::new();
        let time = TimeSession::new();

        construction(c, &allocs, &time);
        error_path(c, &allocs, &time);

        // `alloc_tracker::Session` reports on drop; `all_the_time::Session` has to be asked.
        time.print_to_stdout();
        drop(allocs);
    }

    fn construction(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_client/construction");

        measure(&mut group, allocs, time, "build", || {
            let (builder, body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
            black_box((builder.build(), body_builder));
        });

        let template = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
        measure(&mut group, allocs, time, "clone", || {
            black_box(template.client.clone());
        });
        drop(template);

        // A client opens its WinHTTP session lazily, so the first request through a fresh client
        // additionally pays session creation, callback registration and connection setup. The
        // difference against `fw_request/roundtrip/get_minimal` is that whole cold path.
        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/first");
        measure(&mut group, allocs, time, "first_request", || {
            let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap());
        });
        drop(server.finish());

        group.finish();
    }

    fn error_path(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_client/error_path");

        // Binding and immediately releasing a port yields an address nothing listens on. Loopback
        // answers such a connection attempt with an immediate reset, so the request fails without
        // waiting for any timer - no benchmark iteration depends on elapsed real time.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("http://127.0.0.1:{port}/refused");

        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
        measure(&mut group, allocs, time, "connect_refused", || {
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap_err());
        });

        group.finish();
    }

    fn measure(
        group: &mut BenchmarkGroup<'_, WallTime>,
        allocs: &AllocSession,
        time: &TimeSession,
        name: &'static str,
        operation: impl Fn(),
    ) {
        let allocs_operation = allocs.operation(name);
        let time_operation = time.operation(name);

        group.bench_function(name, |bencher| {
            bencher.iter_custom(|iters| {
                let _allocs = allocs_operation.measure_thread().iterations(iters);
                let _time = time_operation.measure_thread().iterations(iters);

                time_sample(iters, &operation)
            });
        });
    }
}
