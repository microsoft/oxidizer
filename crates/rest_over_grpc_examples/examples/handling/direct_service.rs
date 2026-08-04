// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unknown_lints, reason = "the pinned and latest Clippy versions expose different async-trait lints")]
#![expect(
    clippy::unused_async_trait_impl,
    reason = "synchronous example implements the generated async service trait"
)]

//! Implementing the generated service trait directly.
//!
//! When you are not bridging an existing gRPC stack (`tonic` — see
//! [`basic_transcode`](../transcoding/basic_transcode.rs); another framework —
//! see [`volo_bridge`](volo_bridge.rs)), you implement the generated service
//! trait yourself. The shape is one `async fn` per RPC that takes the
//! decoded request message plus a `&mut Context` and returns `Result<Reply,
//! Status>` (or `Result<ResponseStream<Reply>, Status>` for a server-streaming
//! RPC). `rest_over_grpc::build` generates the trait from your annotated proto;
//! you supply the bodies.
//!
//! This example implements the `Library` trait on a hand-written [`DirectLibrary`]
//! and drives it through the generated [`Transcoder`], showing the three things
//! the handling layer gives you beyond plain request/response:
//!
//! - **Request metadata** — read the request-side gRPC metadata (the HTTP request
//!   headers) via [`Context::request_headers`].
//! - **Response metadata** — set response headers (`Location`, `ETag`, an echoed
//!   trace id) via [`Context::insert_response_header`] /
//!   [`Context::response_headers_mut`]; the transcoder merges them into the reply.
//! - **Rich errors** — return a [`Status`] whose [`Code`] maps to an HTTP status,
//!   optionally carrying `google.rpc`-style structured details.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example direct_service
//! ```

use futures::StreamExt as _;
use futures::executor::block_on;
use http::{HeaderMap, HeaderName, HeaderValue};
use rest_over_grpc::handling::{Context, ResponseStream, Status};
use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
use rest_over_grpc_examples::custom::Transcoder;
use rest_over_grpc_examples::custom::pb::{
    self, CreateShelfRequest, Genre, GetShelfRequest, ListShelvesByGenreRequest, ListShelvesRequest, ListShelvesResponse, Shelf,
};

/// A hand-written implementation of the generated `Library` service trait,
/// backed by a fixed in-memory catalog. In a real service the bodies would call
/// your domain logic; the shapes — decoded request in, `&mut Context` for
/// metadata, `Result<Reply, Status>` out — are exactly what the generated trait
/// prescribes.
#[derive(Clone)]
struct DirectLibrary;

impl pb::Library for DirectLibrary {
    async fn get_shelf(&self, request: GetShelfRequest, cx: &mut Context) -> Result<Shelf, Status> {
        if request.shelf == "missing" {
            return Err(Status::not_found("no such shelf").with_detail(serde_json::json!({
                "@type": "type.googleapis.com/google.rpc.ResourceInfo",
                "resourceType": "library.Shelf",
                "resourceName": format!("shelves/{}", request.shelf),
            })));
        }

        if let Some(trace) = cx.request_headers().get("x-trace-id").cloned() {
            _ = cx.response_headers_mut().insert(HeaderName::from_static("x-trace-id"), trace);
        }
        cx.insert_response_header(HeaderName::from_static("etag"), HeaderValue::from_static("\"shelf-v1\""));

        Ok(Shelf {
            name: format!("shelves/{}", request.shelf),
            theme: "history".to_owned(),
        })
    }

    async fn create_shelf(&self, request: CreateShelfRequest, cx: &mut Context) -> Result<Shelf, Status> {
        let mut created = request.shelf.ok_or_else(|| Status::invalid_argument("shelf is required"))?;
        "shelves/created".clone_into(&mut created.name);
        _ = cx.insert_response_header(HeaderName::from_static("location"), HeaderValue::from_static("/v1/shelves/created"));
        Ok(created)
    }

    async fn list_shelves(&self, request: ListShelvesRequest, _cx: &mut Context) -> Result<ListShelvesResponse, Status> {
        Ok(ListShelvesResponse {
            shelves: catalog(&request.filter),
        })
    }

    async fn list_shelves_by_genre(&self, request: ListShelvesByGenreRequest, _cx: &mut Context) -> Result<ListShelvesResponse, Status> {
        let theme = match request.genre() {
            Genre::History => "history",
            Genre::Science => "science",
            Genre::Unspecified => return Err(Status::invalid_argument("genre is required")),
        };
        Ok(ListShelvesResponse { shelves: catalog(theme) })
    }

    async fn stream_shelves(&self, request: ListShelvesRequest, _cx: &mut Context) -> Result<ResponseStream<Shelf>, Status> {
        let shelves: Vec<Result<Shelf, Status>> = catalog(&request.filter).into_iter().map(Ok).collect();
        Ok(Box::pin(futures::stream::iter(shelves)))
    }
}

/// The fixed catalog, optionally filtered by theme.
fn catalog(filter: &str) -> Vec<Shelf> {
    [("shelves/1", "history"), ("shelves/2", "science")]
        .into_iter()
        .filter(|(_, theme)| filter.is_empty() || *theme == filter)
        .map(|(name, theme)| Shelf {
            name: name.to_owned(),
            theme: theme.to_owned(),
        })
        .collect()
}

fn main() {
    let library = Transcoder::new(DirectLibrary);

    let mut headers = HeaderMap::new();
    _ = headers.insert("x-trace-id", HeaderValue::from_static("trace-42"));
    report_unary(
        "GET /v1/shelves/history",
        block_on(library.transcode("GET", "/v1/shelves/history", headers, b"")),
    );

    report_unary(
        "GET /v1/shelves/missing",
        block_on(library.transcode("GET", "/v1/shelves/missing", HeaderMap::new(), b"")),
    );

    report_unary(
        "POST /v1/shelves",
        block_on(library.transcode("POST", "/v1/shelves", HeaderMap::new(), br#"{"theme":"mystery"}"#)),
    );

    report_streaming(
        "GET /v1/shelves:stream",
        block_on(library.transcode("GET", "/v1/shelves:stream", HeaderMap::new(), b"")),
    );
}

/// Prints a unary reply: its status, the response headers the handler set, and
/// the JSON body.
fn report_unary(label: &str, response: TranscodeResponse) {
    let TranscodeResponse::Unary(http) = response else {
        unreachable!("the driven routes are unary");
    };
    let interesting = ["etag", "location", "x-trace-id"]
        .into_iter()
        .filter_map(|name| {
            http.headers()
                .get(name)
                .map(|value| format!("{name}: {}", value.to_str().unwrap_or("?")))
        })
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "{label} -> {} [{interesting}] {}",
        http.status().as_u16(),
        String::from_utf8_lossy(http.body())
    );
}

/// Prints a streaming reply by collecting its frames (each already encoded in the
/// negotiated `Content-Type`, JSON-array by default).
fn report_streaming(label: &str, response: TranscodeResponse) {
    let TranscodeResponse::Streaming(stream) = response else {
        unreachable!("the streaming route is server-streaming");
    };
    let content_type = stream.content_type().clone();
    let body = block_on(async {
        let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
        frames.concat()
    });
    println!(
        "{label} -> streaming [{}] {}",
        content_type.to_str().expect("generated content type is ASCII"),
        String::from_utf8_lossy(&body)
    );
}
