// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the in-memory log destination.

use std::time::SystemTime;

use log::LevelFilter;
use observed_destination_core::LoggerProviderBuilderExt;
use observed_destination_memory::memory_logs;
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::{Key, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

#[test]
fn exported_logs_carry_the_provider_resource() {
    let exporter = InMemoryLogExporter::default();
    let resource = Resource::builder_empty()
        .with_attributes([KeyValue::new("service.name", "memory-test")])
        .build();
    let provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .register(memory_logs(exporter.clone()))
        .unwrap()
        .build();
    let logger = provider.logger("memory");
    let mut record = logger.create_log_record();
    // A fixed observation time lets this test run under Miri isolation.
    record.set_observed_timestamp(SystemTime::UNIX_EPOCH);

    logger.emit(record);
    provider.force_flush().unwrap();

    let logs = exporter.get_emitted_logs().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].resource.get(&Key::from_static_str("service.name")),
        Some("memory-test".into()),
    );
}

#[test]
fn records_below_the_level_are_not_captured() {
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder()
        .register(memory_logs(exporter.clone()).level(LevelFilter::Warn))
        .unwrap()
        .build();
    let logger = provider.logger("memory");
    let record = |severity| {
        let mut record = logger.create_log_record();
        record.set_severity_number(severity);
        // A fixed observation time lets this test run under Miri isolation.
        record.set_observed_timestamp(SystemTime::UNIX_EPOCH);
        record
    };

    // One record below the configured level and one at it.
    logger.emit(record(Severity::Info));
    logger.emit(record(Severity::Warn));

    // Only the record at the configured level reaches the buffer.
    let logs = exporter.get_emitted_logs().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].record.severity_number(), Some(Severity::Warn));
}

#[test]
fn sdk_provider_shutdown_reaches_exporter() {
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder()
        .register(memory_logs(exporter.clone()))
        .unwrap()
        .build();

    provider.shutdown().unwrap();
    assert!(exporter.is_shutdown_called());
}
