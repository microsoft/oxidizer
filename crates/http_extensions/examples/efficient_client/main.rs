// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates that the `http` crate is viable for allocation-sensitive HTTP clients.
//!
//! The `http` types ([`Request`], [`Response`], [`HeaderMap`], [`HeaderValue`]) are often
//! assumed to be allocation-heavy. This example shows that, used deliberately, they cost a
//! single small heap allocation per request/response round trip - while a client using the
//! common convenience APIs makes dozens.
//!
//! A mocked HTTP/1.1 client sends a request and receives a response, each carrying 10 headers
//! and a 5 KiB body. The mocked connection delivers the response in 3-byte chunks, the way a slow
//! network would. All byte data - payloads, wire buffers and the receive buffer - lives in
//! [`GlobalPool`] memory managed through [`BytesBuf`] and [`BytesView`], so it does not touch the
//! heap once the pool is warm.
//!
//! The efficient client:
//!
//! - Receives bytes into a pooled [`BytesBuf`] and serializes requests into pooled memory,
//!   appending the body [`BytesView`] without copying it.
//! - Turns the complete response head into a single [`Bytes`] instance (one small allocation,
//!   no copy) and creates every header value with [`HeaderValue::from_maybe_shared`] over a
//!   slice of it, so header values share memory with the receive buffer.
//! - Resolves header names to standard or pre-built [`HeaderName`] instances instead of copying.
//! - Decomposes the request into parts and reuses its [`HeaderMap`] for the response headers.
//!   The caller hands the response's map back for the next request, so the map is allocated once.
//! - Returns the response body as a zero-copy [`BytesView`] over the receive buffer.
//!
//! The one remaining allocation is the shared owner that turns the pooled response head into
//! [`Bytes`], which is the only storage [`HeaderValue`] accepts. A naive client is measured
//! alongside for comparison.
//!
//! # Layout
//!
//! - `main.rs` - drives both clients, measures allocations and prints the report.
//! - `efficient.rs` - the allocation-efficient client and request builder.
//! - `naive.rs` - the same client written with common convenience APIs.
//! - `http1.rs` - allocation-free HTTP/1.1 parsing and formatting helpers.
//! - `mock.rs` - the mocked connection that answers in 3-byte chunks.
//!
//! [`Request`]: http::Request
//! [`Response`]: http::Response
//! [`HeaderValue`]: http::HeaderValue
//! [`HeaderValue::from_maybe_shared`]: http::HeaderValue::from_maybe_shared
//! [`BytesBuf`]: bytesbuf::BytesBuf
//! [`Bytes`]: bytes::Bytes

mod efficient;
mod http1;
mod mock;
mod naive;

use alloc_tracker::{Allocator, Session};
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use http::{HeaderMap, HeaderName, StatusCode};
use http_extensions::HttpBodyBuilder;
use layered::Service;
use tick::Clock;

use crate::efficient::{EfficientClient, build_request};
use crate::naive::{NaiveClient, build_naive_request};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const HEADER_COUNT: usize = 10;
const BODY_LEN: usize = 5 * 1024;
const CHUNK_LEN: usize = 3;
const WARMUP_REQUESTS: u64 = 100;
const MEASURED_REQUESTS: u64 = 1_000;

/// Large enough for the whole response, so the response head lands in one contiguous block.
const RECEIVE_BUFFER_LEN: usize = 8 * 1024;

