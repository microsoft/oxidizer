// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the command-line interface.
#![expect(clippy::unwrap_used, reason = "Test setup and assertions should fail immediately")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use seismograph_rallocator::snapshot::{Snapshot, Version};
use seismograph_rallocator::{encode, encoded_len};

fn encoded_snapshot() -> Vec<u8> {
    let snapshot = Snapshot::new(Version::new(0, 1, 0));
    let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
    encode(&snapshot, &mut bytes).unwrap();
    bytes
}

fn encoded_snapshot_with_unknown_section() -> Vec<u8> {
    let mut bytes = encoded_snapshot();
    bytes.extend_from_slice(&999_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes
}

fn directory(name: &str) -> PathBuf {
    PathBuf::from(format!("target/seismograph-test-{}-{name}", std::process::id()))
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and subprocess behavior is exercised by native tests")]
fn snapshot_html_writes_default_output_beside_input() {
    let directory = directory("default");
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("capture.rallocator");
    fs::write(&input, encoded_snapshot()).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_seismograph"))
        .args(["snapshot", "html"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(result.status.success());
    let output = directory.join("capture.html");
    assert!(fs::read_to_string(&output).unwrap().contains("<style>"));
    assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), output.display().to_string());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and subprocess behavior is exercised by native tests")]
fn snapshot_html_reads_native_seismograph_capture() {
    let directory = directory("native");
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("capture.seismograph");
    seismograph::recorder(seismograph::recorder::Configuration {
        arc_dereferences: seismograph::recorder::RecordingPolicy {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });
    seismograph::record(seismograph::recorder::event::EventClass::ArcDereference, || {
        seismograph::recorder::event::Record::object(
            seismograph::recorder::event::EventKind::ArcDeref,
            seismograph::recorder::event::ObjectId::new(42),
        )
    });
    seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
        .unwrap()
        .write_file(&input)
        .unwrap();
    seismograph::recorder(seismograph::recorder::Configuration::default());

    let result = Command::new(env!("CARGO_BIN_EXE_seismograph"))
        .args(["snapshot", "html"])
        .arg(&input)
        .output()
        .unwrap();

    assert!(result.status.success());
    let html = fs::read_to_string(directory.join("capture.html")).unwrap();
    assert!(html.contains("seismograph snapshot"));
    assert!(html.contains("Arc"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and subprocess errors are exercised by native tests")]
fn snapshot_html_reports_parse_and_io_errors() {
    let binary = env!("CARGO_BIN_EXE_seismograph");
    assert_eq!(Command::new(binary).arg("unknown").status().unwrap().code(), Some(2));
    assert!(Command::new(binary).arg("--help").status().unwrap().success());
    assert!(Command::new(binary).arg("--version").status().unwrap().success());

    let directory = directory("errors");
    fs::create_dir_all(&directory).unwrap();
    let missing = directory.join("missing.rallocator");
    let result = Command::new(binary).args(["snapshot", "html"]).arg(&missing).output().unwrap();
    assert_eq!(result.status.code(), Some(2));

    let invalid = directory.join("invalid.rallocator");
    fs::write(&invalid, b"invalid").unwrap();
    let result = Command::new(binary).args(["snapshot", "html"]).arg(&invalid).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .starts_with("seismograph: invalid snapshot:")
    );

    fs::write(&invalid, encoded_snapshot()).unwrap();
    let output = directory.join("missing-parent").join("report.html");
    let result = Command::new(binary)
        .args(["snapshot", "html"])
        .arg(&invalid)
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and subprocess replacement is exercised by native tests")]
fn snapshot_html_requires_force_to_replace_output() {
    let directory = directory("force");
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("capture.rallocator");
    let output = directory.join("capture.html");
    fs::write(&input, encoded_snapshot()).unwrap();
    fs::write(&output, "keep me").unwrap();

    let binary = env!("CARGO_BIN_EXE_seismograph");
    let result = Command::new(binary).args(["snapshot", "html"]).arg(&input).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("refusing to overwrite existing output")
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), "keep me");

    let result = Command::new(binary)
        .args(["snapshot", "html", "--force"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(fs::read_to_string(&output).unwrap().contains("<style>"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and subprocess rendering is exercised by native tests")]
fn snapshot_html_renders_skipped_section_warning() {
    let directory = directory("skipped");
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("capture.rallocator");
    fs::write(&input, encoded_snapshot_with_unknown_section()).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_seismograph"))
        .args(["snapshot", "html"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(result.status.success());
    let html = fs::read_to_string(directory.join("capture.html")).unwrap();
    assert!(html.contains("Compatibility warning"));
    assert!(html.contains("999 (version 1)"));
    assert!(html.contains("unknown identifiers or versions unsupported by this decoder"));
    assert!(html.contains("compatible seismograph version"));

    fs::remove_dir_all(directory).unwrap();
}
