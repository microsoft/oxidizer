// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests covering each transformation.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, PoisonError};

use bytesbuf::BytesView;
use compressors::format::Format;
use compressors::{CompressionStream, CompressorBuilder, DecompressorLimits, Level, Resources};
use futures::StreamExt as _;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ETAG, VARY};
use http::{HeaderValue, StatusCode};
use http_compression::{Client, Compression, CompressionLayer, OriginalBody, Server, UnsupportedCompression};
use http_extensions::{FakeHandler, HttpBodyBuilder, HttpRequest, HttpResponse, HttpResponseBuilder, Result};
use layered::{Layer, Service};
use ohno::Labeled as _;

testing_aids::init_tracing!();

const URL: &str = "https://example.com/resource";

/// Compresses well, so the transforms have something visible to do.
fn payload() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(64)
}

fn builder() -> HttpBodyBuilder {
    HttpBodyBuilder::new_fake()
}

fn compress(format: Format, data: &[u8]) -> BytesView {
    compressors::format::compress(format, data, Resources::global()).unwrap()
}

fn client() -> CompressionLayer<Client> {
    Compression::client(builder())
}

fn server() -> CompressionLayer<Server> {
    Compression::server(builder())
}

fn request(body: BytesView, format_token: Option<&'static str>) -> HttpRequest {
    let mut request = http::Request::post(URL).body(builder().bytes(body)).unwrap();

    if let Some(format_token) = format_token {
        request
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static(format_token));
    }

    request
}

fn bytes(text: &str) -> BytesView {
    BytesView::copied_from_slice(text.as_bytes(), &builder())
}

/// An inner handler that hands the request body back as the response body.
fn echo() -> FakeHandler {
    FakeHandler::from_async_fn(|request: HttpRequest| async move {
        let body = request.into_body().into_bytes().await?;

        HttpResponseBuilder::new_fake().status(StatusCode::OK).bytes(body).build()
    })
}

/// An inner handler returning a fixed response.
fn responds_with(response: impl Fn() -> Result<HttpResponse> + Send + Sync + 'static) -> FakeHandler {
    FakeHandler::from_fn(move |_| response())
}

// Client: response decompression

#[tokio::test]
async fn a_client_decompresses_the_response_it_gets_back() {
    let expected = payload();
    let compressed = compress(Format::Gzip, expected.as_bytes());
    let len = compressed.len() as u64;

    let handler = client().decompress_responses(&[Format::Gzip]).layer(responds_with(move || {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .bytes(compressed.clone())
            .build()
    }));

    let response = handler.execute(request(BytesView::default(), None)).await.unwrap();

    // Neither header describes the decompressed body any more.
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert!(response.headers().get(CONTENT_LENGTH).is_none());

    let wire = response.extensions().get::<OriginalBody>().unwrap().clone();
    assert_eq!(wire.formats(), [Format::Gzip]);
    assert_eq!(wire.content_length(), Some(len));

    assert_eq!(response.into_body().into_text().await.unwrap(), expected);
}

#[tokio::test]
async fn a_client_says_what_it_can_decompress() {
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);

    let handler = client()
        .decompress_responses(&[Format::Zstd, Format::Brotli, Format::Gzip])
        .layer(FakeHandler::from_fn(move |request: HttpRequest| {
            *sink.lock().unwrap_or_else(PoisonError::into_inner) = request.headers().get(ACCEPT_ENCODING).cloned();

            HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
        }));

    handler.execute(request(BytesView::default(), None)).await.unwrap();

    let value = seen.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap();
    // Descending quality values, so the preference is stated rather than implied.
    assert_eq!(value, "zstd, br;q=0.9, gzip;q=0.8");
}

#[tokio::test]
async fn nothing_is_decompressed_or_advertised_until_it_is_configured() {
    let compressed = compress(Format::Gzip, payload().as_bytes());

    let handler = client().layer(responds_with(move || {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .bytes(compressed.clone())
            .build()
    }));

    let response = handler.execute(request(BytesView::default(), None)).await.unwrap();

    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert!(response.extensions().get::<OriginalBody>().is_none());
}

