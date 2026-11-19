// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the command-line interface.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn directory(name: &str) -> PathBuf {
    PathBuf::from(format!("target/seismograph-test-{}-{name}", std::process::id()))
}

#[test]
#[cfg_attr(miri, ignore = "terminal detection and subprocess behavior are exercised by native tests")]
fn monitor_rejects_redirected_output() {
    let result = Command::new(env!("CARGO_BIN_EXE_seismograph")).arg("monitor").output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8(result.stderr).unwrap().contains("interactive terminal"));
    assert!(result.stdout.is_empty());
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and subprocess behavior is exercised by native tests")]
fn view_requires_a_path_and_reports_file_errors() {
    let binary = env!("CARGO_BIN_EXE_seismograph");
    let missing_argument = Command::new(binary).arg("view").output().unwrap();
    assert_eq!(missing_argument.status.code(), Some(2));
    assert!(String::from_utf8(missing_argument.stderr).unwrap().contains("<SNAPSHOT-FILE>"));

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(directory("offline errors"));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("missing capture.seismograph");
    let result = Command::new(binary).arg("view").arg(&path).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8(result.stderr).unwrap().contains(&path.display().to_string()));

    fs::write(&path, b"invalid native snapshot").unwrap();
    let result = Command::new(binary).arg("view").arg(&path).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    fs::remove_dir_all(directory).unwrap();
}
