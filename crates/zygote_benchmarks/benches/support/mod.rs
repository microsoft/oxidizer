// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    dead_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::panic,
    clippy::unwrap_used,
    reason = "each benchmark imports a different subset and should fail immediately when its controlled fixture fails"
)]

use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as NativeCommand, Stdio as NativeStdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};

use zygote_control::{Command, Launcher, Stdio, Zygote};

pub(crate) const TRANSPARENT: &str = env!("CARGO_BIN_EXE_zygote_bench_transparent");
pub(crate) const PREPARED_EMPTY: &str = env!("CARGO_BIN_EXE_zygote_bench_prepared_0");
pub(crate) const PREPARED_1_MIB: &str = env!("CARGO_BIN_EXE_zygote_bench_prepared_1m");
pub(crate) const PREPARED_64_MIB: &str = env!("CARGO_BIN_EXE_zygote_bench_prepared_64m");
pub(crate) const CLOSE_FALLBACK_512: &str = env!("CARGO_BIN_EXE_zygote_bench_transparent_close_fallback_512");
pub(crate) const CLOSE_FALLBACK_4096: &str = env!("CARGO_BIN_EXE_zygote_bench_transparent_close_fallback_4096");
pub(crate) const SIGNALS_IGNORED: &str = env!("CARGO_BIN_EXE_zygote_bench_transparent_signals_ignored");
pub(crate) const SIGNALS_CUSTOM: &str = env!("CARGO_BIN_EXE_zygote_bench_transparent_signals_custom");

#[derive(Clone, Copy)]
pub(crate) enum InputShape {
    Empty,
    Representative,
    Large,
}

impl InputShape {
    pub(crate) const ALL: [Self; 3] = [Self::Empty, Self::Representative, Self::Large];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Representative => "representative",
            Self::Large => "large",
        }
    }

    pub(crate) fn apply_zygote(self, command: &mut Command) {
        match self {
            Self::Empty => {
                command.env_clear();
            }
            Self::Representative => {
                command
                    .args(["alpha", "--count=16", "path/with/a/component"])
                    .env_clear()
                    .env("BENCH_KIND", "representative")
                    .env("BENCH_TOKEN", "0123456789abcdef");
            }
            Self::Large => {
                command.env_clear();
                #[cfg(windows)]
                {
                    for index in 0..8 {
                        command.arg(format!("argument-{index:03}-{}", "a".repeat(2_048)));
                        command.env(format!("BENCH_{index:03}"), "e".repeat(1_024));
                    }
                }
                #[cfg(not(windows))]
                {
                    for index in 0..128 {
                        command.arg(format!("argument-{index:03}-{}", "a".repeat(4_000)));
                    }
                    for index in 0..64 {
                        command.env(format!("BENCH_{index:03}"), "e".repeat(4_000));
                    }
                }
            }
        }
    }

    pub(crate) fn apply_native(self, command: &mut NativeCommand) {
        match self {
            Self::Empty => {
                command.env_clear();
            }
            Self::Representative => {
                command
                    .args(["alpha", "--count=16", "path/with/a/component"])
                    .env_clear()
                    .env("BENCH_KIND", "representative")
                    .env("BENCH_TOKEN", "0123456789abcdef");
            }
            Self::Large => {
                command.env_clear();
                #[cfg(windows)]
                {
                    for index in 0..8 {
                        command.arg(format!("argument-{index:03}-{}", "a".repeat(2_048)));
                        command.env(format!("BENCH_{index:03}"), "e".repeat(1_024));
                    }
                }
                #[cfg(not(windows))]
                {
                    for index in 0..128 {
                        command.arg(format!("argument-{index:03}-{}", "a".repeat(4_000)));
                    }
                    for index in 0..64 {
                        command.env(format!("BENCH_{index:03}"), "e".repeat(4_000));
                    }
                }
            }
        }
    }

    pub(crate) fn payload_bytes(self) -> usize {
        let mut command = NativeCommand::new(TRANSPARENT);
        self.apply_native(&mut command);
        command.get_args().map(os_payload_bytes).sum::<usize>()
            + command
                .get_envs()
                .map(|(key, value)| os_payload_bytes(key) + value.map_or(0, os_payload_bytes))
                .sum::<usize>()
    }
}

fn os_payload_bytes(value: &OsStr) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        value.encode_wide().count() * 2
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;

        value.as_bytes().len()
    }
    #[cfg(not(any(unix, windows)))]
    {
        value.to_string_lossy().len()
    }
}

pub(crate) fn start(program: impl AsRef<OsStr>, workers: usize) -> Zygote {
    let mut builder = Zygote::builder(program);
    builder.workers(workers).unwrap();
    builder.spawn().unwrap()
}

