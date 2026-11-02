// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public crate contracts.

#![cfg(test)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use log::LevelFilter;
use observed_destination_core::{LoggerProviderBuilderExt, MeterProviderBuilderExt};
use observed_destination_stdout::{stdout_logs, stdout_metrics};
use opentelemetry::logs::{Logger, LoggerProvider, Severity};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;

#[test]
fn stdout_destination_registers() {
    let provider = SdkLoggerProvider::builder()
        .register(stdout_logs().level(LevelFilter::Warn))
        .unwrap()
        .build();

    let logger = provider.logger("stdout");

    assert!(!logger.event_enabled(Severity::Info, "stdout", None));
    assert!(logger.event_enabled(Severity::Warn, "stdout", None))
}

#[test]
fn stdout_metric_destination_registers_and_flushes() {
    let provider = SdkMeterProvider::builder().register(stdout_metrics()).unwrap().build();

    provider.force_flush().unwrap();
    provider.shutdown().unwrap();
}
