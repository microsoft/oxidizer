// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "integration tests")]
#![cfg(not(miri))]
#![cfg(zygote_workspace_tests)]
#![expect(
    clippy::format_collect,
    clippy::unwrap_used,
    reason = "test fixtures fail immediately and favor compact byte diagnostics"
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::os::fd::AsFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt as _;
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt as StdCommandExt;
use std::path::{Path, PathBuf};
use std::process::Command as NativeCommand;
#[cfg(windows)]
use std::sync::Mutex;
use std::sync::OnceLock;

#[cfg(windows)]
use zygote_control::SandboxPolicy;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use zygote_control::linux::{AuditArchitecture, SeccompInstruction, SeccompPolicy};
#[cfg(target_os = "linux")]
use zygote_control::linux::{
    CgroupMembership, CommandExt as _, LandlockAccess, LandlockRuleset, Namespace, NamespaceKind, PrivilegePolicy, Support as LinuxSupport,
};
#[cfg(unix)]
use zygote_control::unix::{CommandExt as _, Resource, ResourceLimit};
#[cfg(windows)]
use zygote_control::windows::{
    AppContainerPolicy, CommandExt as _, IntegrityLevel, JobPolicy, MitigationPolicy, RestrictedTokenPolicy, Sid,
};
use zygote_control::{OutputLimits, Stdio, Zygote};
#[cfg(target_os = "linux")]
use zygote_control::{PreforkPoolConfig, WorkerRecoveryPolicy};
#[cfg(target_os = "linux")]
use zygote_rt::protocol::MAX_ITEM_COUNT;
use zygote_rt::protocol::{CONTROL_FD_ENV, NONCE_ENV};

struct Targets {
    controller_example: PathBuf,
    prepared: PathBuf,
    transparent: PathBuf,
}

fn targets() -> &'static Targets {
    static TARGETS: OnceLock<Targets> = OnceLock::new();
    TARGETS.get_or_init(build_targets)
}

