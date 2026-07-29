// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the full generated `Transcoder` in the
//! `rest_over_grpc_tests` crate.
//!
//! Drives `Transcoder::transcode` end to end (route resolution, request transcoding,
//! the trivial `InMemoryLibrary` handler, JSON response encode, and response
//! assembly), one call per transcoding shape. The JSON decode/encode allocates, and
//! Callgrind models allocation as a fixed cost, so these counts reflect the
//! transcoding *instruction* count rather than allocator cost.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use futures::StreamExt as _;
    use futures::executor::block_on;
    use gungraun::{library_benchmark, library_benchmark_group};
    use http::HeaderMap;
    use rest_over_grpc::transcoding::{Transcode, TranscodeResponse};
    use rest_over_grpc_tests::custom::{InMemoryLibrary, Transcoder};

    fn transcoder() -> Transcoder<InMemoryLibrary> {
        Transcoder::new(InMemoryLibrary)
    }

    #[library_benchmark]
    #[bench::get_path_var(transcoder())]
    fn transcode_get_path_var(transcoder: Transcoder<InMemoryLibrary>) -> TranscodeResponse {
        block_on(transcoder.transcode(
            black_box("GET"),
            black_box("/v1/shelves/history"),
            HeaderMap::new(),
            black_box(&b""[..]),
        ))
    }

    #[library_benchmark]
    #[bench::post_body(transcoder())]
    fn transcode_post_body(transcoder: Transcoder<InMemoryLibrary>) -> TranscodeResponse {
        block_on(transcoder.transcode(
            black_box("POST"),
            black_box("/v1/shelves"),
            HeaderMap::new(),
            black_box(&br#"{"name":"ignored","theme":"sci-fi"}"#[..]),
        ))
    }

    #[library_benchmark]
    #[bench::get_query(transcoder())]
    fn transcode_get_query(transcoder: Transcoder<InMemoryLibrary>) -> TranscodeResponse {
        block_on(transcoder.transcode(
            black_box("GET"),
            black_box("/v1/shelves?filter=science"),
            HeaderMap::new(),
            black_box(&b""[..]),
        ))
    }

    // Consume every frame so the benchmark measures the complete stream.
    #[library_benchmark]
    #[bench::streaming(transcoder())]
    fn transcode_streaming(transcoder: Transcoder<InMemoryLibrary>) -> Vec<u8> {
        block_on(async {
            let response = transcoder
                .transcode(
                    black_box("GET"),
                    black_box("/v1/shelves:stream"),
                    HeaderMap::new(),
                    black_box(&b""[..]),
                )
                .await;
            match response {
                TranscodeResponse::Streaming(stream) => {
                    let frames: Vec<Vec<u8>> = stream.into_frames().map(|frame| frame.expect("frame")).collect().await;
                    frames.concat()
                }
                TranscodeResponse::Unary(http) => http.into_body(),
            }
        })
    }

    #[library_benchmark]
    #[bench::miss(transcoder())]
    fn transcode_miss(transcoder: Transcoder<InMemoryLibrary>) -> TranscodeResponse {
        block_on(transcoder.transcode(black_box("GET"), black_box("/v1/nope"), HeaderMap::new(), black_box(&b""[..])))
    }

    library_benchmark_group!(
        name = transcode;
        benchmarks =
            transcode_get_path_var, transcode_post_body, transcode_get_query,
            transcode_streaming, transcode_miss
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::transcode;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = transcode
);
