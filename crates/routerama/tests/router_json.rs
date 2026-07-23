// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for the additive bounded JSON extractor.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::route::json::{Json, JsonRejection};
use routerama::route::{HeaderMap, Method, Request, StatusCode, router};
use serde::Deserialize;

struct FrameBody {
    frames: VecDeque<Bytes>,
}

impl FrameBody {
    fn new(frames: Vec<Bytes>) -> Self {
        Self { frames: frames.into() }
    }
}

impl http_body::Body for FrameBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frames.pop_front().map(|frame| Ok(Frame::data(frame))))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Document {
    title: String,
}

struct JsonApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl JsonApi {
    #[route(POST, "/documents")]
    async fn create(&self, #[body] document: Json<Document, 64>, headers: HeaderMap, method: Method) -> String {
        assert_eq!(headers["x-request"], "available");
        format!("{method}:{}", document.title)
    }
}

#[tokio::test]
async fn json_uses_request_parts_and_decodes_multiple_frames() {
    let response = JsonApi
        .route(
            request(
                "application/json; charset=utf-8",
                vec![Bytes::from_static(b"{\"title\":"), Bytes::from_static(b"\"routerama\"}")],
            ),
            &(),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes();
    assert_eq!(body, b"POST:routerama"[..]);
}

#[tokio::test]
async fn structured_json_content_types_are_accepted() {
    let response = JsonApi
        .route(
            request("application/vnd.example+json", vec![Bytes::from_static(b"{\"title\":\"vendor\"}")]),
            &(),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("the generated response body succeeds")
        .to_bytes();
    assert_eq!(body, b"POST:vendor"[..]);
}

#[tokio::test]
async fn missing_unsupported_malformed_and_duplicate_content_types_are_rejected() {
    let missing = Request::builder()
        .method(Method::POST)
        .uri("/documents")
        .header("x-request", "available")
        .body(FrameBody::new(vec![Bytes::from_static(b"{\"title\":\"missing\"}")]))
        .expect("the test request uses valid static metadata");
    assert_eq!(JsonApi.route(missing, &()).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let unsupported = JsonApi
        .route(request("text/plain", vec![Bytes::from_static(b"{\"title\":\"plain\"}")]), &())
        .await;
    assert_eq!(unsupported.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let malformed = JsonApi
        .route(
            request("application/json; charset", vec![Bytes::from_static(b"{\"title\":\"malformed\"}")]),
            &(),
        )
        .await;
    assert_eq!(malformed.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let duplicate = Request::builder()
        .method(Method::POST)
        .uri("/documents")
        .header("x-request", "available")
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_TYPE, "application/problem+json")
        .body(FrameBody::new(vec![Bytes::from_static(b"{\"title\":\"duplicate\"}")]))
        .expect("the test request uses valid static metadata");
    assert_eq!(JsonApi.route(duplicate, &()).await.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn malformed_json_is_a_bad_request() {
    let response = JsonApi
        .route(request("application/json", vec![Bytes::from_static(b"{\"title\":}")]), &())
        .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oversized_json_is_rejected_before_decoding() {
    let response = JsonApi
        .route(request("application/json", vec![Bytes::from(vec![b' '; 65])]), &())
        .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

struct JsonCatcher;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl JsonCatcher {
    #[route(POST, "/caught-json")]
    async fn json(&self, #[body] document: Json<Document, 64>) -> String {
        document.title.clone()
    }

    #[catch(JsonRejection<Infallible>)]
    async fn catch_json(&self, _rejection: JsonRejection<Infallible>) -> (StatusCode, &'static str) {
        (StatusCode::UNPROCESSABLE_ENTITY, "json-caught")
    }
}

#[tokio::test]
async fn json_rejections_can_use_a_typed_catcher() {
    let request = Request::post("/caught-json")
        .header(CONTENT_TYPE, "application/json")
        .body(FrameBody::new(vec![Bytes::from_static(b"{\"title\":}")]))
        .expect("the test request uses valid static metadata");
    let response = JsonCatcher.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .expect("the generated response body succeeds")
            .to_bytes(),
        b"json-caught"[..]
    );
}

fn request(content_type: &'static str, frames: Vec<Bytes>) -> Request<FrameBody> {
    Request::builder()
        .method(Method::POST)
        .uri("/documents")
        .header("x-request", "available")
        .header(CONTENT_TYPE, content_type)
        .body(FrameBody::new(frames))
        .expect("the test request uses valid static metadata")
}
