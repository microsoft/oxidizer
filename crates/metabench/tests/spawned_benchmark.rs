// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-level coverage for generated native benchmark entry points.

#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

#[cfg(target_os = "linux")]
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::panic;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};

static CARGO_SUBPROCESS: Mutex<()> = Mutex::new(());

// The lock serializes Cargo, not mutable state; release it before resuming a test failure.
fn with_cargo_lock<T>(body: impl FnOnce() -> T) -> T {
    let guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let result = panic::catch_unwind(panic::AssertUnwindSafe(body));
    drop(guard);
    match result {
        Ok(value) => value,
        Err(payload) => panic::resume_unwind(payload),
    }
}

#[test]
fn serialized_failure_preserves_the_payload_without_new_lock_poison() {
    let failure = panic::catch_unwind(|| {
        with_cargo_lock(|| panic::panic_any(("serialized failure", CARGO_SUBPROCESS.is_poisoned())));
    })
    .unwrap_err();
    let (message, previously_poisoned) = *failure.downcast::<(&'static str, bool)>().unwrap();
    assert_eq!(message, "serialized failure");
    with_cargo_lock(|| assert_eq!(CARGO_SUBPROCESS.is_poisoned(), previously_poisoned));
}

fn cargo_target() -> &'static Path {
    static TARGET: OnceLock<PathBuf> = OnceLock::new();

    TARGET
        .get_or_init(|| {
            std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"), PathBuf::from)
        })
        .as_path()
}

fn cargo() -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    for name in [
        "BENCH_ENGINE",
        "METABENCH_INTERNAL_WORKER_MODE",
        "METABENCH_INTERNAL_WORKER_TOKEN",
        "METABENCH_INTERNAL_PROBE_DISCOVERY",
        "METABENCH_INTERNAL_PERF_CONTROL",
        "METABENCH_INTERNAL_PERF_ACK",
        "METABENCH_INTERNAL_VTUNE_RESULT_DIR",
        "METABENCH_INTERNAL_ALLOCATION_ARTIFACT",
        "METABENCH_INTERNAL_ALLOCATION_IDENTITY",
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
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1");
    command
}

fn run(arguments: &[&str]) -> Output {
    with_cargo_lock(|| {
        cargo()
            .args([
                "run",
                "--quiet",
                "-p",
                "metabench",
                "--profile",
                "bench",
                "--example",
                "metabench_basic",
                "--",
            ])
            .args(arguments)
            .output()
            .unwrap()
    })
}

fn run_target(target: &str, arguments: &[&std::ffi::OsStr]) -> Output {
    with_cargo_lock(|| run_target_unlocked(target, arguments))
}

fn run_target_unlocked(target: &str, arguments: &[&std::ffi::OsStr]) -> Output {
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
    let version = Command::new(runner).arg("--version").output();
    let available = version.as_ref().is_ok_and(|output| output.status.success());
    if std::env::var_os("METABENCH_REQUIRE_NATIVE_CONTRACTS").is_some() {
        assert!(available, "required native contracts need the matching gungraun-runner on PATH");
        assert_eq!(
            String::from_utf8_lossy(&version.unwrap().stdout).trim(),
            "gungraun-runner 0.20.0",
            "required native contracts must match the workspace Gungraun version"
        );
        assert!(valgrind_available(), "required native contracts need Valgrind on PATH");
    }
    available
}