fn build_targets() -> Targets {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = manifest_dir.join("test-fixtures/targets");
    let target_dir = manifest_dir.join("../../target/zygote-test-fixtures");
    let target = std::env::var_os("ZYGOTE_TEST_TARGET");
    let mut command = NativeCommand::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command
        .args(["build", "--quiet", "--bins", "--examples", "--manifest-path"])
        .arg(fixture_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .env_remove(CONTROL_FD_ENV)
        .env_remove(NONCE_ENV);
    if let Some(target) = &target {
        command.arg("--target").arg(target);
    }
    let status = command.status().unwrap();
    assert!(status.success(), "fixture package failed to build");

    let extension = std::env::consts::EXE_SUFFIX;
    let binary_dir = target.map_or_else(|| target_dir.join("debug"), |target| target_dir.join(target).join("debug"));
    Targets {
        controller_example: binary_dir.join("examples").join(format!("controller{extension}")),
        prepared: binary_dir.join(format!("zygote-prepared-fixture{extension}")),
        transparent: binary_dir.join(format!("zygote-transparent-fixture{extension}")),
    }
}

fn fields(output: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .map(|line| line.split_once('=').unwrap())
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[cfg(target_os = "linux")]
fn close_standard_input() -> std::io::Result<()> {
    // SAFETY: callers invoke this only in a child-side pre-exec hook that owns
    // the inherited standard-input descriptor.
    if unsafe { libc::close(libc::STDIN_FILENO) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[test]
fn output_limits_validate_boundaries() {
    OutputLimits::new(0, 1).unwrap_err();
    OutputLimits::new(2, 1).unwrap_err();
    assert_eq!(OutputLimits::new(2, 3).unwrap().aggregate(), 3);
}

#[cfg(target_os = "linux")]
#[test]
fn landlock_access_uses_checked_bitflags() {
    let access = LandlockAccess::READ_FILE | LandlockAccess::READ_DIR;
    assert!(access.contains(LandlockAccess::READ_FILE));
    assert!(!access.contains(LandlockAccess::WRITE_FILE));
    assert_eq!(LandlockAccess::from_bits(1 << 63), None);
    assert!(LandlockAccess::empty().is_empty());
}

#[cfg(windows)]
#[test]
fn rejects_invalid_job_limits() {
    JobPolicy::builder().cpu_rate(Some(0)).build().unwrap_err();
}

#[cfg(windows)]
#[test]
fn rejects_empty_restricted_token_policy() {
    RestrictedTokenPolicy::builder().build().unwrap_err();
}

fn hex(value: &OsStr) -> String {
    value.as_encoded_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn report(zygote: &Zygote, value: &str, argument: &OsStr) -> BTreeMap<String, String> {
    let output = zygote
        .command()
        .args([OsStr::new("report"), argument])
        .env_clear()
        .env("ZYGOTE_TEST_VALUE", value)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stderr.is_empty());
    fields(&output.stdout)
}

#[test]
fn documented_controller_example_is_runnable() {
    let output = NativeCommand::new(&targets().controller_example)
        .arg(&targets().transparent)
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains(&format!("env={}", hex(OsStr::new("documented")))));
    assert!(output.contains(&hex(OsStr::new("example"))));
    for argument in ["concurrent-one", "concurrent-two"] {
        assert!(output.contains(&format!("env={}", hex(OsStr::new(argument)))));
        assert!(output.contains(&hex(OsStr::new(argument))));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn controller_with_closed_standard_input_matches_native_runtime_behavior() {
    let mut direct = NativeCommand::new(&targets().transparent);
    direct.arg("stdin-state");
    // SAFETY: the child-side hook closes only its inherited standard input
    // immediately before exec to establish the native comparison case.
    unsafe {
        direct.pre_exec(close_standard_input);
    }
    let direct = direct.output().unwrap();
    assert!(direct.status.success(), "{}", String::from_utf8_lossy(&direct.stderr));

    for timing in ["before-init", "after-init"] {
        let accelerated = NativeCommand::new(&targets().controller_example)
            .arg(&targets().transparent)
            .env("ZYGOTE_TEST_CLOSE_STDIN", timing)
            .output()
            .unwrap();
        assert!(accelerated.status.success(), "{}", String::from_utf8_lossy(&accelerated.stderr));
        assert_eq!(accelerated.stdout, direct.stdout, "{timing}");
    }
}

#[test]
fn direct_transparent_and_prepared_entries_preserve_behavior() {
    let transparent = NativeCommand::new(&targets().transparent)
        .args(["report", "direct"])
        .env("ZYGOTE_TEST_VALUE", "direct-value")
        .output()
        .unwrap();
    assert!(transparent.status.success());
    let transparent = fields(&transparent.stdout);
    assert!(transparent["argv"].ends_with(&format!(",{},{}", hex(OsStr::new("report")), hex(OsStr::new("direct")))));
    assert_eq!(transparent["env"], hex(OsStr::new("direct-value")));

    let prepared = NativeCommand::new(&targets().prepared).args(["report", "direct"]).output().unwrap();
    assert!(prepared.status.success());
    let prepared = fields(&prepared.stdout);
    assert_eq!(prepared["prepared_by"], prepared["pid"]);

    let failed = NativeCommand::new(&targets().prepared)
        .env("ZYGOTE_TEST_PREPARE_FAIL", "1")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8(failed.stderr).unwrap().contains("requested fixture failure"));

    let panicked = NativeCommand::new(&targets().prepared).arg("panic").output().unwrap();
    assert_eq!(panicked.status.code(), Some(101));
}

#[cfg(target_os = "linux")]
#[test]
fn malformed_bootstrap_markers_fail_closed() {
    for target in [&targets().transparent, &targets().prepared] {
        let status = NativeCommand::new(target)
            .env_remove(CONTROL_FD_ENV)
            .env(NONCE_ENV, "00000000000000000000000000000000")
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(125));

        let status = NativeCommand::new(target)
            .env(CONTROL_FD_ENV, "not-a-descriptor")
            .env(NONCE_ENV, "00000000000000000000000000000000")
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(125));
    }
}

#[test]
fn transparent_launches_isolate_repeated_request_state() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let first = report(&zygote, "sentinel-one", OsStr::new("first-request"));
    let second = report(&zygote, "sentinel-two", OsStr::new("second-request"));

    assert_eq!(first["env"], hex(OsStr::new("sentinel-one")));
    assert_eq!(second["env"], hex(OsStr::new("sentinel-two")));
    assert!(first["argv"].contains(&hex(OsStr::new("first-request"))));
    assert!(!first["argv"].contains(&hex(OsStr::new("second-request"))));
    assert!(second["argv"].contains(&hex(OsStr::new("second-request"))));
    assert!(!second["argv"].contains(&hex(OsStr::new("first-request"))));
    assert_ne!(first["pid"], second["pid"]);
    #[cfg(target_os = "linux")]
    {
        assert_eq!(first["fds"], "0,1,2");
        assert_eq!(second["fds"], "0,1,2");
    }
}

#[test]
fn prepared_launches_reuse_state_and_preserve_arguments() {
    let zygote = Zygote::builder(&targets().prepared).spawn().unwrap();
    let first = report(&zygote, "prepared-one", OsStr::new("first"));
    let second = report(&zygote, "prepared-two", OsStr::new("second"));

    #[cfg(target_os = "linux")]
    {
        assert_eq!(first["prepared_by"], second["prepared_by"]);
        assert_ne!(first["prepared_by"], first["pid"]);
        assert_ne!(second["prepared_by"], second["pid"]);
        assert_eq!(first["sigusr1"], "default");
        assert_eq!(second["sigusr1"], "default");
        assert_eq!(first["altstack"], "disabled");
        assert_eq!(second["altstack"], "disabled");
    }

    #[cfg(not(target_os = "linux"))]
    {
        assert_eq!(first["prepared_by"], first["pid"]);
        assert_eq!(second["prepared_by"], second["pid"]);
    }
    assert!(first["argv"].contains(&hex(OsStr::new("first"))));
    assert!(second["argv"].contains(&hex(OsStr::new("second"))));
}

#[cfg(target_os = "linux")]
#[test]
fn prefork_pool_dispatches_single_use_workers_and_replenishes_capacity() {
    let mut pool = PreforkPoolConfig::new(2).unwrap();
    pool.min_idle(1).unwrap();
    pool.refill_threshold(1).unwrap();
    pool.refill_delay(std::time::Duration::from_millis(25));
    let mut builder = Zygote::builder(&targets().prepared);
    builder.prefork_pool(pool);
    let zygote = builder.spawn().unwrap();

    let first = report(&zygote, "prefork", OsStr::new("first"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while zygote.health().idle_prefork_workers != 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let delayed_health = zygote.health();
    assert_eq!(delayed_health.idle_prefork_workers, 2, "{delayed_health:?}");
    assert!(delayed_health.prefork_refills >= 1, "{delayed_health:?}");
    let second = report(&zygote, "prefork", OsStr::new("second"));
    assert_ne!(first["pid"], second["pid"]);
    assert_eq!(first["prepared_by"], second["prepared_by"]);
    assert_ne!(first["prepared_by"], first["pid"]);
    assert_ne!(second["prepared_by"], second["pid"]);
    let health = zygote.health();
    assert_eq!(health.launches, 2);
    assert_eq!(health.prefork_hits, 2);
    assert_eq!(health.prefork_misses, 0);
    assert!(health.prefork_refills >= 1);
    assert!(health.ready_workers >= 1);

    zygote.trim_prefork_pool(0).unwrap();
    let fallback = report(&zygote, "prefork", OsStr::new("fallback"));
    assert_ne!(fallback["pid"], fallback["prepared_by"]);
    assert_eq!(zygote.health().prefork_misses, 1);

    zygote.trim_prefork_pool(2).unwrap();
    let restored = report(&zygote, "prefork", OsStr::new("restored"));
    assert_ne!(restored["pid"], restored["prepared_by"]);
    assert_eq!(zygote.health().prefork_hits, 3);
}

#[cfg(target_os = "linux")]
#[test]
fn worker_recovery_policy_validates_backoff_bounds() {
    let mut policy = WorkerRecoveryPolicy::new(2);
    policy
        .backoff(std::time::Duration::from_secs(2), std::time::Duration::from_secs(1))
        .unwrap_err();
    policy
        .backoff(std::time::Duration::from_millis(1), std::time::Duration::from_millis(4))
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn failed_template_is_replaced_without_blocking_healthy_workers() {
    let mut recovery = WorkerRecoveryPolicy::new(2);
    recovery
        .backoff(std::time::Duration::from_millis(500), std::time::Duration::from_millis(500))
        .unwrap();
    let prepared = &targets().prepared;
    let unique_target = prepared.with_file_name(format!("zygote-prepared-recovery-{}", std::process::id()));
    let _ = fs::remove_file(&unique_target);
    std::os::unix::fs::symlink(prepared, &unique_target).unwrap();
    let mut builder = Zygote::builder(&unique_target);
    builder.workers(2).unwrap().worker_recovery(recovery);
    let zygote = builder.spawn().unwrap();
    let mut templates = Vec::new();
    for task in fs::read_dir("/proc/self/task").unwrap() {
        if let Ok(children) = fs::read_to_string(task.unwrap().path().join("children")) {
            templates.extend(children.split_ascii_whitespace().filter_map(|pid| {
                let pid = pid.parse::<i32>().unwrap();
                let command_line = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
                command_line
                    .split(|byte| *byte == 0)
                    .next()
                    .is_some_and(|program| program == unique_target.as_os_str().as_encoded_bytes())
                    .then_some(pid)
            }));
        }
    }
    templates.sort_unstable();
    templates.dedup();
    assert_eq!(templates.len(), 2);
    // SAFETY: the discovered PID names a live template child owned by this test.
    assert_eq!(unsafe { libc::kill(templates[0], libc::SIGKILL) }, 0);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while zygote.health().ready_workers != 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(zygote.health().ready_workers, 1);
    assert!(zygote.command().arg("exit").arg("0").status().unwrap().success());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while zygote.health().worker_restarts != 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let health = zygote.health();
    assert_eq!(health.worker_restarts, 1);
    assert_eq!(health.ready_workers, 2);
    zygote.shutdown().unwrap();
    fs::remove_file(unique_target).unwrap();
}

#[test]
fn worker_count_rejects_zero() {
    let mut builder = Zygote::builder(&targets().transparent);
    assert_eq!(builder.workers(0).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

#[cfg(target_os = "linux")]
#[test]
fn missing_target_is_reported_without_native_fallback() {
    let error = Zygote::builder("definitely-missing-zygote-target").spawn().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(target_os = "linux")]
#[test]
fn configured_workers_distribute_prepared_launches() {
    let mut builder = Zygote::builder(&targets().prepared);
    builder.workers(4).unwrap();
    let zygote = builder.spawn().unwrap();
    let mut templates = (0..8)
        .map(|index| report(&zygote, "sharded", OsStr::new(&format!("launch-{index}")))["prepared_by"].clone())
        .collect::<Vec<_>>();
    templates.sort();
    templates.dedup();
    assert_eq!(templates.len(), 4);
}

#[cfg(unix)]
#[test]
fn prepared_launch_preserves_non_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let argument = OsString::from_vec(vec![b'n', b'o', b'n', 0xff, b'u', b't', b'f']);
    let zygote = Zygote::builder(&targets().prepared).spawn().unwrap();
    let output = report(&zygote, "non-utf8", &argument);
    assert!(output["argv"].contains(&hex(&argument)));
}

#[test]
fn concurrent_launches_route_output_and_status_to_the_right_request() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let launcher = zygote.launcher();
    let mut pids = (0..8)
        .map(|index| {
            let launcher = launcher.clone();
            std::thread::spawn(move || {
                let sentinel = format!("concurrent-{index}");
                let output = launcher
                    .command()
                    .args(["report", &sentinel])
                    .env_clear()
                    .env("ZYGOTE_TEST_VALUE", &sentinel)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                let output = fields(&output.stdout);
                assert_eq!(output["env"], hex(OsStr::new(&sentinel)));
                assert!(output["argv"].contains(&hex(OsStr::new(&sentinel))));
                output["pid"].clone()
            })
        })
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), 8);
}

#[test]
fn stdio_status_try_wait_and_kill_match_process_semantics() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let mut child = zygote
        .command()
        .arg("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(zygote.health().active_children, 1);
    child.stdin.take().unwrap().write_all(b"pipe-data").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(zygote.health().active_children, 0);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"stdout:pipe-data");
    assert_eq!(output.stderr, b"stderr:pipe-data");

    let status = zygote.command().args(["exit", "37"]).status().unwrap();
    assert_eq!(status.code(), Some(37));
    assert_eq!(zygote.health().active_children, 0);

    let mut sleeping = zygote.command().arg("sleep").spawn().unwrap();
    assert_eq!(zygote.health().active_children, 1);
    assert!(sleeping.id() > 0);
    assert_eq!(sleeping.try_wait().unwrap(), None);
    sleeping.kill().unwrap();
    let status = sleeping.wait().unwrap();
    assert_eq!(zygote.health().active_children, 0);
    assert!(!status.success());
    #[cfg(unix)]
    assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGKILL));
}