#[tokio::test]
async fn a_client_leaves_outgoing_bodies_compressed_and_never_compresses_responses() {
    for format in [Format::Gzip, Format::Brotli] {
        let compressed = compress(format, payload().as_bytes());
        let outgoing = compressed.clone();
        let token = format.content_encoding().unwrap();

        let handler = client()
            .decompress_responses(&[Format::Gzip])
            .on_unsupported(UnsupportedCompression::Fail)
            .layer(FakeHandler::from_async_fn(move |request: HttpRequest| {
                let compressed = compressed.clone();
                async move {
                    assert_eq!(request.headers().get(CONTENT_ENCODING).unwrap(), token);
                    assert_eq!(request.headers().get(ACCEPT_ENCODING).unwrap(), "gzip");
                    assert!(request.extensions().get::<OriginalBody>().is_none());
                    assert_eq!(request.into_body().into_bytes().await.unwrap(), compressed);

                    HttpResponseBuilder::new_fake()
                        .status(StatusCode::OK)
                        .text("plain response")
                        .build()
                }
            }));

        let response = handler.execute(request(outgoing, Some(token))).await.unwrap();

        assert!(response.headers().get(CONTENT_ENCODING).is_none());
        assert!(response.headers().get(VARY).is_none());
        assert_eq!(response.into_body().into_text().await.unwrap(), "plain response");
    }
}

#[tokio::test]
async fn building_a_client_captures_its_role_settings() {
    let expected = payload();
    let gzip = compress(Format::Gzip, expected.as_bytes());
    let brotli = compress(Format::Brotli, expected.as_bytes());
    let layer = client().decompress_responses(&[Format::Gzip]);

    let original = layer.layer(FakeHandler::from_fn(move |request: HttpRequest| {
        assert_eq!(request.headers().get(ACCEPT_ENCODING).unwrap(), "gzip");
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .bytes(gzip.clone())
            .build()
    }));

    let changed = layer
        .advertise(false)
        .decompress_responses(&[Format::Brotli])
        .layer(FakeHandler::from_fn(move |request: HttpRequest| {
            assert!(request.headers().get(ACCEPT_ENCODING).is_none());
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .header(CONTENT_ENCODING, HeaderValue::from_static("br"))
                .bytes(brotli.clone())
                .build()
        }));

    for (handler, format) in [(original, Format::Gzip), (changed, Format::Brotli)] {
        let response = handler.execute(request(BytesView::default(), None)).await.unwrap();
        assert_eq!(response.extensions().get::<OriginalBody>().unwrap().formats(), [format]);
        assert_eq!(response.into_body().into_text().await.unwrap(), expected);
    }
}

// Server: request decompression

#[tokio::test]
async fn a_server_decompresses_the_request_it_receives() {
    let expected = payload();
    let compressed = compress(Format::Gzip, expected.as_bytes());
    let len = compressed.len() as u64;

    let handler = server()
        .decompress_requests(&[Format::Gzip])
        .layer(FakeHandler::from_async_fn(move |request: HttpRequest| async move {
            assert!(request.headers().get(CONTENT_ENCODING).is_none());
            assert!(request.headers().get(CONTENT_LENGTH).is_none());
            let original = request.extensions().get::<OriginalBody>().unwrap();
            assert_eq!(original.formats(), [Format::Gzip]);
            assert_eq!(original.content_length(), Some(len));

            let body = request.into_body().into_bytes().await?;
            HttpResponseBuilder::new_fake().status(StatusCode::OK).bytes(body).build()
        }));

    let mut input = request(compressed, Some("gzip"));
    input
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_str(&len.to_string()).unwrap());
    let response = handler.execute(input).await.unwrap();

    assert_eq!(response.into_body().into_text().await.unwrap(), expected);
}

