// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Keeps HTTP delivery compatible with the SDK's dedicated export threads.
//!
//! Explicit blocking-client selection prevents Cargo feature unification from
//! choosing an async client which is incompatible with current processors and readers.
//!
//! This is a temporary workaround until the SDK supports async export.

use std::time::Duration;
use std::{env, thread};

use observed_destination_core::DestinationResult;
use opentelemetry_otlp::{
    ExporterBuildError, OTEL_EXPORTER_OTLP_TIMEOUT, OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT, WithExportConfig, WithHttpConfig,
};
use reqwest::blocking::Client;

pub(crate) fn with_blocking_http_client<B: WithExportConfig + WithHttpConfig>(
    builder: B,
    timeout: Option<Duration>,
    dedicated_timeout_var: &str,
) -> DestinationResult<B> {
    let timeout = resolve_timeout(timeout, dedicated_timeout_var, OTEL_EXPORTER_OTLP_TIMEOUT, |name| {
        env::var(name).ok()
    });

    let client = thread::Builder::new()
        .spawn(move || Client::builder().timeout(timeout).build())?
        .join()
        .map_err(|_panic_payload| ExporterBuildError::InternalFailure("blocking HTTP client initialization thread panicked".into()))??;

    Ok(builder.with_timeout(timeout).with_http_client(client))
}

fn resolve_timeout(
    explicit_timeout: Option<Duration>,
    primary_env_var: &str,
    secondary_env_var: &str,
    read_env: impl Fn(&str) -> Option<String>,
) -> Duration {
    explicit_timeout
        .or_else(|| {
            // Match OTLP's fallback for missing or malformed environment values.
            [primary_env_var, secondary_env_var]
                .into_iter()
                .find_map(|name| read_env(name)?.parse().ok().map(Duration::from_millis))
        })
        .unwrap_or(OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use opentelemetry_otlp::{OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, OTEL_EXPORTER_OTLP_TIMEOUT};
    use rstest::rstest;

    use crate::http_client::resolve_timeout;

    #[rstest]
    #[case::explicit(Some(Duration::from_millis(123)), Some("456"), Some("789"), 123)]
    #[case::explicit_zero(Some(Duration::ZERO), Some("456"), Some("789"), 0)]
    #[case::signal(None, Some("456"), Some("789"), 456)]
    #[case::signal_zero(None, Some("0"), Some("789"), 0)]
    #[case::general(None, None, Some("789"), 789)]
    #[case::general_zero(None, None, Some("0"), 0)]
    #[case::default(None, None, None, 10_000)]
    #[case::invalid_signal(None, Some("invalid"), Some("789"), 789)]
    #[case::negative_signal(None, Some("-1"), Some("789"), 789)]
    #[case::overflow_signal(None, Some("18446744073709551616"), Some("789"), 789)]
    #[case::invalid_general(None, None, Some("invalid"), 10_000)]
    #[case::both_invalid(None, Some("invalid"), Some("invalid"), 10_000)]
    fn http_timeout_precedence(
        #[values(OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, OTEL_EXPORTER_OTLP_METRICS_TIMEOUT)] signal_timeout_var: &str,
        #[case] explicit: Option<Duration>,
        #[case] signal: Option<&str>,
        #[case] general: Option<&str>,
        #[case] expected_ms: u64,
    ) {
        let timeout = resolve_timeout(explicit, signal_timeout_var, OTEL_EXPORTER_OTLP_TIMEOUT, |name| {
            match name {
                OTEL_EXPORTER_OTLP_TIMEOUT => general,
                name if name == signal_timeout_var => signal,
                _ => panic!("unexpected timeout environment variable"),
            }
            .map(str::to_owned)
        });

        assert_eq!(timeout, Duration::from_millis(expected_ms));
    }
}
