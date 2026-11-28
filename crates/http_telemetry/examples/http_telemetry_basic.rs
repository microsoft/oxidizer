// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends three requests through two client layers to the server middleware and back, and prints
//! the events that the middlewares emit.
//!
//! The first client layer reports each request as a whole, and the second one each attempt. The
//! handler responds to `PUT /books/1`, so each middleware emits its complete event. The example
//! drops `GET /books/2` before the handler responds, as a client does when it gives up on a
//! request. So each middleware emits its abandoned event. The handler returns an error for
//! `POST /books/1/reviews`, so each middleware emits its error event.
//!
//! Each request has an `x-request-id` header, and the response to `PUT /books/1` has an `etag`
//! header. The second client layer logs both headers, so its events show the request header, and
//! its complete event also shows the response header.
//!
//! The `PUT` and `POST` requests have JSON bodies, and so does the response to `PUT /books/1`. The
//! second client layer logs JSON bodies, so its events show the request body, and its complete
//! event also shows the response body. It emits that event when the example drops the response
//! body after it reads the body. So that event comes after the complete events of the other two
//! middlewares.
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
use http::header::{CONTENT_TYPE, ETAG};
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
            (&Method::PUT, "/books/1") => {
                let mut response = HttpResponse::new(bodies.text(r#"{"book":1,"title":"Dune"}"#));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                response.headers_mut().insert(ETAG, HeaderValue::from_static("\"v2\""));
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
            .response_header_data_classes([(ETAG, PUBLIC)])
            .request_body_media_types(["application/json"])
            .expect("the media type is valid")
            .response_body_media_types(["application/json"])
            .expect("the media type is valid"),
        Telemetry::server(sink),
        handler,
    )
        .into_service();
    let book = |book| Uri::default().with_base(server.clone()).with_path_and_query(BookPath { book });

    println!("PUT /books/1");
    let update = HttpRequestBuilder::new(&body_builder)
        .put(book(1))
        .header(REQUEST_ID, "req-1")
        .header(CONTENT_TYPE, "application/json")
        .text(r#"{"title":"Dune"}"#)
        .build()
        .expect("the example URIs and headers are valid");
    let response = block_on(service.execute(update)).expect("the handler responds to /books/1");
    let status = response.status();
    let text = block_on(response.into_body().into_text()).expect("the response body is text");
    println!("The handler responded with {status} and the body {text}.\n");

    println!("GET /books/2");
    let request = HttpRequestBuilder::new(&body_builder)
        .get(book(2))
        .header(REQUEST_ID, "req-2")
        .build()
        .expect("the example URIs and headers are valid");
    let response = service.execute(request).now_or_never();
    assert!(response.is_none(), "the handler does not respond to /books/2");
    println!("The example dropped the request before the handler responded.\n");

    println!("POST /books/1/reviews");
    let review = HttpRequestBuilder::new(&body_builder)
        .post(Uri::default().with_base(server).with_path_and_query(ReviewsPath { book: 1 }))
        .header(REQUEST_ID, "req-3")
        .header(CONTENT_TYPE, "application/json")
        .text(r#"{"stars":5}"#)
        .build()
        .expect("the example URIs and headers are valid");
    let error = block_on(service.execute(review)).expect_err("the handler rejects /books/1/reviews");
    println!("The request returned the error: {error}");
}
