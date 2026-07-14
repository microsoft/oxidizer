// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A layered application demonstrating enrichment, multiple emitters, and metrics.
//!
//! The application has three telemetry layers:
//!
//! | Layer          | Sink id       | Isolation | Purpose                          |
//! |----------------|------------------|-----------|----------------------------------|
//! | **App**        | `APP`            | No        | Service-level request telemetry  |
//! | **Database**   | `DB`             | Yes       | Library-level DB query telemetry |
//! | **Token**      | `TOKEN_ISSUER`   | Yes       | Library-level auth telemetry     |
//!
//! **Key concepts shown:**
//!
//! 1. **Multiple emitters** - each layer has its own `OTel` providers
//! 2. **Composite emitters** - `Sink::composite([a, b])` fans one `emit!`
//!    through multiple emitters
//! 3. **Global enrichment** - `.enrich()` adds context to the app sink
//! 4. **Per-sink enrichment** - `.enrich_for()` targets a specific sink
//! 5. **Batch enrichment** - `.enrich()` adds request-scoped context
//! 6. **Metrics** - histogram, gauge, and counter attributes
//!
//! Run with:
//! ```sh
//! cargo run -p observed --example layered_app
//! ```

mod db;
mod token_issuer;

use std::sync::{Arc, LazyLock};

use data_privacy::classified;
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Event, Sink, SinkId, emit};
use observed_testing::MicrosoftEnterpriseDataTaxonomy;
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLogger, SdkLoggerProvider};

// ---------------------------------------------------------------------------
// Classified newtypes
// ---------------------------------------------------------------------------

/// Service name - identifies the running service.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct ServiceName(pub &'static str);