pub(crate) fn accelerated_exit(launcher: &Launcher, input: InputShape) -> u64 {
    let mut command = launcher.command();
    input.apply_zygote(&mut command);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let status = command.status().unwrap();
    assert!(status.success());
    status_code(status)
}

pub(crate) fn accelerated_entry(launcher: &Launcher, input: InputShape) -> u64 {
    let mut command = launcher.command();
    command.arg("entry");
    input.apply_zygote(&mut command);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let mut marker = [0_u8; 1];
    child.stdout.as_mut().unwrap().read_exact(&mut marker).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    u64::from(marker[0])
}

pub(crate) fn native_exit(program: impl AsRef<OsStr>, input: InputShape) -> u64 {
    let mut command = NativeCommand::new(program);
    input.apply_native(&mut command);
    let status = command
        .stdin(NativeStdio::null())
        .stdout(NativeStdio::null())
        .stderr(NativeStdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    status_code(status)
}

pub(crate) fn native_entry(program: impl AsRef<OsStr>, input: InputShape) -> u64 {
    let mut command = NativeCommand::new(program);
    command.arg("entry");
    input.apply_native(&mut command);
    let mut child = command
        .stdin(NativeStdio::null())
        .stdout(NativeStdio::piped())
        .stderr(NativeStdio::null())
        .spawn()
        .unwrap();
    let mut marker = [0_u8; 1];
    child.stdout.as_mut().unwrap().read_exact(&mut marker).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    u64::from(marker[0])
}

fn status_code(status: std::process::ExitStatus) -> u64 {
    u64::try_from(status.code().unwrap()).unwrap()
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcessMemory {
    pub minor_faults: u64,
    pub major_faults: u64,
    pub rss_kib: u64,
    pub pss_kib: u64,
    pub private_dirty_kib: u64,
    pub descriptors: usize,
    pub threads: u64,
}

impl ProcessMemory {
    fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "minor_faults": self.minor_faults,
            "major_faults": self.major_faults,
            "rss_kib": self.rss_kib,
            "pss_kib": self.pss_kib,
            "private_dirty_kib": self.private_dirty_kib,
            "descriptors": self.descriptors,
            "threads": self.threads,
        })
    }
}

pub(crate) fn write_memory_observations(observations: &[(&str, &str, ProcessMemory)]) {
    let records = observations
        .iter()
        .map(|(fixture, launcher, memory)| {
            serde_json::json!({
                "name": format!("{fixture}/{launcher}"),
                "fixture": fixture,
                "launcher": launcher,
                "metrics": memory.to_json(),
            })
        })
        .collect::<Vec<_>>();
    let report = serde_json::json!({
        "schema_version": 1,
        "unit": {
            "minor_faults": "count",
            "major_faults": "count",
            "rss_kib": "KiB",
            "pss_kib": "KiB",
            "private_dirty_kib": "KiB",
            "descriptors": "count",
            "threads": "count",
        },
        "observations": records,
    });
    let report_path = std::env::var_os("ZYGOTE_MEMORY_REPORT_PATH").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/zygote-benchmarks/memory-observations.json"),
        workspace_path,
    );
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();

    match (
        std::env::var_os("ZYGOTE_MEMORY_BASELINE_PATH"),
        std::env::var("ZYGOTE_MEMORY_MAX_REGRESSION_PERCENT").ok(),
    ) {
        (None, None) => {}
        (Some(baseline), Some(percent)) => {
            let percent = percent.parse::<f64>().unwrap();
            assert!(percent >= 0.0 && percent.is_finite());
            check_memory_regressions(
                &report,
                &serde_json::from_slice(&fs::read(workspace_path(baseline)).unwrap()).unwrap(),
                percent,
            );
        }
        _ => panic!("ZYGOTE_MEMORY_BASELINE_PATH and ZYGOTE_MEMORY_MAX_REGRESSION_PERCENT must be set together"),
    }
    eprintln!("memory observations written to {}", report_path.display());
}

fn workspace_path(path: impl Into<PathBuf>) -> PathBuf {
    let path = path.into();
    if path.is_absolute() {
        path
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
    }
}

