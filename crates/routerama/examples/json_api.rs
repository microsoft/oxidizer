// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded JSON requests and JSON responses.
//!
//! Run with
//! `cargo run -p routerama --example json_api --features json`.
//!
//! The additive `json` feature adds `routerama::route::json::Json<T, LIMIT>`,
//! a `#[body]` extractor whose maximum encoded size is a const generic. There
//! is no default limit and no unbounded form: every route states what it is
//! willing to buffer.
//!
//! `Json` validates the request media type before it buffers anything, so an
//! `application/json` or `application/*+json` `Content-Type` is required. Its
//! [`JsonRejection`] carries the exact reason, and the default mapping is
//! `415` for the media type, `413` for the limit, and `400` for transport
//! failures and malformed JSON. A `#[catch(JsonRejection<..>)]` method
//! replaces that default with an application-shaped error document while
//! keeping the same statuses.
//!
//! Responses are ordinary `IntoResponse` values. A JSON reply is a status, an
//! explicit `Content-Type` header, and a string body, all composed without
//! boxing.
//!
//! [`JsonRejection`]: routerama::route::json::JsonRejection

use core::convert::Infallible;

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use http_body_util::BodyExt as _;
use routerama::response::Body;
use routerama::route::json::{Json, JsonRejection};
use routerama::route::{Request, StatusCode, router};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct Document {
    title: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Serialize)]
struct DocumentReply {
    title: String,
    tags: usize,
}

#[derive(Serialize)]
struct ErrorReply {
    error: &'static str,
}

/// A JSON reply: status, media type, and an already encoded body.
type JsonReply = (StatusCode, [(HeaderName, HeaderValue); 1], String);

fn json_reply<T: Serialize>(status: StatusCode, body: &T) -> JsonReply {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        serde_json::to_string(body).expect("the example reply types serialize"),
    )
}

struct Documents;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl Documents {
    /// Buffers at most 64 bytes of JSON before decoding it.
    #[route(POST, "/documents")]
    async fn create(&self, #[body] document: Json<Document, 64>) -> JsonReply {
        let document = document.into_inner();
        json_reply(
            StatusCode::CREATED,
            &DocumentReply {
                title: document.title,
                tags: document.tags.len(),
            },
        )
    }

    /// Reports every JSON rejection as a JSON error document.
    ///
    /// The rejection's own status is preserved; only the body shape changes.
    #[catch(JsonRejection<Infallible>)]
    async fn catch_json(&self, rejection: JsonRejection<Infallible>) -> JsonReply {
        let (status, reason) = match rejection {
            JsonRejection::UnsupportedMediaType(_) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type"),
            JsonRejection::Body(_) => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            JsonRejection::Malformed(_) => (StatusCode::BAD_REQUEST, "malformed"),
        };
        json_reply(status, &ErrorReply { error: reason })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (status, media_type, body) = post("application/json; charset=utf-8", r#"{"title":"routing"}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(media_type, "application/json");
    assert_eq!(body, br#"{"title":"routing","tags":0}"#[..]);

    // Response serialization escapes request-controlled strings correctly.
    let (status, _, body) = post("application/json", r#"{"title":"quoted \"value\""}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body, br#"{"title":"quoted \"value\"","tags":0}"#[..]);

    // `application/*+json` suffixes are accepted too.
    let (status, _, body) = post("application/merge-patch+json", r#"{"title":"patch","tags":["a"]}"#).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body, br#"{"title":"patch","tags":1}"#[..]);

    // The media type is validated before the body is read.
    let (status, _, body) = post("text/plain", r#"{"title":"routing"}"#).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body, br#"{"error":"unsupported_media_type"}"#[..]);

    // The const-generic limit is enforced while buffering, before decoding.
    let (status, _, body) = post("application/json", &format!(r#"{{"title":"{}"}}"#, "x".repeat(64))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body, br#"{"error":"too_large"}"#[..]);

    // Malformed JSON inside the limit reaches the decoder and fails there.
    let (status, _, body) = post("application/json", r#"{"title":}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, br#"{"error":"malformed"}"#[..]);
}

/// Dispatches one request and returns its status, media type, and body.
///
/// Every branch of this router replies with JSON, including the catcher, so
/// the `Content-Type` header is always present.
async fn post(content_type: &'static str, payload: &str) -> (StatusCode, HeaderValue, Bytes) {
    let request = Request::post("/documents")
        .header(CONTENT_TYPE, content_type)
        .body(Body::from(payload.to_owned()))
        .expect("static request metadata is valid");
    let response = Documents.route(request, &()).await;
    let status = response.status();
    let media_type = response.headers()[CONTENT_TYPE].clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("example response bodies succeed")
        .to_bytes();
    (status, media_type, body)
}
