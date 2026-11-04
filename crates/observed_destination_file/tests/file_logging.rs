// Copyright (c) Microsoft Corporation.

//! Public-API coverage of the native writer boundary.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::fs;

use observed_destination_core::{LogFormatConfig, LoggerProviderBuilderExt};
use observed_destination_file::file_logs;
#[cfg(feature = "rolling_file")]
use observed_destination_file::{FileLimit, rolling_file_logs};
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use tempfile::{Builder, TempDir};

#[cfg(test)]
fn directory() -> TempDir {
    // Keep artifacts inside the workspace, including isolated mutation copies.
    fs::create_dir_all("target/file-logging-tests").unwrap();
    Builder::new().tempdir_in("target/file-logging-tests").unwrap()
}

#[cfg(test)]
fn emit(provider: &SdkLoggerProvider, body: String) {
    let logger = provider.logger("native-test");
    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Warn);
    record.set_body(AnyValue::String(body.into()));
    logger.emit(record);
}

#[test]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_files_append_or_overwrite_and_flush_at_lifecycle_boundaries() {
    for overwrite in [false, true] {
        let directory = directory();
        let path = directory.path().join("application.log");
        fs::write(&path, "existing contents\n").unwrap();
        let provider = SdkLoggerProvider::builder()
            .with_resource(Resource::builder_empty().build())
            .register(file_logs(&path).overwrite(overwrite).format_config(LogFormatConfig::minimal()))
            .unwrap()
            .build();
        provider.force_flush().unwrap();
        emit(&provider, "explicit flush record".into());
        provider.force_flush().unwrap();
        let output = fs::read_to_string(&path).unwrap();
        assert_eq!(output.starts_with("existing contents\n"), !overwrite);
        assert!(output.ends_with("WARN: explicit flush record\n"));
        emit(&provider, "shutdown record".into());
        provider.shutdown().unwrap();
        let output = fs::read_to_string(&path).unwrap();
        assert_eq!(output.matches("explicit flush record").count(), 1);
        assert!(output.ends_with("WARN: shutdown record\n"));
        directory.close().unwrap();
    }
}

#[test]
#[cfg(feature = "rolling_file")]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_rotation_retains_newest_archive_and_compresses_when_requested() {
    for compression in [false, true] {
        let directory = directory();
        let path = directory.path().join("application.log");
        let provider = SdkLoggerProvider::builder()
            .with_resource(Resource::builder_empty().build())
            .register(
                rolling_file_logs(&path)
                    .max_file_size(128)
                    .file_limit(FileLimit::MaxFiles(1))
                    .compression(compression)
                    .format_config(LogFormatConfig::minimal()),
            )
            .unwrap()
            .build();
        provider.force_flush().unwrap();
        // Oversized records exceed the capped byte buffer and force multiple rotations.
        for index in 0..3 {
            emit(&provider, format!("record {index}: {}", "x".repeat(256)));
            provider.force_flush().unwrap();
        }
        emit(&provider, "newest record".into());
        provider.shutdown().unwrap();
        let archives = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|entry| entry != &path)
            .collect::<Vec<_>>();
        assert_eq!(archives.len(), 1);
        let archive = fs::read(&archives[0]).unwrap();
        if compression {
            assert_eq!(archives[0].extension().unwrap(), "gz");
            assert!(archive.starts_with(&[0x1f, 0x8b]));
        } else {
            assert!(String::from_utf8(archive).unwrap().contains("WARN: record 2:"));
        }
        assert!(fs::read_to_string(&path).unwrap().contains("WARN: newest record\n"));
        directory.close().unwrap();
    }
}
