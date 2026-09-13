// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-level coverage for generated native benchmark entry points.

#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};

static CARGO_SUBPROCESS: Mutex<()> = Mutex::new(());

fn cargo_target() -> &'static Path {
    static TARGET: OnceLock<PathBuf> = OnceLock::new();

    TARGET
        .get_or_init(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/metabench-spawned-benchmark"))
        .as_path()
}

fn cargo() -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    for name in [
        "BENCH_ENGINE",
        "METABENCH_INTERNAL_WORKER_MODE",
        "METABENCH_INTERNAL_WORKER_TOKEN",
        "METABENCH_INTERNAL_PERF_CONTROL",
        "METABENCH_INTERNAL_PERF_ACK",
        "METABENCH_INTERNAL_VTUNE_RESULT_DIR",
        "CRITERION_HOME",
        "CARGO_CRITERION_PORT",
        "GUNGRAUN_HOME",
        "GUNGRAUN_SAVE_SUMMARY",
        // When the outer test binary itself runs under `cargo careful`, these
        // variables carry a `--sysroot` pointing at careful's custom std
        // (built without embedded LTO bitcode). The nested `cargo run`
        // spawned here builds the `bench` profile, which enables `lto =
        // "fat"`; linking that profile against the careful sysroot fails
        // with "failed to get bitcode from object file for LTO" because that
        // std was never built with bitcode embedding. The nested build is
        // unrelated to the outer test's own careful checks, so strip these
        // to make it use the ambient toolchain's ordinary std instead.
        "RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
    ] {
        command.env_remove(name);
    }
    command
        .env("CARGO_TARGET_DIR", cargo_target())
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1");
    command
}

fn run(arguments: &[&str]) -> Output {
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    cargo()
        .args([
            "run",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            "basic",
            "--",
        ])
        .args(arguments)
        .output()
        .unwrap()
}

fn run_target(target: &str, arguments: &[&std::ffi::OsStr]) -> Output {
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut command = cargo();
    command.args(["run", "--quiet", "-p", "metabench", "--profile", "bench", "--example", target, "--"]);
    command.args(arguments);
    command.output().unwrap()
}

fn successful_stdout(arguments: &[&str]) -> String {
    let output = run(arguments);
    assert!(
        output.status.success(),
        "arguments: {arguments:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn gungraun_runner_available() -> bool {
    let runner = std::env::var_os("GUNGRAUN_RUNNER").unwrap_or_else(|| "gungraun-runner".into());
    Command::new(runner)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(target_os = "linux")]
#[test]
fn perf_measures_exact_workload_and_writes_metrics() {
    use std::os::unix::fs::PermissionsExt as _;

    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let perf = directory.path().join("perf");
    std::fs::write(
        &perf,
        r#"#!/bin/sh
output=
control=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) output=$2; shift 2 ;;
        --control) control=${2#fifo:}; shift 2 ;;
        --) shift; break ;;
        *) shift ;;
    esac
done
control_fifo=${control%%,*}
ack_fifo=${control#*,}
"$@" &
child=$!
IFS= read -r command < "$control_fifo"
[ "$command" = enable ] || exit 90
printf 'ack\n' > "$ack_fifo"
IFS= read -r command < "$control_fifo"
[ "$command" = disable ] || exit 91
printf 'ack\n' > "$ack_fifo"
wait "$child"
status=$?
printf '%s\n' \
    '{"counter-value":"1234","event":"instructions:u"}' \
    '{"counter-value":"567","event":"cycles:u"}' \
    '{"counter-value":"8","event":"branches:u"}' \
    '{"counter-value":"2","event":"branch-misses:u"}' \
    '{"counter-value":"6","event":"cache-references:u"}' \
    '{"counter-value":"1","event":"cache-misses:u"}' > "$output"
exit "$status"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&perf, std::fs::Permissions::from_mode(0o755)).unwrap();
    let json = directory.path().join("report.json");
    let markdown = directory.path().join("report.md");
    let path = std::env::join_paths(
        std::iter::once(directory.path().to_owned()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    let output = cargo()
        .env("PATH", path)
        .args([
            "run",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            "basic",
            "--",
            "--perf",
            "--show-engine-output",
            "--no-baseline",
            "--export-json",
        ])
        .arg(&json)
        .arg("--export-md")
        .arg(&markdown)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(json).unwrap()).unwrap();
    let perf = &report["entries"][0]["results"]["perf"]["metrics"];
    assert_eq!(perf["instructions"]["value"], 1234.0);
    assert_eq!(perf["instructions"]["display_name"], "HW Instr");
    assert_eq!(perf["cache-misses"]["value"], 1.0);
}

