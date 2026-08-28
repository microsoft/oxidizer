// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-request cost of the `fetch_winhttp` transport, measured against a localhost fixture.
//!
//! The transport's own work is not separable from WinHTTP's through the public API, so each
//! scenario is read by differencing it against a neighbour in the same group: subtracting
//! `get_minimal` from `get_headers_high` isolates header translation, subtracting `get_known_low`
//! from `get_known_high` isolates the per-byte response read path, and so on.
//!
//! Request and response bodies are each covered in both of the shapes the transport treats
//! differently: a declared `Content-Length` and an undeclared length. The two are separate paths on
//! both sides - a declared request length is written in one pass while an undeclared one is
//! chunked, and a declared response length sizes the transport's reads while an undeclared one
//! leaves it reading whatever has arrived.
//!
//! Only the benchmark thread is measured. WinHTTP completes I/O on its own worker threads and the
//! fixture serves on its own runtime threads, so neither appears in the processor-time or
//! allocation figures - which is what makes an in-process fixture usable here at all.

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
    use benchmarking::time_sample_async;
    use bytes::Bytes;
    use bytesbuf::BytesView;
    use criterion::measurement::WallTime;
    use criterion::{BenchmarkGroup, Criterion};
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{Http3Server, ResponsePlan, TestClient, TestServer, client};
    use futures::executor::{LocalPool, block_on};
    use http::header::{CONTENT_ENCODING, CONTENT_LENGTH, TRANSFER_ENCODING};
    use http::{HeaderName, HeaderValue, Version};
    use http_extensions::HttpBodyOptions;
    use tick::Clock;

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

    /// Chunk size the undeclared-length download fixture serves its payload in.
    ///
    /// The payload must span more than one chunk, which is what makes `hyper` pick chunked
    /// transfer-encoding and withhold a `Content-Length`. Beyond that the chunk should be no
    /// smaller than the transport's own preferred read size, because an undeclared-length read
    /// takes whatever has arrived rather than waiting for a full region: smaller chunks let the
    /// fixture's write granularity decide how many reads the transfer costs, which measures the
    /// fixture rather than the transport. Serving in chunks this size keeps the fixture from
    /// forcing extra reads, leaving a saturated comparison against the declared-length scenario.
    /// How a fragmented arrival pattern performs is a separate question this scenario does not
    /// answer.
    const RESPONSE_CHUNK_LEN: usize = 256 * 1024;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        let allocs = AllocSession::new();
        let time = TimeSession::new();

        roundtrip(c, &allocs, &time);
        protocol(c, &allocs, &time);

        // Both sessions print their tables when dropped.
    }

    fn roundtrip(c: &mut Criterion, allocs: &AllocSession, time: &TimeSession) {
        let mut group = c.benchmark_group("fw_request/roundtrip");

        let server = TestServer::http_repeating(ResponsePlan::ok(""));
        let url = server.url("/minimal");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        measure(&mut group, allocs, time, "get_minimal", |_| async {
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap());
        });
        drop(server.finish());

        headers(&mut group, allocs, time, "get_headers_low", LOW_HEADER_COUNT);
        headers(&mut group, allocs, time, "get_headers_high", HIGH_HEADER_COUNT);

        download(&mut group, allocs, time, "get_known_low", LOW_BODY_LEN);
        download(&mut group, allocs, time, "get_known_high", HIGH_BODY_LEN);

        chunked_download(&mut group, allocs, time, "get_unknown_high", HIGH_BODY_LEN);

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
        measure(&mut group, allocs, time, "h1_plaintext", |_| async {
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap());
        });
        drop(server.finish());

        // The fixture presents a self-signed certificate, so validation has to be relaxed for the
        // handshake to complete. Chain building is Schannel's work and happens once per connection
        // rather than once per request, so relaxing it does not distort the measured iterations.
        let tls = WinHttpTlsConfig::builder().accept_invalid_certs(true).build();
        let server = TestServer::https_repeating(ResponsePlan::ok(""), &["localhost"]);

        let url = server.url("/h1-tls");
        let test_client = warmed_client(&[Version::HTTP_11], tls.clone(), &url);
        measure(&mut group, allocs, time, "h1_tls", |_| async {
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap());
        });

        let url = server.url("/h2-tls");
        let test_client = warmed_client(&[Version::HTTP_2], tls, &url);
        measure(&mut group, allocs, time, "h2_tls", |_| async {
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap());
        });
        drop(server.finish());

        let server = Http3Server::start_repeating(ResponsePlan::ok(""));
        let url = server.url("/h3");
        let test_client = warmed_client(
            &[Version::HTTP_3],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
            &url,
        );
        measure(&mut group, allocs, time, "h3_quic", |_| async {
            black_box(test_client.client.get(url.as_str()).fetch_text_body().await.unwrap());
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

        measure(group, allocs, time, name, |_| async {
            let mut request = test_client.client.get(url.as_str());

            for (name, value) in &headers {
                request = request.header(name.clone(), value.clone());
            }

            black_box(request.fetch_text_body().await.unwrap());
        });
        drop(server.finish());
    }

    /// Downloads a response whose length the headers declare, which lets the transport size each
    /// read from the remaining count and demand a full buffer.
    fn download(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &AllocSession, time: &TimeSession, name: &'static str, len: usize) {
        let server = TestServer::http_repeating(ResponsePlan::ok(vec![b'r'; len]));
        let url = server.url("/download");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        assert_response_shape(&test_client, &url, true, len);

        measure(group, allocs, time, name, |_| async {
            let response = test_client.client.get(url.as_str()).fetch().await.unwrap();
            black_box(response.into_body().into_bytes().await.unwrap());
        });
        drop(server.finish());
    }

    /// The undeclared-length counterpart of [`download`], covering the response read path that has
    /// no declared remainder to size its reads from.
    ///
    /// A chunked response withholds `Content-Length`, so each read takes whatever has already
    /// arrived instead of waiting for a full buffer. Differencing this against [`download`] at the
    /// same payload size therefore isolates what reading an undeclared body costs. Only the high
    /// leg is measured: a payload at the low size fits within a single chunk, and a single-frame
    /// response is served with a declared length, so a low leg could not express this shape.
    fn chunked_download(
        group: &mut BenchmarkGroup<'_, WallTime>,
        allocs: &AllocSession,
        time: &TimeSession,
        name: &'static str,
        len: usize,
    ) {
        // One buffer sliced per chunk rather than one allocation per chunk; `Bytes` slices share
        // the same storage.
        let payload = Bytes::from(vec![b'r'; RESPONSE_CHUNK_LEN]);
        let mut chunks = Vec::new();
        let mut remaining = len;
        while remaining > 0 {
            let take = remaining.min(RESPONSE_CHUNK_LEN);
            chunks.push(payload.slice(0..take));
            remaining -= take;
        }
        assert!(chunks.len() > 1, "a single-frame response is served with a declared length");

        let server = TestServer::http_repeating(ResponsePlan::chunks(chunks));
        let url = server.url("/chunked-download");
        let test_client = warmed_client(&[Version::HTTP_11], WinHttpTlsConfig::default(), &url);
        assert_response_shape(&test_client, &url, false, len);

        measure(group, allocs, time, name, |_| async {
            let response = test_client.client.get(url.as_str()).fetch().await.unwrap();
            black_box(response.into_body().into_bytes().await.unwrap());
        });
        drop(server.finish());
    }

    /// Confirms the fixture serves the response body shape its scenario is named for.
    ///
    /// Whether a response declares its length is decided by `hyper` from the frames the plan
    /// scripts, not stated by the plan, so a change to either could quietly turn the undeclared
    /// scenario into a second measurement of the declared one. Checking here fails the benchmark
    /// instead, and runs outside the measured region. The request side needs no counterpart: which
    /// shape the transport sends follows directly from the body the caller hands it.
    ///
    /// The transport takes a length as declared only when the response carries one and carries
    /// neither a content nor a transfer encoding, so all three are checked rather than just the
    /// length. An undeclared body is additionally required to be chunked, because a body delimited
    /// by connection close would also present no length but would cost the following iteration a
    /// new connection. Asserting the delivered size catches a fixture that serves the right shape
    /// at the wrong scale.
    fn assert_response_shape(test_client: &TestClient, url: &str, declared: bool, len: usize) {
        let response = block_on(test_client.client.get(url).fetch()).unwrap();
        let headers = response.headers().clone();
        // Drained rather than dropped, so the pooled connection survives for the measured
        // iterations instead of being reset mid-body.
        let body = block_on(response.into_body().into_bytes()).unwrap();

        assert_eq!(
            headers.contains_key(CONTENT_LENGTH),
            declared,
            "the fixture at {url} no longer serves the scripted body shape"
        );
        assert_eq!(
            headers.contains_key(TRANSFER_ENCODING),
            !declared,
            "the fixture at {url} no longer frames the scripted body shape"
        );
        assert!(
            !headers.contains_key(CONTENT_ENCODING),
            "the fixture at {url} encodes the body, which withholds the length from the transport"
        );
        assert_eq!(body.len(), len, "the fixture at {url} no longer serves the scripted payload size");
    }

    /// Uploads a body whose length is known up front, which the transport declares with a
    /// `Content-Length` and writes in a single pass.
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

        measure(group, allocs, time, name, |_| async {
            let request = test_client.client.post(url.as_str()).text(&payload);
            black_box(request.fetch_text_body().await.unwrap());
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

        measure(group, allocs, time, name, |_| async {
            let frame = BytesView::copied_from_slice(&payload, &test_client.body_builder);
            let body = test_client
                .body_builder
                .stream(futures::stream::iter([Ok(frame)]), &HttpBodyOptions::default());

            let request = test_client.client.post(url.as_str()).body(body);
            black_box(request.fetch_text_body().await.unwrap());
        });
        drop(server.finish());
    }

    /// Builds a client and issues one request through it, so that the measured iterations all run
    /// against an established pooled connection instead of one of them paying connection setup.
    fn warmed_client(versions: &[Version], tls: WinHttpTlsConfig, url: &str) -> TestClient {
        let test_client = client(versions, tls, Clock::new_frozen());
        drop(block_on(test_client.client.get(url).fetch_text_body()).unwrap());
        test_client
    }

    fn measure<F, Fut, R>(
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