fn check_memory_regressions(current: &serde_json::Value, baseline: &serde_json::Value, maximum_percent: f64) {
    const METRICS: [&str; 7] = [
        "minor_faults",
        "major_faults",
        "rss_kib",
        "pss_kib",
        "private_dirty_kib",
        "descriptors",
        "threads",
    ];
    let baseline_records = baseline["observations"].as_array().unwrap();
    for record in current["observations"].as_array().unwrap() {
        let name = record["name"].as_str().unwrap();
        let baseline = baseline_records
            .iter()
            .find(|candidate| candidate["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("memory baseline is missing {name}"));
        for metric in METRICS {
            let value = record["metrics"][metric].as_f64().unwrap();
            let baseline = baseline["metrics"][metric].as_f64().unwrap();
            let limit = baseline * (1.0 + maximum_percent / 100.0);
            assert!(
                value <= limit,
                "{name} {metric} regressed: current {value}, baseline {baseline}, limit {limit} ({maximum_percent}%)"
            );
        }
    }
}

pub(crate) fn observe_accelerated_memory(launcher: &Launcher) -> ProcessMemory {
    let mut command = launcher.command();
    command
        .arg("hold")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let mut marker = [0_u8; 1];
    child.stdout.as_mut().unwrap().read_exact(&mut marker).unwrap();
    assert_eq!(marker, [b'R']);
    let observation = observe_process(child.id());
    child.stdin.as_mut().unwrap().write_all(&[0]).unwrap();
    assert!(child.wait().unwrap().success());
    observation
}

pub(crate) fn observe_native_memory(program: impl AsRef<OsStr>) -> ProcessMemory {
    let mut child = NativeCommand::new(program)
        .arg("hold")
        .stdin(NativeStdio::piped())
        .stdout(NativeStdio::piped())
        .stderr(NativeStdio::null())
        .spawn()
        .unwrap();
    let mut marker = [0_u8; 1];
    child.stdout.as_mut().unwrap().read_exact(&mut marker).unwrap();
    assert_eq!(marker, [b'R']);
    let observation = observe_process(child.id());
    child.stdin.as_mut().unwrap().write_all(&[0]).unwrap();
    assert!(child.wait().unwrap().success());
    observation
}

fn observe_process(pid: u32) -> ProcessMemory {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields = stat.rsplit_once(") ").unwrap().1.split_ascii_whitespace().collect::<Vec<_>>();
    let smaps = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap();
    ProcessMemory {
        minor_faults: fields[7].parse().unwrap(),
        major_faults: fields[9].parse().unwrap(),
        rss_kib: smaps_value(&smaps, "Rss:"),
        pss_kib: smaps_value(&smaps, "Pss:"),
        private_dirty_kib: smaps_value(&smaps, "Private_Dirty:"),
        descriptors: fs::read_dir(format!("/proc/{pid}/fd")).unwrap().count(),
        threads: fs::read_dir(format!("/proc/{pid}/task")).unwrap().count().try_into().unwrap(),
    }
}

fn smaps_value(smaps: &str, key: &str) -> u64 {
    smaps
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap()
        .split_ascii_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[cfg(target_os = "linux")]
pub(crate) fn observe_controller_and_templates() {
    let controller = observe_process(std::process::id());
    eprintln!("controller pid={} {controller:?}", std::process::id());
    let mut workers = Vec::new();
    for task in fs::read_dir("/proc/self/task").unwrap() {
        let task = task.unwrap();
        let children = fs::read_to_string(task.path().join("children")).unwrap();
        for child in children.split_ascii_whitespace() {
            let pid: u32 = child.parse().unwrap();
            workers.push(pid);
        }
    }
    workers.sort_unstable();
    workers.dedup();
    for pid in workers {
        eprintln!("controller direct child (template or fixture) pid={pid} {:?}", observe_process(pid));
    }
}

pub(crate) struct ConcurrentRunner {
    start: Arc<Barrier>,
    finish: Arc<Barrier>,
    stop: Arc<AtomicBool>,
    checksum: Arc<AtomicU64>,
    threads: Vec<JoinHandle<()>>,
}

impl ConcurrentRunner {
    pub(crate) fn new<F>(thread_count: usize, operation: F) -> Self
    where
        F: Fn(usize) -> u64 + Send + Sync + 'static,
    {
        let start = Arc::new(Barrier::new(thread_count + 1));
        let finish = Arc::new(Barrier::new(thread_count + 1));
        let stop = Arc::new(AtomicBool::new(false));
        let checksum = Arc::new(AtomicU64::new(0));
        let operation = Arc::new(operation);
        let threads = (0..thread_count)
            .map(|index| {
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                let stop = Arc::clone(&stop);
                let checksum = Arc::clone(&checksum);
                let operation = Arc::clone(&operation);
                thread::Builder::new()
                    .name(format!("zygote-bench-{index}"))
                    .spawn(move || {
                        loop {
                            start.wait();
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            checksum.fetch_add(operation(index), Ordering::Relaxed);
                            finish.wait();
                        }
                    })
                    .unwrap()
            })
            .collect();
        Self {
            start,
            finish,
            stop,
            checksum,
            threads,
        }
    }

    pub(crate) fn run(&self) -> u64 {
        self.start.wait();
        self.finish.wait();
        self.checksum.load(Ordering::Relaxed)
    }
}

impl Drop for ConcurrentRunner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.start.wait();
        for thread in self.threads.drain(..) {
            thread.join().unwrap();
        }
    }
}

pub(crate) fn fixture_working_directory() -> PathBuf {
    Path::new(TRANSPARENT).parent().unwrap().to_owned()
}
