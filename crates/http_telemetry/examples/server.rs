// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends two requests through the server middleware, and prints the events that it emits.
//!
//! The handler responds to `GET /books/1`, so the middleware emits the complete event. The
//! example drops `GET /books/2` before the handler responds, as a server does when the client
//! disconnects. So the middleware emits the abandoned event.
//!
//! Run the example with this command:
//! ```sh
//! cargo run -p http_telemetry --example server
//! ```

use std::future::pending;
use std::ops::ControlFlow;
use std::sync::Arc;

use data_privacy::RedactionEngine;
use futures::FutureExt;
use futures::executor::block_on;
use http::Request;
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Service, Stack};
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{FlushError, Sink};

/// Prints the name, the severity, the message and the log fields of each event. Then it prints
/// each metric value with the name, the unit and the dimensions of its metric.
#[derive(Default)]
struct PrintProcessor(RedactionEngine);

impl EventProcessor for PrintProcessor {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let severity = event.severity().map_or("-", |severity| severity.as_str());
        println!("[{severity}] {}", event.name());
        if let Some(message) = event.body() {
            println!("  message: {message}");
        }
        let mut dimensions = Vec::new();
        let mut measurements = Vec::new();
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            let value = value(&self.0);
            if let Some(log) = descriptor.log() {
                println!("  {} = {value}", log.key());
            }
            if let Some(metric) = descriptor.metric() {
                match metric.instrument_description() {
                    Some(instrument) => measurements.push((instrument.instrument_name(), instrument.unit(), value)),
                    None => dimensions.push(format!("{}={value}", metric.key())),
                }
            }
            ControlFlow::Continue(())
        });
        for (instrument, unit, value) in measurements {
            println!("  metric {instrument} = {value} {unit} [{}]", dimensions.join(", "));
        }
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn get(body_builder: &HttpBodyBuilder, path: &str) -> HttpRequest {
    Request::get(path)
        .body(body_builder.empty())
        .expect("the example paths are valid URIs")
}

fn main() {
    let sink = Sink::builder("server")
        .processors(vec![Arc::new(PrintProcessor::default())])
        .build();
    let body_builder = HttpBodyBuilder::new_fake();

    let bodies = body_builder.clone();
    let handler = Execute::new(move |request: HttpRequest| {
        let response = (request.uri().path() == "/books/1").then(|| HttpResponse::new(bodies.empty()));
        async move {
            match response {
                Some(response) => Ok::<_, HttpError>(response),
                None => pending().await,
            }
        }
    });
    let service = (Telemetry::server(sink), handler).into_service();

    println!("GET /books/1");
    let response = block_on(service.execute(get(&body_builder, "/books/1"))).expect("the handler responds to /books/1");
    println!("The handler responded with {}.\n", response.status());

    println!("GET /books/2");
    let response = service.execute(get(&body_builder, "/books/2")).now_or_never();
    assert!(response.is_none(), "the handler does not respond to /books/2");
    println!("The example dropped the request before the handler responded.");
}
