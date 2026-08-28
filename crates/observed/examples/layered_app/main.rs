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

#[path = "../support/taxonomy.rs"]
mod taxonomy;

#[path = "../support/redaction.rs"]
mod redaction;

#[path = "../support/otel.rs"]
mod otel;

use std::sync::{Arc, LazyLock};

use data_privacy::classified;
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Sink, SinkId, emit, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLogger, SdkLoggerProvider};

use crate::taxonomy::MicrosoftEnterpriseDataTaxonomy;

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
#[event("http.server.request")]
#[info("HTTP request handled")]
#[histogram(duration_ms, name = "http.server.request.duration")]
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
#[event("app.degraded")]
#[warning("Service running in degraded mode")]
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
            redaction_engine: redaction::passthrough_redaction_engine(),
        })],
        &clock,
    );

    // DB sink - standalone, *isolated* sink with its own destination. A composite
    // fans one `emit!` through both the app's processor and the DB log. Each
    // sink has its own enrichment slot, so DB-log records do not inherit
    // enrichments pushed on the app sink; being isolated, the DB leaf also
    // ignores untargeted (global) entries broadcast through the composite and
    // only picks up entries addressed to `DB`.
    let db_logger = SdkLoggerProvider::builder().with_simple_exporter(DB_LOGS.clone()).build();
    let db_standalone = Sink::new_isolated(
        DB,
        vec![Arc::new(SimpleLogProcessor {
            logger: db_logger.logger("db"),
            redaction_engine: redaction::passthrough_redaction_engine(),
        })],
        &clock,
    );
    let db_emitter = Sink::composite([app_emitter.clone(), db_standalone]);

    // TOKEN_ISSUER sink - same composite pattern.
    let token_logger = SdkLoggerProvider::builder().with_simple_exporter(TOKEN_LOGS.clone()).build();
    let token_standalone = Sink::new_isolated(
        TOKEN_ISSUER,
        vec![Arc::new(SimpleLogProcessor {
            logger: token_logger.logger("token_issuer"),
            redaction_engine: redaction::passthrough_redaction_engine(),
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
                    // Broadcast through the token composite, not the app sink:
                    // the entry has to land on the token leaf's own slot to be
                    // visible there. The app leaf receives a copy too and
                    // filters it out, because the entry targets TOKEN_ISSUER.
                    &token_emitter,
                    TOKEN_ISSUER,
                    TokenIssuerVersionEnrich {
                        version: TokenIssuerVersion("2.1"),
                    },
                )();
            })
            .enrich_for(
                // Same reasoning: broadcast through the DB composite so the DB
                // leaf's slot actually carries the entry.
                &db_emitter,
                DB,
                DbPoolEnrich { pool: DbPool("primary") },
            )();
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

    // --- 5. Inspect captured logs, then tear down ---

    // Flush pending data, but do *not* shut the providers down yet: shutting
    // down an `InMemoryLogExporter` clears the very records we are about to
    // read back.
    let _ = app_logger.force_flush();
    let _ = db_logger.force_flush();
    let _ = token_logger.force_flush();

    report(&app_logs);

    // Records have been read, so the providers can go now.
    let _ = app_logger.shutdown();
    let _ = db_logger.shutdown();
    let _ = token_logger.shutdown();

    println!();
    println!("Done.");
}

/// Prints what each sink captured, demonstrating how enrichment was routed.
fn report(app_logs: &InMemoryLogExporter) {
    println!();
    println!("╔══════════════════════════════════════════╗");
    println!("║        Captured Log Records              ║");
    println!("╚══════════════════════════════════════════╝");

    println!("\n── APP sink (receives global enrichments) ──");
    print_logs(app_logs);

    println!("\n── DB sink (library, isolated, per-sink enrichment only) ──");
    print_logs(&DB_LOGS);

    println!("\n── TOKEN_ISSUER sink (library, isolated, per-sink enrichment only) ──");
    print_logs(&TOKEN_LOGS);
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
