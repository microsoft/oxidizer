// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Real server-streaming to the wire via `transcode`.
//!
//! The `transcode` method yields a `TranscodeResponse` (a buffered unary
//! `HttpResponse` or a `StreamingResponse`), and the `serve_http` /
//! `RestService` adapters forward each encoding frame to the client as the
//! handler produces it — so a slow or unbounded stream reaches the client
//! incrementally instead of all at once, while a unary reply stays buffered.
//!
//! This example negotiates NDJSON via `Accept` and prints each frame as it
//! arrives from the streaming body, then contrasts it with a unary route (which
//! the same adapter buffers).
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example streaming_response
//! ```

use http::header::{ACCEPT, CONTENT_TYPE};
use http::{Method, Request};
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::serving::serve_http;
use rest_over_grpc_examples::tonic_bridge::{LibraryService, Transcoder};

async fn run(library: &'static Transcoder<LibraryService>, method: Method, target: &str, accept: &str) {
    let request = Request::builder()
        .method(method.clone())
        .uri(target)
        .header(ACCEPT, accept)
        .body(Full::new(bytes::Bytes::new()))
        .expect("request builds");

    let response = serve_http(request, library).await;

    println!("\n{method} {target}  (Accept: {accept})");
    println!("  -> {} {}", response.status(), content_type(&response));

    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(frame) => {
                if let Some(data) = frame.data_ref() {
                    print!("  frame: {}", String::from_utf8_lossy(data));
                }
            }
            Err(error) => {
                println!("  stream error: {error}");
                break;
            }
        }
    }
}

fn content_type(response: &http::Response<rest_over_grpc::serving::RestBody>) -> String {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn main() {
    let library: &'static Transcoder<LibraryService> = Box::leak(Box::new(Transcoder::new(LibraryService)));

    futures::executor::block_on(async {
        run(library, Method::GET, "/v1/shelves:stream", "application/x-ndjson").await;

        run(library, Method::GET, "/v1/shelves:stream", "application/json").await;

        run(library, Method::GET, "/v1/shelves/history", "application/json").await;
    });
}