fn valgrind_available() -> bool {
    Command::new("valgrind")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn fresh_consumer_resolves_and_compiles_without_the_workspace_lockfile() {
    with_cargo_lock(|| {
        // Keep the isolated workspace below the repository so Cargo inherits
        // source replacement configuration used by offline CI builds.
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let dependency = serde_json::to_string(env!("CARGO_MANIFEST_DIR")).unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                r#"[package]
name = "metabench-fresh-consumer"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
metabench = {{ path = {dependency} }}
criterion = {{ version = "0.8.2", default-features = false }}
gungraun = "=0.20.0"
"#
            ),
        )
        .unwrap();
        std::fs::create_dir(directory.path().join("src")).unwrap();
        std::fs::write(
            directory.path().join("src/main.rs"),
            r#"#[metabench::benchmark(CASE, "consumer", "case")]
fn workload() -> u64 { std::hint::black_box(42) }
fn benchmarks(c: &mut criterion::Criterion) {
    c.bench_function("consumer/case", |b| b.iter(|| workload()));
}
metabench::main!(criterion = benchmarks, benchmarks = [CASE],);
"#,
        )
        .unwrap();
        let output = cargo()
            .current_dir(directory.path())
            .args(["run", "--quiet", "--offline", "--", "--criterion", "--list"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("consumer/case: benchmark"));
        let lockfile = std::fs::read_to_string(directory.path().join("Cargo.lock")).unwrap();
        assert!(!lockfile.contains("name = \"gungraun-summary\""));
        assert_eq!(lockfile.matches("name = \"gungraun-runner\"").count(), 1);
    });
}

#[cfg(target_os = "linux")]
#[test]
fn perf_measures_exact_workload_and_writes_metrics() {
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
            "metabench_basic",
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
            std::fs::set_permissions(&vtune_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        directory
    })
}

/// The workspace's packaging allowlist deliberately keeps `/examples/**` out
/// of the published `.crate` (examples are dev-only, per the top-level
/// `Cargo.toml` policy), while `/tests/**` (this file) is packaged. A
/// `cargo test` run from a published tarball therefore cannot build the
/// `fake_vtune` fixture this file's vtune tests depend on.
///
/// This guard only covers the vtune tests added alongside it; it is not a
/// claim that the rest of this file (`run`/`run_target`'s unconditional
/// `--example metabench_basic`/`--example parameterized` invocations) can run from a
/// published tarball. Those tests have depended on unpackaged examples since
/// before this fixture existed, and fixing that pre-existing, file-wide gap
/// is a separate concern from hardening the new vtune coverage this guard
/// protects.
fn fake_vtune_fixture_available() -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("fake_vtune.rs")
        .is_file()
}

/// Published crate tarballs contain only the files this crate's packaging
/// allowlist (`Cargo.toml`'s workspace-inherited `include`) selects, which
/// never covers the workspace root manifest one level above `crates/`. Its
/// presence therefore distinguishes a full repository checkout (where
/// `examples/fake_vtune.rs` is expected to exist and its absence would be an
/// accidental move/rename bug) from a published-tarball build (where the
/// fixture's absence is an intentional packaging choice).
fn is_full_repository_checkout() -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml").is_file()
}

