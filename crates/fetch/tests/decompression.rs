// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for automatic response decompression.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::{Arc, Mutex, PoisonError};

use bytesbuf::BytesView;
use compressors::Resources;
use compressors::format::Format;
use fetch::fake::{FakeDeps, FakeHandler};
use fetch::options::{DecompressionMethod, ResponseDecompressionOptions};
use fetch::{HttpClient, HttpClientBuilder, HttpResponseBuilder};
use futures::StreamExt as _;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH};
use http::{HeaderMap, HeaderValue, StatusCode};
use http_compression::OriginalBody;
use http_extensions::HttpBodyOptions;
use ohno::Labeled as _;

testing_aids::init_tracing!();

const URL: &str = "https://example.com/resource";

fn payload() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(64)
}

fn compress(format: Format, data: &[u8]) -> BytesView {
    compressors::format::compress(format, data, Resources::global()).unwrap()
}

fn compress_zeroes(format: Format, len: usize) -> BytesView {
    const CHUNK_SIZE: usize = 64 * 1024;
    let chunk = BytesView::copied_from_slice(&vec![0; CHUNK_SIZE], &fetch::HttpBodyBuilder::new_fake());
    // Share input chunks so a large-output regression does not retain the entire payload.
    let input =
        BytesView::from_views(std::iter::repeat_n(chunk.clone(), len / CHUNK_SIZE).chain(std::iter::once(chunk.range(..len % CHUNK_SIZE))));
    let compressor = compressors::CompressorBuilder::new()
        .level(compressors::Level::FAST)
        .build_format(format, Resources::global())
        .unwrap();
    compressors::compress(input, compressor).unwrap()
}

async fn streamed_len(response: fetch::HttpResponse) -> http_extensions::Result<u64> {
    let mut stream = std::pin::pin!(response.into_body().into_stream());
    let mut len = 0;
    while let Some(chunk) = stream.next().await {
        len += u64::try_from(chunk?.len()).unwrap();
    }
    Ok(len)
}

/// Records the headers of every request the client sends.
#[derive(Clone, Default)]
struct SeenRequests(Arc<Mutex<Vec<HeaderMap>>>);

impl SeenRequests {
    fn last(&self) -> HeaderMap {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).last().cloned().unwrap()
    }
}

fn build(
    format_token: Option<&'static str>,
    body: BytesView,
    configure: impl FnOnce(HttpClientBuilder) -> HttpClient,
) -> (HttpClient, SeenRequests) {
    let seen = SeenRequests::default();
    let recorder = seen.clone();

    let handler = FakeHandler::from_fn(move |request| {
        recorder
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.headers().clone());

        let mut response = HttpResponseBuilder::new_fake().status(StatusCode::OK);
        if let Some(format_token) = format_token {
            response = response.header(CONTENT_ENCODING, HeaderValue::from_static(format_token));
        }

        response.bytes(body.clone()).build()
    });

    (configure(HttpClient::builder_fake(handler, FakeDeps::default())), seen)
}

/// A client that decompresses everything this build has a codec for.
fn client_with(format_token: Option<&'static str>, body: BytesView) -> (HttpClient, SeenRequests) {
    build(format_token, body, |builder| {
        builder.response_decompression(DecompressionMethod::ALL).build()
    })
}

#[tokio::test]
async fn a_default_client_decompresses_nothing() {
    let expected = payload();
    let compressed = compress(Format::Gzip, expected.as_bytes());
    // No call to `response_decompression`: compiling the features in
    // must not change what this client sends or what it hands back.
    let (client, seen) = build(Some("gzip"), compressed.clone(), HttpClientBuilder::build);

    let response = client.get(URL).fetch().await.unwrap();

    assert!(seen.last().get(ACCEPT_ENCODING).is_none());
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert!(response.extensions().get::<OriginalBody>().is_none());
    assert_eq!(response.into_body().into_bytes().await.unwrap(), compressed);
}

#[tokio::test]
async fn an_empty_method_array_keeps_decompression_disabled() {
    let compressed = compress(Format::Gzip, payload().as_bytes());
    let (client, seen) = build(Some("gzip"), compressed.clone(), |builder| {
        builder.response_decompression(&[]).build()
    });
    let response = client.get(URL).fetch().await.unwrap();

    assert!(!seen.last().contains_key(ACCEPT_ENCODING));
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert_eq!(response.into_body().into_bytes().await.unwrap(), compressed);
}

#[tokio::test]
async fn a_gzip_response_is_decompressed() {
    let expected = payload();
    let compressed = compress(Format::Gzip, expected.as_bytes());
    let len = compressed.len() as u64;
    let (client, _) = client_with(Some("gzip"), compressed);

    let response = client.get(URL).fetch().await.unwrap();

    // Neither header describes the decompressed body any more.
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert!(response.headers().get(CONTENT_LENGTH).is_none());

    let wire = response.extensions().get::<OriginalBody>().unwrap().clone();
    assert_eq!(wire.formats(), [Format::Gzip]);
    assert_eq!(wire.content_length(), Some(len));

    assert_eq!(response.into_body().into_text().await.unwrap(), expected);
}

