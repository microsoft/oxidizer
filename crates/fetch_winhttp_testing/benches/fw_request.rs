// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-request cost of the `fetch_winhttp` transport, measured against a localhost fixture.
//!
//! The transport's own work is not separable from WinHTTP's through the public API, so each
//! scenario is read by differencing it against a neighbour in the same group: subtracting
//! `get_minimal` from `get_headers_high` isolates header translation, subtracting `get_body_low`
//! from `get_body_high` isolates the per-byte response read path, and so on.
//!
//! Only the benchmark thread is measured. WinHTTP completes I/O on its own worker threads and the
//! fixture serves on its own runtime threads, so neither appears in the processor-time or
//! allocation figures - which is what makes an in-process fixture usable here at all.

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

    use all_the_time::Session as TimeSession;
    use alloc_tracker::Session as AllocSession;
    use benchmarking::time_sample;
    use bytesbuf::BytesView;
    use criterion::measurement::WallTime;
    use criterion::{BenchmarkGroup, Criterion};
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_testing::{Http3Server, ResponsePlan, TestClient, TestServer, client};
    use futures::executor::block_on;
    use http::{HeaderName, HeaderValue, Version};
    use http_extensions::HttpBodyOptions;

    /// Payload size for the "low" leg of every parameterized body scenario.
    ///
    /// Small enough to fit a single protocol frame and a single transport read, so the leg reports
    /// the fixed per-request cost with almost no per-byte component.
    const LOW_BODY_LEN: usize = 1024;

    /// Payload size for the "high" leg of every parameterized body scenario.
    ///
    /// Large enough to span several reads at the transport's preferred read size, so the difference
    /// against the low leg is dominated by the per-byte path.
    const HIGH_BODY_LEN: usize = 1024 * 1024;

    /// Header count for the "low" leg of the header scenario.
    ///
    /// `fetch` and the transport contribute headers of their own, so a request never carries only
    /// these; this is the floor a caller can express.
    const LOW_HEADER_COUNT: usize = 2;

    /// Header count for the "high" leg of the header scenario, resembling a request carrying a full
    /// set of tracing, authentication and routing metadata.
    const HIGH_HEADER_COUNT: usize = 32;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        let allocs = AllocSession::new();
        let time = TimeSession::new();

        roundtrip(c, &allocs, &time);
        protocol(c, &allocs, &time);

        // `alloc_tracker::Session` reports on drop; `all_the_time::Session` has to be asked.
        time.print_to_stdout();
        drop(allocs);
    }

    fn roundtrip(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_request/roundtrip");

        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/minimal");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        measure(&mut group, allocs, time, "get_minimal", || {
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap());
        });
        drop(server.finish());

        headers(&mut group, allocs, time, "get_headers_low", LOW_HEADER_COUNT);
        headers(&mut group, allocs, time, "get_headers_high", HIGH_HEADER_COUNT);

        download(&mut group, allocs, time, "get_body_low", LOW_BODY_LEN);
        download(&mut group, allocs, time, "get_body_high", HIGH_BODY_LEN);

        known_length_upload(&mut group, allocs, time, "post_known_low", LOW_BODY_LEN);
        known_length_upload(&mut group, allocs, time, "post_known_high", HIGH_BODY_LEN);

        unknown_length_upload(&mut group, allocs, time, "post_unknown_high", HIGH_BODY_LEN);

        group.finish();
    }

    fn protocol(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_request/protocol");

        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/h1");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        measure(&mut group, allocs, time, "h1_plaintext", || {
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap());
        });
        drop(server.finish());

        // The fixture presents a self-signed certificate, so validation has to be relaxed for the
        // handshake to complete. Chain building is Schannel's work and happens once per connection
        // rather than once per request, so relaxing it does not distort the measured iterations.
        let tls = WinHttpTlsConfig::builder().accept_invalid_certs(true).build();
        let server = TestServer::https_repeating(ResponsePlan::ok(""), &["localhost"]);

        let url = server.url("/h1-tls");
        let test_client = warmed_client(&[Version::HTTP_11], tls.clone(), &url);
        measure(&mut group, allocs, time, "h1_tls", || {
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap());
        });

        let url = server.url("/h2-tls");
        let test_client = warmed_client(&[Version::HTTP_2], tls, &url);
        measure(&mut group, allocs, time, "h2_tls", || {
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap());
        });
        drop(server.finish());

        let server = Http3Server::start_repeating(ResponsePlan::ok(""));
        let url = server.url("/h3");
        let test_client = warmed_client(
            &[Version::HTTP_3],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
            &url,
        );
        measure(&mut group, allocs, time, "h3_quic", || {
            black_box(block_on(test_client.client.get(url.as_str()).fetch_text_body()).unwrap());
        });
        drop(server.finish());

        group.finish();
    }

    fn headers(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &AllocSession, time: &TimeSession, name: &'static str, count: usize) {
        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/headers");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        let headers = (0..count)
            .map(|index| {
                (
                    HeaderName::try_from(format!("x-bench-header-{index}")).unwrap(),
                    HeaderValue::from_static("benchmark header value"),
                )
            })
            .collect::<Vec<_>>();

        measure(group, allocs, time, name, || {
            let mut request = test_client.client.get(url.as_str());

            for (name, value) in &headers {
                request = request.header(name.clone(), value.clone());
            }

            black_box(block_on(request.fetch_text_body()).unwrap());
        });
        drop(server.finish());
    }

    fn download(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &AllocSession, time: &TimeSession, name: &'static str, len: usize) {
        let server = TestServer::http_repeating(ResponsePlan::ok(vec![b'r'; len]));
        let url = server.url("/download");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);

        measure(group, allocs, time, name, || {
            let response = block_on(test_client.client.get(url.as_str()).fetch()).unwrap();
            black_box(block_on(response.into_body().into_bytes()).unwrap());
        });
        drop(server.finish());
    }

    fn known_length_upload(
        group: &mut BenchmarkGroup<'_, WallTime>,
        allocs: &AllocSession,
        time: &TimeSession,
        name: &'static str,
        len: usize,
    ) {
        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/upload");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        let payload = "u".repeat(len);

        measure(group, allocs, time, name, || {
            let request = test_client.client.post(url.as_str()).text(&payload);
            black_box(block_on(request.fetch_text_body()).unwrap());
        });
        drop(server.finish());
    }

    /// The unknown-length counterpart of [`known_length_upload`], covering the automatic chunking
    /// path and its terminating zero-length write.
    ///
    /// The two are not a pure A/B: a streamed body is assembled per iteration from a [`BytesView`],
    /// which a text body does not need. That construction stays inside the measured region because
    /// a caller streaming a body pays it too, so the difference between the two reflects the
    /// caller's full cost rather than the transport's alone.
    fn unknown_length_upload(
        group: &mut BenchmarkGroup<'_, WallTime>,
        allocs: &AllocSession,
        time: &TimeSession,
        name: &'static str,
        len: usize,
    ) {
        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/stream");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        let payload = vec![b's'; len];

        measure(group, allocs, time, name, || {
            let frame = BytesView::copied_from_slice(&payload, &test_client.body_builder);
            let body = test_client
                .body_builder
                .stream(futures::stream::iter([Ok(frame)]), &HttpBodyOptions::default());

            let request = test_client.client.post(url.as_str()).body(body);
            black_box(block_on(request.fetch_text_body()).unwrap());
        });
        drop(server.finish());
    }

    /// Builds a client and issues one request through it, so that the measured iterations all run
    /// against an established pooled connection instead of one of them paying connection setup.
    fn warmed_client(versions: &[Version], tls: WinHttpTlsConfig, url: &str) -> TestClient {
        let test_client = client(versions, tls);
        drop(block_on(test_client.client.get(url).fetch_text_body()).unwrap());
        test_client
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