#[tokio::test]
async fn a_server_can_refuse_a_format_it_does_not_decompress() {
    let handler = server()
        .decompress_requests(&[Format::Gzip])
        .on_unsupported(UnsupportedCompression::Fail)
        .layer(echo());

    let error = handler.execute(request(BytesView::default(), Some("br"))).await.unwrap_err();

    assert_eq!(error.label(), "compression_unsupported");
}

#[tokio::test]
async fn a_server_never_advertises_or_decompresses_its_responses() {
    for format in [Format::Gzip, Format::Brotli] {
        let compressed = compress(format, payload().as_bytes());
        let expected = compressed.clone();
        let token = format.content_encoding().unwrap();
        let len = compressed.len().to_string();

        let handler = server()
            .decompress_requests(&[Format::Gzip])
            .compress_responses(&[Format::Gzip])
            .on_unsupported(UnsupportedCompression::Fail)
            .layer(FakeHandler::from_fn(move |request: HttpRequest| {
                assert!(request.headers().get(ACCEPT_ENCODING).is_none());
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(CONTENT_ENCODING, HeaderValue::from_static(token))
                    .bytes(compressed.clone())
                    .build()
            }));

        let response = handler.execute(request(BytesView::default(), None)).await.unwrap();

        assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), token);
        assert_eq!(response.headers().get(CONTENT_LENGTH).unwrap(), len.as_str());
        assert!(response.extensions().get::<OriginalBody>().is_none());
        assert_eq!(response.into_body().into_bytes().await.unwrap(), expected);
    }
}

// Server: response compression

#[tokio::test]
async fn a_server_compresses_the_response_the_caller_accepts() {
    let expected = payload();
    let handler = server().compress_responses(&[Format::Zstd, Format::Gzip]).layer(echo());

    let mut input = request(bytes(&expected), None);
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    let response = handler.execute(input).await.unwrap();

    // zstd is preferred, but the caller only accepts gzip.
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");

    let wire = response.into_body().into_bytes().await.unwrap();
    assert_eq!(
        compressors::gzip::decompress(wire, Resources::global()).unwrap().to_vec(),
        expected.as_bytes()
    );
}

#[tokio::test]
async fn response_compression_defaults_to_fast_and_honours_the_configured_level() {
    for format in [Format::Gzip, Format::Brotli, Format::Zstd, Format::Zlib] {
        for (configured, expected_level) in [
            (None, Level::FAST),
            (Some(Level::MIN), Level::MIN),
            (Some(Level::DEFAULT), Level::DEFAULT),
            (Some(Level::HIGH), Level::HIGH),
        ] {
            let layer = server().compress_responses(&[format]);
            let layer = match configured {
                Some(level) => layer.level(level),
                None => layer,
            };
            let handler = layer.layer(echo());
            let expected = payload();
            let mut input = request(bytes(&expected), None);
            input
                .headers_mut()
                .insert(ACCEPT_ENCODING, HeaderValue::from_static(format.content_encoding().unwrap()));

            let response = handler.execute(input).await.unwrap();
            let actual = response.into_body().into_bytes().await.unwrap();

            let compressor = CompressorBuilder::new()
                .level(expected_level)
                .build_format(format, Resources::global())
                .unwrap();
            let source = futures::stream::iter([Ok::<_, std::io::Error>(bytes(&expected))]);
            let chunks = CompressionStream::compress(source, compressor).collect::<Vec<_>>().await;
            let reference = BytesView::from_views(chunks.into_iter().map(std::result::Result::unwrap));

            assert_eq!(actual.to_vec(), reference.to_vec(), "{format:?}, {configured:?}");
            assert_eq!(
                compressors::format::decompress(format, actual, Resources::global())
                    .unwrap()
                    .to_vec(),
                expected.as_bytes()
            );
        }
    }
}

#[tokio::test]
async fn a_server_sends_plain_text_when_nothing_is_accepted() {
    let expected = payload();
    let handler = server().compress_responses(&[Format::Gzip]).layer(echo());

    // No `Accept-Encoding` at all.
    let response = handler.execute(request(bytes(&expected), None)).await.unwrap();

    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(response.into_body().into_text().await.unwrap(), expected);
}