/// Returns `true` if the calling test should proceed. If the `fake_vtune`
/// fixture is unavailable, this either skips the test (published-tarball
/// case, expected) or panics (full repository checkout, where the fixture's
/// absence means it was accidentally moved or renamed rather than
/// intentionally excluded from packaging).
fn require_fake_vtune_fixture_or_skip() -> bool {
    if fake_vtune_fixture_available() {
        return true;
    }
    assert!(
        !is_full_repository_checkout(),
        "examples/fake_vtune.rs is missing from a full repository checkout; the vtune integration \
         tests depend on it and it should not have been moved or renamed without updating this file"
    );
    eprintln!("skipping: fake_vtune fixture source is not packaged (see fake_vtune_fixture_available)");
    false
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
    if !require_fake_vtune_fixture_or_skip() {
        return;
    }
    let path = path_with_fake_vtune();
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let command_log = directory.path().join("vtune-commands.log");
    let json = directory.path().join("report.json");
    let markdown = directory.path().join("report.md");
    // Also forwards `--vtune-arg -knob`, exercising `fake_vtune`'s validation
    // of the full `-collect-with runsa [<forwarded args>] --start-paused
    // -result-dir <dir>` collection-launch shape (not just the trailing `--`)
    // with a non-empty forwarded-argument list in the middle of it.
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
            "metabench_basic",
            "--",
            "--vtune",
            "--show-engine-output",
            "--no-baseline",
            "--vtune-arg",
            "-knob",
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
    if !require_fake_vtune_fixture_or_skip() {
        return;
    }
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
            "metabench_basic",
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
    if !require_fake_vtune_fixture_or_skip() {
        return;
    }
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
            "metabench_basic",
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
fn vtune_pause_failure_surfaces_vtune_control_error() {
    if !require_fake_vtune_fixture_or_skip() {
        return;
    }
    let path = path_with_fake_vtune();
    let _guard = CARGO_SUBPROCESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Distinct from the resume-failure test above: `resume` succeeds so
    // `vtune::begin` returns a `Guard`, and only the `-command pause` call
    // `Guard::drop` makes at the end of the measured workload fails. This
    // exercises `Guard::drop`'s own error-recording path rather than
    // `begin`'s.
    let output = cargo()
        .env("PATH", path)
        .env("FAKE_VTUNE_FAIL_COMMAND", "pause")
        .args([
            "run",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            "metabench_basic",
            "--",
            "--vtune",
            "--show-engine-output",
            "--no-baseline",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "expected the failed vtune pause to fail the run");
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

    assert!(
        stdout
            .lines()
            .any(|line| { line.contains("_checksums") && line.contains("_rolling") })
    );
    assert!(!stdout.contains("Running gungraun benchmarks"));
}

#[cfg(target_os = "linux")]
#[test]
fn gungraun_measurements_write_report() {
    if !gungraun_runner_available() || !valgrind_available() {
        eprintln!("skipping Gungraun measurements because gungraun-runner or Valgrind is unavailable");
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    let markdown = directory.path().join("report.md");
    let output = run_target(
        "metabench_basic",
        &[
            "--gungraun".as_ref(),
            "--show-engine-output".as_ref(),
            "--no-baseline".as_ref(),
            "--export-json".as_ref(),
            json.as_os_str(),
            "--export-md".as_ref(),
            markdown.as_os_str(),
        ],
    );
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["identity"], "basic/checksum/rolling");
    let result = &entries[0]["results"]["gungraun.callgrind"];
    assert!(result["metrics"]["Ir"]["value"].as_u64().is_some_and(|value| value > 0));
    assert!(!result["raw_artifacts"].as_array().unwrap().is_empty());
    assert!(std::fs::read_to_string(&markdown).unwrap().contains("basic/checksum/rolling"));
}

#[test]
fn setup_calls_do_not_steal_filtered_allocation_measurements() {
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    let output = run_target(
        "measurement_boundaries",
        &[
            "--allocations".as_ref(),
            "--criterion-arg=boundaries/allocate".as_ref(),
            "--export-json".as_ref(),
            json.as_os_str(),
        ],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for (entry, size) in entries.iter().zip([128, 32]) {
        let metrics = &entry["results"]["alloc_tracker"]["metrics"];
        assert_eq!(metrics["Allocations"]["value"], 1);
        assert_eq!(metrics["Allocated bytes"]["value"], size);
    }
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Warming up"));
}

#[test]
fn criterion_worker_bypasses_allocation_tracking() {
    with_cargo_lock(|| {
        let output = cargo()
            .env("METABENCH_TEST_REQUIRE_UNTRACKED", "1")
            .args([
                "run",
                "--quiet",
                "-p",
                "metabench",
                "--profile",
                "bench",
                "--example",
                "measurement_boundaries",
                "--",
                "--criterion",
                "--list",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("boundaries/allocate/small"));
    });
}

#[test]
fn native_probe_cases_reuse_iterators_conditionals_and_lifecycle_hooks() {
    with_cargo_lock(|| {
        let directory = tempfile::tempdir().unwrap();
        let json = directory.path().join("report.json");
        let lifecycle = directory.path().join("lifecycle.txt");
        let output = cargo()
            .env("METABENCH_TEST_LIFECYCLE", &lifecycle)
            .args([
                "run",
                "--quiet",
                "-p",
                "metabench",
                "--profile",
                "bench",
                "--example",
                "native_probe_contracts",
                "--",
                "--allocations",
                "--export-json",
            ])
            .arg(&json)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        let entries = report["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        for (entry, (case, size)) in entries.iter().zip([("conditional", 24), ("sizes_0", 8), ("sizes_1", 16)]) {
            assert_eq!(entry["identity"], format!("contracts/iterator/{case}"));
            assert_eq!(entry["results"]["alloc_tracker"]["metrics"]["Allocations"]["value"], 1);
            assert_eq!(entry["results"]["alloc_tracker"]["metrics"]["Allocated bytes"]["value"], size);
        }
        let events = std::fs::read_to_string(&lifecycle).unwrap();
        assert_eq!(
            events.lines().collect::<Vec<_>>(),
            [
                "suite setup",
                "group setup",
                "group teardown",
                "suite teardown",
                "suite setup",
                "group setup",
                "case setup",
                "case teardown",
                "group teardown",
                "suite teardown",
                "suite setup",
                "group setup",
                "case setup",
                "case teardown",
                "group teardown",
                "suite teardown",
                "suite setup",
                "group setup",
                "case setup",
                "case teardown",
                "group teardown",
                "suite teardown",
            ]
        );
    });
}

#[test]
fn native_vtune_orders_collection_and_teardown_on_success_and_unwind() {
    if !require_fake_vtune_fixture_or_skip() {
        return;
    }
    let path = path_with_fake_vtune();
    with_cargo_lock(|| {
        let directory = tempfile::tempdir().unwrap();
        for panic_workload in [false, true] {
            let events = directory.path().join("events.log");
            let json = directory.path().join("report.json");
            let markdown = directory.path().join("report.md");
            std::fs::write(&events, b"").unwrap();
            std::fs::write(&json, b"complete existing JSON report").unwrap();
            std::fs::write(&markdown, b"complete existing Markdown report").unwrap();
            let mut command = cargo();
            command
                .env("PATH", &path)
                .env("METABENCH_TEST_LIFECYCLE", &events)
                .env("VTUNE_TEST_COMMAND_LOG", &events)
                .env("METABENCH_TEST_ORDERING", "1")
                .args([
                    "run",
                    "--quiet",
                    "-p",
                    "metabench",
                    "--profile",
                    "bench",
                    "--example",
                    "native_probe_contracts",
                    "--",
                    "--vtune",
                    "--criterion-arg=contracts/iterator/conditional",
                    "--criterion-arg=--exact",
                    "--no-baseline",
                    "--export-json",
                ])
                .arg(&json)
                .arg("--export-md")
                .arg(&markdown);
            if panic_workload {
                command.env("METABENCH_TEST_PANIC_WORKLOAD", "1");
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.success(),
                !panic_workload,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let events = std::fs::read_to_string(&events).unwrap();
            let events: Vec<_> = events
                .lines()
                .map(|event| {
                    if event.starts_with("resume ") {
                        "resume"
                    } else if event.starts_with("pause ") {
                        "pause"
                    } else {
                        event
                    }
                })
                .collect();
            let mut expected = vec![
                "suite setup",
                "group setup",
                "group teardown",
                "suite teardown",
                "suite setup",
                "group setup",
                "case setup",
                "resume",
                "workload entry",
            ];
            if panic_workload {
                expected.push("pause");
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(stderr.contains("native workload panic"), "{stderr}");
                assert!(stderr.contains("worker log:"), "{stderr}");
                assert_eq!(std::fs::read(&json).unwrap(), b"complete existing JSON report");
                assert_eq!(std::fs::read(&markdown).unwrap(), b"complete existing Markdown report");
            } else {
                expected.extend(["workload exit", "pause", "case teardown"]);
            }
            expected.extend(["group teardown", "suite teardown"]);
            assert_eq!(events, expected);
            if !panic_workload {
                let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
                assert_eq!(report["entries"].as_array().unwrap().len(), 1);
                assert_eq!(
                    report["entries"][0]["results"]["vtune"]["metrics"]["INST_RETIRED.ANY"]["value"],
                    1234.0
                );
            }
        }
    });
}

#[test]
fn native_allocations_include_prestarted_threads_only_inside_the_window() {
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    let output = run_target(
        "threaded_allocations",
        &[
            "--allocations".as_ref(),
            "--no-baseline".as_ref(),
            "--export-json".as_ref(),
            json.as_os_str(),
        ],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    let metrics = &report["entries"][0]["results"]["alloc_tracker"]["metrics"];
    assert_eq!(metrics["Allocations"]["value"], 4);
    assert_eq!(metrics["Allocated bytes"]["value"], 37 + 19 + 83 + 19);
}

#[test]
fn native_workers_ignore_foreign_collector_environment() {
    if !require_fake_vtune_fixture_or_skip() {
        return;
    }
    let path = path_with_fake_vtune();
    with_cargo_lock(|| {
        let directory = tempfile::tempdir().unwrap();
        let foreign_control = directory.path().join("foreign-control");
        let foreign_ack = directory.path().join("foreign-ack");
        let foreign_allocations = directory.path().join("foreign-allocations.json");
        std::fs::write(&foreign_control, b"untouched control").unwrap();
        std::fs::write(&foreign_ack, b"bad\n").unwrap();
        std::fs::write(&foreign_allocations, b"untouched allocation artifact").unwrap();
        for selector in ["--allocations", "--vtune", "combined"] {
            let json = directory.path().join("report.json");
            let commands = directory.path().join("vtune-commands.log");
            std::fs::write(&commands, b"").unwrap();
            let mut command = cargo();
            command
                .env("PATH", &path)
                .env("VTUNE_TEST_COMMAND_LOG", &commands)
                .env("METABENCH_INTERNAL_PERF_CONTROL", &foreign_control)
                .env("METABENCH_INTERNAL_PERF_ACK", &foreign_ack)
                .env("METABENCH_INTERNAL_VTUNE_RESULT_DIR", directory.path().join("foreign-vtune"))
                .env("METABENCH_INTERNAL_ALLOCATION_ARTIFACT", &foreign_allocations)
                .env("METABENCH_INTERNAL_ALLOCATION_IDENTITY", "foreign/identity")
                .args([
                    "run",
                    "--quiet",
                    "-p",
                    "metabench",
                    "--profile",
                    "bench",
                    "--example",
                    "native_probe_contracts",
                    "--",
                    "--criterion-arg=contracts/iterator/conditional",
                    "--criterion-arg=--exact",
                    "--no-baseline",
                    "--export-json",
                ])
                .arg(&json);
            if selector == "combined" {
                command.args(["--allocations", "--vtune"]);
            } else {
                command.arg(selector);
            }
            if selector == "--vtune" {
                command.env("METABENCH_TEST_REQUIRE_UNTRACKED", "1");
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{selector}: {}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(std::fs::read(&foreign_control).unwrap(), b"untouched control");
            assert_eq!(std::fs::read(&foreign_allocations).unwrap(), b"untouched allocation artifact");
            let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
            assert_eq!(report["entries"].as_array().unwrap().len(), 1);
            if selector != "--vtune" {
                let metrics = &report["entries"][0]["results"]["alloc_tracker"]["metrics"];
                assert_eq!(metrics["Allocations"]["value"], 1);
                assert_eq!(metrics["Allocated bytes"]["value"], 24);
            }
            if selector == "--allocations" {
                assert_eq!(std::fs::read(&commands).unwrap(), b"");
            } else {
                assert_eq!(std::fs::read_to_string(&commands).unwrap().lines().count(), 2);
                assert_eq!(
                    report["entries"][0]["results"]["vtune"]["metrics"]["INST_RETIRED.ANY"]["value"],
                    1234.0
                );
            }
        }
    });
}

#[cfg(target_os = "linux")]
const FAKE_PERF_CONTROL_SCRIPT: &str = r#"#!/bin/sh
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
case "$FAKE_PERF_FAILURE" in
    connection-control) export METABENCH_INTERNAL_PERF_CONTROL="$output.missing" ;;
    connection-ack) export METABENCH_INTERNAL_PERF_ACK="$output.missing" ;;
    enable-closed|disable-closed)
        # A private FIFO lets close produce EOF despite the launcher's original keepalive.
        ack_fifo="$output.closed"
        mkfifo -m 600 "$ack_fifo" || exit 92
        export METABENCH_INTERNAL_PERF_ACK="$ack_fifo"
        ;;
esac
"$@" &
child=$!
if [ "$FAKE_PERF_FAILURE" != connection-control ]; then
    exec 3<"$control_fifo"
    if [ "$FAKE_PERF_FAILURE" != connection-ack ]; then
        exec 4>"$ack_fifo"
        IFS= read -r command <&3
        [ "$command" = enable ] || exit 90
        printf '%s\n' enable >> "$FAKE_PERF_EVENTS"
        case "$FAKE_PERF_FAILURE" in
            enable-malformed) printf 'bad\n' >&4 ;;
            enable-closed) exec 4>&- ;;
            *)
                printf 'ack\n' >&4
                IFS= read -r command <&3
                [ "$command" = disable ] || exit 91
                printf '%s\n' disable >> "$FAKE_PERF_EVENTS"
                case "$FAKE_PERF_FAILURE" in
                    disable-malformed) printf 'bad\n' >&4 ;;
                    disable-closed) exec 4>&- ;;
                    *) printf 'ack\n' >&4 ;;
                esac
                ;;
        esac
    fi
