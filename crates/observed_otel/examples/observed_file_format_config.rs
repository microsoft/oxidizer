// Copyright (c) Microsoft Corporation.

#![allow(
    clippy::unwrap_used,
    reason = "Non-production code uses unwrap to fail immediately when a test or demonstration cannot proceed"
)]

//! Example demonstrating `LogFormatConfig` with file destinations.
//!
//! Writes to three files with different format configurations so you can
//! compare the output side by side.
//!
//! Run with:
//! ```sh
//! cargo run -p observed_otel --example file_format_config
//! ```

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use log::LevelFilter;
use microsoft_enterprise_data_taxonomy::MicrosoftEnterpriseDataTaxonomy;
use observed::{emit, event};
use observed_destination_core::LogFormatConfig;
use observed_destination_file::file_logs;
use observed_otel::RedactionPolicy;

const PUBLIC: MicrosoftEnterpriseDataTaxonomy =
    MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData;

#[event("db.query.completed")]
#[info("Query on table {table} took {query_time_ms}ms")]
struct DbQueryCompleted {
    #[data_class(PUBLIC)]
    table: &'static str,
    #[data_class(PUBLIC)]
    query_time_ms: i64,
    #[data_class(PUBLIC)]
    rows_returned: i64,
}

#[event("connection.failed")]
#[error("Connection to {host} failed after {retry_count} retries")]
struct ConnectionFailed {
    #[data_class(PUBLIC)]
    host: &'static str,
    #[data_class(PUBLIC)]
    retry_count: i64,
    #[data_class(PUBLIC)]
    error_code: &'static str,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let temp_dir = tempfile::tempdir()?;
    let dir = temp_dir.path();
    println!("Writing log files to: {}", dir.display());

    let full_log = dir.join("full.log");
    let minimal_log = dir.join("minimal.log");
    let custom_log = dir.join("custom.log");

    let redaction = RedactionPolicy::new().class(
        MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData,
        SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
    );

    let sink = observed_otel::builder()
        .redaction(redaction)
        // Default format (includes all fields: target, library, event name)
        .log(
            file_logs(&full_log)
                .level(LevelFilter::Trace)
                .overwrite(true)
                .format_config(LogFormatConfig::default()),
        )
        // Minimal format (only timestamp, severity, body with placeholders expanded)
        .log(
            file_logs(&minimal_log)
                .level(LevelFilter::Trace)
                .overwrite(true)
                .format_config(LogFormatConfig::minimal()),
        )
        // Custom format (selective fields)
        .log(
            file_logs(&custom_log)
                .level(LevelFilter::Trace)
                .overwrite(true)
                .format_config(LogFormatConfig {
                    include_target: true,
                    include_library: false,
                    include_event_name: true,
                    format_body: false,
                }),
        )
        .build()?;

    emit!(
        sink,
        DbQueryCompleted {
            table: "users",
            query_time_ms: 42,
            rows_returned: 15,
        }
    );

    emit!(
        sink,
        ConnectionFailed {
            host: "db-replica-3.internal",
            retry_count: 3,
            error_code: "ERR_TIMEOUT",
        }
    );

    sink.flush()?;

    // Print file contents for comparison.
    for (label, path) in [
        ("FULL", &full_log),
        ("MINIMAL", &minimal_log),
        ("CUSTOM", &custom_log),
    ] {
        println!(
            "\n─── {label} ({}) ───",
            path.file_name().unwrap().display()
        );
        print!("{}", std::fs::read_to_string(path)?);
    }

    Ok(())
}