#[tokio::test]
async fn building_a_server_captures_its_role_settings() {
    let layer = server().compress_responses(&[Format::Gzip]);
    let original = layer.layer(echo());
    let changed = layer.compress_responses(&[Format::Brotli]).layer(echo());
    let expected = payload();

    for (handler, format) in [(original, Format::Gzip), (changed, Format::Brotli)] {
        let mut input = request(bytes(&expected), None);
        input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));

        let response = handler.execute(input).await.unwrap();

        assert_eq!(
            response.headers().get(CONTENT_ENCODING).unwrap(),
            format.content_encoding().unwrap()
        );
        let wire = response.into_body().into_bytes().await.unwrap();
        assert_eq!(
            compressors::format::decompress(format, wire, Resources::global()).unwrap().to_vec(),
            expected.as_bytes()
        );
    }
}

// Both halves together

#[tokio::test]
async fn a_client_and_a_server_round_trip_through_each_other() {
    let expected = payload();

    // The server sits inside: it decompresses what arrives and compresses what it sends.
    let server = server()
        .decompress_requests(&[Format::Gzip])
        .compress_responses(&[Format::Brotli, Format::Gzip])
        .layer(echo());

    // The client sits outside: it decompresses whatever the server chose to send.
    let client = client().decompress_responses(&[Format::Brotli, Format::Gzip]).layer(server);

    let mut input = request(compress(Format::Gzip, expected.as_bytes()), Some("gzip"));
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("br"));

    let response = client.execute(input).await.unwrap();

    // Both hops are invisible: the payload survives being decompressed on the way in
    // and compressed again on the way out.
    assert_eq!(response.extensions().get::<OriginalBody>().unwrap().formats(), [Format::Brotli]);
    assert_eq!(response.into_body().into_text().await.unwrap(), expected);
}

#[tokio::test]
async fn decompression_limits_use_the_same_error_label_in_both_roles() {
    let compressed = compress(Format::Gzip, payload().as_bytes());
    let response_body = compressed.clone();
    let limits = DecompressorLimits::new().max_output_len(NonZeroU64::new(16).unwrap());

    let client = client()
        .decompress_responses(&[Format::Gzip])
        .limits(limits)
        .layer(responds_with(move || {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                .bytes(response_body.clone())
                .build()
        }));
    let response = client.execute(request(BytesView::default(), None)).await.unwrap();
    let error = response.into_body().into_bytes().await.unwrap_err();
    assert_eq!(error.label(), "compression_limit_exceeded");

    let server = server().decompress_requests(&[Format::Gzip]).limits(limits).layer(echo());
    let error = server.execute(request(compressed, Some("gzip"))).await.unwrap_err();
    assert_eq!(error.label(), "compression_limit_exceeded");
}

#[tokio::test]
async fn a_no_content_response_is_never_given_a_body() {
    for status in [StatusCode::NO_CONTENT, StatusCode::NOT_MODIFIED] {
        let handler = server().compress_responses(&[Format::Gzip]).layer(FakeHandler::from_fn(move |_| {
            HttpResponseBuilder::new_fake().status(status).build()
        }));

        let mut input = request(BytesView::default(), None);
        input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = handler.execute(input).await.unwrap();

        // Compressing an absent body would invent bytes the caller must not get.
        assert!(response.headers().get(CONTENT_ENCODING).is_none(), "status {status}");
        assert_eq!(response.into_body().into_bytes().await.unwrap().len(), 0, "status {status}");
    }
}

#[tokio::test]
async fn an_empty_body_is_not_grown_by_compressing_it() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(echo());

    let mut input = request(BytesView::default(), None);
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    let response = handler.execute(input).await.unwrap();

    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(response.into_body().into_bytes().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_negotiated_response_says_it_varies_by_accept_encoding() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(echo());

    let mut input = request(bytes(&payload()), None);
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    let response = handler.execute(input).await.unwrap();

    assert_eq!(response.headers().get(VARY).unwrap(), "accept-encoding");
}