/// HTTP request method.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
pub(crate) struct HttpMethod(pub &'static str);

/// HTTP route path.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
pub(crate) struct HttpRoute(pub &'static str);

/// Request identifier - server-generated, non-personal.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
pub(crate) struct RequestId(pub String);

/// Deployment region.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct DeploymentRegion(pub &'static str);

/// Token issuer version.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct TokenIssuerVersion(pub &'static str);

/// Database connection pool name.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
pub(crate) struct DbPool(pub &'static str);

// ---------------------------------------------------------------------------
// Enrichment types
// ---------------------------------------------------------------------------

#[derive(Enrichment)]
struct ServiceNameEnrich {
    #[dimension(log = "service.name")]
    service_name: ServiceName,
}

#[derive(Enrichment)]
struct DeploymentRegionEnrich {
    #[dimension(log = "deployment.region")]
    deployment_region: DeploymentRegion,
}

#[derive(Enrichment)]
struct TokenIssuerVersionEnrich {
    #[dimension(log = "token.issuer.version")]
    version: TokenIssuerVersion,
}

#[derive(Enrichment)]
struct DbPoolEnrich {
    #[dimension(log = "db.pool")]
    pool: DbPool,
}

#[derive(Enrichment)]
pub(crate) struct RequestIdEnrich {
    #[dimension(log = "request.id")]
    pub(crate) request_id: RequestId,
}

#[derive(Enrichment)]
pub(crate) struct HttpContextEnrich {
    #[dimension(log = "http.method")]
    pub(crate) method: HttpMethod,
    #[dimension(log = "http.route")]
    pub(crate) route: HttpRoute,
}

// ---------------------------------------------------------------------------
// Sink statics
// ---------------------------------------------------------------------------

/// Service-level sink id.
pub static APP: SinkId = SinkId::new("app");

/// Shared in-memory log exporter for the DB library sink.
static DB_LOGS: LazyLock<InMemoryLogExporter> = LazyLock::new(InMemoryLogExporter::default);

/// Database library sink id - composed via `Sink::composite`.
pub static DB: SinkId = SinkId::new("db");

/// Shared in-memory log exporter for the `TOKEN_ISSUER` library sink.
static TOKEN_LOGS: LazyLock<InMemoryLogExporter> = LazyLock::new(InMemoryLogExporter::default);

/// Token issuer library sink id - composed via `Sink::composite`.
pub static TOKEN_ISSUER: SinkId = SinkId::new("token_issuer");

// ---------------------------------------------------------------------------
// App-level events
// ---------------------------------------------------------------------------

/// An inbound HTTP request handled by the service.
#[derive(Event)]
#[event(name = "http.server.request")]
#[log(severity = info, message = "HTTP request handled")]
#[metric(kind = histogram, field = duration_ms, name = "http.server.request.duration")]
struct HttpServerRequest {
    /// Request duration - recorded as a histogram metric.
    #[unredacted]
    duration_ms: f64,

    /// HTTP status code.
    #[unredacted]
    status: i64,

    /// Number of retries before the request succeeded.
    #[unredacted]
    retries: i64,
}

/// A generic service warning event.
#[derive(Event)]
#[event(name = "app.degraded")]
#[log(severity = warn, message = "Service running in degraded mode")]
struct AppDegraded {
    /// Error code identifying the degradation cause.
    #[unredacted]
    error_code: i64,
}

// ---------------------------------------------------------------------------
// Simple log processor
// ---------------------------------------------------------------------------

struct SimpleLogProcessor {
    logger: SdkLogger,
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

// ---------------------------------------------------------------------------
// Request handler - simulates a real request lifecycle
// ---------------------------------------------------------------------------

/// Simulates handling an inbound HTTP request that reads from the database
/// and validates an authentication token.
fn handle_request(app_emitter: &Sink, db_emitter: &Sink, token_emitter: &Sink, request_id: &str) {
    // Batch enrichment - adds request-level context to all nested events.
    (|| {
        // Per-request enrichment - visible to all non-isolated emitters.
        (|| {
            // 1. Validate the caller's token - emitted through the token-issuer
            //    library sink so events fan out to both app and token logs.
            token_issuer::validate_token(token_emitter, true);

            // 2. Query the database - emitted through the DB library sink.
            let rows = db::query_users(db_emitter, 7);

            // 3. Emit the top-level request event on the app sink.
            emit!(
                app_emitter,
                HttpServerRequest {
                    duration_ms: 23.7,
                    status: 200,
                    retries: 0,
                }
            );

            println!("  -> returned {rows} rows to client");
        })
        .enrich(
            app_emitter,
            RequestIdEnrich {
                request_id: RequestId(request_id.to_owned()),
            },
        )();
    })
    .enrich(
        app_emitter,
        HttpContextEnrich {
            method: HttpMethod("GET"),
            route: HttpRoute("/api/users"),
        },
    )();
}

/// Simulates a request where token validation fails.
fn handle_bad_request(app_emitter: &Sink, token_emitter: &Sink) {
    (|| {
        (|| {
            // Token validation fails - the token_issuer sink captures the failure metric.
            token_issuer::validate_token(token_emitter, false);

            emit!(
                app_emitter,
                HttpServerRequest {
                    duration_ms: 1.2,
                    status: 401,
                    retries: 0,
                }
            );
        })
        .enrich(
            app_emitter,
            RequestIdEnrich {
                request_id: RequestId("req-bad".to_owned()),
            },
        )();
    })
    .enrich(
        app_emitter,
        HttpContextEnrich {
            method: HttpMethod("POST"),
            route: HttpRoute("/api/admin"),
        },
    )();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    // --- 1. Set up the APP sink ---

    let app_logs = InMemoryLogExporter::default();
    let app_logger = SdkLoggerProvider::builder().with_simple_exporter(app_logs.clone()).build();

    let clock = tick::SimpleClock::new_system();

    // App sink - receives global enrichments (not isolated).
    let app_emitter = Sink::new(
        APP,
        vec![Arc::new(SimpleLogProcessor {
            logger: app_logger.logger("app"),
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        &clock,
    );

    // DB sink - standalone sink with its own destination. A composite
    // fans one `emit!` through both the app's processor and the DB log. Each
    // sink has its own enrichment slot, so DB-log records do not inherit
    // enrichments pushed on the app sink.
    let db_logger = SdkLoggerProvider::builder().with_simple_exporter(DB_LOGS.clone()).build();
    let db_standalone = Sink::new(
        DB,
        vec![Arc::new(SimpleLogProcessor {
            logger: db_logger.logger("db"),
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        &clock,
    );
    let db_emitter = Sink::composite([app_emitter.clone(), db_standalone]);

    // TOKEN_ISSUER sink - same composite pattern.
    let token_logger = SdkLoggerProvider::builder().with_simple_exporter(TOKEN_LOGS.clone()).build();
    let token_standalone = Sink::new(
        TOKEN_ISSUER,
        vec![Arc::new(SimpleLogProcessor {
            logger: token_logger.logger("token_issuer"),
            redaction_engine: data_privacy::RedactionEngine::default(),
        })],
        &clock,
    );
    let token_emitter = Sink::composite([app_emitter.clone(), token_standalone]);

    // --- 2. Add enrichments and handle requests ---

    // Global enrichment - attached to "app" but NOT to "db" / "token_issuer" (isolated).
    (|| {
        (|| {
            // Per-sink enrichment - only attached to the DB sink.
            (|| {
                // Per-sink enrichment - only attached to the token issuer sink.
                (|| {
                    // --- 3. Handle requests ---

                    println!();
                    println!("=== Handling request req-001 ===");
                    handle_request(&app_emitter, &db_emitter, &token_emitter, "req-001");

                    println!();
                    println!("=== Handling request req-002 ===");
                    handle_request(&app_emitter, &db_emitter, &token_emitter, "req-002");

                    println!();
                    println!("=== Handling bad request ===");
                    handle_bad_request(&app_emitter, &token_emitter);

                    // --- 4. Emit a service-level warning ---

                    println!();
                    println!("=== Emitting service degradation warning ===");
                    emit!(app_emitter, AppDegraded { error_code: 5003 });
                })
                .enrich_for(
                    &app_emitter,
                    TOKEN_ISSUER,
                    TokenIssuerVersionEnrich {
                        version: TokenIssuerVersion("2.1"),
                    },
                )();
            })
            .enrich_for(&app_emitter, DB, DbPoolEnrich { pool: DbPool("primary") })();
        })
        .enrich(
            &app_emitter,
            DeploymentRegionEnrich {
                deployment_region: DeploymentRegion("westus2"),
            },
        )();
    })
    .enrich(
        &app_emitter,
        ServiceNameEnrich {
            service_name: ServiceName("user-api"),
        },
    )();

    // --- 5. Tear down and inspect captured logs ---

    // Shutdown providers (flushes pending data).
    let _ = app_logger.shutdown();

    println!();
    println!("╔══════════════════════════════════════════╗");
    println!("║        Captured Log Records              ║");
    println!("╚══════════════════════════════════════════╝");

    println!("\n── APP sink (receives global enrichments) ──");
    print_logs(&app_logs);

    println!("\n── DB sink (library, isolated, per-sink enrichment only) ──");
    print_logs(&DB_LOGS);

    println!("\n── TOKEN_ISSUER sink (library, isolated, per-sink enrichment only) ──");
    print_logs(&TOKEN_LOGS);

    println!();
    println!("Done.");
}

/// Pretty-prints all captured log records.
fn print_logs(exporter: &InMemoryLogExporter) {
    let logs = exporter.get_emitted_logs().expect("should get logs");
    if logs.is_empty() {
        println!("  (no logs captured)");
        return;
    }
    for log in &logs {
        let name = log.record.event_name().unwrap_or("?");
        let severity = log.record.severity_text().unwrap_or("UNKNOWN");
        print!("  [{severity}] {name}");
        for (key, value) in log.record.attributes_iter() {
            print!("  {key}={value:?}");
        }
        println!();
    }
}
