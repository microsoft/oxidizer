// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates scoped enrichments and batch enrichment.
//!
//! Enrichments are key-value pairs attached to every event emitted within
//! a closure's scope. They stack and unwind automatically.
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example enrichments
//! ```

use std::sync::Arc;

use data_privacy::{DataClass, classified};
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Sink, emit, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
use taxonomy::MicrosoftEnterpriseDataTaxonomy;

#[path = "support/taxonomy.rs"]
mod taxonomy;

#[path = "support/redaction.rs"]
mod redaction;

#[path = "support/otel.rs"]
mod otel;

// ---------------------------------------------------------------------------
// Classified newtypes - every enrichment value has a concrete type with a
// compile-time data classification from the Microsoft Enterprise taxonomy.
// ---------------------------------------------------------------------------

/// Service name - identifies the running service.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct ServiceName(pub &'static str);

/// HTTP request method.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct HttpMethod(pub &'static str);

/// HTTP route path.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct HttpRoute(pub &'static str);

/// Tenant identifier - organization-level identifier.
#[classified(MicrosoftEnterpriseDataTaxonomy::AccountData)]
#[derive(Clone)]
struct TenantId(pub &'static str);

// ---------------------------------------------------------------------------
// Enrichment types
// ---------------------------------------------------------------------------

#[derive(Enrichment)]
struct ServiceNameEnrich {
    #[dimension(log = "service.name")]
    service_name: ServiceName,
}

#[derive(Enrichment)]
struct TenantIdEnrich {
    #[dimension(log = "tenant.id")]
    tenant_id: TenantId,
}

#[derive(Enrichment)]
struct HttpContextEnrich {
    #[dimension(log = "http.method")]
    method: HttpMethod,
    #[dimension(log = "http.route")]
    route: HttpRoute,
}

const DC: DataClass = DataClass::new("example", "public");

#[event("db.query")]
#[info]
struct DbQuery {
    #[data_class(DC)]
    rows_returned: i64,
}

#[event("cache.hit")]
#[info]
struct CacheHit {
    #[data_class(DC)]
    key_count: i64,
}

/// A simple processor that forwards events to an `OTel` logger provider.
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

/// Simulates a database lookup inside an enrichment scope.
fn handle_request(sink: &Sink) {
    // Batch enrichment - adds multiple attributes to all nested events.
    (|| {
        // Inner enrichment - stacks on top of the span.
        (|| {
            emit!(sink, DbQuery { rows_returned: 42 });
            emit!(sink, CacheHit { key_count: 3 });
        })
        .enrich(
            sink,
            TenantIdEnrich {
                tenant_id: TenantId("contoso"),
            },
        )();

        // After the enrichment scope ends, events no longer carry its attributes.
        emit!(sink, CacheHit { key_count: 1 });
    })
    .enrich(
        sink,
        HttpContextEnrich {
            method: HttpMethod("GET"),
            route: HttpRoute("/users"),
        },
    )();
}

fn main() {
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();

    // Build and register a sink with a processor.
    let sink = Sink::new(
        "enrichments",
        vec![Arc::new(SimpleLogProcessor {
            logger: provider.logger("enrichments"),
            redaction_engine: redaction::passthrough_redaction_engine(),
        })],
        tick::SimpleClock::new_system(),
    );

    // Top-level enrichment visible throughout the program.
    (|| {
        handle_request(&sink);
    })
    .enrich(
        &sink,
        ServiceNameEnrich {
            service_name: ServiceName("example-svc"),
        },
    )();

    // Read emitted logs *before* shutdown because OTel 0.32's
    // `InMemoryLogExporter` clears its buffer on shutdown by default.
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();

    // Print captured logs showing enrichment stacking.
    for (i, log) in logs.iter().enumerate() {
        let name = log.record.event_name().unwrap_or("?");
        println!("--- Event {i}: {name} ---");
        for (key, value) in log.record.attributes_iter() {
            println!("  {key} = {value:?}");
        }
        println!();
    }
}
