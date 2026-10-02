// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A client written with common convenience APIs, for comparison.

use std::sync::Mutex;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use http::header::CONTENT_LENGTH;
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use layered::Service;

use crate::http1::malformed;
use crate::mock::MockConnection;

/// Builds a request using common convenience APIs, for comparison.
pub(crate) fn build_naive_request(
    body_builder: &HttpBodyBuilder,
    payload: &BytesView,
    authorization: &str,
    correlation_id: &str,
) -> Result<HttpRequest, HttpError> {
    Request::builder()
        .method("POST")
        .uri("http://example.com/api/items")
        .header("host", "example.com")
        .header("user-agent", "oxidizer-demo/1.0")
        .header("accept", "application/octet-stream")
        .header("accept-encoding", "identity")
        .header("accept-language", "en-US")
        .header("content-type", "application/octet-stream")
        .header("cache-control", "no-cache")
        .header("x-client-version", "1.0.0")
        .header("authorization", authorization)
        .header("x-correlation-id", correlation_id)
        .body(body_builder.bytes(payload.to_vec()))
        .map_err(malformed)
}

/// An HTTP/1.1 client written with common convenience APIs, for comparison.
#[derive(Debug)]
pub(crate) struct NaiveClient {
    body_builder: HttpBodyBuilder,
    connection: Mutex<MockConnection>,
}

impl NaiveClient {
    pub(crate) fn new(memory: &GlobalPool, body_builder: HttpBodyBuilder) -> Self {
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
