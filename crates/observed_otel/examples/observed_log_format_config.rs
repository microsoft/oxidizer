// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating `LogFormatConfig` options for controlling log output format.
//!
//! Prints every log twice: once in full (default) format, once in minimal format.
//!
//! Run with:
//! ```sh
//! cargo run -p observed_otel --example observed_log_format_config
//! ```

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use log::LevelFilter;
use observed::{emit, event};
use observed_destination_core::LogFormatConfig;
use observed_destination_stdout::stdout_logs;
use observed_otel::RedactionPolicy;
use observed_testing::MicrosoftEnterpriseDataTaxonomy;

const PUBLIC: MicrosoftEnterpriseDataTaxonomy = MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData;

#[event("http.request.completed")]
#[info("Request to {path} completed in {duration_ms}ms")]
struct HttpRequestCompleted {
    #[data_class(PUBLIC)]
    path: &'static str,
    #[data_class(PUBLIC)]
    duration_ms: f64,
    #[data_class(PUBLIC)]
    status: i64,
}

#[event("cache.eviction")]
#[warning("Cache eviction")]
struct CacheEviction {
    #[data_class(PUBLIC)]
    cache_name: &'static str,
    #[data_class(PUBLIC)]
    evicted_keys: i64,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let redaction = RedactionPolicy::new().class(PUBLIC, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough));

    // Two stdout destinations with different format configs so you can compare.
    let sink = observed_otel::builder()
        .redaction(redaction.clone())
        // Full format: library info, event name, raw message template + attributes
        .log(stdout_logs().level(LevelFilter::Trace).format_config(LogFormatConfig::default()))
        // Minimal format: just timestamp, severity, and message with placeholders expanded
        .log(stdout_logs().level(LevelFilter::Trace).format_config(LogFormatConfig::minimal()))
        .build()?;

    println!("─── Default format vs. Minimal format ───\n");

    emit!(
        sink,
        HttpRequestCompleted {
            path: "/api/users",
            duration_ms: 42.5,
            status: 200,
        }
    );

    emit!(
        sink,
        CacheEviction {
            cache_name: "sessions",
            evicted_keys: 37,
        }
    );

    sink.flush()?;

    println!("\n─── Custom format (event name, no library) ───\n");

    let custom = observed_otel::builder()
        .redaction(redaction)
        .log(stdout_logs().level(LevelFilter::Trace).format_config(LogFormatConfig {
            include_target: false,
            include_library: false,
            include_event_name: true,
            format_body: true,
        }))
        .build()?;

    emit!(
        custom,
        HttpRequestCompleted {
            path: "/api/health",
            duration_ms: 1.2,
            status: 200,
        }
    );

    custom.flush()?;

    Ok(())
}