fi
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
"#;

#[cfg(target_os = "linux")]
fn bounded_output(command: &Command) -> Output {
    let mut bounded = Command::new("timeout");
    bounded
        .args(["--kill-after=2s", "20s"])
        .arg(command.get_program())
        .args(command.get_args());
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            bounded.env(name, value);
        } else {
            bounded.env_remove(name);
        }
    }
    bounded.output().unwrap()
}

#[cfg(target_os = "linux")]
fn build_example(target: &str) -> PathBuf {
    let build = cargo()
        .args([
            "build",
            "--quiet",
            "-p",
            "metabench",
            "--profile",
            "bench",
            "--example",
            target,
            "--message-format=json",
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    String::from_utf8_lossy(&build.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|message| {
            (message["reason"] == "compiler-artifact" && message["target"]["name"] == target)
                .then(|| message["executable"].as_str().map(PathBuf::from))
                .flatten()
        })
        .unwrap()
}

#[cfg(target_os = "linux")]
fn example_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    for (name, value) in cargo().get_envs() {
        if let Some(value) = value {
            command.env(name, value);
        } else {
            command.env_remove(name);
        }
    }
    command
}

#[cfg(target_os = "linux")]
#[test]
fn native_perf_control_failures_are_bounded_and_preserve_reports() {
    with_cargo_lock(|| {
        let executable = build_example("native_probe_contracts");
        let directory = tempfile::tempdir().unwrap();
        let perf = directory.path().join("perf");
        std::fs::write(&perf, FAKE_PERF_CONTROL_SCRIPT).unwrap();
        std::fs::set_permissions(&perf, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(directory.path().to_owned()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        for failure in [
            "success",
            "enable-malformed",
            "enable-closed",
            "disable-malformed",
            "disable-closed",
            "connection-control",
            "connection-ack",
        ] {
            let events = directory.path().join("events.log");
            let json = directory.path().join("report.json");
            let markdown = directory.path().join("report.md");
            std::fs::write(&events, b"").unwrap();
            std::fs::write(&json, b"complete existing JSON report").unwrap();
            std::fs::write(&markdown, b"complete existing Markdown report").unwrap();
            let mut command = example_command(&executable);
            command
                .env("PATH", &path)
                .env("FAKE_PERF_FAILURE", failure)
                .env("FAKE_PERF_EVENTS", &events)
                .args([
                    "--perf",
                    "--criterion-arg=contracts/iterator/conditional",
                    "--criterion-arg=--exact",
                    "--no-baseline",
                    "--timeout=5s",
                    "--export-json",
                ])
                .arg(&json)
                .arg("--export-md")
                .arg(&markdown);
            let output = bounded_output(&command);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_ne!(output.status.code(), Some(124), "{failure}: outer timeout expired: {stderr}");
            assert_ne!(
                output.status.code(),
                Some(137),
                "{failure}: outer timeout killed the process: {stderr}"
            );
            let commands = std::fs::read_to_string(&events).unwrap();
            let expected_commands = if failure.starts_with("connection-") {
                ""
            } else if failure.starts_with("enable-") {
                "enable\n"
            } else {
                "enable\ndisable\n"
            };
            assert_eq!(commands, expected_commands, "{failure}: {stderr}");
            if failure == "success" {
                assert!(output.status.success(), "{failure}: {stderr}");
                let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
                assert_eq!(report["entries"].as_array().unwrap().len(), 1);
                assert_eq!(report["entries"][0]["results"]["perf"]["metrics"]["instructions"]["value"], 1234.0);
            } else {
                assert!(!output.status.success(), "{failure}: control failure incorrectly succeeded");
                assert!(stderr.contains("failed to control Linux perf counters"), "{failure}: {stderr}");
                assert!(stderr.contains("worker log:"), "{failure}: {stderr}");
                let diagnostic = if failure.ends_with("malformed") {
                    "unexpected perf acknowledgement"
                } else if failure.ends_with("closed") {
                    "failed to fill whole buffer"
                } else {
                    "No such file or directory"
                };
                assert!(stderr.contains(diagnostic), "{failure}: {stderr}");
                assert_eq!(std::fs::read(&json).unwrap(), b"complete existing JSON report");
                assert_eq!(std::fs::read(&markdown).unwrap(), b"complete existing Markdown report");
            }
        }
    });
}

#[test]
fn probe_listing_uses_native_cases_without_criterion_registration() {
    let output = run_target("native_probe_contracts", &["--allocations".as_ref(), "--list".as_ref()]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("contracts/iterator/sizes_0: benchmark"), "{stdout}");
    assert!(stdout.contains("contracts/iterator/sizes_1: benchmark"), "{stdout}");
    assert!(stdout.contains("contracts/iterator/conditional: benchmark"), "{stdout}");
    assert!(!stdout.contains("disabled"), "{stdout}");
}

#[test]
fn smoke_and_empty_measurements_preserve_existing_reports() {
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    let markdown = directory.path().join("report.md");
    std::fs::write(&json, b"existing JSON").unwrap();
    std::fs::write(&markdown, b"existing Markdown").unwrap();
    for selector in ["--criterion", "--allocations"] {
        let output = run_target(
            "measurement_boundaries",
            &[
                selector.as_ref(),
                "--test".as_ref(),
                "--export-json".as_ref(),
                json.as_os_str(),
                "--export-md".as_ref(),
                markdown.as_os_str(),
            ],
        );
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::fs::read(&json).unwrap(), b"existing JSON");
        assert_eq!(std::fs::read(&markdown).unwrap(), b"existing Markdown");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Warming up"));
    }
    for selector in ["--criterion", "--allocations"] {
        let output = run_target(
            "measurement_boundaries",
            &[
                selector.as_ref(),
                "--criterion-arg=no-such-case".as_ref(),
                "--export-json".as_ref(),
                json.as_os_str(),
                "--export-md".as_ref(),
                markdown.as_os_str(),
            ],
        );
        assert!(!output.status.success());
        assert_eq!(std::fs::read(&json).unwrap(), b"existing JSON");
        assert_eq!(std::fs::read(&markdown).unwrap(), b"existing Markdown");
    }
}

#[test]
fn failed_native_setup_retains_worker_diagnostics() {
    with_cargo_lock(|| {
        let output = cargo()
            .env("METABENCH_TEST_FAIL_SETUP", "1")
            .args([
                "run",
                "--quiet",
                "-p",
                "metabench",
                "--profile",
                "bench",
                "--example",
                "measurement_boundaries",
                "--",
                "--allocations",
                "--criterion-arg=small",
                "--no-output",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("native setup failed"), "{stderr}");
        assert!(stderr.contains("worker log:"), "{stderr}");
    });
}

#[test]
fn exact_filter_and_const_workload_support_allocation_probes() {
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    let output = run_target(
        "measurement_boundaries",
        &[
            "--allocations".as_ref(),
            "--criterion-arg=boundaries/const".as_ref(),
            "--criterion-arg=--exact".as_ref(),
            "--criterion-arg=--profile-time=60".as_ref(),
            "--criterion-arg=--load-baseline=missing".as_ref(),
            "--export-json".as_ref(),
            json.as_os_str(),
        ],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["identity"], "boundaries/const");
    assert_eq!(
        report["entries"][0]["results"]["alloc_tracker"]["metrics"]["Allocations"]["value"],
        0
    );
}

#[test]
fn timed_out_worker_reports_captured_diagnostics_and_preserves_baseline() {
    with_cargo_lock(|| {
        let directory = tempfile::tempdir().unwrap();
        let json = directory.path().join("report.json");
        std::fs::write(&json, b"existing baseline").unwrap();
        let output = cargo()
            .env("METABENCH_TEST_TIMEOUT", "1")
            .args([
                "run",
                "--quiet",
                "-p",
                "metabench",
                "--profile",
                "bench",
                "--example",
                "measurement_boundaries",
                "--",
                "--allocations",
                "--criterion-arg=small",
                "--timeout",
                "2s",
                "--export-json",
            ])
            .arg(&json)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("selected workload is waiting"), "{stderr}");
        assert!(stderr.contains("worker log:"), "{stderr}");
        assert!(stderr.contains("exceeded the 2s timeout"), "{stderr}");
        assert_eq!(std::fs::read(&json).unwrap(), b"existing baseline");
    });
}

#[cfg(target_os = "linux")]
struct ReportSnapshot {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

#[cfg(target_os = "linux")]
impl ReportSnapshot {
    fn new(path: PathBuf) -> Self {
        Self {
            original: Self::read(&path),
            path,
        }
    }

    #[expect(
        clippy::panic,
        reason = "unexpected snapshot I/O must fail the test rather than look like an absent report"
    )]
    fn read(path: &Path) -> Option<Vec<u8>> {
        match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => panic!("failed to snapshot {}: {error}", path.display()),
        }
    }

    fn assert_unchanged(&self) {
        assert_eq!(Self::read(&self.path), self.original, "{}", self.path.display());
    }
}

#[cfg(target_os = "linux")]
impl Drop for ReportSnapshot {
    fn drop(&mut self) {
        let result = if let Some(bytes) = &self.original {
            std::fs::write(&self.path, bytes)
        } else {
            match std::fs::remove_file(&self.path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        };
        if let Err(error) = result {
            eprintln!("failed to restore smoke-test report {}: {error}", self.path.display());
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn default_engine_smoke_checks_do_not_publish_measurements() {
    if !gungraun_runner_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let json = directory.path().join("report.json");
    std::fs::write(&json, b"existing baseline").unwrap();
    let output = run_target(
        "measurement_boundaries",
        &["--test".as_ref(), "--export-json".as_ref(), json.as_os_str()],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(std::fs::read(&json).unwrap(), b"existing baseline");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Warming up"));
    with_cargo_lock(|| {
        let root = cargo_target().join("metabench/measurement_boundaries");
        let reports = ["report.json", "report.md"].map(|name| ReportSnapshot::new(root.join(name)));
        let output = run_target_unlocked("measurement_boundaries", &[]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Warming up"));
        for report in &reports {
            report.assert_unchanged();
        }
    });
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
