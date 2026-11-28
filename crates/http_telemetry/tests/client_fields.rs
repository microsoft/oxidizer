// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The fields of the client events that come from the request URI, and the fields that
//! `dimension` adds.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::sync::Arc;

use common::{CLIENT_ABANDONED, CLIENT_COMPLETE, CLIENT_ERROR, Capture, TOTAL_ABANDONED, TOTAL_COMPLETE, TOTAL_ERROR, client};
use future_assert::FutureTestExt;
use http::StatusCode;
use http_extensions::{HttpRequest, HttpRequestBuilder, UriTemplateLabel};
use layered::{Layer, Service};
use observed::{Sink, SinkId, Value};
use templated_uri::{BaseUri, EscapedString, Uri, templated};
use tick::SimpleClock;

/// The path of one book, with the label `book`.
#[templated(template = "/books/{book}", label = "book", unredacted)]
#[derive(Clone)]
struct LabeledBookPath {
    book: EscapedString,
}

/// The fields `keys` of the complete event of `request`, each as a log field and as a metric
/// dimension.
fn fields<const N: usize>(request: HttpRequest, keys: [&str; N]) -> [(Option<Value>, Option<Value>); N] {
    let capture = Capture::new(|name| name == CLIENT_COMPLETE);
    client(common::sink_with(&capture))
        .layer(common::responding(StatusCode::OK))
        .execute(request)
        .unwrap_ready()
        .unwrap();
    let event = capture.single();
    keys.map(|key| (event.log(key).cloned(), event.dimension(key).cloned()))
}

/// `value` as a log field and as a metric dimension.
fn log_and_metric(value: Option<Value>) -> (Option<Value>, Option<Value>) {
    (value.clone(), value)
}

/// `request` with the extension `label`.
fn with_label(mut request: HttpRequest, label: UriTemplateLabel) -> HttpRequest {
    request.extensions_mut().insert(label);
    request
}

#[test]
fn server_address_and_port_come_from_uri_authority() {
    let rows = [
        ("https://example.com/books/1", Some("example.com"), Some(443)),
        ("http://example.com/books/1", Some("example.com"), Some(80)),
        ("https://example.com:8443/books/1", Some("example.com"), Some(8443)),
        ("ftp://example.com/books/1", Some("example.com"), None),
        ("http://[2001:db8::1]:8080/books/1", Some("[2001:db8::1]"), Some(8080)),
        ("/books/1", None, None),
    ];

    assert_eq!(
        rows.map(|(uri, ..)| fields(common::get_to(uri), ["server.address", "server.port"])),
        rows.map(|(_, address, port)| [log_and_metric(address.map(Value::from)), log_and_metric(port.map(Value::I64))])
    );
}

#[test]
fn url_scheme_is_http_https_other_scheme_text_or_other() {
    let rows = [
        ("https://example.com/books/1", "https"),
        ("http://example.com/books/1", "http"),
        ("ftp://example.com/books/1", "ftp"),
        ("/books/1", "_OTHER"),
    ];

    assert_eq!(
        rows.map(|(uri, _)| fields(common::get_to(uri), ["url.scheme"])),
        rows.map(|(_, scheme)| [log_and_metric(Some(Value::from(scheme)))])
    );
}

#[test]
fn url_template_comes_from_label_extension_else_templated_path_else_unknown() {
    let labeled_path = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path_and_query(LabeledBookPath {
            book: EscapedString::from_static("1"),
        });
    let requests = [
        with_label(common::get_book(), UriTemplateLabel::new_static("book_by_id")),
        with_label(
            common::get_to("https://example.com/books/1"),
            UriTemplateLabel::new(String::from("book_by_id")),
        ),
        HttpRequestBuilder::new_fake().get(labeled_path).build().unwrap(),
        common::get_book(),
        HttpRequestBuilder::new_fake().get("https://example.com/books/1").build().unwrap(),
        common::get_to("https://example.com/books/1"),
    ];

    assert_eq!(
        requests.map(|request| fields(request, ["url.template"])),
        ["book_by_id", "book_by_id", "book", "/books/{book}", "unknown", "unknown"]
            .map(|template| [log_and_metric(Some(Value::from(template)))])
    );
}

#[test]
fn redacted_path_is_redacted_by_each_processor() {
    let passthrough = Capture::new(|name| name == CLIENT_COMPLETE);
    let public = Capture::with_engine(|name| name == CLIENT_COMPLETE, common::public_engine());
    let sink = Sink::builder(SinkId::new("two_processors"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(passthrough.clone()), Arc::new(public.clone())])
        .build();
    let service = client(sink).layer(common::responding(StatusCode::OK));

    service.execute(common::get_book()).unwrap_ready().unwrap();
    // Without a `PathAndQuery` extension, the path and the query of the URI are classified data.
    service
        .execute(common::get_to("https://example.com/books/1?page=2"))
        .unwrap_ready()
        .unwrap();

    let paths = |capture: &Capture| -> Vec<_> {
        capture
            .events()
            .iter()
            .map(|event| event.log("url.path.redacted").cloned())
            .collect()
    };
    assert_eq!(
        [paths(&passthrough), paths(&public)],
        [
            vec![Some(Value::from("/books/1")), Some(Value::from("/books/1?page=2"))],
            vec![Some(Value::from("/books/")), Some(Value::from(""))],
        ]
    );
}

#[test]
fn dimension_adds_field_to_every_event() {
    let (sink, capture) = common::capture_sink();

    for report_total_duration in [false, true] {
        let layer = client(sink.clone())
            .report_total_duration(report_total_duration)
            .dimension("http.client.name", "books");
        layer
            .layer(common::responding(StatusCode::OK))
            .execute(common::get())
            .unwrap_ready()
            .unwrap();
        layer.layer(common::FAILING).execute(common::get()).unwrap_ready().unwrap_err();
        drop(layer.layer(common::NeverResponding).execute(common::get()));
    }

    let client_names: Vec<_> = capture
        .events()
        .iter()
        .map(|event| {
            (
                event.name,
                event.log("http.client.name").cloned(),
                event.dimension("http.client.name").cloned(),
            )
        })
        .collect();
    assert_eq!(
        client_names,
        [
            CLIENT_COMPLETE,
            CLIENT_ERROR,
            CLIENT_ABANDONED,
            TOTAL_COMPLETE,
            TOTAL_ERROR,
            TOTAL_ABANDONED
        ]
        .map(|name| (name, Some(Value::from("books")), Some(Value::from("books"))))
        .to_vec()
    );
}

#[test]
fn dimension_with_same_key_replaces_value() {
    let (sink, capture) = common::capture_sink();
    let service = client(sink)
        .dimension("http.client.name", "books")
        .dimension("fetch.runtime", "tokio")
        .dimension("http.client.name", String::from("library"))
        .layer(common::responding(StatusCode::OK));

    service.execute(common::get()).unwrap_ready().unwrap();

    let event = capture.single();
    assert_eq!(
        ["http.client.name", "fetch.runtime"].map(|key| (event.log(key).cloned(), event.dimension(key).cloned())),
        [
            log_and_metric(Some(Value::from("library"))),
            log_and_metric(Some(Value::from("tokio")))
        ]
    );
}
