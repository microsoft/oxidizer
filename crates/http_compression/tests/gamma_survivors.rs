// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory survivor contracts using the existing fake HTTP service and body builder.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::num::NonZeroU64;

use bytesbuf::BytesView;
use compressors::format::Format;
use compressors::{DecompressorLimits, Resources};
use futures::executor::block_on;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, VARY};
use http::{HeaderValue, Method, StatusCode};
use http_compression::{Compression, OriginalBody};
use http_extensions::{FakeHandler, HttpBodyBuilder, HttpRequest, HttpResponseBuilder};
use layered::{Layer as _, Service as _};
use ohno::Labeled as _;

fn request(method: Method, accept: Option<&str>) -> HttpRequest {
    let mut request = http::Request::builder()
        .method(method)
        .uri("https://example.com/")
        .body(HttpBodyBuilder::new_fake().empty())
        .unwrap();
    if let Some(accept) = accept {
        request
            .headers_mut()
            .insert(ACCEPT_ENCODING, HeaderValue::from_str(accept).unwrap());
    }
    request
}

#[test]
fn gamma_round2_implicit_identity_accepts_existing_identity_coding() {
    block_on(async {
        let handler = Compression::server()
            .compress_responses(&[Format::Gzip])
            .layer(FakeHandler::from_fn(|_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(CONTENT_ENCODING, HeaderValue::from_static("IdEnTiTy"))
                    .text("uncompressed")
                    .build()
            }));
        let response = handler.execute(request(Method::GET, Some("gzip"))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "IdEnTiTy");
        assert_eq!(response.into_body().into_text().await.unwrap(), "uncompressed");
    });
}

#[test]
fn gamma_round2_disabled_request_decompression_ignores_fail_policy() {
    block_on(async {
        let handler = Compression::server()
            .decompress_requests(&[])
            .on_unsupported(http_compression::UnsupportedCompression::Fail)
            .layer(FakeHandler::from_fn(|request: HttpRequest| {
                assert_eq!(request.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
                assert!(request.extensions().get::<OriginalBody>().is_none());
                HttpResponseBuilder::new_fake().status(StatusCode::OK).text("untouched").build()
            }));
        let request = http::Request::builder()
            .uri("https://example.com/")
            .header(CONTENT_ENCODING, "gzip")
            .body(HttpBodyBuilder::new_fake().text("opaque encoded bytes"))
            .unwrap();
        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.into_body().into_text().await.unwrap(), "untouched");
    });
}

