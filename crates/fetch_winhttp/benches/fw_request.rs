// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures the per-request cost of the `fetch_winhttp` transport.
//!
//! The public API cannot separate transport work from WinHTTP work. Compare
//! nearby scenarios in the same group. For example, compare
//! `get_headers_high` with `get_minimal` to measure header translation. Compare
//! `get_known_high` with `get_known_low` to measure the per-byte read path.
//!
//! Request and response bodies use declared-length and unknown-length cases.
//! The transport supplies the total length for a declared-length request and
//! chunks an unknown-length request. A declared response length sets the read
//! size. An unknown-length response reads the available data.
//!
//! Processor-time and allocation measurements include only the benchmark
//! thread. They exclude WinHTTP worker threads and fixture runtime threads.
//! Elapsed time includes waits for WinHTTP and the fixture.

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
    /// Fits in one protocol frame and one transport read. This case measures
    /// mostly the fixed cost of a request.
    const LOW_BODY_LEN: usize = 1024;

    /// Payload size for the "high" leg of every parameterized body scenario.
    ///
    /// Spans several preferred-size reads. The difference from the low case
    /// measures mostly the per-byte path.
    const HIGH_BODY_LEN: usize = 1024 * 1024;

    /// Header count for the "low" leg of the header scenario.
    ///
    /// `fetch` and the transport add more headers. This count is the smallest
    /// value that a caller can provide.
    const LOW_HEADER_COUNT: usize = 2;

    /// Header count for the high case. It represents a request with tracing,
    /// authentication, and routing metadata.
    const HIGH_HEADER_COUNT: usize = 32;

    /// Chunk size the undeclared-length download fixture serves its payload in.
    ///
    /// The payload must use more than one chunk so `hyper` omits
    /// `Content-Length`. Each chunk is at least the preferred transport read
    /// size. Smaller chunks can make the fixture control the number of reads
    /// and distort the transport measurement. This scenario does not measure a
    /// fragmented arrival pattern.
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

        // The fixture has a self-signed certificate, so the client accepts it.
        // Schannel builds the chain once per connection, outside the measured
        // warm-request iterations.
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

    /// Downloads a declared-length response. The remaining length sets each
    /// read size and lets the transport request a full buffer.
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

    /// Downloads an unknown-length response. No declared remainder sets the
    /// read size.
    ///
    /// A chunked response omits `Content-Length`, so each read takes available
    /// data instead of waiting for a full buffer. Compare this case with
    /// [`download`] at the same payload size. Only the high case can use more
    /// than one chunk and preserve the unknown-length response shape.
    fn chunked_download(
        group: &mut BenchmarkGroup<'_, WallTime>,
        allocs: &AllocSession,
        time: &TimeSession,
        name: &'static str,
        len: usize,
    ) {
        // All chunks are slices of one `Bytes` allocation.
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

    /// Makes sure that the fixture serves the expected response body shape.
    ///
    /// `hyper` derives the length from the scripted frames. This check prevents
    /// the unknown-length scenario from becoming another declared-length
    /// measurement. It runs outside the measured region.
    ///
    /// A declared body must have `Content-Length` and no content or transfer
    /// encoding. An unknown-length body must use chunked framing, not connection
    /// close. The size check keeps both cases at the expected scale.
    fn assert_response_shape(test_client: &TestClient, url: &str, declared: bool, len: usize) {
        let response = block_on(test_client.client.get(url).fetch()).unwrap();
        let headers = response.headers().clone();
        // Drain the body so the measured iterations can reuse the connection.
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

    /// Makes sure that the transport sends the expected request body shape.
    ///
    /// This is the upload counterpart of [`assert_response_shape`]. It detects
    /// changes in how `fetch` derives a body length.
    ///
    /// Repeating fixtures do not record requests because large uploads can
    /// distort allocation results. This check uses a separate recording
    /// fixture and connection.
    fn assert_request_shape(declared: bool, len: usize, send: impl FnOnce(&str)) {
        let server = TestServer::http([ResponsePlan::ok("")]);
        let url = server.url("/shape");
        send(&url);
        let snapshot = server.finish();

        let request = snapshot.requests.first().expect("the shape fixture served no request");
        let declared_length = request
            .headers
            .get(CONTENT_LENGTH)
            .map(|value| value.to_str().unwrap().parse::<usize>().unwrap());

        assert_eq!(
            declared_length.is_some(),
            declared,
            "the transport no longer sends the expected body shape"
        );
        assert_eq!(
            request.headers.contains_key(TRANSFER_ENCODING),
            !declared,
            "the transport no longer frames the body as expected"
        );
        if let Some(declared_length) = declared_length {
            assert_eq!(declared_length, len, "the declared length disagrees with the body sent");
        }
        assert_eq!(request.body.len(), len, "the transport sent a different payload size");
    }

    /// Uploads a known-length body with `Content-Length`.
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
        assert_request_shape(true, len, |shape_url| {
            drop(block_on(test_client.client.post(shape_url).text(&payload).fetch_text_body()).unwrap());
        });

        measure(group, allocs, time, name, |_| async {
            let request = test_client.client.post(url.as_str()).text(&payload);
            black_box(request.fetch_text_body().await.unwrap());
        });
        drop(server.finish());
    }

    /// Uploads an unknown-length body with automatic chunking and a final
    /// zero-length write.
    ///
    /// Each iteration creates the streamed body from a [`BytesView`]. This work
    /// stays in the measured region because callers also pay this cost. The
    /// result measures the full caller cost, not only transport work.
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
        assert_request_shape(false, len, |shape_url| {
            let frame = BytesView::copied_from_slice(&payload, &test_client.body_builder);
            let body = test_client
                .body_builder
                .stream(futures::stream::iter([Ok(frame)]), &HttpBodyOptions::default());
            drop(block_on(test_client.client.post(shape_url).body(body).fetch_text_body()).unwrap());
        });

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

    /// Builds a client and sends one warm-up request. Every measured iteration
    /// reuses the established connection.
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

                // Enter the executor once per sample. Each iteration awaits one
                // request.
                let mut pool = LocalPool::new();
                pool.run_until(time_sample_async(iters, operation))
            });
        });
    }
}
