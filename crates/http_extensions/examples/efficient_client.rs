// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates that the `http` crate is viable for allocation-sensitive HTTP clients.
//!
//! The `http` types ([`Request`], [`Response`], [`HeaderMap`], [`HeaderValue`]) are often
//! assumed to be allocation-heavy. This example shows that, used deliberately, they cost only
//! two small heap allocations per request/response round trip - while a client using the
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
//! The two remaining allocations are the unique per-request `x-request-id` value and the shared
//! owner of the response head. A naive client is measured alongside for comparison.

use std::sync::Mutex;

use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use http::header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HOST, USER_AGENT};
use http::request::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri, Version};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use layered::Service;
use recoverable::RecoveryInfo;
use tick::Clock;

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
static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
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
                i,
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
            build_naive_request(&body_builder, &payload, authorization, correlation_id, i)?
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

/// Builds a request that reuses `headers` (and its capacity) from a previous response.
///
/// Static values use [`HeaderValue::from_static`] and shared values use
/// [`HeaderValue::from_maybe_shared`]; neither allocates. The per-request identifier is the only
/// value that needs fresh storage.
fn build_request(
    body_builder: &HttpBodyBuilder,
    payload: &BytesView,
    authorization: &Bytes,
    correlation_id: &Bytes,
    request_id: u64,
    mut headers: HeaderMap,
) -> Result<HttpRequest, HttpError> {
    headers.clear();
    headers.reserve(HEADER_COUNT);

    headers.insert(HOST, HeaderValue::from_static("example.com"));
    headers.insert(USER_AGENT, HeaderValue::from_static("oxidizer-demo/1.0"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/octet-stream"));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(X_CLIENT_VERSION.clone(), HeaderValue::from_static("1.0.0"));

    // Cloning `Bytes` only bumps a reference count; `from_maybe_shared` validates without copying.
    headers.insert(AUTHORIZATION, shared_value(authorization.clone())?);
    headers.insert(X_CORRELATION_ID.clone(), shared_value(correlation_id.clone())?);

    // A unique per-request value needs storage of its own: exactly one small allocation.
    // `HeaderValue::from(request_id)` would make two, because it over-allocates a `BytesMut`
    // and then needs a shared header when freezing it.
    let mut digits = [0; 20];
    let request_id = Bytes::copy_from_slice(format_decimal(request_id, &mut digits));
    headers.insert(X_REQUEST_ID.clone(), shared_value(request_id)?);

    // Cloning a view shares the pooled payload memory; nothing is copied or allocated.
    let mut request = Request::new(body_builder.bytes(payload.clone()));
    *request.method_mut() = Method::POST;
    *request.uri_mut() = Uri::from_static("http://example.com/api/items");
    *request.headers_mut() = headers;

    Ok(request)
}

fn malformed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), "malformed_message")
}

fn shared_value(bytes: Bytes) -> Result<HeaderValue, HttpError> {
    HeaderValue::from_maybe_shared(bytes).map_err(malformed)
}

/// An HTTP/1.1 client that keeps per-request heap allocations to a minimum.
#[derive(Debug)]
struct EfficientClient {
    memory: GlobalPool,
    body_builder: HttpBodyBuilder,
    connection: Mutex<MockConnection>,
}

impl EfficientClient {
    fn new(memory: GlobalPool, body_builder: HttpBodyBuilder) -> Self {
        Self {
            connection: Mutex::new(MockConnection::new(&memory)),
            memory,
            body_builder,
        }
    }

