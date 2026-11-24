// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends three requests through two client layers to the server middleware and back, and prints
//! the events that the middlewares emit.
//!
//! The first client layer reports each request as a whole, and the second one each attempt. The
//! handler responds to `GET /books/1`, so each middleware emits its complete event. The example
//! drops `GET /books/2` before the handler responds, as a client does when it gives up on a
//! request. So each middleware emits its abandoned event. The handler returns an error for
//! `POST /books/1/reviews`, so each middleware emits its error event.
//!
//! Each request has an `x-request-id` header, and the response to `GET /books/1` has an `etag`
//! header. The second client layer logs both headers, so its events show the request header, and
//! its complete event also shows the response header.
//!
//! Run the example with this command:
//! ```sh
//! cargo run -p http_telemetry --example http_telemetry_basic
//! ```

#[path = "support/print.rs"]
mod print;

use std::future::pending;
use std::sync::Arc;

use data_privacy::{DataClass, RedactionEngine};
use futures::FutureExt;
use futures::executor::block_on;
use http::header::ETAG;
use http::{HeaderName, HeaderValue, Method};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Service, Stack};
use observed::Sink;
use print::PrintProcessor;
use templated_uri::{BaseUri, Uri, templated};

/// The header that each request has, and that the events of the second client layer show.
const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The data class whose values the printer shows unchanged.
const PUBLIC: DataClass = DataClass::new("example", "public");

/// The path of one book.
#[templated(template = "/books/{book}", unredacted)]
#[derive(Clone)]
struct BookPath {
    book: u32,
}

/// The path of the reviews of one book.
#[templated(template = "/books/{book}/reviews", unredacted)]
#[derive(Clone)]
struct ReviewsPath {
    book: u32,
}

fn main() {
    let sink = Sink::builder("basic")
        .processors(vec![Arc::new(PrintProcessor(
            RedactionEngine::builder().suppress_redaction(PUBLIC).build(),
        ))])
        .build();
    let body_builder = HttpBodyBuilder::new_fake();
    let server = BaseUri::from_static("https://example.com");

    let bodies = body_builder.clone();
    let handler = Execute::new(move |request: HttpRequest| {
        let result = match (request.method(), request.uri().path()) {
            (&Method::GET, "/books/1") => {
                let mut response = HttpResponse::new(bodies.empty());
                response.headers_mut().insert(ETAG, HeaderValue::from_static("\"v1\""));
                Some(Ok(response))
            }
            (&Method::POST, "/books/1/reviews") => Some(Err(HttpError::validation("the review has no text"))),
            _ => None,
        };
        async move {
            match result {
                Some(result) => result,
                None => pending().await,
            }
        }
    });
    let service = (
        Telemetry::client(sink.clone()).report_total_duration(true),
        Telemetry::client(sink.clone())
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(ETAG, PUBLIC)]),
        Telemetry::server(sink),
        handler,
    )
        .into_service();
    let get = |book, request_id: &'static str| {
        HttpRequestBuilder::new(&body_builder)
            .get(Uri::default().with_base(server.clone()).with_path_and_query(BookPath { book }))
            .header(REQUEST_ID, request_id)
            .build()
            .expect("the example URIs and headers are valid")
    };

    println!("GET /books/1");
    let response = block_on(service.execute(get(1, "req-1"))).expect("the handler responds to /books/1");
    println!("The handler responded with {}.\n", response.status());

    println!("GET /books/2");
    let response = service.execute(get(2, "req-2")).now_or_never();
    assert!(response.is_none(), "the handler does not respond to /books/2");
    println!("The example dropped the request before the handler responded.\n");

    println!("POST /books/1/reviews");
    let review = HttpRequestBuilder::new(&body_builder)
        .post(Uri::default().with_base(server).with_path_and_query(ReviewsPath { book: 1 }))
        .header(REQUEST_ID, "req-3")
        .build()
        .expect("the example URIs and headers are valid");
    let error = block_on(service.execute(review)).expect_err("the handler rejects /books/1/reviews");
    println!("The request returned the error: {error}");
}
