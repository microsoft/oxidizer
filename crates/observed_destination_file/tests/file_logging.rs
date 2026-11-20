// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public-API coverage of the native writer boundary.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::fs;
use std::io::{Error, ErrorKind};
#[cfg(feature = "rolling_file")]
use std::path::{Path, PathBuf};
#[cfg(feature = "rolling_file")]
use std::time::Duration;
use std::time::SystemTime;

use observed_destination_core::{IntoLogDestination, LogFormatConfig, LoggerProviderBuilderExt};
#[cfg(feature = "rolling_file")]
use observed_destination_file::{FileLimit, rolling_file_logs};
use observed_destination_file::{FileLogInitError, file_logs};
use ohno::ErrorExt as _;
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use tempfile::{Builder, TempDir};

/// The header for the empty resource that every test provider uses.
const HEADER: &str = "<Resource: >\n";

#[cfg(test)]
fn directory() -> TempDir {
    // Keep artifacts inside the workspace, including isolated mutation copies.
    fs::create_dir_all("target/file-logging-tests").unwrap();
    Builder::new().tempdir_in("target/file-logging-tests").unwrap()
}

/// Registers `destination` and applies the empty resource before any record.
#[cfg(test)]
fn provider_for(destination: impl IntoLogDestination) -> SdkLoggerProvider {
    let provider = SdkLoggerProvider::builder()
        .with_resource(Resource::builder_empty().build())
        .register(destination)
        .unwrap()
        .build();
    provider.force_flush().unwrap();
    provider
}

#[cfg(test)]
fn emit(provider: &SdkLoggerProvider, body: String) {
    let logger = provider.logger("native-test");
    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Warn);
    // A fixed timestamp makes each line's bytes predictable.
    record.set_timestamp(SystemTime::UNIX_EPOCH);
    record.set_body(AnyValue::String(body.into()));
    logger.emit(record);
}

/// The minimal-format line that `emit` produces for `body`.
#[cfg(test)]
fn line(body: &str) -> String {
    format!("[1970-01-01T00:00:00.000000000Z] WARN: {body}\n")
}

#[cfg(test)]
#[cfg(feature = "rolling_file")]
fn archive_paths(directory: &TempDir, path: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| entry != path)
        .collect()
}

#[test]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_files_append_or_overwrite_and_flush_at_lifecycle_boundaries() {
    for overwrite in [false, true] {
        let directory = directory();
        let path = directory.path().join("application.log");
        fs::write(&path, "existing contents\n").unwrap();
        let provider = provider_for(file_logs(&path).overwrite(overwrite).format_config(LogFormatConfig::minimal()));
        emit(&provider, "explicit flush record".into());
        provider.force_flush().unwrap();
        // Appended output starts directly after the existing contents.
        let mut expected = if overwrite {
            String::new()
        } else {
            "existing contents\n".to_owned()
        };
        expected.push_str(HEADER);
        expected.push_str(&line("explicit flush record"));
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        emit(&provider, "shutdown record".into());
        provider.shutdown().unwrap();
        expected.push_str(&line("shutdown record"));
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        directory.close().unwrap();
    }
}

#[test]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_registration_creates_a_missing_file() {
    let directory = directory();
    let path = directory.path().join("application.log");

    let provider = provider_for(file_logs(&path).format_config(LogFormatConfig::minimal()));
    emit(&provider, "first record".into());
    provider.shutdown().unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{HEADER}{}", line("first record")));
    directory.close().unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_registration_reports_file_open_errors() {
    let directory = directory();
    // Registration creates a missing file, but not its missing parent directory.
    let path = directory.path().join("missing").join("application.log");

    let error = SdkLoggerProvider::builder().register(file_logs(&path)).unwrap_err();

    let init_error = error.downcast_ref::<FileLogInitError>().unwrap();
    assert_eq!(init_error.find_source::<Error>().unwrap().kind(), ErrorKind::NotFound);
    directory.close().unwrap();
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
        let archives = archive_paths(&directory, &path);
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

#[test]
#[cfg(feature = "rolling_file")]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_rotation_starts_only_after_the_file_exceeds_its_size_limit() {
    let first_file = format!("{HEADER}{}", line("first record"));
    // A file at the limit keeps the next write; one byte over the limit rotates it.
    for (max_file_size, rotated) in [(first_file.len(), false), (first_file.len() - 1, true)] {
        let directory = directory();
        let path = directory.path().join("application.log");
        let provider = provider_for(
            rolling_file_logs(&path)
                .max_file_size(max_file_size)
                .format_config(LogFormatConfig::minimal()),
        );
        emit(&provider, "first record".into());
        provider.force_flush().unwrap();

        // The rotating writer checks the limit when the next write arrives.
        emit(&provider, "next record".into());
        provider.shutdown().unwrap();

        let archives = archive_paths(&directory, &path);
        if rotated {
            assert_eq!(archives.len(), 1);
            assert_eq!(fs::read_to_string(&archives[0]).unwrap(), first_file);
            assert_eq!(fs::read_to_string(&path).unwrap(), line("next record"));
        } else {
            assert!(archives.is_empty());
            assert_eq!(fs::read_to_string(&path).unwrap(), format!("{first_file}{}", line("next record")));
        }
        directory.close().unwrap();
    }
}

#[test]
#[cfg(feature = "rolling_file")]
#[cfg_attr(miri, ignore = "requires native filesystem access")]
fn native_rotation_applies_within_one_flush_of_many_records() {
    let directory = directory();
    let path = directory.path().join("application.log");
    let provider = provider_for(
        rolling_file_logs(&path)
            .max_file_size(128)
            // Only a full byte buffer or the explicit flush writes output.
            .flush_interval(Duration::from_hours(1))
            .format_config(LogFormatConfig::minimal()),
    );

    // These records total about three times the size limit.
    for index in 0..8 {
        emit(&provider, format!("record {index}"));
    }
    provider.force_flush().unwrap();

    // The byte buffer is capped at the size limit, so these records reach the
    // rotating writer in several writes and the file rotates between them.
    assert!(!archive_paths(&directory, &path).is_empty());
    provider.shutdown().unwrap();
    directory.close().unwrap();
}
