// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Probes whether native `WinHTTP` can perform full-duplex HTTP/2 request/response streaming:
//! continuing `WinHttpWriteData` uploads while `WinHttpReceiveResponse`/`WinHttpReadData` observe
//! the response on the same request handle.
//!
//! It also probes the unknown-length case that matters for gRPC/client-streaming uploads, whose
//! total size is never known up front: requests sent with `dwTotalLength` set to the
//! `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` sentinel on a request handle opened with
//! `WINHTTP_FLAG_AUTOMATIC_CHUNKING` - the exact native flag/total-length lowering
//! `fetch_winhttp_impl` uses for an unknown-length body (`crates/fetch_winhttp_impl/src/body/write.rs`
//! and `request.rs`, as landed by PR #687), and never a manually added `Transfer-Encoding` header.
//! An earlier version of this probe instead added `Transfer-Encoding: chunked` by hand, which is
//! not the API `fetch_winhttp_impl` uses and produced a false negative result; see
//! `docs/full-duplex-streaming-experiment.md` for why that probe was invalid and what the
//! corrected one found. See that same document for the full empirical record and its implications
//! for gRPC-style duplex support over `WinHTTP`.

#[cfg(not(windows))]
fn main() {
    eprintln!("This WinHTTP experiment only runs on Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows::run()
}

#[cfg(windows)]
mod windows {
    use std::convert::Infallible;
    use std::ffi::c_void;
    use std::net::TcpListener;
    use std::ptr;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, anyhow, ensure};
    use bytes::Bytes;
    use http_body_util::{BodyExt, Channel, Full};
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use rcgen::{CertifiedKey as GeneratedCertificate, generate_simple_self_signed};
    use rustls::ServerConfig;
    use rustls::crypto::ring::sign::any_supported_type;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::server::{ClientHello, ResolvesServerCert};
    use rustls::sign::CertifiedKey;
    use tokio::time::{sleep, timeout};
    use tokio_rustls::TlsAcceptor;
    use windows_sys::Win32::Networking::WinHttp::{
        SECURITY_FLAG_IGNORE_UNKNOWN_CA, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE,
        WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED,
        WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_PROTOCOL_FLAG_HTTP2, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
        WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
        WinHttpSetTimeouts, WinHttpWriteData,
    };

    /// Certificate/connect name. This experiment is about send/receive concurrency, not host
    /// separation, so the client connects straight to the name the certificate was issued for.
    const HOST: &str = "localhost";
    const CHUNK1: &[u8] = b"upload-chunk-one";
    const CHUNK2: &[u8] = b"upload-chunk-two-final";
    const RESPONSE_FIRST_CHUNK: &[u8] = b"response-chunk-a";
    const RESPONSE_FINAL_CHUNK: &[u8] = b"response-chunk-b-final";

    /// Bounds every server-side frame wait so a client that never sends the expected chunk cannot
    /// hang this experiment; the connection is abandoned with a recorded note instead.
    const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
    /// Bounds every blocking `WinHTTP` call so an unsupported handle state manifests as
    /// `ERROR_WINHTTP_TIMEOUT` rather than an indefinite hang.
    const RESOLVE_TIMEOUT_MS: i32 = 5_000;
    const CONNECT_TIMEOUT_MS: i32 = 5_000;
    const DATA_TIMEOUT_MS: i32 = 8_000;
    /// Delay the duplex server inserts between observing the first request chunk and sending
    /// response headers/body, so the client's blocking receive call has a clear, measurable
    /// window during which the upload has deliberately not finished.
    const SEQUENTIAL_RESPONSE_DELAY: Duration = Duration::from_millis(200);
    const CONCURRENT_RESPONSE_DELAY: Duration = Duration::from_millis(400);

    pub(super) fn run() -> Result<()> {
        rustls::crypto::ring::default_provider()
            .install_default()
            .map_err(|provider| anyhow!("a rustls crypto provider is already installed: {provider:?}"))?;

        run_baseline_case().context("sequencing control failed")?;
        println!();

        let sequential = run_sequential_case().context("sequential-interleave case failed")?;
        println!();

        let concurrent = run_concurrent_case().context("concurrent send/receive case failed")?;
        println!();

        match (sequential, concurrent) {
            (ChunkWriteOutcome::Succeeded, _) => println!(
                "DECISIVE: after WinHttpReceiveResponse observed headers and a response body chunk \
                 for a still-incomplete upload, a further WinHttpWriteData call on the same request \
                 handle succeeded on a single thread (sequential interleave). Full-duplex request/\
                 response streaming is supported on this host."
            ),
            (ChunkWriteOutcome::Failed(sequential_error), ChunkWriteOutcome::Succeeded) => println!(
                "DECISIVE: sequential interleave rejected the follow-up write (Win32 error \
                 {sequential_error}), but a send-only WinHttpWriteData call genuinely overlapping a \
                 receive-only WinHttpReceiveResponse call on a second thread succeeded. Full-duplex \
                 streaming requires the documented concurrent send-only/receive-only thread pairing \
                 on this host."
            ),
            (ChunkWriteOutcome::Failed(sequential_error), ChunkWriteOutcome::Failed(concurrent_error)) => println!(
                "DECISIVE (negative): neither sequential interleave (Win32 error {sequential_error}) \
                 nor a genuinely overlapping concurrent send-only/receive-only thread pairing (Win32 \
                 error {concurrent_error}) permits writing more request data once \
                 WinHttpReceiveResponse has observed the response for an incomplete upload. This host \
                 does not support full-duplex HTTP/2 request/response streaming through WinHTTP."
            ),
        }
        println!();

        run_unknown_length_baseline_case().context("unknown-length sequencing control failed")?;
        println!();

        let unknown_sequential = run_unknown_length_sequential_case().context("unknown-length sequential-interleave case failed")?;
        println!();

        let unknown_concurrent = run_unknown_length_concurrent_case().context("unknown-length concurrent send/receive case failed")?;
        println!();

        print_unknown_length_verdict(unknown_sequential, unknown_concurrent);

        Ok(())
    }

    /// Outcome of attempting to continue an upload after the response has begun arriving.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ChunkWriteOutcome {
        Succeeded,
        Failed(u32),
    }

    /// Outcome of the two send-only writes this probe layers onto an unknown-length upload: the
    /// remaining payload chunk, and - always attempted regardless of whether that first write
    /// succeeded - the documented null-buffer, zero-length write that ends a
    /// `WINHTTP_FLAG_AUTOMATIC_CHUNKING` request body (`fetch_winhttp_impl`'s
    /// `WinHttpBodyWriter::end_automatic_chunking`).
    #[derive(Debug, Clone, Copy)]
    struct UnknownLengthWriteAttempt {
        chunk2: ChunkWriteOutcome,
        terminal: ChunkWriteOutcome,
    }

    impl UnknownLengthWriteAttempt {
        /// Collapses the two send-only steps into one outcome: the upload only completed if both
        /// the remaining chunk and the terminal write succeeded, and the first Win32 error is the
        /// one that best explains an incomplete upload.
        fn combined(self) -> ChunkWriteOutcome {
            match (self.chunk2, self.terminal) {
                (ChunkWriteOutcome::Succeeded, ChunkWriteOutcome::Succeeded) => ChunkWriteOutcome::Succeeded,
                (ChunkWriteOutcome::Failed(code), _) | (_, ChunkWriteOutcome::Failed(code)) => ChunkWriteOutcome::Failed(code),
            }
        }
    }

    /// Non-duplex sequencing control: the server only responds after observing the complete
    /// request body. This validates that the shared `ServerObservation` timestamps actually
    /// distinguish "responded before the final chunk" from "responded after it", rather than the
    /// duplex cases below merely being an artifact of how this harness measures time.
    fn run_baseline_case() -> Result<()> {
        let (server, observation) = BaselineServer::start(HOST)?;
        let client = DuplexClient::open()?;
        let total_len = u32::try_from(CHUNK1.len() + CHUNK2.len())?;
        let request = client.start_post(HOST, server.port(), total_len)?;

        let written1 = request.write_chunk(CHUNK1)?;
        ensure!(
            usize::try_from(written1)? == CHUNK1.len(),
            "baseline: the first chunk was not fully written"
        );
        let written2 = request.write_chunk(CHUNK2)?;
        ensure!(
            usize::try_from(written2)? == CHUNK2.len(),
            "baseline: the second chunk was not fully written"
        );

        request.receive_response().context("baseline: WinHttpReceiveResponse failed")?;
        let status = request.status_code()?;
        let protocol = request.protocol_used()?;
        let body = request.read_remaining()?;

        drop(request);
        // WinHTTP keeps HTTP/2 connections pooled at the session level for reuse, so the
        // underlying socket does not necessarily close just because the request handle does.
        // Drop the session too so the server observes a clean connection shutdown.
        drop(client);
        server.join()?;

        let recorded = observation.lock().expect("observation mutex poisoned").clone();
        println!(
            "baseline (sequencing control): status={status}, protocol={protocol}, response body={:?}",
            String::from_utf8_lossy(&body)
        );
        print_server_notes(&recorded);

        ensure!(status == 200, "baseline request did not return HTTP 200");
        ensure!(protocol == WINHTTP_PROTOCOL_FLAG_HTTP2, "baseline request did not negotiate HTTP/2");
        ensure!(
            recorded.chunk1.as_deref() == Some(CHUNK1),
            "baseline server did not observe the first chunk correctly"
        );
        ensure!(
            recorded.chunk2.as_deref() == Some(CHUNK2),
            "baseline server did not observe the second chunk correctly"
        );
        let chunk2_at = recorded.chunk2_at.context("baseline server never recorded the second chunk")?;
        let response_at = recorded
            .response_sent_at
            .context("baseline server never recorded sending a response")?;
        ensure!(
            response_at >= chunk2_at,
            "sequencing control invalid: the baseline server responded before observing the complete request body"
        );
        println!("sequencing control confirmed: non-duplex handling responds only after the full request body arrives.");
        Ok(())
    }

    /// Writes the first chunk, then calls `WinHttpReceiveResponse` and reads the first response
    /// chunk on a single thread, before attempting a further `WinHttpWriteData` call for the
    /// second chunk. All of this is inherently "sequential" from `WinHTTP`'s perspective (each
    /// blocking call fully completes before the next begins), so this case isolates whether
    /// `WinHttpReceiveResponse` itself ends the data transfer for a still-incomplete upload.
    fn run_sequential_case() -> Result<ChunkWriteOutcome> {
        let (server, observation) = DuplexServer::start(HOST, SEQUENTIAL_RESPONSE_DELAY)?;
        let client = DuplexClient::open()?;
        let total_len = u32::try_from(CHUNK1.len() + CHUNK2.len())?;
        let request = client.start_post(HOST, server.port(), total_len)?;

        let written1 = request.write_chunk(CHUNK1)?;
        ensure!(
            usize::try_from(written1)? == CHUNK1.len(),
            "sequential: the first chunk was not fully written"
        );

        request.receive_response().context("sequential: WinHttpReceiveResponse failed")?;
        let status = request.status_code()?;
        let protocol = request.protocol_used()?;
        ensure!(
            protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
            "sequential: request did not negotiate HTTP/2"
        );

        // Decisive, non-timing-based proof that the response was already flowing while the
        // upload was still incomplete: inspect the server's live state the instant headers
        // became available on the client, before attempting to send any more request data.
        let response_before_final_chunk = {
            let recorded = observation.lock().expect("observation mutex poisoned");
            recorded.response_sent_at.is_some() && recorded.chunk2.is_none()
        };
        ensure!(
            response_before_final_chunk,
            "sequential: the response was not observably available before the client attempted the final upload chunk"
        );

        let first_chunk = request
            .read_available()?
            .context("sequential: no response body was available immediately after headers arrived")?;
        ensure!(
            first_chunk == RESPONSE_FIRST_CHUNK,
            "sequential: unexpected response body before the final upload chunk"
        );
        println!(
            "sequential: status={status}, protocol={protocol}, response observed before the final chunk \
             was sent (server chunk2 not yet seen, response already sent)."
        );

        let write2 = request.write_chunk(CHUNK2);
        let outcome = match &write2 {
            Ok(bytes_written) => {
                ensure!(
                    usize::try_from(*bytes_written)? == CHUNK2.len(),
                    "sequential: the second chunk was not fully written"
                );
                println!("sequential: WinHttpWriteData(chunk2) succeeded while the response was already flowing.");
                ChunkWriteOutcome::Succeeded
            }
            Err(error) => {
                let code = error.downcast_ref::<WinHttpError>().map_or(0, |error| error.code);
                println!("sequential: WinHttpWriteData(chunk2) failed after headers were received: {error} (Win32 error {code})");
                ChunkWriteOutcome::Failed(code)
            }
        };

        if write2.is_ok() {
            let rest = request.read_remaining()?;
            ensure!(rest == RESPONSE_FINAL_CHUNK, "sequential: unexpected trailing response content");
        }
        drop(request);
        // WinHTTP keeps HTTP/2 connections pooled at the session level for reuse, so the
        // underlying socket does not necessarily close just because the request handle does.
        drop(client);
        server.join()?;

        let recorded = observation.lock().expect("observation mutex poisoned").clone();
        print_server_notes(&recorded);
        if write2.is_ok() {
            ensure!(
                recorded.chunk2.as_deref() == Some(CHUNK2),
                "sequential: the server did not observe the second chunk despite a successful write"
            );
            println!("sequential: server confirmed receiving the second chunk after already responding.");
        }

        Ok(outcome)
    }

    /// Writes the first chunk, then starts a receive-only `WinHttpReceiveResponse` call on one
    /// thread and a send-only `WinHttpWriteData` call for the second chunk on another thread,
    /// releasing both through a barrier so their blocking windows genuinely overlap. This directly
    /// exercises the documented exception: "an application may do a send-only operation on one
    /// thread at the same time that another thread is performing a receive-only operation."
    fn run_concurrent_case() -> Result<ChunkWriteOutcome> {
        let (server, observation) = DuplexServer::start(HOST, CONCURRENT_RESPONSE_DELAY)?;
        let client = DuplexClient::open()?;
        let total_len = u32::try_from(CHUNK1.len() + CHUNK2.len())?;
        let request = client.start_post(HOST, server.port(), total_len)?;

        let written1 = request.write_chunk(CHUNK1)?;
        ensure!(
            usize::try_from(written1)? == CHUNK1.len(),
            "concurrent: the first chunk was not fully written"
        );

        let raw = SendPtr(request.raw());
        let barrier = Barrier::new(2);
        let (receive_outcome, write_outcome) = thread::scope(|scope| -> Result<(ThreadOutcome<()>, ThreadOutcome<u32>)> {
            let barrier = &barrier;
            let receiver = scope.spawn(move || {
                let raw = raw;
                barrier.wait();
                let start = Instant::now();
                let result = winhttp_receive_response(raw.0);
                let end = Instant::now();
                ThreadOutcome { start, end, result }
            });
            let writer = scope.spawn(move || {
                let raw = raw;
                barrier.wait();
                let start = Instant::now();
                let result = winhttp_write(raw.0, CHUNK2);
                let end = Instant::now();
                ThreadOutcome { start, end, result }
            });
            let receive_outcome = receiver.join().map_err(|_panic| anyhow!("receive-only thread panicked"))?;
            let write_outcome = writer.join().map_err(|_panic| anyhow!("send-only thread panicked"))?;
            Ok((receive_outcome, write_outcome))
        })?;

        let overlap = write_outcome.start < receive_outcome.end && receive_outcome.start < write_outcome.end;
        println!(
            "concurrent: receive-only WinHttpReceiveResponse active for {:?} (result={:?}); send-only \
             WinHttpWriteData(chunk2) active for {:?} (result={:?}); overlapping={overlap}",
            receive_outcome.end.duration_since(receive_outcome.start),
            result_summary(&receive_outcome.result),
            write_outcome.end.duration_since(write_outcome.start),
            result_summary(&write_outcome.result),
        );

        let outcome = match &write_outcome.result {
            Ok(bytes_written) => {
                ensure!(
                    usize::try_from(*bytes_written)? == CHUNK2.len(),
                    "concurrent: the second chunk was not fully written"
                );
                ChunkWriteOutcome::Succeeded
            }
            Err(error) => ChunkWriteOutcome::Failed(error.downcast_ref::<WinHttpError>().map_or(0, |error| error.code)),
        };

        if receive_outcome.result.is_ok() {
            let status = request.status_code()?;
            let protocol = request.protocol_used()?;
            ensure!(
                protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
                "concurrent: request did not negotiate HTTP/2"
            );
            println!("concurrent: status={status}, protocol={protocol}");

            if outcome == ChunkWriteOutcome::Succeeded {
                let body = request.read_remaining()?;
                ensure!(
                    body.starts_with(RESPONSE_FIRST_CHUNK),
                    "concurrent: unexpected response body content"
                );
                ensure!(
                    body.ends_with(RESPONSE_FINAL_CHUNK),
                    "concurrent: response body did not reach the final chunk"
                );
            }
        }

        drop(request);
        // WinHTTP keeps HTTP/2 connections pooled at the session level for reuse, so the
        // underlying socket does not necessarily close just because the request handle does.
        drop(client);
        server.join()?;

        let recorded = observation.lock().expect("observation mutex poisoned").clone();
        print_server_notes(&recorded);
        if outcome == ChunkWriteOutcome::Succeeded {
            ensure!(
                recorded.chunk2.as_deref() == Some(CHUNK2),
                "concurrent: the server did not observe the second chunk despite a successful write"
            );
            println!("concurrent: server confirmed receiving the second chunk after already responding.");
        }

        Ok(outcome)
    }

    /// Writes the final upload chunk and then, always, the documented null-buffer, zero-length
    /// write that ends a `WINHTTP_FLAG_AUTOMATIC_CHUNKING` upload - bundled together so the
    /// concurrent case's send-only thread performs both send-only operations from a single scoped
    /// closure. The terminal write is attempted even when the chunk above was rejected, because
    /// this probe wants to know whether `WinHTTP` treats the "end of body" signal as exempt from
    /// whatever caused an ordinary payload write to fail, not only whether it is accepted on the
    /// already-known-good path.
    fn write_final_chunk_then_end_automatic_chunking(request: *mut c_void) -> Result<UnknownLengthWriteAttempt> {
        let chunk2 = match winhttp_write(request, CHUNK2) {
            Ok(bytes_written) => {
                ensure!(
                    usize::try_from(bytes_written)? == CHUNK2.len(),
                    "the final upload chunk was not fully written"
                );
                ChunkWriteOutcome::Succeeded
            }
            Err(error) => ChunkWriteOutcome::Failed(error.downcast_ref::<WinHttpError>().map_or(0, |error| error.code)),
        };

        let terminal = match winhttp_end_automatic_chunking(request) {
            Ok(()) => ChunkWriteOutcome::Succeeded,
            Err(error) => ChunkWriteOutcome::Failed(error.downcast_ref::<WinHttpError>().map_or(0, |error| error.code)),
        };

        Ok(UnknownLengthWriteAttempt { chunk2, terminal })
    }

    /// Non-duplex sequencing control for the unknown-length upload: the server reads the entire
    /// request body - including the documented null-buffer, zero-length terminal write that ends a
    /// `WINHTTP_FLAG_AUTOMATIC_CHUNKING` request - before responding, exactly as `run_baseline_case`
    /// does for a known-length upload. This validates that the corrected native API
    /// (`WINHTTP_FLAG_AUTOMATIC_CHUNKING` on `WinHttpOpenRequest` plus
    /// `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` on `WinHttpSendRequest`, with
    /// `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` set and no manually added `Transfer-Encoding` header -
    /// exactly what PR #687's `fetch_winhttp_impl` sends) completes an HTTP/2-required
    /// unknown-length upload end to end, before the duplex cases below reorder it. The earlier
    /// version of this probe added `Transfer-Encoding: chunked` by hand instead and could not even
    /// reach this sequencing control: `WinHttpSendRequest` itself rejected
    /// `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` alongside that header (Win32 error 12190) - the
    /// automatic-chunking flag has no such conflict.
    fn run_unknown_length_baseline_case() -> Result<()> {
        let (server, observation) = BaselineServer::start(HOST)?;
        let client = DuplexClient::open()?;
        let request = client.start_post_unknown_length(HOST, server.port())?;

        let written1 = request.write_chunk(CHUNK1)?;
        ensure!(
            usize::try_from(written1)? == CHUNK1.len(),
            "unknown-length baseline: the first chunk was not fully written"
        );
        let written2 = request.write_chunk(CHUNK2)?;
        ensure!(
            usize::try_from(written2)? == CHUNK2.len(),
            "unknown-length baseline: the second chunk was not fully written"
        );
        request
            .end_automatic_chunking()
            .context("unknown-length baseline: the null-buffer terminal write failed")?;

        request
            .receive_response()
            .context("unknown-length baseline: WinHttpReceiveResponse failed")?;
        let status = request.status_code()?;
        let protocol = request.protocol_used()?;
        let body = request.read_remaining()?;

        drop(request);
        // WinHTTP keeps HTTP/2 connections pooled at the session level for reuse, so the
        // underlying socket does not necessarily close just because the request handle does.
        drop(client);
        server.join()?;

        let recorded = observation.lock().expect("observation mutex poisoned").clone();
        println!(
            "unknown-length baseline (sequencing control): status={status}, protocol={protocol}, \
             response body={:?}",
            String::from_utf8_lossy(&body)
        );
        print_server_notes(&recorded);

        ensure!(status == 200, "unknown-length baseline request did not return HTTP 200");
        ensure!(
            protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
            "unknown-length baseline request did not negotiate HTTP/2"
        );
        ensure!(
            recorded.chunk1.as_deref() == Some(CHUNK1),
            "unknown-length baseline server did not observe the first chunk correctly"
        );
        ensure!(
            recorded.chunk2.as_deref() == Some(CHUNK2),
            "unknown-length baseline server did not observe the second chunk correctly"
        );
        let chunk2_at = recorded
            .chunk2_at
            .context("unknown-length baseline server never recorded the second chunk")?;
        let response_at = recorded
            .response_sent_at
            .context("unknown-length baseline server never recorded sending a response")?;
        ensure!(
            response_at >= chunk2_at,
            "sequencing control invalid: the unknown-length baseline server responded before observing \
             the complete request body"
        );
        println!(
            "unknown-length sequencing control confirmed: WINHTTP_FLAG_AUTOMATIC_CHUNKING + \
             WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH complete an HTTP/2-required upload end-to-end with no \
             Transfer-Encoding header."
        );
        Ok(())
    }

    /// Unknown-length analogue of `run_sequential_case`, using the corrected
    /// `WINHTTP_FLAG_AUTOMATIC_CHUNKING` + `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` API instead of the
    /// earlier, invalid probe's manually added `Transfer-Encoding: chunked` header: writes the
    /// first chunk, then calls `WinHttpReceiveResponse` and reads the first response chunk while
    /// the automatically-chunked upload is still open - before the remaining chunk or the required
    /// null-buffer terminal write have been sent - and only then attempts to finish the upload on
    /// the same thread.
    fn run_unknown_length_sequential_case() -> Result<ChunkWriteOutcome> {
        let (server, observation) = DuplexServer::start(HOST, SEQUENTIAL_RESPONSE_DELAY)?;
        let client = DuplexClient::open()?;
        let request = client.start_post_unknown_length(HOST, server.port())?;

        let written1 = request.write_chunk(CHUNK1)?;
        ensure!(
            usize::try_from(written1)? == CHUNK1.len(),
            "unknown-length sequential: the first chunk was not fully written"
        );

        request
            .receive_response()
            .context("unknown-length sequential: WinHttpReceiveResponse failed before the upload finished")?;
        let status = request.status_code()?;
        let protocol = request.protocol_used()?;
        ensure!(
            protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
            "unknown-length sequential: request did not negotiate HTTP/2"
        );

        let response_before_final_chunk = {
            let recorded = observation.lock().expect("observation mutex poisoned");
            recorded.response_sent_at.is_some() && recorded.chunk2.is_none()
        };
        ensure!(
            response_before_final_chunk,
            "unknown-length sequential: the response was not observably available before the client \
             attempted the final upload chunk"
        );

        let first_chunk = request
            .read_available()?
            .context("unknown-length sequential: no response body was available immediately after headers arrived")?;
        ensure!(
            first_chunk == RESPONSE_FIRST_CHUNK,
            "unknown-length sequential: unexpected response body before the final upload chunk"
        );
        println!(
            "unknown-length sequential: status={status}, protocol={protocol}, response observed \
             before the final chunk was sent (server chunk2 not yet seen, response already sent)."
        );

        let attempt = write_final_chunk_then_end_automatic_chunking(request.raw())?;
        match attempt.chunk2 {
            ChunkWriteOutcome::Succeeded => {
                println!("unknown-length sequential: WinHttpWriteData(chunk2) succeeded while the response was already flowing.");
            }
            ChunkWriteOutcome::Failed(code) => {
                println!("unknown-length sequential: WinHttpWriteData(chunk2) failed after headers were received (Win32 error {code}).");
            }
        }
        match attempt.terminal {
            ChunkWriteOutcome::Succeeded => println!(
                "unknown-length sequential: the null-buffer terminal write succeeded, ending the \
                 automatically chunked upload."
            ),
            ChunkWriteOutcome::Failed(code) => {
                println!("unknown-length sequential: the null-buffer terminal write failed (Win32 error {code}).");
            }
        }

        let outcome = attempt.combined();
        if outcome == ChunkWriteOutcome::Succeeded {
            let rest = request.read_remaining()?;
            ensure!(
                rest == RESPONSE_FINAL_CHUNK,
                "unknown-length sequential: unexpected trailing response content"
            );
        }
        drop(request);
        // WinHTTP keeps HTTP/2 connections pooled at the session level for reuse, so the
        // underlying socket does not necessarily close just because the request handle does.
        drop(client);
        server.join()?;

        let recorded = observation.lock().expect("observation mutex poisoned").clone();
        print_server_notes(&recorded);
        if outcome == ChunkWriteOutcome::Succeeded {
            ensure!(
                recorded.chunk2.as_deref() == Some(CHUNK2),
                "unknown-length sequential: the server did not observe the second chunk despite a successful write"
            );
            println!("unknown-length sequential: server confirmed receiving the second chunk after already responding.");
        }

        Ok(outcome)
    }

    /// Unknown-length analogue of `run_concurrent_case`: releases a receive-only
    /// `WinHttpReceiveResponse` call and a send-only thread that writes the remaining chunk and the
    /// null-buffer terminal write, through a barrier on two threads sharing the same request
    /// handle, corroborating whatever `run_unknown_length_sequential_case` found through the
    /// documented concurrent send-only/receive-only exception instead of strict single-thread
    /// ordering.
    fn run_unknown_length_concurrent_case() -> Result<ChunkWriteOutcome> {
        let (server, observation) = DuplexServer::start(HOST, CONCURRENT_RESPONSE_DELAY)?;
        let client = DuplexClient::open()?;
        let request = client.start_post_unknown_length(HOST, server.port())?;

        let written1 = request.write_chunk(CHUNK1)?;
        ensure!(
            usize::try_from(written1)? == CHUNK1.len(),
            "unknown-length concurrent: the first chunk was not fully written"
        );

        let raw = SendPtr(request.raw());
        let barrier = Barrier::new(2);
        let (receive_outcome, write_outcome) =
            thread::scope(|scope| -> Result<(ThreadOutcome<()>, ThreadOutcome<UnknownLengthWriteAttempt>)> {
                let barrier = &barrier;
                let receiver = scope.spawn(move || {
                    let raw = raw;
                    barrier.wait();
                    let start = Instant::now();
                    let result = winhttp_receive_response(raw.0);
                    let end = Instant::now();
                    ThreadOutcome { start, end, result }
                });
                let writer = scope.spawn(move || {
                    let raw = raw;
                    barrier.wait();
                    let start = Instant::now();
                    let result = write_final_chunk_then_end_automatic_chunking(raw.0);
                    let end = Instant::now();
                    ThreadOutcome { start, end, result }
                });
                let receive_outcome = receiver.join().map_err(|_panic| anyhow!("receive-only thread panicked"))?;
                let write_outcome = writer.join().map_err(|_panic| anyhow!("send-only thread panicked"))?;
                Ok((receive_outcome, write_outcome))
            })?;

        let overlap = write_outcome.start < receive_outcome.end && receive_outcome.start < write_outcome.end;
        let write_duration = write_outcome.end.duration_since(write_outcome.start);
        let receive_duration = receive_outcome.end.duration_since(receive_outcome.start);
        let attempt = match &write_outcome.result {
            Ok(attempt) => *attempt,
            Err(error) => return Err(anyhow!("unknown-length concurrent: send-only thread failed unexpectedly: {error}")),
        };
        println!(
            "unknown-length concurrent: receive-only WinHttpReceiveResponse active for {receive_duration:?} \
             (result={}); send-only WinHttpWriteData(chunk2)+terminal active for {write_duration:?} \
             (chunk2={:?}, terminal={:?}); overlapping={overlap}",
            result_summary(&receive_outcome.result),
            attempt.chunk2,
            attempt.terminal,
        );

        let outcome = attempt.combined();

        if receive_outcome.result.is_ok() {
            let status = request.status_code()?;
            let protocol = request.protocol_used()?;
            ensure!(
                protocol == WINHTTP_PROTOCOL_FLAG_HTTP2,
                "unknown-length concurrent: request did not negotiate HTTP/2"
            );
            println!("unknown-length concurrent: status={status}, protocol={protocol}");

            if outcome == ChunkWriteOutcome::Succeeded {
                let body = request.read_remaining()?;
                ensure!(
                    body.starts_with(RESPONSE_FIRST_CHUNK),
                    "unknown-length concurrent: unexpected response body content"
                );
                ensure!(
                    body.ends_with(RESPONSE_FINAL_CHUNK),
                    "unknown-length concurrent: response body did not reach the final chunk"
                );
            }
        }

        drop(request);
        // WinHTTP keeps HTTP/2 connections pooled at the session level for reuse, so the
        // underlying socket does not necessarily close just because the request handle does.
        drop(client);
        server.join()?;

        let recorded = observation.lock().expect("observation mutex poisoned").clone();
        print_server_notes(&recorded);
        if outcome == ChunkWriteOutcome::Succeeded {
            ensure!(
                recorded.chunk2.as_deref() == Some(CHUNK2),
                "unknown-length concurrent: the server did not observe the second chunk despite a successful write"
            );
            println!("unknown-length concurrent: server confirmed receiving the second chunk after already responding.");
        }

        Ok(outcome)
    }

    /// Prints the combined decisive verdict for the unknown-length probe, mirroring the combined
    /// verdict `run()` already prints for the known-length probe.
    fn print_unknown_length_verdict(sequential: ChunkWriteOutcome, concurrent: ChunkWriteOutcome) {
        match (sequential, concurrent) {
            (ChunkWriteOutcome::Succeeded, _) => println!(
                "DECISIVE (unknown length): after WinHttpReceiveResponse observed headers and a \
                 response body chunk for a still-incomplete WINHTTP_FLAG_AUTOMATIC_CHUNKING upload (no \
                 Transfer-Encoding header, dwTotalLength=WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH, HTTP/2 \
                 required), a further WinHttpWriteData call and the documented null-buffer terminal \
                 write both succeeded on the same request handle from the same thread (sequential \
                 interleave). True unbounded, gRPC-style full-duplex request/response streaming is \
                 supported on this host using the same native automatic-chunking API PR #687 uses."
            ),
            (ChunkWriteOutcome::Failed(sequential_error), ChunkWriteOutcome::Succeeded) => println!(
                "DECISIVE (unknown length): sequential interleave rejected the follow-up chunk/terminal \
                 write (Win32 error {sequential_error}), but a send-only chunk+terminal write genuinely \
                 overlapping a receive-only WinHttpReceiveResponse call on a second thread succeeded. \
                 Unbounded full-duplex streaming with WINHTTP_FLAG_AUTOMATIC_CHUNKING requires the \
                 documented concurrent send-only/receive-only thread pairing on this host."
            ),
            (ChunkWriteOutcome::Failed(sequential_error), ChunkWriteOutcome::Failed(concurrent_error)) => println!(
                "DECISIVE (unknown length, negative): neither sequential interleave (Win32 error \
                 {sequential_error}) nor a genuinely overlapping concurrent send-only/receive-only \
                 thread pairing (Win32 error {concurrent_error}) permits completing a \
                 WINHTTP_FLAG_AUTOMATIC_CHUNKING upload once WinHttpReceiveResponse has observed the \
                 response for an incomplete body. This host does not support true unbounded, gRPC-style \
                 full-duplex request/response streaming through native WinHTTP even with the correct \
                 automatic-chunking API PR #687 uses."
            ),
        }
    }

    fn result_summary<T>(result: &Result<T>) -> String {
        match result {
            Ok(_) => "Ok".to_owned(),
            Err(error) => {
                let code = error.downcast_ref::<WinHttpError>().map_or(0, |error| error.code);
                format!("Err(Win32 error {code}: {error})")
            }
        }
    }

    struct ThreadOutcome<T> {
        start: Instant,
        end: Instant,
        result: Result<T>,
    }

    /// Server-observed timeline for one request. Shared with the test driver through an
    /// `Arc<Mutex<_>>` so the client can inspect live server-side ordering rather than inferring
    /// success merely because a `WinHTTP` call returned.
    #[derive(Default, Debug, Clone)]
    struct ServerObservation {
        sni: Option<String>,
        alpn: Option<String>,
        chunk1: Option<Vec<u8>>,
        chunk1_at: Option<Instant>,
        response_sent_at: Option<Instant>,
        chunk2: Option<Vec<u8>>,
        chunk2_at: Option<Instant>,
        request_end_at: Option<Instant>,
        notes: Vec<String>,
    }

    type SharedObservation = Arc<Mutex<ServerObservation>>;

    fn note(observation: &SharedObservation, message: impl Into<String>) {
        observation.lock().expect("observation mutex poisoned").notes.push(message.into());
    }

    /// Prints any server-side notes recorded during a case (timeouts, unexpected stream
    /// endings, and similar events), so a case that only partially completes still leaves an
    /// exact, explained sequence in the output rather than silence.
    fn print_server_notes(recorded: &ServerObservation) {
        for note in &recorded.notes {
            println!("  server note: {note}");
        }
    }

    /// Duplex HTTP/2 server: responds with headers and a first body chunk as soon as it observes
    /// the first request chunk, then keeps reading the request body (observing a later chunk)
    /// concurrently with the response already flowing.
    struct DuplexServer {
        port: u16,
        thread: JoinHandle<Result<()>>,
    }

    impl DuplexServer {
        fn start(certificate_name: &str, response_delay: Duration) -> Result<(Self, SharedObservation)> {
            let observed_sni = Arc::new(Mutex::new(None));
            let config = server_config(certificate_name, Arc::clone(&observed_sni))?;
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            listener.set_nonblocking(true)?;

            let observation: SharedObservation = Arc::new(Mutex::new(ServerObservation::default()));
            let observation_for_thread = Arc::clone(&observation);
            let thread = thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()?
                    .block_on(serve_duplex(listener, config, observed_sni, observation_for_thread, response_delay))
            });

            Ok((Self { port, thread }, observation))
        }

        fn port(&self) -> u16 {
            self.port
        }

        fn join(self) -> Result<()> {
            self.thread.join().map_err(|_panic| anyhow!("duplex server thread panicked"))?
        }
    }

    async fn serve_duplex(
        listener: TcpListener,
        config: ServerConfig,
        observed_sni: Arc<Mutex<Option<String>>>,
        observation: SharedObservation,
        response_delay: Duration,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let (stream, _) = listener.accept().await?;
        let tls = TlsAcceptor::from(Arc::new(config)).accept(stream).await?;
        let alpn = tls
            .get_ref()
            .1
            .alpn_protocol()
            .map(|protocol| String::from_utf8_lossy(protocol).into_owned());
        {
            let mut recorded = observation.lock().expect("observation mutex poisoned");
            recorded.sni.clone_from(&observed_sni.lock().expect("SNI recorder poisoned"));
            recorded.alpn = alpn;
        }

        let observation_for_service = Arc::clone(&observation);
        let service = service_fn(move |request: Request<Incoming>| {
            let observation = Arc::clone(&observation_for_service);
            async move {
                let mut incoming = request.into_body();
                let chunk1 = match timeout(FRAME_TIMEOUT, next_data_frame(&mut incoming)).await {
                    Ok(Ok(Some(bytes))) => bytes,
                    Ok(Ok(None)) => {
                        note(&observation, "request ended before the first chunk arrived");
                        return Ok::<_, Infallible>(duplex_error_response());
                    }
                    Ok(Err(error)) => {
                        note(&observation, format!("error reading the first chunk: {error}"));
                        return Ok::<_, Infallible>(duplex_error_response());
                    }
                    Err(_elapsed) => {
                        note(&observation, "timed out waiting for the first chunk");
                        return Ok::<_, Infallible>(duplex_error_response());
                    }
                };
                {
                    let mut recorded = observation.lock().expect("observation mutex poisoned");
                    recorded.chunk1 = Some(chunk1.to_vec());
                    recorded.chunk1_at = Some(Instant::now());
                }

                sleep(response_delay).await;

                let (mut sender, body) = Channel::<Bytes, Infallible>::new(4);
                let response = Response::builder()
                    .status(StatusCode::OK)
                    .body(body)
                    .expect("a status code and a streaming body always build a valid response");

                let observation_for_task = Arc::clone(&observation);
                tokio::spawn(async move {
                    if sender.send_data(Bytes::from_static(RESPONSE_FIRST_CHUNK)).await.is_ok() {
                        let mut recorded = observation_for_task.lock().expect("observation mutex poisoned");
                        recorded.response_sent_at = Some(Instant::now());
                    } else {
                        note(
                            &observation_for_task,
                            "the client closed the response body before the first chunk was sent",
                        );
                    }

                    match timeout(FRAME_TIMEOUT, next_data_frame(&mut incoming)).await {
                        Ok(Ok(Some(bytes))) => {
                            let mut recorded = observation_for_task.lock().expect("observation mutex poisoned");
                            recorded.chunk2 = Some(bytes.to_vec());
                            recorded.chunk2_at = Some(Instant::now());
                        }
                        Ok(Ok(None)) => note(&observation_for_task, "request ended before a second chunk arrived"),
                        Ok(Err(error)) => note(&observation_for_task, format!("error reading the second chunk: {error}")),
                        Err(_elapsed) => note(&observation_for_task, "timed out waiting for the second chunk"),
                    }

                    if let Err(error) = timeout(FRAME_TIMEOUT, drain_to_end(&mut incoming)).await {
                        note(&observation_for_task, format!("timed out draining the request body: {error}"));
                    }
                    {
                        let mut recorded = observation_for_task.lock().expect("observation mutex poisoned");
                        recorded.request_end_at = Some(Instant::now());
                    }

                    if let Err(error) = sender.send_data(Bytes::from_static(RESPONSE_FINAL_CHUNK)).await {
                        note(&observation_for_task, format!("could not send the final response chunk: {error}"));
                    }
                    drop(sender);
                });

                Ok::<_, Infallible>(response)
            }
        });

        if let Err(error) = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls), service)
            .await
        {
            note(&observation, format!("HTTP/2 connection ended with an error: {error}"));
        }
        Ok(())
    }

    fn duplex_error_response() -> Response<Channel<Bytes, Infallible>> {
        let (sender, body) = Channel::<Bytes, Infallible>::new(1);
        drop(sender);
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(body)
            .expect("a status code and an empty streaming body always build a valid response")
    }

    /// Non-duplex baseline server: reads the entire request body before responding, matching
    /// ordinary request/response handling for the sequencing control.
    struct BaselineServer {
        port: u16,
        thread: JoinHandle<Result<()>>,
    }

    impl BaselineServer {
        fn start(certificate_name: &str) -> Result<(Self, SharedObservation)> {
            let observed_sni = Arc::new(Mutex::new(None));
            let config = server_config(certificate_name, Arc::clone(&observed_sni))?;
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            let port = listener.local_addr()?.port();
            listener.set_nonblocking(true)?;

            let observation: SharedObservation = Arc::new(Mutex::new(ServerObservation::default()));
            let observation_for_thread = Arc::clone(&observation);
            let thread = thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()?
                    .block_on(serve_baseline(listener, config, observed_sni, observation_for_thread))
            });

            Ok((Self { port, thread }, observation))
        }

        fn port(&self) -> u16 {
            self.port
        }

        fn join(self) -> Result<()> {
            self.thread.join().map_err(|_panic| anyhow!("baseline server thread panicked"))?
        }
    }

    async fn serve_baseline(
        listener: TcpListener,
        config: ServerConfig,
        observed_sni: Arc<Mutex<Option<String>>>,
        observation: SharedObservation,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let (stream, _) = listener.accept().await?;
        let tls = TlsAcceptor::from(Arc::new(config)).accept(stream).await?;
        {
            let mut recorded = observation.lock().expect("observation mutex poisoned");
            recorded.sni.clone_from(&observed_sni.lock().expect("SNI recorder poisoned"));
        }

        let observation_for_service = Arc::clone(&observation);
        let service = service_fn(move |request: Request<Incoming>| {
            let observation = Arc::clone(&observation_for_service);
            async move {
                let mut incoming = request.into_body();
                let chunk1 = timeout(FRAME_TIMEOUT, next_data_frame(&mut incoming))
                    .await
                    .map_err(|elapsed| anyhow!("timed out waiting for the first chunk: {elapsed}"))
                    .and_then(|inner| inner)?
                    .context("request ended before the first chunk arrived")?;
                {
                    let mut recorded = observation.lock().expect("observation mutex poisoned");
                    recorded.chunk1 = Some(chunk1.to_vec());
                    recorded.chunk1_at = Some(Instant::now());
                }

                let chunk2 = timeout(FRAME_TIMEOUT, next_data_frame(&mut incoming))
                    .await
                    .map_err(|elapsed| anyhow!("timed out waiting for the second chunk: {elapsed}"))
                    .and_then(|inner| inner)?
                    .context("request ended before the second chunk arrived")?;
                {
                    let mut recorded = observation.lock().expect("observation mutex poisoned");
                    recorded.chunk2 = Some(chunk2.to_vec());
                    recorded.chunk2_at = Some(Instant::now());
                }

                timeout(FRAME_TIMEOUT, drain_to_end(&mut incoming))
                    .await
                    .map_err(|elapsed| anyhow!("timed out draining the request body: {elapsed}"))
                    .and_then(|inner| inner)?;

                let response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Full::new(Bytes::from_static(RESPONSE_FIRST_CHUNK)))
                    .expect("a status code and a fixed body always build a valid response");
                {
                    let mut recorded = observation.lock().expect("observation mutex poisoned");
                    recorded.request_end_at = Some(Instant::now());
                    recorded.response_sent_at = Some(Instant::now());
                }

                Ok::<_, anyhow::Error>(response)
            }
        });

        hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls), service)
            .await
            .context("HTTP/2 baseline connection ended with an error")
    }

    fn server_config(certificate_name: &str, observed_sni: Arc<Mutex<Option<String>>>) -> Result<ServerConfig> {
        let GeneratedCertificate { cert, signing_key } = generate_simple_self_signed(vec![certificate_name.to_owned()])?;
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        let signing_key = any_supported_type(&private_key)?;
        let resolver = Arc::new(RecordingResolver {
            certified_key: Arc::new(CertifiedKey::new(vec![CertificateDer::from(cert.der().to_vec())], signing_key)),
            observed_sni,
        });
        let mut config = ServerConfig::builder().with_no_client_auth().with_cert_resolver(resolver);
        config.alpn_protocols = vec![b"h2".to_vec()];
        Ok(config)
    }

    #[derive(Debug)]
    struct RecordingResolver {
        certified_key: Arc<CertifiedKey>,
        observed_sni: Arc<Mutex<Option<String>>>,
    }

    impl ResolvesServerCert for RecordingResolver {
        fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            *self.observed_sni.lock().expect("SNI recorder poisoned") = client_hello.server_name().map(ToOwned::to_owned);
            Some(Arc::clone(&self.certified_key))
        }
    }

    async fn next_data_frame(body: &mut Incoming) -> Result<Option<Bytes>> {
        loop {
            match body.frame().await {
                None => return Ok(None),
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) if !data.is_empty() => return Ok(Some(data)),
                    Ok(_) | Err(_) => {}
                },
                Some(Err(error)) => return Err(error.into()),
            }
        }
    }

    async fn drain_to_end(body: &mut Incoming) -> Result<()> {
        while let Some(frame) = body.frame().await {
            frame?;
        }
        Ok(())
    }

    #[derive(Debug)]
    struct WinHttpError {
        operation: &'static str,
        code: u32,
    }

    impl std::fmt::Display for WinHttpError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} failed with Win32 error {}", self.operation, self.code)
        }
    }

    impl std::error::Error for WinHttpError {}

    struct InternetHandle(*mut c_void);

    impl InternetHandle {
        fn new(handle: *mut c_void, operation: &'static str) -> Result<Self> {
            if handle.is_null() {
                return Err(last_error(operation));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for InternetHandle {
        fn drop(&mut self) {
            // SAFETY: The handle is non-null, owned by this wrapper, and closed exactly once here.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }

    /// A `Copy`, thread-movable handle value used only to hand the same request handle to a
    /// matched send-only/receive-only thread pair, as `WinHTTP`'s concurrency documentation
    /// permits. The owning `InternetHandle` in `DuplexRequest` is never dropped while any thread
    /// holding a copy is still running, because `thread::scope` joins both threads first.
    #[derive(Clone, Copy)]
    struct SendPtr(*mut c_void);

    // SAFETY: See the `SendPtr` doc comment: WinHTTP documents that "an application may do a
    // send-only operation on one thread at the same time that another thread is performing a
    // receive-only operation" using the same request handle. This wrapper exists solely to move a
    // copy of that handle into exactly one send-only and one receive-only thread for the duration
    // of `run_concurrent_case`, never to close the handle or perform any other operation from
    // those threads.
    unsafe impl Send for SendPtr {}

    struct DuplexClient {
        session: InternetHandle,
    }

    impl DuplexClient {
        fn open() -> Result<Self> {
            let agent = wide("fetch-winhttp-full-duplex-probe");
            // SAFETY: All pointers reference valid, null-terminated UTF-16 strings for the call.
            let session = unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, ptr::null(), ptr::null(), 0) };
            Ok(Self {
                session: InternetHandle::new(session, "WinHttpOpen")?,
            })
        }

        fn start_post(&self, host: &str, port: u16, total_len: u32) -> Result<DuplexRequest> {
            let host_wide = wide(host);
            // SAFETY: The session is live and the host pointer is valid for the call.
            let connection = unsafe { WinHttpConnect(self.session.0, host_wide.as_ptr(), port, 0) };
            let connection = InternetHandle::new(connection, "WinHttpConnect")?;

            let verb = wide("POST");
            let path = wide("/");
            // SAFETY: The connection is live and all provided UTF-16 pointers remain valid.
            let request = unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    verb.as_ptr(),
                    path.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    WINHTTP_FLAG_SECURE,
                )
            };
            let request = InternetHandle::new(request, "WinHttpOpenRequest")?;

            let security_flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA;
            set_option(
                &request,
                WINHTTP_OPTION_SECURITY_FLAGS,
                (&raw const security_flags).cast(),
                size_of::<u32>().try_into()?,
                "WINHTTP_OPTION_SECURITY_FLAGS",
            )?;

            let protocols = WINHTTP_PROTOCOL_FLAG_HTTP2;
            set_option(
                &request,
                WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
                (&raw const protocols).cast(),
                size_of::<u32>().try_into()?,
                "WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL",
            )?;
            let required = 1_i32;
            set_option(
                &request,
                WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED,
                (&raw const required).cast(),
                size_of::<i32>().try_into()?,
                "WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED",
            )?;

            // SAFETY: The request is live; these timeouts bound every subsequent blocking call so
            // an unsupported or serialized handle state cannot hang this experiment indefinitely.
            if unsafe { WinHttpSetTimeouts(request.0, RESOLVE_TIMEOUT_MS, CONNECT_TIMEOUT_MS, DATA_TIMEOUT_MS, DATA_TIMEOUT_MS) } == 0 {
                return Err(last_error("WinHttpSetTimeouts"));
            }

            // SAFETY: The request is live; no optional data accompanies the headers because the
            // whole body is streamed afterward through WinHttpWriteData.
            if unsafe { WinHttpSendRequest(request.0, ptr::null(), 0, ptr::null(), 0, total_len, 0) } == 0 {
                return Err(last_error("WinHttpSendRequest"));
            }

            Ok(DuplexRequest { request, connection })
        }

        /// Starts a POST request the same way `start_post` does, except the request handle is
        /// opened with `WINHTTP_FLAG_AUTOMATIC_CHUNKING` and `WinHttpSendRequest` receives
        /// `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` for `dwTotalLength`, matching a
        /// gRPC/client-streaming upload whose total size is not known up front. This is the exact
        /// native flag/total-length lowering `fetch_winhttp_impl` uses for an unknown-length
        /// request body (`crates/fetch_winhttp_impl/src/body/write.rs`'s `RequestBodyFraming` and
        /// `request.rs`'s `execute`, plus `convert.rs`'s `request_open_flags`, as landed by
        /// PR #687): no `Transfer-Encoding` header is ever added by the caller, because `WinHTTP`
        /// performs the chunked framing itself once `WINHTTP_FLAG_AUTOMATIC_CHUNKING` is set on the
        /// request handle - `fetch_winhttp_impl` in fact rejects a caller-supplied
        /// `Transfer-Encoding` header outright rather than forwarding it. Duplicated rather than
        /// routed through `start_post` so the known-length setup this probe already validated is
        /// never touched by the unknown-length path.
        ///
        /// `WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED` is set here exactly as `start_post` sets it:
        /// `fetch_winhttp_impl`'s `protocol_options` sets it whenever the caller's supported HTTP
        /// versions exclude HTTP/1.1, and PR #687's own
        /// `http2_streams_unknown_length_uploads_and_preserves_response_trailers` integration test
        /// drives exactly that combination successfully - `WINHTTP_FLAG_AUTOMATIC_CHUNKING` plus a
        /// required HTTP/2 negotiation raises none of the earlier, invalid
        /// `Transfer-Encoding`-header probe's conflicts. An earlier version of this probe added
        /// `Transfer-Encoding: chunked` by hand instead of setting this flag; see
        /// `docs/full-duplex-streaming-experiment.md` for why that was invalid.
        fn start_post_unknown_length(&self, host: &str, port: u16) -> Result<DuplexRequest> {
            let host_wide = wide(host);
            // SAFETY: The session is live and the host pointer is valid for the call.
            let connection = unsafe { WinHttpConnect(self.session.0, host_wide.as_ptr(), port, 0) };
            let connection = InternetHandle::new(connection, "WinHttpConnect")?;

            let verb = wide("POST");
            let path = wide("/");
            // SAFETY: The connection is live and all provided UTF-16 pointers remain valid.
            let request = unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    verb.as_ptr(),
                    path.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    WINHTTP_FLAG_SECURE | WINHTTP_FLAG_AUTOMATIC_CHUNKING,
                )
            };
            let request = InternetHandle::new(request, "WinHttpOpenRequest")?;

            let security_flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA;
            set_option(
                &request,
                WINHTTP_OPTION_SECURITY_FLAGS,
                (&raw const security_flags).cast(),
                size_of::<u32>().try_into()?,
                "WINHTTP_OPTION_SECURITY_FLAGS",
            )?;

            let protocols = WINHTTP_PROTOCOL_FLAG_HTTP2;
            set_option(
                &request,
                WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
                (&raw const protocols).cast(),
                size_of::<u32>().try_into()?,
                "WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL",
            )?;
            let required = 1_i32;
            set_option(
                &request,
                WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED,
                (&raw const required).cast(),
                size_of::<i32>().try_into()?,
                "WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED",
            )?;

            // SAFETY: The request is live; these timeouts bound every subsequent blocking call so
            // an unsupported or serialized handle state cannot hang this experiment indefinitely.
            if unsafe { WinHttpSetTimeouts(request.0, RESOLVE_TIMEOUT_MS, CONNECT_TIMEOUT_MS, DATA_TIMEOUT_MS, DATA_TIMEOUT_MS) } == 0 {
                return Err(last_error("WinHttpSetTimeouts"));
            }

            // SAFETY: The request is live; no optional data accompanies the headers because the
            // whole body is streamed afterward through WinHttpWriteData, with WinHTTP performing
            // the chunked framing itself under WINHTTP_FLAG_AUTOMATIC_CHUNKING.
            if unsafe { WinHttpSendRequest(request.0, ptr::null(), 0, ptr::null(), 0, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH, 0) } == 0 {
                return Err(last_error("WinHttpSendRequest"));
            }

            Ok(DuplexRequest { request, connection })
        }
    }

    struct DuplexRequest {
        request: InternetHandle,
        // Declared after `request` so it is dropped after the request handle, and kept only to
        // hold the connection open for the request's lifetime; its handle value is never read.
        #[expect(dead_code, reason = "held only for RAII drop-order relative to `request`, never read")]
        connection: InternetHandle,
    }

    impl DuplexRequest {
        fn raw(&self) -> *mut c_void {
            self.request.0
        }

        fn write_chunk(&self, data: &[u8]) -> Result<u32> {
            winhttp_write(self.raw(), data)
        }

        fn receive_response(&self) -> Result<()> {
            winhttp_receive_response(self.raw())
        }

        /// Ends a `WINHTTP_FLAG_AUTOMATIC_CHUNKING` upload with the documented null-buffer,
        /// zero-length write, mirroring `fetch_winhttp_impl`'s
        /// `WinHttpBodyWriter::end_automatic_chunking` exactly - a null `lpBuffer`, not a
        /// zero-length write over a valid (if empty) buffer pointer.
        fn end_automatic_chunking(&self) -> Result<()> {
            winhttp_end_automatic_chunking(self.raw())
        }

        fn read_available(&self) -> Result<Option<Vec<u8>>> {
            winhttp_read_available(self.raw())
        }

        fn read_remaining(&self) -> Result<Vec<u8>> {
            let mut all = Vec::new();
            while let Some(chunk) = self.read_available()? {
                all.extend_from_slice(&chunk);
            }
            Ok(all)
        }

        fn status_code(&self) -> Result<u32> {
            let mut status = 0_u32;
            let mut status_size = size_of::<u32>().try_into()?;
            // SAFETY: The output pointers refer to initialized writable storage of the declared size.
            if unsafe {
                WinHttpQueryHeaders(
                    self.raw(),
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    ptr::null(),
                    (&raw mut status).cast(),
                    &raw mut status_size,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(last_error("WinHttpQueryHeaders"));
            }
            Ok(status)
        }

        fn protocol_used(&self) -> Result<u32> {
            query_option_u32(
                &self.request,
                WINHTTP_OPTION_HTTP_PROTOCOL_USED,
                "WINHTTP_OPTION_HTTP_PROTOCOL_USED",
            )
        }
    }

    fn winhttp_write(request: *mut c_void, data: &[u8]) -> Result<u32> {
        let mut written = 0_u32;
        let len = u32::try_from(data.len())?;
        // SAFETY: `request` is a live WinHTTP request handle and `data` remains valid for the call.
        if unsafe { WinHttpWriteData(request, data.as_ptr().cast(), len, &raw mut written) } == 0 {
            return Err(last_error("WinHttpWriteData"));
        }
        Ok(written)
    }

    /// Sends the documented null-buffer, zero-length `WinHttpWriteData` call that ends a
    /// `WINHTTP_FLAG_AUTOMATIC_CHUNKING` request body, matching `fetch_winhttp_impl`'s
    /// `WinHttpBodyWriter::end_automatic_chunking` (`body/write.rs`) exactly: a null `lpBuffer`
    /// paired with a zero length, not `winhttp_write(request, &[])`'s valid-but-empty slice
    /// pointer. Whether this distinction matters on native `WinHTTP` is untested by this probe -
    /// it exists so the probe reproduces the same call PR #687 makes rather than an
    /// implementation detail this probe happened to differ on.
    fn winhttp_end_automatic_chunking(request: *mut c_void) -> Result<()> {
        let mut written = 0_u32;
        // SAFETY: `request` is a live WinHTTP request handle opened with
        // WINHTTP_FLAG_AUTOMATIC_CHUNKING; a null buffer paired with a zero length is the
        // documented way to end an automatically chunked upload.
        if unsafe { WinHttpWriteData(request, ptr::null(), 0, &raw mut written) } == 0 {
            return Err(last_error("WinHttpWriteData(terminal)"));
        }
        ensure!(
            written == 0,
            "the null-buffer terminal write reported writing a nonzero number of bytes"
        );
        Ok(())
    }

    fn winhttp_receive_response(request: *mut c_void) -> Result<()> {
        // SAFETY: `request` is a live request handle; the reserved parameter must be null.
        if unsafe { WinHttpReceiveResponse(request, ptr::null_mut()) } == 0 {
            return Err(last_error("WinHttpReceiveResponse"));
        }
        Ok(())
    }

    fn winhttp_read_available(request: *mut c_void) -> Result<Option<Vec<u8>>> {
        let mut available = 0_u32;
        // SAFETY: `request` is a live request handle and `available` is a valid output location.
        if unsafe { WinHttpQueryDataAvailable(request, &raw mut available) } == 0 {
            return Err(last_error("WinHttpQueryDataAvailable"));
        }
        if available == 0 {
            return Ok(None);
        }
        let mut buffer = vec![0_u8; available as usize];
        let mut read = 0_u32;
        // SAFETY: `buffer` has `available` writable bytes and `read` is a valid output location.
        if unsafe { WinHttpReadData(request, buffer.as_mut_ptr().cast(), available, &raw mut read) } == 0 {
            return Err(last_error("WinHttpReadData"));
        }
        buffer.truncate(read as usize);
        Ok(Some(buffer))
    }

    fn query_option_u32(handle: &InternetHandle, option: u32, operation: &'static str) -> Result<u32> {
        let mut value = 0_u32;
        let mut value_len = size_of::<u32>().try_into()?;
        // SAFETY: The handle is live and the output buffer has the declared writable size.
        if unsafe { WinHttpQueryOption(handle.0, option, (&raw mut value).cast(), &raw mut value_len) } == 0 {
            return Err(last_error(operation));
        }
        Ok(value)
    }

    fn set_option(handle: &InternetHandle, option: u32, value: *const c_void, value_len: u32, operation: &'static str) -> Result<()> {
        // SAFETY: The handle is live and value points to a buffer of value_len bytes for this call.
        if unsafe { WinHttpSetOption(handle.0, option, value, value_len) } == 0 {
            return Err(last_error(operation));
        }
        Ok(())
    }

    fn last_error(operation: &'static str) -> anyhow::Error {
        WinHttpError {
            operation,
            code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0).cast_unsigned(),
        }
        .into()
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