#[tokio::test]
async fn an_uncompressed_response_still_says_it_varies() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(echo());

    // No `Accept-Encoding`, so nothing is compressed - but a cache still must not
    // reuse this body for a caller that does send one.
    let response = handler.execute(request(bytes(&payload()), None)).await.unwrap();

    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(response.headers().get(VARY).unwrap(), "accept-encoding");
}

#[tokio::test]
async fn an_existing_vary_is_kept() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(FakeHandler::from_fn(|_| {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(VARY, HeaderValue::from_static("origin"))
            .text("body")
            .build()
    }));

    let response = handler.execute(request(BytesView::default(), None)).await.unwrap();

    let varies = response
        .headers()
        .get_all(VARY)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(varies, ["origin", "accept-encoding"]);
}

/// A body that yields one data frame and then a trailer frame.
struct TrailingBody {
    data: Option<BytesView>,
    trailers: Option<http::HeaderMap>,
}

impl http_body::Body for TrailingBody {
    type Data = BytesView;
    type Error = http_extensions::HttpError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<std::result::Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        if let Some(data) = this.data.take() {
            return std::task::Poll::Ready(Some(Ok(http_body::Frame::data(data))));
        }

        std::task::Poll::Ready(this.trailers.take().map(|t| Ok(http_body::Frame::trailers(t))))
    }
}

#[tokio::test]
async fn trailers_survive_every_format() {
    for format in [Format::Gzip, Format::Brotli, Format::Zstd, Format::Zlib] {
        let expected = payload();
        let compressed = compress(format, expected.as_bytes());
        let token = format.content_encoding().unwrap();

        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-checksum", HeaderValue::from_static("abc123"));

        let handler = server()
            .decompress_requests(&[format])
            .layer(FakeHandler::from_async_fn(|request: HttpRequest| async move {
                let mut body = Box::pin(request.into_body());
                let mut seen = None;
                while let Some(frame) = std::future::poll_fn(|cx| http_body::Body::poll_frame(body.as_mut(), cx)).await {
                    if let Err(f) = frame?.into_data() {
                        seen = f.into_trailers().ok();
                    }
                }
                let found = seen.and_then(|t| t.get("x-checksum").cloned()).is_some();
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .text(found.to_string())
                    .build()
            }));

        let mut input = http::Request::post(URL)
            .body(builder().body(
                TrailingBody {
                    data: Some(compressed),
                    trailers: Some(trailers),
                },
                &http_extensions::HttpBodyOptions::default(),
            ))
            .unwrap();
        input.headers_mut().insert(CONTENT_ENCODING, HeaderValue::from_str(token).unwrap());

        let response = handler.execute(input).await.unwrap();
        let found = response.into_body().into_text().await.unwrap();

        assert_eq!(found, "true", "trailers lost for {token}");
    }
}

const CONTENT_TYPE_CASES: &[(&str, bool)] = &[
    ("text/event-stream", false),
    ("Text/Event-Stream; charset=utf-8", false),
    ("text/event-stream; comment=\"a;b\"", false),
    ("application/grpc", false),
    ("application/grpc+proto", false),
    ("application/grpc+json; charset=utf-8", false),
    ("APPLICATION/GRPC+PROTO", false),
    ("image/png", false),
    ("image/jpeg", false),
    ("IMAGE/WEBP; quality=90", false),
    ("audio/mpeg", false),
    ("VIDEO/MP4", false),
    ("application/zip", false),
    ("application/gzip", false),
    ("application/x-gzip", false),
    ("application/zstd", false),
    ("application/x-bzip2", false),
    ("application/x-xz", false),
    ("application/x-7z-compressed", false),
    ("application/vnd.rar", false),
    ("application/x-rar-compressed", false),
    ("image/svg+xml", true),
    ("IMAGE/SVG+XML; charset=utf-8", true),
    ("application/json", true),
    ("application/json; charset=\"utf-8\"", true),
    ("application/ld+json", true),
    ("application/x-ndjson", true),
    ("application/grpc+proto", false),
    ("APPLICATION/GRPC+JSON", false),
    ("text/plain; charset=utf-8", true),
    ("text/event-streaming", true),
    ("application/grpc-web", true),
    ("application/grpc-web+proto", true),
    ("application/grpc-web-text+proto", true),
    ("not a media type", false),
    ("text/plain, text/event-stream", false),
    ("image/svg+xml; charset", false),
    ("application/", false),
    ("/json", false),
];

