// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serving the `tonic`-bridged service over a web stack via the `tower` adapter.
//!
//! Wraps the generated [`Transcoder`] as a [`tower_service::Service<http::Request>`]
//! with [`RestService::new`](rest_over_grpc::serving::RestService::new) — no hand-written
//! wiring closure — then drives it with [`http::Request`]s the way a `hyper` /
//! `axum` / `tower` server would. The transcoder's handler ([`LibraryService`])
//! is implemented only against `tonic`'s generated server trait; the emitted
//! bridge does the REST transcoding.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example tower_service
//! ```

use bytes::Bytes;
use http::{Method, Request};
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::serving::RestService;
use rest_over_grpc_examples::tonic_bridge::{LibraryService, Transcoder};
use tower_service::Service as _;

fn main() {
    // `RestService::new` wraps the generated `Transcoder` (any `Transcode`) as a
    // `tower` service directly — no hand-written wiring closure needed.
    let mut service = RestService::new(Transcoder::new(LibraryService));

    for target in ["/v1/shelves/history", "/v1/shelves:stream", "/v1/nope"] {
        let request = Request::builder()
            .method(Method::GET)
            .uri(target)
            .body(Full::new(Bytes::new()))
            .expect("request builds");

        // `call` is what a `hyper`/`axum`/`tower` server invokes per request.
        let response = futures::executor::block_on(service.call(request)).expect("the transcoder is infallible");

        let status = response.status().as_u16();
        let body = futures::executor::block_on(response.into_body().collect())
            .expect("body collects")
            .to_bytes();
        println!("GET {target} -> {status} {}", String::from_utf8_lossy(&body));
    }
}
