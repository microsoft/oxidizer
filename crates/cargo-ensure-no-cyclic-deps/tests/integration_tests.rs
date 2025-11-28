// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::path::PathBuf;
use std::process::Command;

fn get_fixture_path(fixture_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(fixture_name)
}

#[test]
fn test_workspace_with_cycle() {
    let fixture_path = get_fixture_path("with_cycle");
    let manifest_path = fixture_path.join("Cargo.toml");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cargo-ensure-no-cyclic-deps"));
    cmd.arg("ensure-no-cyclic-deps").arg("--manifest-path").arg(manifest_path);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Error: Cyclic dependencies detected!"))
        .stderr(predicate::str::contains("Cycle 1:"))
        .stderr(
            predicate::str::contains("crate_a")
                .and(predicate::str::contains("crate_b"))
                .and(predicate::str::contains("crate_c")),
        );
}

#[test]
fn test_workspace_without_cycle() {
    let fixture_path = get_fixture_path("without_cycle");
    let manifest_path = fixture_path.join("Cargo.toml");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cargo-ensure-no-cyclic-deps"));
    cmd.arg("ensure-no-cyclic-deps").arg("--manifest-path").arg(manifest_path);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("No cyclic dependencies found."));
}

#[test]
fn test_workspace_with_dev_cycle() {
    let fixture_path = get_fixture_path("with_dev_cycle");
    let manifest_path = fixture_path.join("Cargo.toml");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cargo-ensure-no-cyclic-deps"));
    cmd.arg("ensure-no-cyclic-deps").arg("--manifest-path").arg(manifest_path);

    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Error: Cyclic dependencies detected!"))
        .stderr(predicate::str::contains("Cycle 1:"))
        .stderr(predicate::str::contains("lib_main").and(predicate::str::contains("lib_test_helpers")));
}

#[test]
fn test_command_without_manifest_path() {
    // This test runs in the current workspace (oxidizer-github)
    // which should not have cycles
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cargo-ensure-no-cyclic-deps"));
    cmd.arg("ensure-no-cyclic-deps");

    // We expect success since the oxidizer workspace shouldn't have cycles
    cmd.assert().success();
}