#[tokio::test]
async fn every_compiled_method_round_trips() {
    let expected = payload();

    // The token spelling is part of the wire contract, so it is written out
    // here rather than taken from the same source the client reads.
    for (token, format) in [
        ("gzip", Format::Gzip),
        ("deflate", Format::Zlib),
        ("br", Format::Brotli),
        ("zstd", Format::Zstd),
    ] {
        let (client, _) = client_with(Some(token), compress(format, expected.as_bytes()));
        let response = client.get(URL).fetch().await.unwrap();

        assert_eq!(body_text(response).await, expected, "token {token}");
    }
}

async fn body_text(response: fetch::HttpResponse) -> String {
    response.into_body().into_text().await.unwrap()
}

#[tokio::test]
async fn the_advertised_order_states_the_preference() {
    let (client, seen) = client_with(None, BytesView::default());

    drop(client.get(URL).fetch().await.unwrap());

    // Descending quality values, so a server honours the order rather than
    // guessing at it. `deflate` ranks last: servers disagree over whether it
    // means the zlib-wrapped form or raw DEFLATE.
    assert_eq!(
        seen.last().get(ACCEPT_ENCODING).unwrap(),
        "zstd, br;q=0.9, gzip;q=0.8, deflate;q=0.7"
    );
}

#[tokio::test]
async fn a_caller_supplied_accept_encoding_is_left_alone() {
    let (client, seen) = client_with(None, BytesView::default());

    drop(
        client
            .get(URL)
            .header(ACCEPT_ENCODING, HeaderValue::from_static("br"))
            .fetch()
            .await
            .unwrap(),
    );

    assert_eq!(seen.last().get(ACCEPT_ENCODING).unwrap(), "br");
}

#[tokio::test]
async fn narrowing_the_method_set_passes_the_rest_through() {
    let compressed = compress(Format::Brotli, payload().as_bytes());
    let (client, seen) = build(Some("br"), compressed, |builder| {
        builder.response_decompression(&[DecompressionMethod::Gzip]).build()
    });

    let response = client.get(URL).fetch().await.unwrap();

    assert_eq!(seen.last().get(ACCEPT_ENCODING).unwrap(), "gzip");
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "br");
}

#[tokio::test]
async fn clearing_methods_disables_decompression_and_advertisement() {
    let compressed = compress(Format::Gzip, payload().as_bytes());
    let (client, seen) = build(Some("gzip"), compressed.clone(), |builder| {
        builder
            .response_decompression(
                ResponseDecompressionOptions::new()
                    .methods(&[DecompressionMethod::Gzip])
                    .max_output_len(1)
                    .methods(&[]),
            )
            .build()
    });
    let response = client.get(URL).fetch().await.unwrap();

    assert!(!seen.last().contains_key(ACCEPT_ENCODING));
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert_eq!(response.into_body().into_bytes().await.unwrap(), compressed);
}

#[tokio::test]
async fn replacing_options_replaces_both_methods_and_limits() {
    let expected = payload();
    let (client, seen) = build(Some("br"), compress(Format::Brotli, expected.as_bytes()), |builder| {
        builder
            .response_decompression(
                ResponseDecompressionOptions::new()
                    .methods(&[DecompressionMethod::Gzip])
                    .max_output_len(1),
            )
            .response_decompression(&[DecompressionMethod::Brotli])
            .build()
    });
    let response = client.get(URL).fetch().await.unwrap();

    assert_eq!(seen.last().get(ACCEPT_ENCODING).unwrap(), "br");
    assert_eq!(body_text(response).await, expected);
}

