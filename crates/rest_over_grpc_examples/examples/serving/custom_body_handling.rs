// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Custom request-body handling by calling the generated `transcode` directly.
//!
//! The `serve_http` / `RestService` adapters keep body handling simple: they
//! buffer the whole body (with only an optional `RestService::with_max_body_bytes`
//! size cap) and return an `http::Response<RestBody>` carrying just a status and
//! content type. When you need an HTTP-level check before transcoding (like
//! a `415`), a custom over-limit response body, or a cross-cutting response
//! header, own the body step yourself: read the bytes under your own policy,
//! call the generated `transcode` with them, then build your own response.
//!
//! The generated `transcode` takes the body as a complete `&[u8]` (it decodes the
//! whole thing as JSON), so this is about *what policy produces those bytes and
//! how you build the response* — not about streaming the body into the decoder.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example custom_body_handling
//! ```

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
use rest_over_grpc_examples::custom::{InMemoryLibrary, Transcoder};

/// A deliberately small cap so the example can show a `413` without a huge body.
const MAX_BODY: usize = 1024;

/// Marker for a body that exceeds [`MAX_BODY`].
struct TooLarge;

async fn handle(library: &Transcoder<InMemoryLibrary>, request: Request<Full<Bytes>>) -> Response<Vec<u8>> {
    let (parts, body) = request.into_parts();

    // Reject the wrong content type up front (`415`), rather than letting it
    // surface later as an opaque JSON parse error: the generated `transcode`
    // decodes the body as JSON without checking `Content-Type`.
    let expects_body = matches!(parts.method, Method::POST | Method::PUT | Method::PATCH);
    if expects_body && !is_json(parts.headers.get(CONTENT_TYPE)) {
        return json(StatusCode::UNSUPPORTED_MEDIA_TYPE, br#"{"error":"expected application/json"}"#);
    }

    // Read the body under a size cap, returning a custom `413` body. The
    // built-in `RestService::with_max_body_bytes` cap returns a generic `413`;
    // owning the read lets us shape the response.
    let bytes = match read_capped(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(TooLarge) => return json(StatusCode::PAYLOAD_TOO_LARGE, br#"{"error":"payload too large"}"#),
    };

    // Hand the finished bytes and request headers to the generated transcoder —
    // the neutral contract. Every route this example exercises is unary, so the
    // response is always buffered.
    let target = parts.uri.path_and_query().map_or("/", |pq| pq.as_str());
    let TranscodeResponse::Unary(response) = library.transcode(parts.method.as_str(), target, parts.headers, &bytes).await else {
        return json(StatusCode::INTERNAL_SERVER_ERROR, br#"{"error":"unexpected streaming response"}"#);
    };

    // Build our own wire response, adding a header the adapters can't attach.
    Response::builder()
        .status(response.status())
        .header(CONTENT_TYPE, response.content_type())
        .header("access-control-allow-origin", "*")
        .body(response.into_body())
        .expect("response builds")
}

/// Collects a body, failing if it exceeds `max`.
///
/// A real server reading a streaming body would track the running total and bail
/// as soon as it crosses `max`; a [`Full`] body is already in memory, so here we
/// simply check the collected length.
async fn read_capped(body: Full<Bytes>, max: usize) -> Result<Bytes, TooLarge> {
    let bytes = body.collect().await.map(http_body_util::Collected::to_bytes).unwrap_or_default();
    if bytes.len() > max { Err(TooLarge) } else { Ok(bytes) }
}

fn is_json(value: Option<&HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

fn json(status: StatusCode, body: &[u8]) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .expect("response builds")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let library = Transcoder::new(InMemoryLibrary);

    // (label, method, target, content-type, body)
    let requests = [
        ("read", Method::GET, "/v1/shelves/history", None, Vec::new()),
        (
            "create",
            Method::POST,
            "/v1/shelves",
            Some("application/json"),
            br#"{"theme":"mystery"}"#.to_vec(),
        ),
        (
            "wrong content type",
            Method::POST,
            "/v1/shelves",
            Some("text/plain"),
            b"not json".to_vec(),
        ),
        (
            "over cap",
            Method::POST,
            "/v1/shelves",
            Some("application/json"),
            vec![b'a'; MAX_BODY + 1],
        ),
    ];

    for (label, method, target, content_type, body) in requests {
        let mut builder = Request::builder().method(method.clone()).uri(target);
        if let Some(content_type) = content_type {
            builder = builder.header(CONTENT_TYPE, content_type);
        }
        let request = builder.body(Full::new(Bytes::from(body))).expect("request builds");

        let response = handle(&library, request).await;

        println!(
            "{label}: {method} {target} -> {} {}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}