#[cfg(windows)]
#[test]
fn absent_standard_handles_do_not_enable_unrelated_handle_inheritance() {
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};

    static STANDARD_HANDLE_LOCK: Mutex<()> = Mutex::new(());
    struct RestoreStandardHandles([(u32, HANDLE); 3]);
    impl Drop for RestoreStandardHandles {
        fn drop(&mut self) {
            for (identifier, handle) in self.0 {
                // SAFETY: each saved value is the process's previous standard handle.
                assert_ne!(unsafe { SetStdHandle(identifier, handle) }, 0);
            }
        }
    }
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    // Build fixtures before temporarily removing process-global standard handles.
    let transparent = targets().transparent.clone();
    let artifact_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/zygote-null-stdio");
    fs::create_dir_all(&artifact_dir).unwrap();
    let sentinel_path = artifact_dir.join(format!("{}.sentinel", std::process::id()));
    let report_path = artifact_dir.join(format!("{}.txt", std::process::id()));
    let _ = fs::remove_file(&sentinel_path);
    let _ = fs::remove_file(&report_path);
    let _sentinel_cleanup = Cleanup(sentinel_path.clone());
    let _report_cleanup = Cleanup(report_path.clone());
    let sentinel = fs::File::create(&sentinel_path).unwrap();
    assert_ne!(
        // SAFETY: sentinel owns a live file handle and only its inheritance flag is changed.
        unsafe { SetHandleInformation(sentinel.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
        0
    );
    let sentinel_argument = OsString::from((sentinel.as_raw_handle() as usize).to_string());

    let lock = STANDARD_HANDLE_LOCK.lock().unwrap();
    let identifiers = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    // SAFETY: every selector is a documented standard-handle identifier.
    let original = identifiers.map(|identifier| (identifier, unsafe { GetStdHandle(identifier) }));
    let restore = RestoreStandardHandles(original);
    for identifier in identifiers {
        // SAFETY: Windows documents NULL as an absent standard handle.
        assert_ne!(unsafe { SetStdHandle(identifier, std::ptr::null_mut()) }, 0);
    }

    let zygote = Zygote::builder(&transparent).spawn().unwrap();
    let mut child = zygote
        .command()
        .args([
            OsStr::new("handle-inheritance-state"),
            sentinel_argument.as_os_str(),
            sentinel_path.as_os_str(),
            report_path.as_os_str(),
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("child with absent standard handles did not exit within five seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    drop(restore);
    drop(lock);
    assert!(status.success());
    assert_eq!(fs::read(report_path).unwrap(), b"unavailable");
}

#[test]
fn captured_output_limit_terminates_an_unbounded_writer() {
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    let artifact_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/zygote-output-limit");
    fs::create_dir_all(&artifact_dir).unwrap();
    let pid_path = artifact_dir.join(format!("{}.pid", std::process::id()));
    let _ = fs::remove_file(&pid_path);
    let _cleanup = Cleanup(pid_path.clone());
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let error = zygote
        .command()
        .args([OsStr::new("write-forever-survives-pipes"), pid_path.as_os_str()])
        .output_limits(OutputLimits::new(32 * 1024, 48 * 1024).unwrap())
        .output()
        .unwrap_err();
    assert!(error.to_string().contains("output exceeded"));
    let pid = fs::read_to_string(pid_path).unwrap().parse::<u32>().unwrap();
    assert_process_reaped(pid);
}

#[cfg(target_os = "linux")]
fn assert_process_reaped(pid: u32) {
    assert!(
        !Path::new("/proc").join(pid.to_string()).exists(),
        "output() returned while child {pid} still existed"
    );
    // SAFETY: signal zero does not alter the target process.
    assert_eq!(unsafe { libc::kill(pid.cast_signed(), 0) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[cfg(all(unix, not(target_os = "linux")))]
fn assert_process_reaped(pid: u32) {
    // SAFETY: signal zero does not alter the target process.
    assert_eq!(unsafe { libc::kill(pid.cast_signed(), 0) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[cfg(windows)]
fn assert_process_reaped(pid: u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject};

    // SAFETY: OpenProcess only observes whether the recorded PID still names a process.
    let process = unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_INVALID_PARAMETER.cast_signed()),
            "failed to observe the terminated child"
        );
        return;
    }
    // Windows can retain an openable terminated process object after all
    // application-owned handles close. Its signalled state and terminal exit
    // code, rather than OpenProcess failure, prove cleanup completed.
    // SAFETY: process is a live synchronization/query handle opened above.
    assert_eq!(unsafe { WaitForSingleObject(process, 0) }, WAIT_OBJECT_0);
    let still_active = u32::try_from(STILL_ACTIVE).unwrap();
    let mut exit_code = still_active;
    // SAFETY: process remains live and exit_code is writable.
    assert_ne!(unsafe { GetExitCodeProcess(process, &raw mut exit_code) }, 0);
    assert_ne!(exit_code, still_active);
    // SAFETY: process is owned by this test and closed exactly once.
    assert_ne!(unsafe { CloseHandle(process) }, 0);
}

#[test]
fn command_and_stdio_builders_report_their_state() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let mut command = zygote.command();
    command
        .args(["one", "two"])
        .env("REPLACED", "first")
        .env("REPLACED", "second")
        .env("REMOVED", "value")
        .env_remove("REMOVED")
        .current_dir("relative")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    assert_eq!(command.get_program(), targets().transparent.as_os_str());
    assert_eq!(command.get_args().collect::<Vec<_>>(), [OsStr::new("one"), OsStr::new("two")]);
    assert_eq!(command.get_current_dir(), Some(Path::new("relative")));
    let environment = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(environment[OsStr::new("REPLACED")], Some(OsString::from("second")));
    assert_eq!(environment[OsStr::new("REMOVED")], None);
    assert!(!format!("{command:?}").contains("second"));

    assert_eq!(format!("{:?}", Stdio::inherit()), "Stdio(\"inherit\")");
    assert_eq!(format!("{:?}", Stdio::null()), "Stdio(\"null\")");
    assert_eq!(format!("{:?}", Stdio::piped()), "Stdio(\"piped\")");
    let file = fs::File::open(targets().transparent.as_path()).unwrap();
    assert_eq!(format!("{:?}", Stdio::from(file)), "Stdio(\"file\")");
}

#[cfg(target_os = "linux")]
#[test]
fn invalid_launch_values_are_rejected_before_application_entry() {
    use std::os::unix::ffi::OsStringExt;

    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();

    let error = zygote
        .command()
        .env(OsString::from_vec(b"BAD=KEY".to_vec()), "value")
        .spawn()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    let error = zygote.command().arg(OsString::from_vec(b"nul\0arg".to_vec())).spawn().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    let mut command = zygote.command();
    command.umask(0o1000);
    assert_eq!(command.spawn().unwrap_err().kind(), std::io::ErrorKind::InvalidInput);

    let mut command = zygote.command();
    command.new_session().process_group(0);
    assert_eq!(command.spawn().unwrap_err().kind(), std::io::ErrorKind::InvalidInput);

    let mut command = zygote.command();
    command.args(std::iter::repeat_n("argument", MAX_ITEM_COUNT));
    assert_eq!(command.spawn().unwrap_err().kind(), std::io::ErrorKind::InvalidInput);

    let error = zygote
        .command()
        .arg("report")
        .current_dir("definitely-missing-zygote-directory")
        .spawn()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(all(unix, not(target_os = "linux")))]
#[test]
fn native_unix_rejects_out_of_range_umask() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let mut command = zygote.command();
    command.umask(0o1000);
    assert_eq!(command.spawn().unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

#[cfg(target_os = "linux")]
#[test]
fn privilege_reduction_is_installed_before_started() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let output = zygote
        .command()
        .arg("report")
        .privilege_policy(
            PrivilegePolicy::builder()
                .no_new_privileges(true)
                .disable_dumping(true)
                .clear_capabilities(true)
                .build()
                .unwrap(),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = fields(&output.stdout);
    assert_eq!(output["no_new_privs"], "1");
    assert_eq!(output["dumpable"], "0");
    assert_eq!(output["fds"], "0,1,2");
}

#[cfg(target_os = "linux")]
#[test]
fn all_capability_sets_are_cleared_when_supported() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert_ne!(
            std::env::var_os("ZYGOTE_REQUIRE_LINUX_SANDBOX"),
            Some(OsString::from("1")),
            "CI requires a privileged capability-reduction host"
        );
        eprintln!("skipped: complete capability reduction requires a privileged test host");
        return;
    }
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let output = zygote
        .command()
        .arg("report")
        .privilege_policy(PrivilegePolicy::secure_default())
        .output()
        .unwrap();
    let fields = fields(&output.stdout);
    assert_eq!(fields["cap_effective"], "0000000000000000");
    assert_eq!(fields["cap_permitted"], "0000000000000000");
    assert_eq!(fields["cap_inheritable"], "0000000000000000");
    let securebits = fields["securebits"].parse::<i32>().unwrap();
    assert_eq!(securebits & (1 | 2 | 4 | 8 | 32 | 64 | 128), 1 | 2 | 4 | 8 | 32 | 64 | 128);
}

#[cfg(target_os = "linux")]
#[test]
fn distinct_namespace_entry_is_applied_or_explicitly_skipped() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    if !LinuxSupport::probe().setns {
        assert_ne!(
            std::env::var_os("ZYGOTE_REQUIRE_LINUX_SANDBOX"),
            Some(OsString::from("1")),
            "CI requires namespace entry support"
        );
        eprintln!("skipped: setns is unavailable");
        return;
    }

    let mut helper = ChildGuard(
        NativeCommand::new(&targets().transparent)
            .arg("unshare-net-hold")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut report = String::new();
    BufReader::new(helper.0.stdout.take().unwrap()).read_line(&mut report).unwrap();
    if let Some(error) = report.trim().strip_prefix("unshare_error=") {
        assert_ne!(
            std::env::var_os("ZYGOTE_REQUIRE_LINUX_SANDBOX"),
            Some(OsString::from("1")),
            "CI requires creation of a distinct network namespace: errno {error}"
        );
        eprintln!("skipped: host forbids creating a network namespace: errno {error}");
        return;
    }
    let target_identity = report.trim().strip_prefix("netns=").unwrap().to_owned();
    let current = fs::metadata("/proc/self/ns/net").unwrap();
    let current_identity = format!("{}:{}", current.dev(), current.ino());
    assert_ne!(target_identity, current_identity);

    let descriptor = fs::File::open(format!("/proc/{}/ns/net", helper.0.id())).unwrap();
    let namespace = Namespace::new(NamespaceKind::Network, descriptor.into()).unwrap();
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let output = zygote.command().arg("report").namespace(namespace).unwrap().output();
    match output {
        Ok(output) => {
            assert!(output.status.success());
            let child = fields(&output.stdout);
            assert_eq!(child["netns"], target_identity);
            assert_ne!(child["netns"], current_identity);
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            assert_ne!(
                std::env::var_os("ZYGOTE_REQUIRE_LINUX_SANDBOX"),
                Some(OsString::from("1")),
                "CI requires permission to enter the selected namespace"
            );
            eprintln!("skipped: host forbids entering the distinct network namespace");
        }
        Err(error) => panic!("distinct namespace entry failed: {error}"),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn landlock_enforces_path_rules_or_is_explicitly_skipped() {
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    if LinuxSupport::probe().landlock_abi.is_none() {
        assert_ne!(
            std::env::var_os("ZYGOTE_REQUIRE_LANDLOCK"),
            Some(OsString::from("1")),
            "CI requires Landlock support"
        );
        eprintln!("skipped: Landlock is unavailable");
        return;
    }
    let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/zygote-landlock");
    fs::create_dir_all(&parent).unwrap();
    let root = parent.join(format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let _cleanup = Cleanup(root.clone());
    let allowed = root.join("allowed");
    let denied = root.join("denied");
    fs::create_dir_all(&allowed).unwrap();
    fs::create_dir_all(&denied).unwrap();
    let allowed_file = fs::File::open(&allowed).unwrap();
    let rights = LandlockAccess::WRITE_FILE | LandlockAccess::MAKE_REGULAR;
    let mut ruleset = match LandlockRuleset::create(rights) {
        Ok(ruleset) => ruleset,
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP)) => {
            assert_ne!(
                std::env::var_os("ZYGOTE_REQUIRE_LANDLOCK"),
                Some(OsString::from("1")),
                "CI requires Landlock rulesets"
            );
            eprintln!("skipped: Landlock is unavailable: {error}");
            return;
        }
        Err(error) => panic!("Landlock ruleset creation failed: {error}"),
    };
    ruleset.add_path_beneath(allowed_file.as_fd(), rights).unwrap();
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let allowed_path = allowed.join("created");
    let denied_path = denied.join("created");
    assert!(!denied_path.exists());
    let mut command = zygote.command();
    command
        .args([OsStr::new("touch-pair"), allowed_path.as_os_str(), denied_path.as_os_str()])
        .privilege_policy(PrivilegePolicy::builder().no_new_privileges(true).build().unwrap())
        .landlock(ruleset);
    let output = command.output().unwrap();
    assert!(output.status.success());
    let report = fields(&output.stdout);
    assert_eq!(report["allowed"], "Ok(())");
    assert_eq!(report["denied"], "Err(PermissionDenied)");
    assert_eq!(fs::read(&allowed_path).unwrap(), b"created");
    assert!(!denied_path.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn cgroup_membership_is_verified_or_explicitly_skipped() {
    if !LinuxSupport::probe().cgroup_v2 {
        assert_ne!(
            std::env::var_os("ZYGOTE_REQUIRE_LINUX_SANDBOX"),
            Some(OsString::from("1")),
            "CI requires cgroup v2"
        );
        eprintln!("skipped: cgroup v2 is unavailable");
        return;
    }
    let group = Path::new("/sys/fs/cgroup").join(format!("zygote-control-test-{}", std::process::id()));
    if let Err(error) = fs::create_dir(&group) {
        assert_ne!(
            std::env::var_os("ZYGOTE_REQUIRE_LINUX_SANDBOX"),
            Some(OsString::from("1")),
            "CI requires a delegated writable cgroup v2 hierarchy"
        );
        eprintln!("skipped: cannot create delegated cgroup: {error}");
        return;
    }
    let wrong = fs::OpenOptions::new().write(true).open(group.join("cgroup.freeze")).unwrap();
    assert_eq!(CgroupMembership::new(&wrong).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    let directory = fs::File::open(&group).unwrap();
    let membership = CgroupMembership::new(&directory).unwrap();
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let output = zygote.command().arg("report").cgroup(membership).output().unwrap();
    assert!(output.status.success());
    assert!(fields(&output.stdout)["cgroup"].contains(group.file_name().unwrap().to_str().unwrap()));
    fs::remove_dir(group).unwrap();
}

#[cfg(windows)]
#[test]
fn portable_privilege_reduction_lowers_windows_integrity() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let output = zygote
        .command()
        .arg("report")
        .sandbox(SandboxPolicy::new().reduce_privileges())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(fields(&output.stdout)["integrity"], "4096");
}

#[cfg(windows)]
#[test]
fn native_token_policy_cannot_weaken_portable_privilege_reduction() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let token = RestrictedTokenPolicy::builder()
        .integrity(Some(IntegrityLevel::Medium))
        .build()
        .unwrap();
    let output = zygote
        .command()
        .arg("report")
        .sandbox(SandboxPolicy::new().reduce_privileges())
        .restricted_token(token)
        .unwrap()
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(fields(&output.stdout)["integrity"], "4096");
}

#[cfg(windows)]
#[test]
fn windows_native_sandbox_controls_launch_before_entry() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let token = RestrictedTokenPolicy::builder()
        .integrity(Some(IntegrityLevel::Low))
        .build()
        .unwrap();
    let job = JobPolicy::builder()
        .active_process_limit(Some(1))
        .kill_on_close(true)
        .build()
        .unwrap();
    let output = zygote
        .command()
        .arg("report")
        .restricted_token(token)
        .unwrap()
        .job(job)
        .unwrap()
        .mitigations(MitigationPolicy::default().dep().prohibit_dynamic_code())
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = fields(&output.stdout);
    assert_eq!(report["integrity"], "4096");
    assert_eq!(report["job"], "1");
    assert_eq!(report["dep"], "1");
    assert_eq!(report["dynamic_code"], "1");
    assert_eq!(report["child_spawn"], "denied");
}

#[cfg(windows)]
#[test]
#[expect(
    clippy::items_after_statements,
    clippy::maybe_infinite_iter,
    clippy::multiple_unsafe_ops_per_block,
    reason = "the privileged Windows fixture keeps its one-shot FFI profile lifecycle local to the opt-in test"
)]
fn appcontainer_policy_is_visible_in_the_launched_child_token() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetLengthSid, PSID};
    if std::env::var_os("ZYGOTE_TEST_APPCONTAINER") != Some(OsString::from("1")) {
        eprintln!("skipped: AppContainer profile creation and fixture ACLs require a provisioned Windows test host");
        return;
    }
    #[link(name = "userenv")]
    unsafe extern "system" {
        fn CreateAppContainerProfile(
            name: *const u16,
            display_name: *const u16,
            description: *const u16,
            capabilities: *const windows_sys::Win32::Security::SID_AND_ATTRIBUTES,
            capability_count: u32,
            sid: *mut PSID,
        ) -> i32;
        fn DeleteAppContainerProfile(name: *const u16) -> i32;
    }

    let name: Vec<u16> = OsStr::new(&format!(
        "zygote-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();
    let mut raw_sid: PSID = std::ptr::null_mut();
    // SAFETY: the profile name is nul-terminated and the SID output is writable.
    let result = unsafe { CreateAppContainerProfile(name.as_ptr(), name.as_ptr(), name.as_ptr(), std::ptr::null(), 0, &raw mut raw_sid) };
    assert_eq!(result, 0, "unable to create AppContainer test profile: HRESULT {result:#x}");
    struct Profile {
        name: Vec<u16>,
        sid: OsString,
        fixture_directory: PathBuf,
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            let status = NativeCommand::new("icacls")
                .arg(&self.fixture_directory)
                .args(["/remove"])
                .arg(format!("*{}", self.sid.to_string_lossy()))
                .args(["/T", "/C", "/Q"])
                .status()
                .unwrap();
            assert!(status.success());
            // SAFETY: the profile name stays nul-terminated until deletion.
            assert_eq!(unsafe { DeleteAppContainerProfile(self.name.as_ptr()) }, 0);
        }
    }
    // SAFETY: a successful CreateAppContainerProfile returns a valid SID allocated by LocalAlloc.
    let (sid, sid_string) = unsafe {
        let mut sid_text = std::ptr::null_mut();
        assert_ne!(ConvertSidToStringSidW(raw_sid, &raw mut sid_text), 0);
        let sid_len = (0..).find(|index| *sid_text.add(*index) == 0).unwrap();
        let sid_string = OsString::from_wide(std::slice::from_raw_parts(sid_text, sid_len));
        LocalFree(sid_text.cast());
        let bytes = std::slice::from_raw_parts(raw_sid.cast::<u8>(), GetLengthSid(raw_sid) as usize);
        let sid = Sid::from_bytes(bytes).unwrap();
        LocalFree(raw_sid);
        (sid, sid_string)
    };
    let fixture_directory = targets().transparent.parent().unwrap().to_owned();
    let status = NativeCommand::new("icacls")
        .arg(&fixture_directory)
        .args(["/grant"])
        .arg(format!("*{}:(OI)(CI)RX", sid_string.to_string_lossy()))
        .args(["/T", "/C", "/Q"])
        .status()
        .unwrap();
    assert!(status.success(), "unable to grant the AppContainer profile access to test fixtures");
    let _profile = Profile {
        name,
        sid: sid_string,
        fixture_directory,
    };
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let mut command = zygote.command();
    command
        .arg("report")
        .appcontainer(AppContainerPolicy {
            appcontainer_sid: sid,
            capabilities: Vec::new(),
        })
        .unwrap();
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert_eq!(fields(&output.stdout)["appcontainer"], "1");
}

#[cfg(windows)]
#[test]
fn windows_exit_259_and_empty_environment_are_preserved() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let status = zygote.command().args(["exit", "259"]).status().unwrap();
    assert_eq!(status.code(), Some(259));

    let output = zygote.command().arg("report").env_clear().output().unwrap();
    assert_eq!(fields(&output.stdout)["env"], "<unset>");
}

#[cfg(windows)]
#[test]
fn windows_environment_updates_ignore_name_case() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let output = zygote
        .command()
        .arg("report")
        .env_clear()
        .env("zygote_test_value", "first")
        .env("ZYGOTE_TEST_VALUE", "second")
        .output()
        .unwrap();
    assert_eq!(fields(&output.stdout)["env"], hex(OsStr::new("second")));

    let output = zygote
        .command()
        .arg("report")
        .env("Zygote_Test_Value", "secret")
        .env_remove("ZYGOTE_TEST_VALUE")
        .output()
        .unwrap();
    assert_eq!(fields(&output.stdout)["env"], "<unset>");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn seccomp_filter_is_active_before_started() {
    let policy = SeccompPolicy::new(
        AuditArchitecture::X86_64,
        vec![
            SeccompInstruction {
                code: 0x20,
                jump_true: 0,
                jump_false: 0,
                value: 4,
            },
            SeccompInstruction {
                code: 0x15,
                jump_true: 1,
                jump_false: 0,
                value: 0xc000_003e,
            },
            SeccompInstruction {
                code: 0x06,
                jump_true: 0,
                jump_false: 0,
                value: 0x8000_0000,
            },
            SeccompInstruction {
                code: 0x20,
                jump_true: 0,
                jump_false: 0,
                value: 0,
            },
            SeccompInstruction {
                code: 0x15,
                jump_true: 0,
                jump_false: 1,
                value: u32::try_from(libc::SYS_getppid).unwrap(),
            },
            SeccompInstruction {
                code: 0x06,
                jump_true: 0,
                jump_false: 0,
                value: 0x0005_0000 | libc::EPERM as u32,
            },
            SeccompInstruction {
                code: 0x06,
                jump_true: 0,
                jump_false: 0,
                value: 0x7fff_0000,
            },
        ],
    )
    .unwrap();
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let mut command = zygote.command();
    command
        .arg("getppid")
        .privilege_policy(PrivilegePolicy::builder().no_new_privileges(true).build().unwrap())
        .seccomp(policy);
    let output = fields(&command.output().unwrap().stdout);
    assert_eq!(output["result"], "-1");
    assert_eq!(output["fd3"], "closed");
}

#[cfg(unix)]
#[test]
fn unprivileged_unix_options_are_applied() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    // SAFETY: getgid has no preconditions.
    let gid = unsafe { libc::getgid() };
    let mut command = zygote.command();
    command
        .args(["report", "unix-options"])
        .env_clear()
        .arg0("custom-argv-zero")
        .process_group(0)
        .umask(0o027)
        .resource_limit(ResourceLimit {
            resource: Resource::OpenFiles,
            soft: 32,
            hard: 32,
        })
        .resource_limit(ResourceLimit {
            resource: Resource::OpenFiles,
            soft: 64,
            hard: 64,
        });
    let output = command.output().unwrap();
    assert!(output.status.success());
    let output = fields(&output.stdout);
    assert!(output["argv"].starts_with(&hex(OsStr::new("custom-argv-zero"))));
    assert_eq!(output["uid"], uid.to_string());
    assert_eq!(output["gid"], gid.to_string());
    assert_eq!(output["pgrp"], output["pid"]);
    assert_eq!(output["umask"], "27");
    assert_eq!(output["nofile"], "64:64");

    let mut session = zygote.command();
    session.arg("report").new_session();
    let session = fields(&session.output().unwrap().stdout);
    assert_eq!(session["pgrp"], session["pid"]);
    assert_eq!(session["session"], session["pid"]);

    let mut inherited_session = zygote.command();
    inherited_session.arg("report").new_session().inherit_session();
    let inherited_session = fields(&inherited_session.output().unwrap().stdout);
    assert_ne!(inherited_session["session"], inherited_session["pid"]);
}

