// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Extending a generated service with hand-written routes and a custom 404,
//! using `try_transcode`.
//!
//! `try_transcode` returns `None` only when no generated route matches the
//! method and path — distinct from a handler's own not-found, which comes back
//! as `Some(404)`. That lets a caller layer extra endpoints and a bespoke
//! fallback around the transcoder without swallowing genuine handler errors.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example custom_fallback
//! ```

use http::HeaderMap;
use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
use rest_over_grpc_examples::tonic_bridge::{LibraryService, Transcoder};

/// Handles a request, adding a hand-written `/healthz` endpoint alongside the
/// generated REST routes and a custom 404 for anything unmatched.
async fn handle(library: &Transcoder<LibraryService>, method: &str, target: &str, headers: HeaderMap, body: &[u8]) -> TranscodeResponse {
    let path = target.split('?').next().unwrap_or(target);

    // A non-generated route, handled before the transcoder.
    if method == "GET" && path == "/healthz" {
        return HttpResponse::ok_json(br#"{"status":"ok"}"#.to_vec()).into();
    }

    // Delegate to the generated routes. `None` means none matched — a genuine
    // routing miss, as opposed to a handler returning `Status::not_found`, which
    // comes back as `Some(404)` and is served as-is.
    library
        .try_transcode(method, target, headers, body)
        .await
        .unwrap_or_else(custom_not_found)
}

/// A bespoke 404 body, richer than the transcoder's default status JSON.
fn custom_not_found() -> TranscodeResponse {
    HttpResponse::json(
        http::StatusCode::NOT_FOUND,
        br#"{"error":"unknown endpoint","see":"https://example.test/docs/api"}"#.to_vec(),
    )
    .into()
}

fn main() {
    let library = Transcoder::new(LibraryService);

    let requests = [
        ("GET", "/v1/shelves/history"), // a generated route
        ("GET", "/healthz"),            // the hand-written route
        ("GET", "/v1/shelves/missing"), // matched route, handler returns not-found
        ("GET", "/nope"),               // no route → custom fallback
    ];

    for (method, target) in requests {
        // Every route in this example is unary, so the response is always buffered.
        let TranscodeResponse::Unary(response) = futures::executor::block_on(handle(&library, method, target, HeaderMap::new(), b""))
        else {
            unreachable!("this example only exercises unary routes");
        };
        println!(
            "{method} {target} -> {} {}",
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()),
        );
    }
}