#[tokio::test]
async fn output_limits_apply_at_the_exact_boundary_for_every_method_and_pipeline() {
    let expected = payload();
    let len = u64::try_from(expected.len()).unwrap();
    for (token, format, method) in [
        ("gzip", Format::Gzip, DecompressionMethod::Gzip),
        ("deflate", Format::Zlib, DecompressionMethod::Deflate),
        ("br", Format::Brotli, DecompressionMethod::Brotli),
        ("zstd", Format::Zstd, DecompressionMethod::Zstd),
    ] {
        let compressed = compress(format, expected.as_bytes());
        for minimal in [false, true] {
            for limit in [len, len - 1] {
                let (client, _) = build(Some(token), compressed.clone(), |builder| {
                    let builder = if minimal { builder.minimal_pipeline() } else { builder };
                    let options = ResponseDecompressionOptions::new().methods(&[method]).max_output_len(None);
                    let options = if minimal {
                        options.max_output_len(Some(limit))
                    } else {
                        options.max_output_len(limit)
                    };
                    builder
                        .response_body_options(HttpBodyOptions::new().buffer_limit(1))
                        .response_decompression(options)
                        .build()
                });
                let response = client.get(URL).fetch().await.unwrap();
                let result = streamed_len(response).await;

                if limit == len {
                    assert_eq!(result.unwrap(), len, "{token}, minimal={minimal}");
                } else {
                    assert_eq!(
                        result.unwrap_err().label(),
                        "compression_limit_exceeded",
                        "{token}, minimal={minimal}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn codec_output_defaults_are_preserved_even_when_stream_count_is_limited() {
    const LEN: usize = 64 * 1024 * 1024 + 1;
    let compressed = compress_zeroes(Format::Brotli, LEN);
    let options = ResponseDecompressionOptions::from(&[DecompressionMethod::Brotli]);
    for options in [options.clone(), options.max_streams(1)] {
        let (client, _) = build(Some("br"), compressed.clone(), |builder| {
            builder
                .response_body_options(HttpBodyOptions::new().buffer_limit(1))
                .response_decompression(options)
                .build()
        });

        assert_eq!(
            streamed_len(client.get(URL).fetch().await.unwrap()).await.unwrap(),
            u64::try_from(LEN).unwrap()
        );
    }
}

#[tokio::test]
async fn none_removes_a_previously_configured_output_limit() {
    let expected = payload();
    let (client, _) = build(Some("gzip"), compress(Format::Gzip, expected.as_bytes()), |builder| {
        builder
            .response_body_options(HttpBodyOptions::new().buffer_limit(1))
            .response_decompression(
                ResponseDecompressionOptions::new()
                    .methods(&[DecompressionMethod::Gzip])
                    .max_output_len(1)
                    .max_output_len(None),
            )
            .build()
    });

    assert_eq!(
        streamed_len(client.get(URL).fetch().await.unwrap()).await.unwrap(),
        u64::try_from(expected.len()).unwrap()
    );
}

#[tokio::test]
async fn codec_stream_count_defaults_are_preserved_even_when_output_is_limited() {
    for (token, format, method) in [
        ("gzip", Format::Gzip, DecompressionMethod::Gzip),
        ("zstd", Format::Zstd, DecompressionMethod::Zstd),
    ] {
        let member = compress(format, b"");
        let members = BytesView::from_views(std::iter::repeat_n(member, 1025));
        let options = ResponseDecompressionOptions::from(&[method]);
        for options in [options.clone(), options.max_output_len(1)] {
            let (client, _) = build(Some(token), members.clone(), |builder| {
                builder.response_decompression(options).build()
            });
            assert_eq!(streamed_len(client.get(URL).fetch().await.unwrap()).await.unwrap(), 0, "{token}");
        }
    }
}

#[tokio::test]
async fn stream_limits_count_empty_members_even_with_unbounded_output() {
    for (token, format, method) in [
        ("gzip", Format::Gzip, DecompressionMethod::Gzip),
        ("zstd", Format::Zstd, DecompressionMethod::Zstd),
    ] {
        let member = compress(format, b"");
        for count in [2, 3] {
            let members = BytesView::from_views(std::iter::repeat_n(member.clone(), count));
            let (client, _) = build(Some(token), members, |builder| {
                let options = ResponseDecompressionOptions::new()
                    .methods(&[method])
                    .max_streams(2)
                    .max_output_len(None);
                builder.response_decompression(options).build()
            });
            let result = streamed_len(client.get(URL).fetch().await.unwrap()).await;

            if count == 2 {
                assert_eq!(result.unwrap(), 0, "{token}, count={count}");
            } else {
                assert_eq!(result.unwrap_err().label(), "compression_limit_exceeded", "{token}, count={count}");
            }
        }
    }
}

#[tokio::test]
async fn a_corrupt_body_fails_when_it_is_read() {
    let mut corrupt = compress(Format::Gzip, payload().as_bytes()).to_vec();
    let tail = corrupt.len() - 8;
    corrupt[tail..].fill(0);

    let (client, _) = client_with(
        Some("gzip"),
        BytesView::copied_from_slice(&corrupt, &fetch::HttpBodyBuilder::new_fake()),
    );

    // Decompression is lazy, so the response itself is fine and the body is not.
    let response = client.get(URL).fetch().await.unwrap();
    let error = response.into_body().into_text().await.unwrap_err();

    assert_eq!(error.label(), "compression_invalid");
}

#[tokio::test]
async fn the_buffer_limit_applies_to_the_decompressed_body() {
    let expected = payload();
    let options = ResponseDecompressionOptions::new().methods(&[DecompressionMethod::Gzip]);
    for options in [options.clone(), options.max_output_len(None)] {
        let (client, _) = build(Some("gzip"), compress(Format::Gzip, expected.as_bytes()), |builder| {
            builder
                .response_body_options(HttpBodyOptions::default().buffer_limit(16))
                .response_decompression(options)
                .build()
        });

        let response = client.get(URL).fetch().await.unwrap();
        let error = response.into_body().into_text().await.unwrap_err();

        // The tighter buffering policy is independent of the decompression output cap.
        assert_eq!(error.label(), "body_size_limit");
    }
}

#[tokio::test]
async fn decompression_also_applies_to_the_minimal_pipeline() {
    let expected = payload();
    let (client, _) = build(Some("gzip"), compress(Format::Gzip, expected.as_bytes()), |builder| {
        builder
            .minimal_pipeline()
            .response_decompression(ResponseDecompressionOptions::new().methods(&[DecompressionMethod::Gzip]))
            .build()
    });

    let response = client.get(URL).fetch().await.unwrap();

    assert_eq!(body_text(response).await, expected);
}