// Custom header names are built once; cloning a name created from a static string never allocates.
static X_CORRELATION_ID: HeaderName = HeaderName::from_static("x-correlation-id");
static X_CLIENT_VERSION: HeaderName = HeaderName::from_static("x-client-version");

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ohno::AppError> {
    let memory = GlobalPool::new();
    let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_tokio());

    // All byte data lives in pooled memory: once the pool is warm, reserving does not touch the heap.
    let mut payload = memory.reserve(BODY_LEN);
    payload.put_byte_repeated(b'q', BODY_LEN);
    let payload = payload.consume_all();

    // Values that outlive a single request (credentials, session identifiers) are kept as
    // `Bytes`. Every request shares them through `HeaderValue::from_maybe_shared` instead of
    // copying them.
    let authorization = BytesView::copied_from_slice(b"Bearer eyJhbGciOiJSUzI1NiJ9.e30.c2lnbmF0dXJl", &memory).to_bytes();
    let correlation_id = BytesView::copied_from_slice(b"0b7e2d4c-1a3f-4e5d-9c8b-6a7f0e1d2c3b", &memory).to_bytes();

    let session = Session::new().no_stdout().no_file();

    // Efficient client.
    let client = EfficientClient::new(memory.clone(), body_builder.clone());
    let build_op = session.operation("1. efficient: build request");
    let execute_op = session.operation("2. efficient: execute");
    let mut recycled_headers = HeaderMap::with_capacity(HEADER_COUNT);

    for i in 0..WARMUP_REQUESTS + MEASURED_REQUESTS {
        let measure = i >= WARMUP_REQUESTS;

        let request = {
            let _span = measure.then(|| build_op.measure_thread().iterations(1));
            build_request(
                &body_builder,
                &payload,
                &authorization,
                &correlation_id,
                std::mem::take(&mut recycled_headers),
            )?
        };

        let response = {
            let _span = measure.then(|| execute_op.measure_thread().iterations(1));
            client.execute(request).await?
        };

        let (parts, body) = response.into_parts();
        verify_response(parts.status, &parts.headers, body.into_bytes().await?.len());

        if i == 0 {
            println!("Sample response: {} with {} headers", parts.status, parts.headers.len());
            for (name, value) in &parts.headers {
                println!("  {name}: {value:?}");
            }
            println!();
        }

        // Hand the header map back for the next request; its capacity is reused.
        recycled_headers = parts.headers;
    }

    // Naive client.
    let client = NaiveClient::new(&memory, body_builder.clone());
    let build_op = session.operation("3. naive: build request");
    let execute_op = session.operation("4. naive: execute");
    let authorization = std::str::from_utf8(&authorization)?;
    let correlation_id = std::str::from_utf8(&correlation_id)?;

    for i in 0..WARMUP_REQUESTS + MEASURED_REQUESTS {
        let measure = i >= WARMUP_REQUESTS;

        let request = {
            let _span = measure.then(|| build_op.measure_thread().iterations(1));
            build_naive_request(&body_builder, &payload, authorization, correlation_id)?
        };

        let response = {
            let _span = measure.then(|| execute_op.measure_thread().iterations(1));
            client.execute(request).await?
        };

        let (parts, body) = response.into_parts();
        verify_response(parts.status, &parts.headers, body.into_bytes().await?.len());
    }

    print_report(&session);

    Ok(())
}

fn verify_response(status: StatusCode, headers: &HeaderMap, body_len: usize) {
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.len(), HEADER_COUNT);
    assert_eq!(headers[&X_CORRELATION_ID], "0b7e2d4c-1a3f-4e5d-9c8b-6a7f0e1d2c3b");
    assert_eq!(body_len, BODY_LEN);
}

#[expect(clippy::cast_precision_loss, reason = "allocation counts are far below f64 precision limits")]
fn print_report(session: &Session) {
    let report = session.to_report();
    let mut operations: Vec<_> = report.operations().collect();
    operations.sort_by_key(|(name, _)| *name);

    println!(
        "Per-request heap usage ({HEADER_COUNT} headers and {BODY_LEN}-byte bodies each way, \
         response received in {CHUNK_LEN}-byte chunks, {MEASURED_REQUESTS} requests):"
    );
    println!("{:<30} {:>12} {:>12}", "operation", "allocations", "bytes");
    let mut totals = [(0.0, 0.0); 2];
    for (name, operation) in operations {
        let iterations = operation.total_iterations().max(1) as f64;
        let allocations = operation.total_allocations_count() as f64 / iterations;
        let bytes = operation.total_bytes_allocated() as f64 / iterations;
        println!("{name:<30} {allocations:>12.1} {bytes:>12.1}");

        let total = &mut totals[usize::from(name.contains("naive"))];
        total.0 += allocations;
        total.1 += bytes;
    }

    println!();
    println!("{:<30} {:>12.1} {:>12.1}", "efficient: round trip", totals[0].0, totals[0].1);
    println!("{:<30} {:>12.1} {:>12.1}", "naive: round trip", totals[1].0, totals[1].1);
}
