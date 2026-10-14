// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authenticated generated-worker rejection tests for the indexed probe protocol.

#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "build",
            "--quiet",
            "--jobs",
            "2",
            "--package",
            "metabench",
            "--example",
            "native_probe_contracts",
            "--message-format=json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|message| {
            (message["target"]["name"] == "native_probe_contracts")
                .then(|| message["executable"].as_str().map(PathBuf::from))
                .flatten()
        })
        .unwrap()
}

fn assert_rejected(executable: &Path, directory: &Path, arguments: &[String], expected: &str) {
    let token = tempfile::Builder::new().prefix("metabench-worker-").tempdir_in(directory).unwrap();
    let events = directory.join("events");
    let _ = std::fs::remove_file(&events);
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env("TMPDIR", directory)
        .env("TMP", directory)
        .env("TEMP", directory)
        .env("METABENCH_INTERNAL_WORKER_MODE", "allocations")
        .env("METABENCH_INTERNAL_WORKER_TOKEN", token.path())
        .env("METABENCH_TEST_LIFECYCLE", &events);
    for name in [
        "BENCH_ENGINE",
        "METABENCH_INTERNAL_PROBE_DISCOVERY",
        "METABENCH_INTERNAL_PERF_CONTROL",
        "METABENCH_INTERNAL_PERF_ACK",
        "METABENCH_INTERNAL_VTUNE_RESULT_DIR",
        "METABENCH_INTERNAL_ALLOCATION_IDENTITY",
        "METABENCH_INTERNAL_ALLOCATION_ARTIFACT",
        "CARGO_CRITERION_PORT",
    ] {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    assert!(!token.path().exists(), "worker must consume its authentication token");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        format!("metabench: native benchmark probe failed: {expected}\n"),
        "{arguments:?}"
    );
    assert!(!stderr.contains("panicked"), "{arguments:?}: {stderr}");
    let events = std::fs::read_to_string(events).unwrap_or_default();
    assert!(
        !events.contains("case setup") && !events.contains("case teardown"),
        "{arguments:?}: {events}"
    );
}

#[test]
fn authenticated_workers_reject_malformed_arguments_and_table_indices() {
    let executable = fixture();
    let directory = tempfile::Builder::new()
        .prefix("metabench-protocol-")
        .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")))
        .unwrap();
    let base = ["--metabench-probe", "0", "0", "0", "-"].map(String::from);
    let protocol_error = "expected private native probe worker arguments";
    for count in 0..5 {
        assert_rejected(&executable, directory.path(), &base[..count], protocol_error);
    }
    let mut extra = base.to_vec();
    extra.push("extra".into());
    assert_rejected(&executable, directory.path(), &extra, protocol_error);
    for verb in ["--unknown", "--metabench-probe-list", "metabench-probe"] {
        let mut arguments = base.clone();
        arguments[0] = verb.into();
        assert_rejected(&executable, directory.path(), &arguments, protocol_error);
    }
    for field in 1..5 {
        for invalid in ["not-a-number", "-1", "99999999999999999999999999999999999999999"] {
            let mut arguments = base.clone();
            arguments[field] = invalid.into();
            let expected = invalid.parse::<usize>().unwrap_err().to_string();
            assert_rejected(&executable, directory.path(), &arguments, &expected);
        }
    }
    for (field, expected) in [
        (1, "probe group index is out of bounds"),
        (2, "probe benchmark index is out of bounds"),
        (3, "probe case index is out of bounds"),
    ] {
        for index in ["1".to_owned(), usize::MAX.to_string()] {
            let mut arguments = base.clone();
            // The fixture has two native cases, unlike its single group and benchmark.
            arguments[field] = if field == 3 && index == "1" { "2".into() } else { index };
            assert_rejected(&executable, directory.path(), &arguments, expected);
        }
    }
}