#[test]
fn gamma_afternoon_connect_and_disabled_compression_preserve_origin_responses() {
    block_on(async {
        for (method, status, enabled, expected) in [
            (Method::CONNECT, StatusCode::BAD_REQUEST, true, false),
            (Method::CONNECT, StatusCode::OK, true, false),
            (Method::GET, StatusCode::OK, false, false),
        ] {
            let handler = Compression::server()
                .compress_responses(if enabled { &[Format::Gzip] } else { &[] })
                .layer(FakeHandler::from_fn(move |_| {
                    HttpResponseBuilder::new_fake().status(status).text("payload").build()
                }));
            let accept = if enabled { "gzip" } else { "*;q=0, identity;q=0" };
            let response = handler.execute(request(method, Some(accept))).await.unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers().contains_key(CONTENT_ENCODING), expected);
            assert_eq!(response.headers().contains_key(VARY), expected);
            let body = response.into_body().into_bytes().await.unwrap();
            let plain = if expected {
                compressors::format::decompress(Format::Gzip, body, Resources::global()).unwrap()
            } else {
                body
            };
            assert_eq!(plain.to_vec(), b"payload");
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip round trips; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_negotiation_quality_invalid_members_and_existing_codings() {
    block_on(async {
        for (accept, expected) in [
            ("gzip;Q=0.7, br;q=0.2", Some("gzip")),
            ("gzip;q=0.7;bad=1, br;q=0.2", Some("br")),
            ("gzip;q=0./, br;q=0.2", Some("br")),
            ("gzip;q=0.abc, br;q=0.2", Some("br")),
            ("gzip;q=0, *;q=1", Some("br")),
            ("gzip;q=0.999, identity;q=1", None),
            ("gzip;q=0.001, identity;q=0", Some("gzip")),
            ("gzip;q=0, br;q=0", None),
            ("gzip;q=1.001, br;q=0.999", Some("br")),
        ] {
            let handler = Compression::server()
                .compress_responses(&[Format::Gzip, Format::Brotli])
                .layer(FakeHandler::from_fn(|_| {
                    HttpResponseBuilder::new_fake().status(StatusCode::OK).text("payload").build()
                }));
            let response = handler.execute(request(Method::GET, Some(accept))).await.unwrap();
            assert_eq!(
                response.headers().get(CONTENT_ENCODING).map(|h| h.to_str().unwrap()),
                expected,
                "{accept}"
            );
            assert_eq!(response.headers().get(VARY).unwrap(), "accept-encoding");
            let bytes = response.into_body().into_bytes().await.unwrap();
            let plain = match expected {
                Some("gzip") => compressors::format::decompress(Format::Gzip, bytes, Resources::global()).unwrap(),
                Some("br") => compressors::format::decompress(Format::Brotli, bytes, Resources::global()).unwrap(),
                None => bytes,
                _ => unreachable!(),
            };
            assert_eq!(plain.to_vec(), b"payload");
        }
        for coding in [
            "identity",
            ",identity",
            " , identity, ",
            "gzip, identity",
            ", gzip",
            "gzip, , identity",
        ] {
            let coding = HeaderValue::from_str(coding).unwrap();
            let handler = Compression::server()
                .compress_responses(&[Format::Gzip])
                .layer(FakeHandler::from_fn(move |_| {
                    HttpResponseBuilder::new_fake()
                        .status(StatusCode::OK)
                        .header(CONTENT_ENCODING, coding.clone())
                        .text("existing")
                        .build()
                }));
            let response = handler.execute(request(Method::GET, Some("gzip, identity;q=1"))).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.into_body().into_text().await.unwrap(), "existing");
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip compression; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_type_patterns_and_builtin_exclusions() {
    block_on(async {
        for (pattern, actual, expected) in [
            ("text/plain", "application/plain", false),
            ("application/plain", "text/plain", false),
            ("application/ld+json", "application/problem+json", false),
            ("application/ld+json", "application/ld+json", true),
            ("application/json", "application/ld+json", true),
            ("text/*", "text/plain;charset=utf-8", true),
            ("*/*", "image/png", false),
            ("*/*", "image/svg+xml", true),
            ("*/*", "text/event-stream", false),
            ("*/*", "audio/plain", false),
            ("*/*", "video/plain", false),
            ("*/*", "application/grpc", false),
            ("*/*", "application/gzip", false),
            ("*/*", "application/x-gzip", false),
            ("*/*", "application/zip", false),
            ("*/*", "application/zstd", false),
            ("*/*", "application/x-bzip2", false),
            ("*/*", "application/x-xz", false),
            ("*/*", "application/x-7z-compressed", false),
            ("*/*", "application/vnd.rar", false),
            ("*/*", "application/x-rar-compressed", false),
            ("*/*", "text/gzip", true),
            ("*/*", "text/grpc", true),
        ] {
            let handler = Compression::server()
                .compress_responses(&[Format::Gzip])
                .compressible_types([pattern])
                .unwrap()
                .layer(FakeHandler::from_fn(move |_| {
                    HttpResponseBuilder::new_fake()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, HeaderValue::from_static(actual))
                        .text("payload")
                        .build()
                }));
            let response = handler.execute(request(Method::GET, Some("gzip"))).await.unwrap();
            assert_eq!(response.headers().contains_key(CONTENT_ENCODING), expected, "{pattern}/{actual}");
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip compression; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_response_metadata_vary_members_and_sensitive_etag() {
    block_on(async {
        for vary in [
            "origin, Accept-Encoding",
            "Accept-Encoding, origin",
            "origin, *",
            "origin, accept-encoding, cookie",
        ] {
            let mut etag = HeaderValue::from_static("\"private\"");
            etag.set_sensitive(true);
            let handler = Compression::server()
                .compress_responses(&[Format::Gzip])
                .layer(FakeHandler::from_fn(move |_| {
                    HttpResponseBuilder::new_fake()
                        .status(StatusCode::OK)
                        .header(VARY, HeaderValue::from_static(vary))
                        .header(ETAG, etag.clone())
                        .text("payload")
                        .build()
                }));
            let response = handler.execute(request(Method::GET, Some("gzip"))).await.unwrap();
            assert_eq!(response.headers().get_all(VARY).iter().count(), 1);
            assert_eq!(response.headers().get(VARY).unwrap(), vary);
            assert_eq!(response.headers().get(ETAG).unwrap(), "W/\"private\"");
            assert!(response.headers().get(ETAG).unwrap().is_sensitive());
            let length = response.headers().get(CONTENT_LENGTH).unwrap().clone();
            let wire = response.into_body().into_bytes().await.unwrap();
            assert_eq!(length, HeaderValue::from(wire.len()));
        }
        for method in [Method::HEAD, Method::CONNECT] {
            let handler = Compression::server()
                .compress_responses(&[Format::Gzip])
                .layer(FakeHandler::from_fn(|_| {
                    HttpResponseBuilder::new_fake().status(StatusCode::OK).text("payload").build()
                }));
            let response = handler.execute(request(method, Some("identity;q=0,*;q=0"))).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(CONTENT_ENCODING));
            assert!(!response.headers().contains_key(VARY));
        }
    });
}

#[test]
fn gamma_survivor_client_advertising_toggle_and_quality_floor() {
    block_on(async {
        for advertised in [false, true] {
            let handler = Compression::client()
                .decompress_responses(&[Format::Gzip])
                .advertise(false)
                .advertise(advertised)
                .layer(FakeHandler::from_fn(move |request: HttpRequest| {
                    assert_eq!(request.headers().contains_key(ACCEPT_ENCODING), advertised);
                    HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
                }));
            handler.execute(request(Method::GET, None)).await.unwrap();
        }
        let formats = [Format::Gzip; 12];
        let handler = Compression::client()
            .decompress_responses(&formats)
            .layer(FakeHandler::from_fn(|request: HttpRequest| {
                let expected = "gzip, gzip;q=0.9, gzip;q=0.8, gzip;q=0.7, gzip;q=0.6, gzip;q=0.5, gzip;q=0.4, gzip;q=0.3, gzip;q=0.2, gzip;q=0.1, gzip;q=0.1, gzip;q=0.1";
                assert_eq!(request.headers().get(ACCEPT_ENCODING).unwrap(), expected);
                HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
            }));
        handler.execute(request(Method::GET, None)).await.unwrap();
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip decompression; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_decompression_skips_empty_identity_and_preserves_original_metadata() {
    block_on(async {
        for coding in [", gzip", "identity, gzip", "gzip,identity", "gzip,,identity"] {
            let wire = compressors::format::compress(Format::Gzip, b"payload", Resources::global()).unwrap();
            let length = wire.len() as u64;
            let handler = Compression::server()
                .decompress_requests(&[Format::Gzip])
                .layer(FakeHandler::from_async_fn(move |request: HttpRequest| async move {
                    assert!(!request.headers().contains_key(CONTENT_ENCODING));
                    assert!(!request.headers().contains_key(CONTENT_LENGTH));
                    assert_eq!(request.headers().get(CONTENT_RANGE).unwrap(), "bytes 0-1/2");
                    let original = request.extensions().get::<OriginalBody>().unwrap();
                    assert_eq!(original.formats(), [Format::Gzip]);
                    assert_eq!(original.content_length(), Some(length));
                    assert_eq!(request.into_body().into_text().await.unwrap(), "payload");
                    HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
                }));
            let mut input = request(Method::POST, None);
            *input.body_mut() = HttpBodyBuilder::new_fake().bytes(wire);
            input.headers_mut().insert(CONTENT_ENCODING, HeaderValue::from_static(coding));
            input
                .headers_mut()
                .insert(CONTENT_LENGTH, HeaderValue::from_str(&length.to_string()).unwrap());
            input.headers_mut().insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0-1/2"));
            handler.execute(input).await.unwrap();
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip error paths; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_corrupt_and_over_limit_body_labels() {
    block_on(async {
        for limited in [false, true] {
            let wire = if limited {
                compressors::format::compress(Format::Gzip, b"payload", Resources::global()).unwrap()
            } else {
                BytesView::copied_from_slice(b"invalid gzip", Resources::global().memory())
            };
            let handler = Compression::client()
                .decompress_responses(&[Format::Gzip])
                .limits(DecompressorLimits::new().max_output_len(NonZeroU64::new(1).unwrap()))
                .layer(FakeHandler::from_fn(move |_| {
                    HttpResponseBuilder::new_fake()
                        .status(StatusCode::OK)
                        .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                        .bytes(wire.clone())
                        .build()
                }));
            let response = handler.execute(request(Method::GET, None)).await.unwrap();
            let error = response.into_body().into_bytes().await.unwrap_err();
            assert_eq!(
                error.label(),
                if limited {
                    "compression_limit_exceeded"
                } else {
                    "compression_invalid"
                }
            );
        }
        let handler = Compression::server()
            .decompress_requests(&[Format::Gzip])
            .layer(FakeHandler::from_fn(|_| {
                panic!("too many layers must fail before calling the inner handler")
            }));
        let mut input = request(Method::POST, None);
        *input.body_mut() = HttpBodyBuilder::new_fake().text("payload");
        input
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_str(&["gzip"; 17].join(",")).unwrap());
        let error = handler.execute(input).await.unwrap_err();
        assert_eq!(error.label(), "compression_limit_exceeded");
        assert!(
            error.to_string().contains("body uses more than 16 content-coding layers"),
            "{error}"
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip round trips; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_layer_uses_injected_resource_memory() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::mem::{CallbackMemory, GlobalPool};
    static RESERVATIONS: AtomicUsize = AtomicUsize::new(0);
    fn reserve(memory: &GlobalPool, size: usize) -> bytesbuf::BytesBuf {
        RESERVATIONS.fetch_add(1, Ordering::Relaxed);
        memory.reserve(size)
    }
    block_on(async {
        let resources = Resources::new(CallbackMemory::new(GlobalPool::new(), reserve));
        let before = RESERVATIONS.load(Ordering::Relaxed);
        let handler = Compression::server()
            .compress_responses(&[Format::Gzip])
            .resources(resources)
            .layer(FakeHandler::from_fn(|_| {
                HttpResponseBuilder::new_fake().status(StatusCode::OK).text("payload").build()
            }));
        let response = handler.execute(request(Method::GET, Some("gzip"))).await.unwrap();
        let compressed = response.into_body().into_bytes().await.unwrap();
        assert_eq!(
            compressors::format::decompress(Format::Gzip, compressed, Resources::global())
                .unwrap()
                .to_vec(),
            b"payload"
        );
        assert!(RESERVATIONS.load(Ordering::Relaxed) > before);
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip decompression; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_bodyless_statuses_and_failed_connect_metadata() {
    block_on(async {
        let wire = compressors::format::compress(Format::Gzip, b"payload", Resources::global()).unwrap();
        for status in [
            StatusCode::CONTINUE,
            StatusCode::SWITCHING_PROTOCOLS,
            StatusCode::NO_CONTENT,
            StatusCode::RESET_CONTENT,
            StatusCode::NOT_MODIFIED,
            StatusCode::PARTIAL_CONTENT,
        ] {
            let wire = wire.clone();
            let expected = wire.clone();
            let handler = Compression::client()
                .decompress_responses(&[Format::Gzip])
                .layer(FakeHandler::from_fn(move |_| {
                    HttpResponseBuilder::new_fake()
                        .status(status)
                        .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                        .bytes(wire.clone())
                        .build()
                }));
            let response = handler.execute(request(Method::GET, None)).await.unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
            assert!(response.extensions().get::<OriginalBody>().is_none());
            assert_eq!(response.into_body().into_bytes().await.unwrap(), expected);
        }
        let handler = Compression::client()
            .decompress_responses(&[Format::Gzip])
            .layer(FakeHandler::from_fn(move |_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::BAD_REQUEST)
                    .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                    .bytes(wire.clone())
                    .build()
            }));
        let response = handler.execute(request(Method::CONNECT, None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.headers().contains_key(CONTENT_ENCODING));
        assert_eq!(response.into_body().into_text().await.unwrap(), "payload");
    });
}

#[test]
fn gamma_survivor_client_forwards_inner_service_error() {
    let handler = Compression::client()
        .decompress_responses(&[Format::Gzip])
        .layer(FakeHandler::from_fn(|_| {
            Err(http_extensions::HttpError::other(
                "inner failure",
                seatbelt::RecoveryInfo::never(),
                ohno::ErrorLabel::from_static("gamma_fake"),
            ))
        }));
    let error = block_on(handler.execute(request(Method::GET, None))).unwrap_err();
    assert_eq!(error.label(), "gamma_fake");
    assert!(error.to_string().contains("inner failure"));
}

#[test]
fn gamma_survivor_wildcard_rejection_does_not_match_unknown_or_empty_tokens() {
    block_on(async {
        for accept in ["xyzzy;q=1,*;q=0", ",;q=1,*;q=0"] {
            let handler = Compression::server()
                .compress_responses(&[Format::Gzip])
                .layer(FakeHandler::from_fn(|_| {
                    HttpResponseBuilder::new_fake().status(StatusCode::OK).text("payload").build()
                }));
            let response = handler.execute(request(Method::GET, Some(accept))).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{accept}");
        }
    });
}

#[test]
fn gamma_survivor_empty_and_raw_format_configuration_is_disabled() {
    block_on(async {
        for formats in [&[][..], &[Format::Deflate][..]] {
            let client = Compression::client()
                .decompress_responses(formats)
                .advertise(true)
                .layer(FakeHandler::from_fn(|request: HttpRequest| {
                    assert!(!request.headers().contains_key(ACCEPT_ENCODING));
                    HttpResponseBuilder::new_fake().status(StatusCode::OK).text("payload").build()
                }));
            client.execute(request(Method::GET, None)).await.unwrap();
            let server = Compression::server().compress_responses(formats).layer(FakeHandler::from_fn(|_| {
                HttpResponseBuilder::new_fake().status(StatusCode::OK).text("payload").build()
            }));
            let response = server.execute(request(Method::GET, Some("identity;q=0,*;q=0"))).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(VARY));
            assert!(!response.headers().contains_key(CONTENT_ENCODING));
            assert_eq!(response.into_body().into_text().await.unwrap(), "payload");
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover gzip compression; zlib-rs teardown violates Stacked Borrows")]
fn gamma_survivor_empty_etag_and_rejected_wire_metadata() {
    block_on(async {
        let handler = Compression::server()
            .compress_responses(&[Format::Gzip])
            .layer(FakeHandler::from_fn(|_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(ETAG, HeaderValue::from_static(""))
                    .text("payload")
                    .build()
            }));
        let response = handler.execute(request(Method::GET, Some("gzip"))).await.unwrap();
        assert_eq!(response.headers().get(ETAG).unwrap(), "W/");
        let wire = compressors::format::compress(Format::Gzip, b"payload", Resources::global()).unwrap();
        let handler = Compression::server()
            .compress_responses(&[Format::Gzip])
            .layer(FakeHandler::from_fn(move |_| {
                HttpResponseBuilder::new_fake()
                    .status(StatusCode::OK)
                    .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
                    .header(CONTENT_LENGTH, HeaderValue::from_str(&wire.len().to_string()).unwrap())
                    .bytes(wire.clone())
                    .build()
            }));
        let response = handler.execute(request(Method::GET, Some("br, identity;q=0"))).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
        assert!(!response.headers().contains_key(CONTENT_ENCODING));
        assert!(!response.headers().contains_key(CONTENT_LENGTH));
        assert_eq!(response.into_body().into_bytes().await.unwrap().len(), 0);
    });
}
