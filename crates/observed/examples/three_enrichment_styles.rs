// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Three enrichment scopes - nested struct enrichments.
//!
//! `handle_request` enriches with the route, inside it a closure adds
//! the HTTP method via `EnrichFnExt`, and an async block adds the request ID
//! via `EnrichFutureExt`.
//!
//! The innermost event carries **all three** enrichment entries.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example three_enrichment_styles
//! ```

use std::sync::Arc;

use data_privacy::{DataClass, classified};
use observed::enrichment::{EnrichFnExt, EnrichFutureExt};
use observed::{Enrichment, Sink, emit, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use taxonomy::MicrosoftEnterpriseDataTaxonomy as DC;

#[path = "support/taxonomy.rs"]
mod taxonomy;

#[path = "support/redaction.rs"]
mod redaction;

#[path = "support/otel.rs"]
mod otel;

#[classified(DC::PublicNonPersonalData)]
#[derive(Clone)]
struct HttpRoute(pub String);

#[classified(DC::PublicNonPersonalData)]
#[derive(Clone)]
struct HttpMethod(pub &'static str);

#[classified(DC::PublicNonPersonalData)]
#[derive(Clone)]
struct RequestId(pub &'static str);

#[derive(Debug, Clone, Enrichment)]
struct RouteContext {
    #[dimension(log = "http.route")]
    route: HttpRoute,
}

#[derive(Debug, Enrichment)]
struct MethodContext {
    #[dimension(log = "http.method")]
    method: HttpMethod,
}

#[derive(Debug, Enrichment)]
struct RequestIdContext {
    #[dimension(log = "request.id")]
    request_id: RequestId,
}

#[tokio::main]
async fn main() {
    let (sink, provider) = init_telemetry();

    handle_request(&sink, HttpRoute("/users".to_owned())).await;

    let _ = provider.shutdown();
}

const EXAMPLE_DC: DataClass = DataClass::new("example", "public");

#[event("http.request")]
#[info]
struct HttpRequest {
    #[data_class(EXAMPLE_DC)]
    status: i64,
}

async fn handle_request(sink: &Sink, route: HttpRoute) {
    async {
        tokio::task::yield_now().await;
        emit!(sink, HttpRequest { status: 1 });
        (|| {
            emit!(sink, HttpRequest { status: 2 });
        })
        .enrich(sink, MethodContext { method: HttpMethod("GET") })(); // EnrichFnExt on closure
    }
    .enrich(
        sink,
        RequestIdContext {
            request_id: RequestId("r-42"),
        },
    ) // EnrichFutureExt on async block
    .enrich(sink, RouteContext { route }) // outer scope
    .await;
}

struct SimpleLogProcessor {
    logger: opentelemetry_sdk::logs::SdkLogger,
    redaction_engine: data_privacy::RedactionEngine,
}

impl observed::processing::EventProcessor for SimpleLogProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        use opentelemetry::logs::Logger;

        let mut record = self.logger.create_log_record();
        if otel::populate_log_record(&mut record, event, &self.redaction_engine) {
            self.logger.emit(record);
        }
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

fn init_telemetry() -> (Sink, SdkLoggerProvider) {
    let provider = SdkLoggerProvider::builder()
        .with_simple_exporter(opentelemetry_stdout::LogExporter::default())
        .build();

    let logger = provider.logger("example");
    let processor = SimpleLogProcessor {
        logger,
        redaction_engine: redaction::passthrough_redaction_engine(),
    };

    let sink = Sink::new(
        "three_enrichment_styles",
        vec![Arc::new(processor)],
        tick::SimpleClock::new_system(),
    );

    (sink, provider)
}
