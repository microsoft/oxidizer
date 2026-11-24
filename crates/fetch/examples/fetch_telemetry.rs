// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! # HTTP client telemetry
//!
//! This example:
//!
//! 1. Creates an `observed` sink with a processor that prints each event.
//! 2. Passes the sink to an HTTP client through its deps.
//! 3. Sends a request that the client retries. It gives two attempt events and one total event.
//! 4. Sends a request with a templated URI. Its events show the template and mask the secret part of
//!    the path.
//! 5. Sends a request inside an enrichment scope. Its events have a custom dimension.
//! 6. Uses a fake handler without network I/O.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use data_privacy::simple_redactor::SimpleRedactor;
use data_privacy::{DataClass, RedactionEngine, Sensitive};
use fetch::fake::{FakeDeps, FakeHandler};
use fetch::{HttpClient, HttpResponseBuilder};
use http::StatusCode;
use observed::enrichment::EnrichFutureExt;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{Enrichment, FlushError, Sink};
use templated_uri::{BaseUri, EscapedString, Uri, templated};
use tick::ClockControl;

const UNKNOWN: DataClass = DataClass::new("unknown", "unknown");

#[templated(template = "/path/to{/public,secret}")]
#[derive(Clone)]
struct ResourcePath {
    #[unredacted]
    public: EscapedString,
    secret: Sensitive<EscapedString>,
}

/// A custom dimension for the events of each request in its scope.
#[derive(Enrichment)]
struct Region {
    #[dimension(log = "cloud.region", metric = "cloud.region")]
    #[unredacted]
    region: &'static str,
}

/// Prints the name, the severity, the message and every field of each event.
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
        let mut print = |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            if let Some(log) = descriptor.log() {
                println!("  {} = {}", log.key(), value(&self.0));
            }
            ControlFlow::Continue(())
        };
        let _ = event.visit_fields(&mut print);
        let _ = event.visit_enrichments(&mut print);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // The engine masks classified text, such as the secret part of a templated path.
    let engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    let sink = Sink::builder("example").processors(vec![Arc::new(PrintProcessor(engine))]).build();

    // The fake handler responds to the first call with 500 and to every later call with 200.
    let calls = AtomicUsize::new(0);
    let handler = FakeHandler::from_fn(move |_| {
        let status = if calls.fetch_add(1, Ordering::Relaxed) == 0 {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        };
        HttpResponseBuilder::new_fake().status(status).build()
    });

    // The clock fires each timer at once, so the retry delay passes without a wait. The timeouts are
    // large, because the clock can fire a timeout before the response.
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(handler, FakeDeps::with_clock(&clock, sink.clone()))
        .name("example_client")
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|timeout| timeout.timeout(Duration::MAX))
                .attempt_timeout(|timeout| timeout.timeout(Duration::MAX))
        })
        .build();

    println!("A request that the client retries:");
    _ = client.get("https://example.com/books").fetch().await?;

    let resource_path = ResourcePath {
        public: EscapedString::from_static("public_resource"),
        secret: Sensitive::new(EscapedString::from_static("secret_resource"), UNKNOWN),
    };
    let target = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path_and_query(resource_path);

    println!("\nA request with a templated URI:");
    _ = client.get(target).fetch().await?;

    println!("\nA request inside an enrichment scope:");
    _ = async { client.get("https://example.com/books").fetch().await }
        .enrich(&sink, Region { region: "westus" })
        .await?;

    Ok(())
}
