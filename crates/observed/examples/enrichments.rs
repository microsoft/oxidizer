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
use observed::{Enrichment, Event, Sink, emit};
use observed_testing::MicrosoftEnterpriseDataTaxonomy;
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

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

#[derive(Event)]
#[event(name = "db.query")]
#[log(severity = info)]
struct DbQuery {
    #[data_class(DC)]
    rows_returned: i64,
}

#[derive(Event)]
#[event(name = "cache.hit")]
#[log(severity = info)]
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
        use std::ops::ControlFlow;

        use opentelemetry::logs::{AnyValue, LogRecord, Logger};

        let mut record = self.logger.create_log_record();
        record.set_event_name(event.name());
        if let Some(severity) = event.severity() {
            record.set_severity_number(opentelemetry::logs::Severity::from(severity));
            record.set_severity_text(severity.as_str());
        }
        record.set_timestamp(std::time::SystemTime::now());
        if let Some(body) = event.body() {
            record.set_body(AnyValue::String(body.into_owned().into()));
        }

        let engine = &self.redaction_engine;
        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });
        if let Some(file) = event.source_file() {
            record.add_attribute(
                opentelemetry::Key::from_static_str("code.file.path"),
                AnyValue::String(file.into_owned().into()),
            );
        }
        if let Some(line) = event.source_line() {
            record.add_attribute(
                opentelemetry::Key::from_static_str("code.line.number"),
                AnyValue::Int(i64::from(line)),
            );
        }
        self.logger.emit(record);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    // Build and register an sink with a processor.
    let sink = Sink::new(
        "enrichments",
        vec![Arc::new(SimpleLogProcessor {
            logger: provider.logger("enrichments"),
            redaction_engine: data_privacy::RedactionEngine::default(),
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

    let _ = provider.shutdown();

    // Print captured logs showing enrichment stacking.
    for (i, log) in exporter.get_emitted_logs().expect("should get logs").iter().enumerate() {
        let name = log.record.event_name().unwrap_or("?");
        println!("--- Event {i}: {name} ---");
        for (key, value) in log.record.attributes_iter() {
            println!("  {key} = {value:?}");
        }
        println!();
    }
}