    /// Serializes the request head into pooled memory and appends the body without copying it.
    fn serialize_request(&self, parts: &Parts, body: BytesView) -> BytesView {
        let method = parts.method.as_str().as_bytes();
        let target = parts.uri.path_and_query().map_or("/", |p| p.as_str()).as_bytes();
        let mut digits = [0; 20];
        let content_length = format_decimal(body.len() as u64, &mut digits);

        let head_len = method.len()
            + 1
            + target.len()
            + b" HTTP/1.1\r\n".len()
            + parts.headers.iter().map(|(n, v)| n.as_str().len() + 2 + v.len() + 2).sum::<usize>()
            + b"content-length: ".len()
            + content_length.len()
            + b"\r\n\r\n".len();

        let mut head = self.memory.reserve(head_len);
        head.put_slice(method);
        head.put_byte(b' ');
        head.put_slice(target);
        head.put_slice(b" HTTP/1.1\r\n".as_slice());
        for (name, value) in &parts.headers {
            head.put_slice(name.as_str().as_bytes());
            head.put_slice(b": ".as_slice());
            head.put_slice(value.as_bytes());
            head.put_slice(b"\r\n".as_slice());
        }
        head.put_slice(b"content-length: ".as_slice());
        head.put_slice(content_length);
        head.put_slice(b"\r\n\r\n".as_slice());

        let mut wire = head.consume_all();
        wire.append(body);
        wire
    }

    /// Reads one response, appending its headers to `headers`.
    fn read_response(&self, connection: &mut MockConnection, headers: &mut HeaderMap) -> Result<(StatusCode, BytesView), HttpError> {
        let mut receive = self.memory.reserve(RECEIVE_BUFFER_LEN);

        let mut terminator = HeadTerminator::default();
        let head_len = loop {
            let scanned = receive.len();
            self.receive_chunk(connection, &mut receive)?;

            let mut received = receive.peek();
            received.advance(scanned);
            if let Some(len) = terminator.scan(&received) {
                break scanned + len;
            }
        };

        // One small allocation for the shared owner. No data is copied as long as the head
        // occupies a single memory block, which the receive buffer size guarantees here.
        let head = receive.consume(head_len).to_bytes();
        let status = parse_head(&head, headers)?;

        let body_len = headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or_else(|| HttpError::validation("missing or invalid content-length"))?;

        while receive.len() < body_len {
            self.receive_chunk(connection, &mut receive)?;
        }

        // Zero-copy: the body is a view over the receive buffer.
        Ok((status, receive.consume(body_len)))
    }

    fn receive_chunk(&self, connection: &mut MockConnection, receive: &mut BytesBuf) -> Result<(), HttpError> {
        if receive.remaining_capacity() < CHUNK_LEN {
            receive.reserve(RECEIVE_BUFFER_LEN, &self.memory);
        }

        let chunk = connection.read();
        if chunk.is_empty() {
            return Err(HttpError::validation("connection closed before the response was complete"));
        }

        receive.put_slice(chunk);
        Ok(())
    }
}

impl Service<HttpRequest> for EfficientClient {
    type Out = Result<HttpResponse, HttpError>;

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        let (mut parts, body) = input.into_parts();
        let body = body.into_bytes().await?;

        let mut connection = self
            .connection
            .lock()
            .expect("lock is never poisoned because no code panics while holding it");
        connection.write(&self.serialize_request(&parts, body));

        // The request headers are on the wire; recycle the map and its capacity for the response.
        parts.headers.clear();
        let (status, body) = self.read_response(&mut connection, &mut parts.headers)?;

        let mut response = Response::new(self.body_builder.bytes(body));
        *response.status_mut() = status;
        *response.version_mut() = Version::HTTP_11;
        *response.headers_mut() = parts.headers;

        Ok(response)
    }
}

/// Tracks progress towards the `\r\n\r\n` sequence that terminates an HTTP/1.1 head, so each
/// received byte is inspected exactly once no matter how the data is chunked.
#[derive(Debug, Default)]
struct HeadTerminator {
    matched: usize,
}

impl HeadTerminator {
    const PATTERN: &[u8] = b"\r\n\r\n";

    /// Returns the head length relative to the start of `data`, once the terminator is found.
    fn scan(&mut self, data: &BytesView) -> Option<usize> {
        let mut offset = 0;
        for (slice, _meta) in data.slices() {
            for &byte in slice {
                offset += 1;
                self.matched = if byte == Self::PATTERN[self.matched] {
                    self.matched + 1
                } else {
                    usize::from(byte == b'\r')
                };

                if self.matched == Self::PATTERN.len() {
                    return Some(offset);
                }
            }
        }

        None
    }
}