#[cfg(unix)]
#[test]
fn cwd_file_stdio_and_umask_are_observable() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let artifact_dir = manifest_dir.join("../../target/zygote-test-artifacts");
    fs::create_dir_all(&artifact_dir).unwrap();
    let created = artifact_dir.join(format!("created-{}", std::process::id()));
    let captured = artifact_dir.join(format!("captured-{}", std::process::id()));
    let _ = fs::remove_file(&created);
    let _ = fs::remove_file(&captured);

    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let report = zygote
        .command()
        .arg("report")
        .current_dir(manifest_dir.join("test-fixtures/targets"))
        .output()
        .unwrap();
    let report = fields(&report.stdout);
    assert_eq!(
        report["cwd"],
        hex(manifest_dir.join("test-fixtures/targets").canonicalize().unwrap().as_os_str())
    );

    let mut touch = zygote.command();
    touch.args([OsStr::new("touch"), created.as_os_str()]).umask(0o027);
    let touched = touch.output().unwrap();
    assert_eq!(fields(&touched.stdout)["mode"], "640");

    let file = fs::File::create(&captured).unwrap();
    let status = zygote.command().args(["echo"]).stdin(Stdio::null()).stdout(file).status().unwrap();
    assert!(status.success());
    assert_eq!(fs::read(&captured).unwrap(), b"stdout:");

    fs::remove_file(created).unwrap();
    fs::remove_file(captured).unwrap();
}

#[test]
fn explicit_shutdown_rejects_later_launcher_requests() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let launcher = zygote.launcher();
    zygote.shutdown().unwrap();
    let error = launcher.command().arg("report").spawn().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn dropping_zygote_rejects_later_launcher_requests() {
    let launcher = {
        let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
        zygote.launcher()
    };
    let error = launcher.command().arg("report").spawn().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[cfg(target_os = "linux")]
#[test]
fn detached_child_can_be_signalled_after_template_shutdown() {
    let zygote = Zygote::builder(&targets().transparent).spawn().unwrap();
    let mut child = zygote.command().arg("sleep").spawn().unwrap();
    let pid = child.id();
    zygote.shutdown().unwrap();

    assert_eq!(child.try_wait().unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);

    let process = Path::new("/proc").join(pid.to_string());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while process.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!process.exists(), "detached child {pid} was not reaped after termination");
}