/// Builds the `examples/fake_vtune.rs` fixture once per test binary run and
/// returns the directory containing it under the literal name `vtune`
/// (`vtune.exe` on Windows) so tests can prepend that directory to `PATH`
/// and have `Command::new("vtune")` resolve to it. Written in Rust rather
/// than a POSIX shell script so the vtune control-protocol and CSV-report
/// tests below run identically on Linux, macOS, and Windows; see
/// `examples/fake_vtune.rs` for the exact protocol it understands.
fn fake_vtune_directory() -> &'static Path {
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

    DIRECTORY.get_or_init(|| {
        let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let build_output = cargo()
            .args([
                "build",
                "--quiet",
                "-p",
                "metabench",
                "--profile",
                "bench",
                "--example",
                "fake_vtune",
                "--message-format=json",
            ])
            .output()
            .unwrap();
        assert!(
            build_output.status.success(),
            "failed to build the fake vtune fixture; stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );
        // Locate the built executable through cargo's JSON build log rather
        // than assuming a directory layout for the `bench` profile (custom
        // profiles may or may not share the built-in `release` directory).
        let built = String::from_utf8_lossy(&build_output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|message| {
                (message["reason"] == "compiler-artifact" && message["target"]["name"] == "fake_vtune")
                    .then(|| message["executable"].as_str().map(PathBuf::from))
                    .flatten()
            })
            .expect("cargo build --message-format=json always reports the built example's executable path");
        let directory = tempfile::tempdir().unwrap().keep();
        let vtune_path = directory.join(if cfg!(windows) { "vtune.exe" } else { "vtune" });
        std::fs::copy(&built, &vtune_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&vtune_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        directory
    })
}

/// Prepends [`fake_vtune_directory`] to the current `PATH` so a spawned
/// `metabench` worker resolves `vtune` to the fixture.
fn path_with_fake_vtune() -> std::ffi::OsString {
    std::env::join_paths(
        std::iter::once(fake_vtune_directory().to_owned()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap()
}

#[test]
fn vtune_measures_exact_workload_and_writes_metrics() {
    let path = path_with_fake_vtune();
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let command_log = directory.path().join("vtune-commands.log");
    let json = directory.path().join("report.json");
    let markdown = directory.path().join("report.md");
    let output = cargo()
        .env("PATH", path)
        .env("VTUNE_TEST_COMMAND_LOG", &command_log)
        .args([
            "run",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            "basic",
            "--",
            "--vtune",
            "--show-engine-output",
            "--no-baseline",
            "--export-json",
        ])
        .arg(&json)
        .arg("--export-md")
        .arg(&markdown)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The fixture rejects any control verb other than resume/pause, so
    // finding both confirms `begin`/`Guard::drop` actually invoked the real
    // control protocol rather than skipping it.
    let commands = std::fs::read_to_string(&command_log).unwrap();
    assert!(
        commands
            .lines()
            .any(|line| line.starts_with("resume ") && line.len() > "resume ".len()),
        "commands: {commands}"
    );
    assert!(
        commands
            .lines()
            .any(|line| line.starts_with("pause ") && line.len() > "pause ".len()),
        "commands: {commands}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("vtune-report-invoked"),
        "expected --show-engine-output to surface the vtune report command's own stderr; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("vtune-worker-invoked"),
        "expected --show-engine-output to surface the vtune worker command's own stderr; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(json).unwrap()).unwrap();
    let vtune = &report["entries"][0]["results"]["vtune"]["metrics"];
    assert_eq!(vtune["INST_RETIRED.ANY"]["value"], 1234.0);
    assert_eq!(vtune["INST_RETIRED.ANY"]["display_name"], "INST RETIRED ANY");
    assert_eq!(vtune["CPU_CLK_UNHALTED.THREAD"]["value"], 567.0);
}

#[test]
fn vtune_suppresses_report_output_without_show_engine_output() {
    let path = path_with_fake_vtune();
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    // Deliberately omits `--show-engine-output`: both vtune commands' own
    // stdout/stderr must be suppressed, distinguishing each `!show_output`
    // guard in `launch_vtune_worker` from its negation.
    let output = cargo()
        .env("PATH", path)
        .args([
            "run",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            "basic",
            "--",
            "--vtune",
            "--no-baseline",
            "--export-json",
        ])
        .arg(&json)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("vtune-report-invoked"),
        "expected the vtune report command's stderr to be suppressed without --show-engine-output; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("vtune-worker-invoked"),
        "expected the vtune worker command's stderr to be suppressed without --show-engine-output; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn vtune_command_failure_surfaces_vtune_control_error() {
    let path = path_with_fake_vtune();
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Fails the `-command resume` call `vtune::begin` makes from inside the
    // worker, so the worker should surface `Error::VtuneControl` instead of
    // silently measuring nothing.
    let output = cargo()
        .env("PATH", path)
        .env("FAKE_VTUNE_FAIL_COMMAND", "resume")
        .args([
            "run",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            "basic",
            "--",
            "--vtune",
            "--show-engine-output",
            "--no-baseline",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "expected the failed vtune resume to fail the run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to control the VTune collection"), "stderr: {stderr}");
}

#[test]
fn help_reaches_metabench_parent() {
    let stdout = successful_stdout(&["--help"]);

    assert!(stdout.starts_with("Usage: BENCHMARK [METABENCH OPTIONS] [ENGINE OPTIONS]\n"));
    assert!(stdout.contains("BENCH_ENGINE may select criterion, gungraun, perf, vtune, or allocations"));
}

#[test]
fn criterion_native_listing_is_forwarded() {
    let stdout = successful_stdout(&["--criterion", "--list"]);

    assert!(stdout.lines().any(|line| line.starts_with("basic/checksum/rolling:")));
    assert!(!stdout.contains("Running criterion benchmarks"));
}

#[test]
fn gungraun_listing_is_forwarded() {
    if !gungraun_runner_available() {
        eprintln!("skipping Gungraun listing because gungraun-runner is unavailable");
        return;
    }

    let stdout = successful_stdout(&["--gungraun", "--list"]);

    assert!(stdout.lines().any(|line| {
        line.contains("__metabench_group_d23a5d29e30556e2f3f66fae89e4544b::__metabench_benchmark_ecb5d0c1194ff78d70d02d708fc2a852")
    }));
    assert!(!stdout.contains("Running gungraun benchmarks"));
}

#[test]
fn criterion_and_allocations_write_parameterized_report() {
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    let markdown = directory.path().join("report.md");
    let arguments = [
        std::ffi::OsStr::new("--criterion"),
        std::ffi::OsStr::new("--allocations"),
        std::ffi::OsStr::new("--no-baseline"),
        std::ffi::OsStr::new("--export-json"),
        json.as_os_str(),
        std::ffi::OsStr::new("--export-md"),
        markdown.as_os_str(),
        std::ffi::OsStr::new("--criterion-arg=--quick"),
    ];

    let output = run_target("parameterized", &arguments);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    let identities = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["identity"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            "parameterized/sort/unstable/size_1024",
            "parameterized/sort/unstable/size_128",
            "parameterized/sort/unstable/size_8192",
        ]
    );
    assert!(markdown.is_file());
    assert!(
        report["metadata"]["rustc_version"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
    );
    for entry in report["entries"].as_array().unwrap() {
        assert!(entry["results"]["alloc_tracker"]["metrics"]["Allocated bytes"]["value"].is_u64());
        assert!(entry["results"]["alloc_tracker"]["metrics"]["Allocations"]["value"].is_u64());
    }
}