/// Parses the status line and headers. Header values are zero-copy slices of `head`.
fn parse_head(head: &Bytes, headers: &mut HeaderMap) -> Result<StatusCode, HttpError> {
    let mut lines = HeadLines { head, position: 0 };

    let status_line = lines.next().ok_or_else(|| HttpError::validation("missing status line"))?;
    let status = head[status_line]
        .strip_prefix(b"HTTP/1.1 ")
        .and_then(|rest| rest.get(..3))
        .and_then(|code| StatusCode::from_bytes(code).ok())
        .ok_or_else(|| HttpError::validation("invalid status line"))?;

    for line in lines {
        let colon = head[line.clone()]
            .iter()
            .position(|&b| b == b':')
            .ok_or_else(|| HttpError::validation("invalid header line"))?;

        let name = resolve_header_name(&head[line.start..line.start + colon])?;

        let mut value = line.start + colon + 1..line.end;
        while value.start < value.end && head[value.start].is_ascii_whitespace() {
            value.start += 1;
        }
        while value.end > value.start && head[value.end - 1].is_ascii_whitespace() {
            value.end -= 1;
        }

        // `Bytes::slice` only bumps a reference count, so the value shares the head's memory.
        headers.append(name, shared_value(head.slice(value))?);
    }

    Ok(status)
}

/// Resolves a header name without allocating for standard and expected custom names.
fn resolve_header_name(name: &[u8]) -> Result<HeaderName, HttpError> {
    for known in [&X_REQUEST_ID, &X_CORRELATION_ID] {
        if name.eq_ignore_ascii_case(known.as_str().as_bytes()) {
            return Ok(known.clone());
        }
    }

    // Standard names are recognized (case-insensitively) without allocating; other names are copied.
    HeaderName::from_bytes(name).map_err(malformed)
}

/// Iterates the byte ranges of the lines in an HTTP/1.1 head, stopping at the empty line.
#[derive(Debug)]
struct HeadLines<'a> {
    head: &'a [u8],
    position: usize,
}

impl Iterator for HeadLines<'_> {
    type Item = std::ops::Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.head.get(self.position..)?;
        let len = rest.windows(2).position(|w| w == b"\r\n")?;
        let line = self.position..self.position + len;
        self.position = line.end + 2;
        (!line.is_empty()).then_some(line)
    }
}

/// Formats `value` as decimal digits into `buffer` without allocating.
fn format_decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut start = buffer.len();
    loop {
        start -= 1;
        let digit = u8::try_from(value % 10).expect("a single decimal digit always fits in u8");
        buffer[start] = b'0' + digit;
        value /= 10;
        if value == 0 {
            return &buffer[start..];
        }
    }
}

/// Builds a request using common convenience APIs, for comparison.
fn build_naive_request(
    body_builder: &HttpBodyBuilder,
    payload: &BytesView,
    authorization: &str,
    correlation_id: &str,
    request_id: u64,
) -> Result<HttpRequest, HttpError> {
    Request::builder()
        .method("POST")
        .uri("http://example.com/api/items")
        .header("host", "example.com")
        .header("user-agent", "oxidizer-demo/1.0")
        .header("accept", "application/octet-stream")
        .header("accept-encoding", "identity")
        .header("content-type", "application/octet-stream")
        .header("cache-control", "no-cache")
        .header("x-client-version", "1.0.0")
        .header("authorization", authorization)
        .header("x-correlation-id", correlation_id)
        .header("x-request-id", request_id.to_string())
        .body(body_builder.bytes(payload.to_vec()))
        .map_err(malformed)
}

/// An HTTP/1.1 client written with common convenience APIs, for comparison.
#[derive(Debug)]
struct NaiveClient {
    body_builder: HttpBodyBuilder,
    connection: Mutex<MockConnection>,
}

impl NaiveClient {
    fn new(memory: &GlobalPool, body_builder: HttpBodyBuilder) -> Self {
        Self {
            body_builder,
            connection: Mutex::new(MockConnection::new(memory)),
        }
    }
}

