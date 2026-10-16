// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Observe stdout in fresh processes without sharing logging activation with other tests.

use std::process::Command;

use serial_test::serial;
use testing_aids::tracing_logs::write_to_stdout;

testing_aids::init_tracing!();

#[test]
#[serial]
#[cfg_attr(
    miri,
    ignore = "subprocess stdout routing is covered natively and by cargo careful; Miri retains formatter and buffer tests"
)]
fn stdout_is_silent_until_enabled_and_filters_levels() {
    const CHILD_ENV: &str = "TESTING_AIDS_STDOUT_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        emit_levels("before");
        write_to_stdout();
        emit_levels("after");
        return;
    }

    for mutation in ["0", "1"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "stdout_is_silent_until_enabled_and_filters_levels", "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("MUTATION_TESTING", mutation)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        for level in ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] {
            assert!(!stdout.contains(&format!("before-{level}")), "{stdout}");
            let expected = mutation == "0" && matches!(level, "ERROR" | "WARN" | "INFO");
            assert_eq!(stdout.contains(&format!("after-{level}")), expected, "{stdout}");
        }
    }
}

fn emit_levels(phase: &str) {
    tracing::error!("{phase}-ERROR");
    tracing::warn!("{phase}-WARN");
    tracing::info!("{phase}-INFO");
    tracing::debug!("{phase}-DEBUG");
    tracing::trace!("{phase}-TRACE");
}
