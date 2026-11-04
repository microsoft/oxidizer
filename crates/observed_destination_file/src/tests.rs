// Copyright (c) Microsoft Corporation.

//! File logging across registration, processing, and worker lifetime using fake writers.

use std::time::Duration;

use observed_destination_core::{LogFormatConfig, LoggerProviderBuilderExt};
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;

use crate::file_logs;
use crate::file_writer::FileWriterOpener;
use crate::file_writer::fakes::FakeFileWriterOpener;

#[test]
#[cfg_attr(
    miri,
    ignore = "OpenTelemetry default resource detection calls current_exe, unsupported in Miri isolation"
)]
fn force_flush_writes_records_with_resource_configured_after_registration() {
    let factory = FakeFileWriterOpener::new("");

    let provider = SdkLoggerProvider::builder()
        .register(
            file_logs("application.log")
                .format_config(LogFormatConfig::minimal())
                .flush_ack_timeout(Duration::from_secs(12))
                .build()
                .writer_opener(FileWriterOpener::fake(factory.clone())),
        )
        .unwrap()
        .with_resource(Resource::builder_empty().with_service_name("example").build())
        .build();

    // Acknowledge resource delivery before emission; channel selection has no priority.
    provider.force_flush().unwrap();
    let logger = provider.logger("test");
    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Warn);
    record.set_body(AnyValue::String("request".into()));

    logger.emit(record);
    provider.force_flush().unwrap();

    let output = factory.output();
    assert!(output.contains("WARN: request\n"));
    assert!(output.contains("service.name=example"));
    assert_eq!(output.matches("<Resource:").count(), 1);

    provider.shutdown().unwrap();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OpenTelemetry default resource detection calls current_exe, unsupported in Miri isolation"
)]
fn shutdown_flushes_pending_records() {
    let factory = FakeFileWriterOpener::new("");

    let provider = SdkLoggerProvider::builder()
        .register(
            file_logs("application.log")
                .format_config(LogFormatConfig::minimal())
                .build()
                .writer_opener(FileWriterOpener::fake(factory.clone())),
        )
        .unwrap()
        .build();
    let logger = provider.logger("test");
    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Error);
    record.set_body(AnyValue::String("last record".into()));

    logger.emit(record);
    // Deliberately omit force_flush: shutdown must drain and join the writer.
    provider.shutdown().unwrap();

    assert!(factory.output().ends_with("ERROR: last record\n"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OpenTelemetry default resource detection calls current_exe, unsupported in Miri isolation"
)]
fn custom_buffer_sizes_preserve_records_across_flush_and_shutdown() {
    let body = "x".repeat(1024);
    // Cover bypass, a record larger than capacity, and buffers larger than both records.
    for buffer_size in [0, 64, 256 * 1024, 1024 * 1024] {
        let factory = FakeFileWriterOpener::new("");
        let provider = SdkLoggerProvider::builder()
            .register(
                file_logs("application.log")
                    .buffer_size(buffer_size)
                    .format_config(LogFormatConfig::minimal())
                    .build()
                    .writer_opener(FileWriterOpener::fake(factory.clone())),
            )
            .unwrap()
            .build();
        let logger = provider.logger("test");
        for severity in [Severity::Warn, Severity::Error] {
            let mut record = logger.create_log_record();
            record.set_severity_number(severity);
            record.set_body(AnyValue::String(body.clone().into()));
            logger.emit(record);
            if severity == Severity::Warn {
                provider.force_flush().unwrap();
                assert!(factory.output().contains(&body));
            }
        }
        provider.shutdown().unwrap();

        let output = factory.output();
        assert_eq!(output.matches(&body).count(), 2);
        assert_eq!(output.matches("<Resource:").count(), 1);
        assert!(output.contains(&format!("WARN: {body}\n")));
        assert!(output.ends_with(&format!("ERROR: {body}\n")));
    }
}