async fn assert_content_type_policy(layer: &CompressionLayer<Server>) {
    for &(content_type, should_compress) in CONTENT_TYPE_CASES {
        let handler = layer.layer(FakeHandler::from_fn(move |_| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static(content_type))
                .header(CONTENT_LENGTH, HeaderValue::from_str(&payload().len().to_string()).unwrap())
                .header(ETAG, HeaderValue::from_static("\"original\""))
                .bytes(bytes(&payload()))
                .build()
        }));
        let mut input = request(BytesView::default(), None);
        input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = handler.execute(input).await.unwrap();
        assert_eq!(response.headers().contains_key(CONTENT_ENCODING), should_compress, "{content_type}");
        assert_eq!(response.headers().get(VARY).unwrap(), "accept-encoding");
        if !should_compress {
            assert_eq!(response.headers().get(ETAG).unwrap(), "\"original\"");
            assert_eq!(
                response.headers().get(CONTENT_LENGTH).unwrap().to_str().unwrap(),
                payload().len().to_string()
            );
            assert_eq!(response.into_body().into_text().await.unwrap(), payload(), "{content_type}");
        }
    }
}

#[tokio::test]
async fn safe_content_type_exclusions_apply_without_a_custom_filter() {
    assert_content_type_policy(&server().compress_responses(&[Format::Gzip])).await;
}

#[tokio::test]
async fn a_wildcard_allowlist_cannot_override_the_builtin_exclusions() {
    assert_content_type_policy(&server().compress_responses(&[Format::Gzip]).compressible_types(["*/*"])).await;
}

#[tokio::test]
async fn a_response_with_an_unreadable_content_type_is_left_unchanged() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(responds_with(|| {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_bytes(b"\xff").unwrap())
            .bytes(bytes(&payload()))
            .build()
    }));
    let mut input = request(BytesView::default(), None);
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let response = handler.execute(input).await.unwrap();
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    assert_eq!(response.into_body().into_text().await.unwrap(), payload());
}

#[tokio::test]
async fn only_the_chosen_content_types_are_compressed() {
    use http_compression::DEFAULT_COMPRESSIBLE_TYPES;

    // `application/ld+json` is covered by `application/json` via its suffix;
    // a JPEG is already compressed and must be left alone.
    let cases = [
        ("text/html; charset=utf-8", true),
        ("text/markdown", true),
        ("text/csv", true),
        ("text/event-stream", false),
        ("application/grpc", false),
        ("application/json", true),
        ("application/ld+json", true),
        ("image/svg+xml", true),
        ("image/jpeg", false),
        ("video/mp4", false),
        ("application/zip", false),
    ];

    for (content_type, expected) in cases {
        let handler = server()
            .compress_responses(&[Format::Gzip])
            .compressible_types(DEFAULT_COMPRESSIBLE_TYPES.iter().copied())
            .layer(FakeHandler::from_fn(move |_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, HeaderValue::from_static(content_type))
                    .text("a body long enough to be worth compressing ".repeat(20))
                    .build()
            }));

        let mut input = request(BytesView::default(), None);
        input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = handler.execute(input).await.unwrap();
        let compressed = response.headers().get(CONTENT_ENCODING).is_some();

        assert_eq!(compressed, expected, "content type {content_type}");
    }
}

