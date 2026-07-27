// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transcoding REST/JSON requests to a `tonic`-bridged service.
//!
//! [`LibraryService`] implements only `tonic`'s generated server trait; the
//! blanket `impl` emitted by `rest_over_grpc::build` makes it a `rest_over_grpc`
//! service too, so wrapping it in the generated [`Transcoder`] is all it takes to
//! transcode REST requests — `(method, target, headers, body)` in, a
//! [`TranscodeResponse`](rest_over_grpc::transcoding::TranscodeResponse) out
//! (a buffered unary reply or a server-streaming frame stream).
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example basic_transcode
//! ```

use futures::StreamExt as _;
use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
use rest_over_grpc_examples::tonic_bridge::{LibraryService, Transcoder};

fn main() {
    let library = Transcoder::new(LibraryService);

    let requests = [
        ("GET", "/v1/shelves/history"), // unary route (GetShelf)
        ("GET", "/v1/shelves:stream"),  // server-streaming route (frame stream)
        ("GET", "/v1/nope"),            // no route → 404
    ];

    for (method, target) in requests {
        // `transcode` resolves the route, transcodes the request into the gRPC
        // message, invokes the bridged handler, and encodes the reply as JSON —
        // buffered for a unary RPC, or as a frame stream for a streaming RPC.
        let response = futures::executor::block_on(library.transcode(method, target, http::HeaderMap::new(), b""));
        match response {
            TranscodeResponse::Unary(http) => {
                println!(
                    "{method} {target} -> {} {}",
                    http.status().as_u16(),
                    String::from_utf8_lossy(http.body())
                );
            }
            TranscodeResponse::Streaming(stream) => {
                let body = futures::executor::block_on(async {
                    let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
                    frames.concat()
                });
                println!("{method} {target} -> streaming {}", String::from_utf8_lossy(&body));
            }
        }
    }
}