impl Service<HttpRequest> for NaiveClient {
    type Out = Result<HttpResponse, HttpError>;

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        let (parts, body) = input.into_parts();
        let body = body.into_bytes().await?;

        let mut wire = format!("{} {} HTTP/1.1\r\n", parts.method, parts.uri.path()).into_bytes();
        for (name, value) in &parts.headers {
            let value = value.to_str().map_err(malformed)?;
            wire.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        wire.extend_from_slice(format!("content-length: {}\r\n\r\n", body.len()).as_bytes());
        wire.extend_from_slice(&body.to_vec());

        let mut connection = self
            .connection
            .lock()
            .expect("lock is never poisoned because no code panics while holding it");
        connection.write(&BytesView::from(wire));

        let mut received = Vec::new();
        let head_len = loop {
            received.extend_from_slice(connection.read());
            if let Some(position) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                break position + 4;
            }
        };

        let head = String::from_utf8(received[..head_len].to_vec()).map_err(malformed)?;
        let mut lines = head.split("\r\n").filter(|line| !line.is_empty());
        let status: StatusCode = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| HttpError::validation("invalid status line"))?;

        let mut headers = HeaderMap::new();
        for line in lines {
            let (name, value) = line.split_once(':').ok_or_else(|| HttpError::validation("invalid header line"))?;
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(malformed)?;
            let value = HeaderValue::from_str(value.trim()).map_err(malformed)?;
            headers.append(name, value);
        }

        let body_len: usize = headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| HttpError::validation("missing or invalid content-length"))?;

        while received.len() < head_len + body_len {
            received.extend_from_slice(connection.read());
        }

        let body = BytesView::from(received[head_len..].to_vec());
        let mut response = Response::builder()
            .status(status)
            .body(self.body_builder.bytes(body))
            .map_err(malformed)?;
        *response.headers_mut() = headers;

        Ok(response)
    }
}

/// A mocked HTTP/1.1 connection. Each request is answered with a canned response that is
/// delivered in chunks of [`CHUNK_LEN`] bytes.
#[derive(Debug)]
struct MockConnection {
    response: BytesView,
    position: usize,
}

impl MockConnection {
    fn new(memory: &GlobalPool) -> Self {
        let head = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/octet-stream\r\n\
             Content-Length: {BODY_LEN}\r\n\
             Date: Thu, 01 Oct 2026 12:00:00 GMT\r\n\
             Server: mock/1.0\r\n\
             Cache-Control: no-store\r\n\
             ETag: \"5f3c-1a2b\"\r\n\
             Vary: Accept-Encoding\r\n\
             Connection: keep-alive\r\n\
             X-Request-Id: 4f6c1e2a-9b8d-4c3e-a1f0-7d2e5b6c8a90\r\n\
             X-Correlation-Id: 0b7e2d4c-1a3f-4e5d-9c8b-6a7f0e1d2c3b\r\n\
             \r\n"
        );

        // The canned response lives in pooled memory, in a single block.
        let mut response = memory.reserve(head.len() + BODY_LEN);
        response.put_slice(head.as_bytes());
        response.put_byte_repeated(b'r', BODY_LEN);
        let response = response.consume_all();
        assert_eq!(
            response.first_slice().len(),
            response.len(),
            "a reservation smaller than the block size occupies a single span"
        );

        let position = response.len();
        Self { response, position }
    }

    /// Sends a request; the mocked server answers it with the canned response.
    fn write(&mut self, request: &BytesView) {
        assert!(request.len() > BODY_LEN, "the request must carry its body");
        self.position = 0;
    }

    /// Returns the next chunk of the response, or an empty slice once it is fully read.
    fn read(&mut self) -> &[u8] {
        let start = self.position;
        self.position = (start + CHUNK_LEN).min(self.response.len());
        &self.response.first_slice()[start..self.position]
    }
}

fn verify_response(status: StatusCode, headers: &HeaderMap, body_len: usize) {
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.len(), HEADER_COUNT);
    assert_eq!(headers[&X_REQUEST_ID], "4f6c1e2a-9b8d-4c3e-a1f0-7d2e5b6c8a90");
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
