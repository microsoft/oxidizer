use std::fs;
use std::path::PathBuf;
use std::process::Command;

use rallocator_telemetry::snapshot::{Snapshot, Version};
use rallocator_telemetry::{encode, encoded_len};

fn encoded_snapshot() -> Vec<u8> {
    let snapshot = Snapshot::new(Version::new(0, 1, 0));
    let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
    encode(&snapshot, &mut bytes).unwrap();
    bytes
}

fn directory(name: &str) -> PathBuf {
    PathBuf::from(format!("target/rallocator_cli-test-{}-{name}", std::process::id()))
}

#[test]
fn snapshot_html_writes_default_output_beside_input() {
    let directory = directory("default");
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("capture.rallocator");
    fs::write(&input, encoded_snapshot()).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_rallocator_cli"))
        .args(["snapshot", "html"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(result.status.success());
    let output = directory.join("snapshot.html");
    assert!(fs::read_to_string(&output).unwrap().contains("<style>"));
    assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), output.display().to_string());

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn snapshot_html_reports_parse_and_io_errors() {
    let binary = env!("CARGO_BIN_EXE_rallocator_cli");
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
            .starts_with("rallocator: invalid snapshot:")
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
