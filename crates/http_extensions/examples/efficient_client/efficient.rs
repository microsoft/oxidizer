// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The allocation-efficient client.

use std::sync::Mutex;

use bytes::Bytes;
use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use http::header::{
    ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HOST, USER_AGENT,
};
use http::request::Parts;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, Version};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use layered::Service;

use crate::http1::{HeadTerminator, format_decimal, parse_head, shared_value};
use crate::mock::MockConnection;
use crate::{CHUNK_LEN, HEADER_COUNT, RECEIVE_BUFFER_LEN, X_CLIENT_VERSION, X_CORRELATION_ID};

/// Builds a request that reuses `headers` (and its capacity) from a previous response.
///
/// Static values use [`HeaderValue::from_static`] and shared values use
/// [`HeaderValue::from_maybe_shared`]; neither allocates.
pub(crate) fn build_request(
    body_builder: &HttpBodyBuilder,
    payload: &BytesView,
    authorization: &Bytes,
    correlation_id: &Bytes,
    mut headers: HeaderMap,
) -> Result<HttpRequest, HttpError> {
    headers.clear();
    headers.reserve(HEADER_COUNT);

    headers.insert(HOST, HeaderValue::from_static("example.com"));
    headers.insert(USER_AGENT, HeaderValue::from_static("oxidizer-demo/1.0"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/octet-stream"));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(X_CLIENT_VERSION.clone(), HeaderValue::from_static("1.0.0"));

    // Cloning `Bytes` only bumps a reference count; `from_maybe_shared` validates without copying.
    headers.insert(AUTHORIZATION, shared_value(authorization.clone())?);
    headers.insert(X_CORRELATION_ID.clone(), shared_value(correlation_id.clone())?);

    // Cloning a view shares the pooled payload memory; nothing is copied or allocated.
    let mut request = Request::new(body_builder.bytes(payload.clone()));
    *request.method_mut() = Method::POST;
    *request.uri_mut() = Uri::from_static("http://example.com/api/items");
    *request.headers_mut() = headers;

    Ok(request)
}

/// An HTTP/1.1 client that keeps per-request heap allocations to a minimum.
#[derive(Debug)]
pub(crate) struct EfficientClient {
    memory: GlobalPool,
    body_builder: HttpBodyBuilder,
    connection: Mutex<MockConnection>,
}

impl EfficientClient {
    pub(crate) fn new(memory: GlobalPool, body_builder: HttpBodyBuilder) -> Self {
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
