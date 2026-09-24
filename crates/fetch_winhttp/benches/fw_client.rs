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
        clippy::needless_pass_by_ref_mut,
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

    use all_the_time::Session as TimeSession;
    use alloc_tracker::Session as AllocSession;
    use benchmarking::{time_sample, time_sample_async};
    use criterion::measurement::WallTime;
    use criterion::{BenchmarkGroup, Criterion};
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{ResetServer, ResponsePlan, TestServer, client, client_builder};
    use futures::executor::LocalPool;
    use http::Version;
    use tick::Clock;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        let allocs = AllocSession::new();
        let time = TimeSession::new();

        construction(c, &allocs, &time);
        error_path(c, &allocs, &time);

        // Both sessions print their tables when dropped.
    }

    fn construction(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_client/construction");

        measure_sync(&mut group, allocs, time, "build", || {
            let (builder, body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
            black_box((builder.build(), body_builder));
        });

        let template = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
        measure_sync(&mut group, allocs, time, "clone", || {
            black_box(template.client.clone());
        });
        drop(template);

        // Building a fresh client materializes the initial per-thread transport (and
        // its WinHTTP session) immediately; the first request then additionally pays
        // connection setup. The difference against `fw_request/roundtrip/get_minimal`
        // is that whole cold path.
        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/first");
        measure_async(&mut group, allocs, time, "first_request", |_| async {
            let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap());
        });
        drop(server.finish());

        group.finish();
    }

    fn error_path(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_client/error_path");

        // The fixture accepts the connection and resets it, so the request fails
        // on a reset that is already in flight rather than on any timer.
        let server = ResetServer::start();
        let url = server.url("/reset");

        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
        measure_async(&mut group, allocs, time, "connection_reset", |_| async {
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap_err());
        });

        group.finish();
    }

    fn measure_sync(
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

    fn measure_async<F, Fut, R>(
        group: &mut BenchmarkGroup<'_, WallTime>,
        allocs: &AllocSession,
        time: &TimeSession,
        name: &'static str,
        operation: F,
    ) where
        F: Fn(u64) -> Fut + Copy,
        Fut: Future<Output = R>,
    {
        let allocs_operation = allocs.operation(name);
        let time_operation = time.operation(name);

        group.bench_function(name, |bencher| {
            bencher.iter_custom(|iters| {
                let _allocs = allocs_operation.measure_thread().iterations(iters);
                let _time = time_operation.measure_thread().iterations(iters);

                // One executor entry for the whole sample; each iteration only awaits the request.
                let mut pool = LocalPool::new();
                pool.run_until(time_sample_async(iters, operation))
            });
        });
    }
}