#[tokio::test]
async fn structured_suffix_allowlist_matches_only_the_requested_type() {
    for (content_type, expected) in [
        ("application/ld+json", true),
        ("application/ld+xml", false),
        ("application/foo+ld", false),
    ] {
        let handler = server()
            .compress_responses(&[Format::Gzip])
            .compressible_types(["application/ld+json"])
            .layer(FakeHandler::from_fn(move |_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, HeaderValue::from_static(content_type))
                    .text("a body long enough to be worth compressing ".repeat(20))
                    .build()
            }));

        let mut input = request(BytesView::default(), None);
        input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = handler.execute(input).await.unwrap();
        assert_eq!(
            response.headers().contains_key(CONTENT_ENCODING),
            expected,
            "content type {content_type}"
        );
    }
}

#[tokio::test]
async fn base_subtype_allowlist_matches_suffix_not_structured_prefix() {
    for (allowed, content_type, expected) in [
        ("application/soap", "application/soap", true),
        ("application/soap", "application/soap+xml", false),
        ("application/xml", "application/soap+xml", true),
    ] {
        let handler = server()
            .compress_responses(&[Format::Gzip])
            .compressible_types([allowed])
            .layer(FakeHandler::from_fn(move |_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, HeaderValue::from_static(content_type))
                    .text("a body long enough to be worth compressing ".repeat(20))
                    .build()
            }));

        let mut input = request(BytesView::default(), None);
        input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = handler.execute(input).await.unwrap();
        assert_eq!(
            response.headers().contains_key(CONTENT_ENCODING),
            expected,
            "allowlist {allowed}, content type {content_type}"
        );
    }
}

#[tokio::test]
async fn a_body_without_a_content_type_is_left_alone_once_filtering_is_on() {
    use http_compression::DEFAULT_COMPRESSIBLE_TYPES;

    let handler = server()
        .compress_responses(&[Format::Gzip])
        .compressible_types(DEFAULT_COMPRESSIBLE_TYPES.iter().copied())
        .layer(echo());

    let mut input = request(bytes(&payload()), None);
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    let response = handler.execute(input).await.unwrap();

    assert!(response.headers().get(CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn compression_weakens_a_strong_validator() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(FakeHandler::from_fn(|_| {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(ETAG, HeaderValue::from_static("\"v1\""))
            .text("a body long enough to be worth compressing ".repeat(20))
            .build()
    }));

    let mut input = request(BytesView::default(), None);
    input.headers_mut().insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    let response = handler.execute(input).await.unwrap();

    // The compressed body is a different representation, so it must not keep a
    // strong validator the identity body also claims.
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert_eq!(response.headers().get(ETAG).unwrap(), "W/\"v1\"");
}

#[tokio::test]
async fn transformations_remove_content_digest_and_preserve_repr_digest() {
    let expected = payload();
    let compressed = compress(Format::Gzip, expected.as_bytes());

    let client_handler = client().decompress_responses(&[Format::Gzip]).layer(responds_with(move || {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .header("content-digest", HeaderValue::from_static("sha-256=:Y29udGVudA==:"))
            .header("repr-digest", HeaderValue::from_static("sha-256=:cmVwcg==:"))
            .bytes(compressed.clone())
            .build()
    }));

    let response = client_handler.execute(request(BytesView::default(), None)).await.unwrap();
    assert!(response.headers().get("content-digest").is_none());
    assert_eq!(response.headers().get("repr-digest").unwrap(), "sha-256=:cmVwcg==:");

    let server_handler = server().compress_responses(&[Format::Gzip]).layer(responds_with(|| {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header("content-digest", HeaderValue::from_static("sha-256=:Y29udGVudA==:"))
            .header("repr-digest", HeaderValue::from_static("sha-256=:cmVwcg==:"))
            .text(payload())
            .build()
    }));
    let input = http::Request::get(URL)
        .header(ACCEPT_ENCODING, "gzip")
        .body(builder().empty())
        .unwrap();

    let response = server_handler.execute(input).await.unwrap();
    assert!(response.headers().get("content-digest").is_none());
    assert_eq!(response.headers().get("repr-digest").unwrap(), "sha-256=:cmVwcg==:");
}

#[tokio::test]
async fn a_head_response_is_never_decompressed() {
    let compressed = compress(Format::Gzip, payload().as_bytes());

    let handler = client().decompress_responses(&[Format::Gzip]).layer(FakeHandler::from_fn(move |_| {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .bytes(compressed.clone())
            .build()
    }));

    let input = http::Request::head(URL).body(builder().empty()).unwrap();
    let response = handler.execute(input).await.unwrap();

    // A `HEAD` response describes a body it does not carry, so stripping the
    // metadata would leave the caller unable to make sense of what it says.
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert!(response.extensions().get::<OriginalBody>().is_none());
}

#[tokio::test]
async fn a_head_response_is_never_compressed() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(responds_with(|| {
        HttpResponseBuilder::new_fake().status(StatusCode::OK).text(payload()).build()
    }));
    let input = http::Request::head(URL)
        .header(ACCEPT_ENCODING, "gzip")
        .body(builder().empty())
        .unwrap();

    let response = handler.execute(input).await.unwrap();

    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(response.into_body().into_text().await.unwrap(), payload());
}

#[tokio::test]
async fn a_successful_connect_response_is_never_decompressed() {
    let compressed = compress(Format::Gzip, payload().as_bytes());
    let expected = compressed.clone();
    let handler = client().decompress_responses(&[Format::Gzip]).layer(responds_with(move || {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .bytes(compressed.clone())
            .build()
    }));
    let input = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(URL)
        .body(builder().empty())
        .unwrap();

    let response = handler.execute(input).await.unwrap();

    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert!(response.extensions().get::<OriginalBody>().is_none());
    assert_eq!(response.into_body().into_bytes().await.unwrap(), expected);
}

#[tokio::test]
async fn a_successful_connect_response_is_never_compressed() {
    let handler = server().compress_responses(&[Format::Gzip]).layer(responds_with(|| {
        HttpResponseBuilder::new_fake().status(StatusCode::OK).text(payload()).build()
    }));
    let input = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(URL)
        .header(ACCEPT_ENCODING, "gzip")
        .body(builder().empty())
        .unwrap();

    let response = handler.execute(input).await.unwrap();

    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert!(response.headers().get(VARY).is_none());
    assert_eq!(response.into_body().into_text().await.unwrap(), payload());
}

#[tokio::test]
async fn an_empty_list_member_does_not_stop_decompression() {
    let expected = payload();
    let compressed = compress(Format::Gzip, expected.as_bytes());

    let handler = client().decompress_responses(&[Format::Gzip]).layer(FakeHandler::from_fn(move |_| {
        HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip,,"))
            .bytes(compressed.clone())
            .build()
    }));

    let response = handler.execute(request(BytesView::default(), None)).await.unwrap();

    // Empty members come from headers being combined, not from the sender
    // meaning anything by them.
    assert_eq!(response.into_body().into_text().await.unwrap(), expected);
}

#[tokio::test]
async fn content_encoding_layers_are_bounded_before_body_reading() {
    let accepted = std::iter::repeat_n("gzip", 16).collect::<Vec<_>>().join(", ");
    let rejected = std::iter::repeat_n("gzip", 17).collect::<Vec<_>>().join(", ");

    for (encodings, expected_layers) in [(accepted, Some(16)), (rejected, None)] {
        let value = HeaderValue::from_str(&encodings).unwrap();
        let handler = client().decompress_responses(&[Format::Gzip]).layer(responds_with(move || {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .header(CONTENT_ENCODING, value.clone())
                .text("body is intentionally never polled")
                .build()
        }));

        let result = handler.execute(request(BytesView::default(), None)).await;
        if let Some(expected_layers) = expected_layers {
            let response = result.unwrap();
            assert_eq!(
                response.extensions().get::<OriginalBody>().unwrap().formats().len(),
                expected_layers
            );
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.label(), "compression_limit_exceeded");
        }
    }
}
