// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use alloc_tracker::Session;
use command_group::{CommandGroup, GroupChild};
use criterion::Criterion;

use crate::arguments::{
    Arguments, FailureMode, PERF_ACK_ENV, PERF_CONTROL_ENV, WORKER_BENCHMARK_ENV, WORKER_FILTERS_ENV, WORKER_MODE_ENV, WORKER_TOKEN_ENV,
};
#[cfg(all(target_os = "linux", not(miri)))]
use crate::artifact::perf_path;
use crate::artifact::{self, ARTIFACT_DIR_ENV, ArtifactDirectory, allocation_path, parse_allocations, parse_perf, write_allocations};
use crate::bencher::MeasurementControl;
use crate::report::{BenchmarkEntry, BenchmarkReport};
use crate::{Bencher, BenchmarkSuite, Error, Mode};

const ALLOCATION_SAMPLES: u64 = 1;
const HELP: &str = "\
Usage: BENCHMARK [METABENCH OPTIONS] [ENGINE OPTIONS]

Engines:
  --criterion              Filter the run to Criterion benchmarks
  --callgrind              Filter the run to Callgrind benchmarks
  --perf                   Filter the run to Linux perf benchmarks
  --allocations            Filter the run to allocation benchmarks
  --all-engines            Run every engine selected by benchmark code

Engine options (repeat once per argument):
  --criterion-arg ARG
  --callgrind-arg ARG
  --perf-arg ARG

Metabench options:
  --export-md PATH          Override the default Markdown report path
  --export-json PATH        Override the default JSON report path
  --output PATH             Override both paths with PATH.md and PATH.json
  --no-output               Do not write report files
  --baseline PATH           Override the previous report used as the baseline
  --no-baseline             Do not compare against an existing report
  --filter GLOB             Run matching group/benchmark/case identities
  --list                    List matching benchmarks without running
  --timeout DURATION        Limit each engine worker (for example 30s or 5m)
  --fail-fast               Stop after the first engine failure
  --keep-going              Continue through engine failures (default)
  --regression-threshold PERCENT
  --show-engine-output      Show raw child-engine stdout and stderr
  -h, --help

With no filter, benchmark code determines which engines run. Individual
selectors may be combined and run in command-line order.
Arguments after -- are accepted only when exactly one engine is selected.
";
#[derive(Clone, Copy)]
struct WorkerOptions<'a> {
    artifact_root: Option<&'a std::path::Path>,
    show_engine_output: bool,
    filters: &'a [String],
    timeout: Option<Duration>,
}

/// Runs a metabench benchmark executable.
///
/// # Errors
///
/// Returns an error when arguments are invalid, a requested backend is
/// unavailable, or a worker process cannot be launched or exits unsuccessfully.
pub(crate) fn run(register: fn(&mut BenchmarkSuite), benchmark_target: &'static str) -> Result<(), Error> {
    if let Some(worker_mode) = env::var_os(WORKER_MODE_ENV) {
        consume_worker_token()?;
        let worker_mode = worker_mode
            .to_str()
            .ok_or_else(|| Error::InvalidWorkerMode(worker_mode.to_string_lossy().into_owned()))?
            .parse()
            .map_err(|_error| Error::InvalidWorkerMode(worker_mode.to_string_lossy().into_owned()))?;
        return run_worker(worker_mode, register);
    }

    let arguments = Arguments::parse()?;
    if arguments.help {
        std::io::stdout().write_all(HELP.as_bytes()).map_err(Error::HelpOutput)?;
        return Ok(());
    }
    run_parent(&arguments, register, benchmark_target)
}

/// Dispatches Gungraun controller and runner-child invocations before normal modes.
///
/// # Errors
///
/// Returns an error when the metabench worker protocol or an ordinary benchmark
/// mode fails.
#[doc(hidden)]
pub fn run_with_gungraun(register: fn(&mut BenchmarkSuite), gungraun_main: fn(), benchmark_target: &'static str) -> Result<(), Error> {
    if env::args_os().nth(1).as_deref() == Some(OsStr::new("--gungraun-run")) {
        gungraun_main();
        return Ok(());
    }

    if env::var_os(WORKER_MODE_ENV).as_deref() == Some(OsStr::new("callgrind")) {
        consume_worker_token()?;
        gungraun_main();
        return Ok(());
    }

    run(register, benchmark_target)
}

fn run_parent(arguments: &Arguments, register: fn(&mut BenchmarkSuite), benchmark_target: &str) -> Result<(), Error> {
    let executable = env::current_exe().map_err(Error::CurrentExecutable)?;
    let mut benchmarks = BenchmarkSuite::register(register)?;
    apply_parent_filters(&mut benchmarks, &arguments.filters)?;

    if arguments.explicit_engine_filter {
        for &mode in &arguments.modes {
            if !benchmarks.contains_mode(mode) {
                return Err(Error::EngineNotConfigured(mode.as_str()));
            }
        }
    }
    let modes = arguments
        .modes
        .iter()
        .copied()
        .filter(|mode| benchmarks.contains_mode(*mode))
        .filter(|mode| !(!arguments.explicit_engine_filter && !mode_supported(*mode, cfg!(target_os = "linux"))))
        .collect::<Vec<_>>();

    validate_backend_arguments(arguments, &modes)?;
    if arguments.behavior.list {
        return list_benchmarks(&benchmarks, &modes);
    }
    let artifacts = ArtifactDirectory::create()?;
    let worker_options = WorkerOptions {
        artifact_root: Some(artifacts.path()),
        show_engine_output: arguments.behavior.show_engine_output,
        filters: &arguments.filters,
        timeout: arguments.timeout,
    };

    let mut failures = Vec::new();
    let mut completed_modes = Vec::new();
    let multiple_engines = modes.len() > 1;
    for mode in modes {
        if !mode_supported(mode, cfg!(target_os = "linux")) {
            return Err(Error::UnsupportedMode {
                mode: mode.as_str(),
                reason: "Linux perf is supported only on Linux",
            });
        }

        let result = if mode == Mode::Perf {
            let mut result = Ok(());
            for (group, benchmark) in benchmarks
                .benchmarks()
                .filter(|(_, benchmark)| benchmark.engines.contains_mode(mode))
            {
                let benchmark_id = benchmark.identity(group);
                print_progress(&benchmark_id, mode)?;
                result = launch_worker(mode, &executable, arguments.args_for(mode), Some(&benchmark_id), worker_options)
                    .and_then(|status| ensure_success(mode, status));
                if result.is_err() {
                    break;
                }
            }
            result
        } else {
            for benchmark in benchmark_names_for(&benchmarks, mode) {
                print_progress(&benchmark, mode)?;
            }
            launch_worker(mode, &executable, arguments.args_for(mode), None, worker_options).and_then(|status| ensure_success(mode, status))
        };

        record_mode_result(
            mode,
            result,
            multiple_engines,
            arguments.behavior.failure_mode,
            &mut completed_modes,
            &mut failures,
        )?;
    }

    if !failures.is_empty() {
        return Err(Error::MultipleModeFailures(failures));
    }

    let report = build_report(&artifacts, &benchmarks, &completed_modes, arguments, benchmark_target)?;
    write_console_report(&report, arguments, benchmark_target)
}

const fn mode_supported(mode: Mode, is_linux: bool) -> bool {
    !matches!(mode, Mode::Perf) || is_linux
}

fn record_mode_result(
    mode: Mode,
    result: Result<(), Error>,
    multiple_engines: bool,
    failure_mode: FailureMode,
    completed_modes: &mut Vec<Mode>,
    failures: &mut Vec<String>,
) -> Result<(), Error> {
    match result {
        Ok(()) => {
            completed_modes.push(mode);
            Ok(())
        }
        Err(error) if multiple_engines && failure_mode == FailureMode::KeepGoing => {
            failures.push(error.to_string());
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn write_console_report(report: &BenchmarkReport, arguments: &Arguments, benchmark_target: &str) -> Result<(), Error> {
    let mut output = report.render_console_table();
    append_generated_paths(&mut output, arguments, benchmark_target)?;
    std::io::stdout().write_all(output.as_bytes()).map_err(Error::ConsoleOutput)
}

fn append_generated_paths(output: &mut String, arguments: &Arguments, benchmark_target: &str) -> Result<(), Error> {
    let (json_path, markdown_path) = report_paths(arguments, benchmark_target)?;
    if let (Some(json_path), Some(markdown_path)) = (json_path, markdown_path) {
        let _ = writeln!(output, "Generated {} and {}", json_path.display(), markdown_path.display());
    }
    Ok(())
}

fn validate_backend_arguments(arguments: &Arguments, modes: &[Mode]) -> Result<(), Error> {
    for &mode in modes {
        if mode == Mode::Allocations {
            let unsupported = arguments.args_for(mode).to_vec();
            if !unsupported.is_empty() {
                return Err(Error::UnsupportedArguments {
                    mode: mode.as_str(),
                    arguments: unsupported,
                });
            }
        }
    }
    Ok(())
}

fn apply_parent_filters(benchmarks: &mut BenchmarkSuite, filters: &[String]) -> Result<(), Error> {
    if benchmarks.retain_filters(filters) {
        Ok(())
    } else {
        Err(Error::NoBenchmarksMatched(filters.to_vec()))
    }
}

fn list_benchmarks(benchmarks: &BenchmarkSuite, modes: &[Mode]) -> Result<(), Error> {
    let entries = benchmarks
        .benchmarks()
        .map(|(group, benchmark)| {
            let engines = modes
                .iter()
                .copied()
                .filter(|mode| benchmark.engines.contains_mode(*mode))
                .map(engine_name)
                .collect::<Vec<_>>()
                .join(", ");
            (benchmark.identity(group), engines)
        })
        .collect::<Vec<_>>();
    let width = entries
        .iter()
        .map(|(identity, _)| identity.len())
        .max()
        .unwrap_or("Benchmark".len())
        .max("Benchmark".len());
    let mut output = format!("{:<width$}  Engines\n{:-<width$}  -------\n", "Benchmark", "");
    for (identity, engines) in entries {
        let _ = writeln!(output, "{identity:<width$}  {engines}");
    }
    std::io::stdout().write_all(output.as_bytes()).map_err(Error::ConsoleOutput)
}

fn build_report(
    artifacts: &ArtifactDirectory,
    benchmarks: &BenchmarkSuite,
    completed_modes: &[Mode],
    arguments: &Arguments,
    benchmark_target: &str,
) -> Result<BenchmarkReport, Error> {
    let benchmark_identities = benchmarks
        .benchmarks()
        .map(|(group, benchmark)| (group.to_owned(), benchmark.name.clone(), benchmark.case.clone()))
        .collect::<Vec<_>>();
    let mut report = BenchmarkReport::new(benchmark_identities)?;
    report.metadata.measured_backends = completed_modes.iter().map(|mode| report_engine_name(*mode).to_owned()).collect();
    let entry_indices = report
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id(), index))
        .collect::<std::collections::HashMap<_, _>>();
    if completed_modes.contains(&Mode::Criterion) {
        let selected = benchmark_names_for(benchmarks, Mode::Criterion);
        let metrics = artifact::parse_criterion(&artifacts.criterion_path())?;
        attach_metrics(&mut report, &entry_indices, "Criterion", &selected, metrics, |entry, value| {
            entry.wall_clock = Some(value);
        })?;
    }
    if completed_modes.contains(&Mode::Callgrind) {
        let selected = benchmark_names_for(benchmarks, Mode::Callgrind);
        let metrics = artifact::parse_callgrind(&artifacts.gungraun_path(), &selected)?;
        attach_metrics(&mut report, &entry_indices, "Callgrind", &selected, metrics, |entry, value| {
            entry.callgrind = Some(value);
        })?;
    }
    if completed_modes.contains(&Mode::Perf) {
        for name in benchmark_names_for(benchmarks, Mode::Perf) {
            let index = entry_indices.get(&name).ok_or_else(|| Error::UnknownBenchmark(name.clone()))?;
            let entry = &mut report.entries[*index];
            entry.perf = Some(parse_perf(&artifacts.perf_path(&name))?);
        }
    }
    if completed_modes.contains(&Mode::Allocations) {
        let selected = benchmark_names_for(benchmarks, Mode::Allocations);
        let allocations = parse_allocations(&artifacts.allocation_path())?;
        attach_metrics(&mut report, &entry_indices, "allocation", &selected, allocations, |entry, value| {
            entry.allocations = Some(value);
        })?;
    }
    let (json_path, markdown_path) = report_paths(arguments, benchmark_target)?;
    let baseline_path = if arguments.automatic_baseline {
        arguments
            .baseline
            .as_deref()
            .or_else(|| json_path.as_deref().filter(|path| path.is_file()))
    } else {
        None
    };
    if let Some(path) = baseline_path {
        let baseline = BenchmarkReport::read_json(path)?;
        report.apply_baseline(&baseline, arguments.regression_threshold)?;
    }
    report.write_reports(json_path.as_deref(), markdown_path.as_deref())?;
    Ok(report)
}

fn report_paths(arguments: &Arguments, benchmark_target: &str) -> Result<(Option<PathBuf>, Option<PathBuf>), Error> {
    if arguments.behavior.no_output {
        return Ok((None, None));
    }
    let root = default_target_directory()?.join("metabench").join(benchmark_target);
    Ok((
        arguments.export_json.clone().or_else(|| Some(root.join("report.json"))),
        arguments.export_markdown.clone().or_else(|| Some(root.join("report.md"))),
    ))
}

fn default_target_directory() -> Result<PathBuf, Error> {
    if let Some(directory) = env::var_os("CARGO_TARGET_DIR") {
        return Ok(PathBuf::from(directory));
    }
    let current = env::current_dir().map_err(Error::CurrentDirectory)?;
    let workspace = current
        .ancestors()
        .find(|directory| is_workspace_manifest(&directory.join("Cargo.toml")))
        .unwrap_or(&current);
    Ok(workspace.join("target"))
}

fn is_workspace_manifest(path: &std::path::Path) -> bool {
    fs::read_to_string(path).is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "[workspace]"))
}

fn print_progress(benchmark: &str, mode: Mode) -> Result<(), Error> {
    let engine = engine_name(mode);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "Benchmarking {benchmark} under {engine}...").map_err(Error::ConsoleOutput)?;
    stdout.flush().map_err(Error::ConsoleOutput)
}

const fn engine_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Criterion => "Criterion",
        Mode::Callgrind => "Gungraun",
        Mode::Perf => "perf",
        Mode::Allocations => "allocations",
    }
}

const fn report_engine_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Criterion => "criterion",
        Mode::Callgrind => "gungraun",
        Mode::Perf => "perf",
        Mode::Allocations => "allocations",
    }
}

fn attach_metrics<T>(
    report: &mut BenchmarkReport,
    entry_indices: &std::collections::HashMap<String, usize>,
    backend: &str,
    benchmark_names: &[String],
    mut metrics: std::collections::BTreeMap<String, T>,
    mut attach: impl FnMut(&mut BenchmarkEntry, T),
) -> Result<(), Error> {
    for benchmark_name in benchmark_names {
        let index = entry_indices
            .get(benchmark_name)
            .ok_or_else(|| Error::UnknownBenchmark(benchmark_name.clone()))?;
        let entry = &mut report.entries[*index];
        let metric = metrics.remove(benchmark_name).ok_or_else(|| Error::ArtifactFormat {
            path: PathBuf::from(backend),
            message: format!("missing {backend} result for {}", entry.id()),
        })?;
        attach(entry, metric);
    }

    if let Some(name) = metrics.into_keys().next() {
        return Err(Error::ArtifactFormat {
            path: PathBuf::from(backend),
            message: format!("unexpected {backend} result for {name}"),
        });
    }
    Ok(())
}

fn benchmark_names_for(benchmarks: &BenchmarkSuite, mode: Mode) -> Vec<String> {
    benchmarks
        .benchmarks()
        .filter(|(_, benchmark)| benchmark.engines.contains_mode(mode))
        .map(|(group, benchmark)| benchmark.identity(group))
        .collect()
}

fn launch_worker(
    mode: Mode,
    executable: &std::path::Path,
    passthrough: &[OsString],
    benchmark: Option<&str>,
    options: WorkerOptions<'_>,
) -> Result<ExitStatus, Error> {
    let mut command = match mode {
        Mode::Criterion | Mode::Callgrind | Mode::Allocations => Command::new(executable),
        Mode::Perf => {
            return launch_perf_worker(executable, passthrough, benchmark, options);
        }
    };

    execute_worker_command(mode, &mut command, passthrough, benchmark, options)
}

fn execute_worker_command(
    mode: Mode,
    command: &mut Command,
    passthrough: &[OsString],
    benchmark: Option<&str>,
    options: WorkerOptions<'_>,
) -> Result<ExitStatus, Error> {
    command.args(passthrough).env(WORKER_MODE_ENV, mode.as_str());
    if let Some(benchmark) = benchmark {
        command.env(WORKER_BENCHMARK_ENV, benchmark);
    }
    if let Some(artifact_root) = options.artifact_root {
        command.env(ARTIFACT_DIR_ENV, artifact_root);
        match mode {
            Mode::Criterion => {
                command
                    .env("CRITERION_HOME", artifact_root.join("criterion"))
                    .env_remove("CARGO_CRITERION_PORT");
            }
            Mode::Callgrind => {
                command
                    .env("GUNGRAUN_HOME", artifact_root.join("gungraun"))
                    .env("GUNGRAUN_SAVE_SUMMARY", "json");
            }
            Mode::Perf | Mode::Allocations => {}
        }
    }
    if !options.show_engine_output {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let filters = serde_json::to_string(options.filters).map_err(|error| Error::InvalidFilter(error.to_string()))?;
    command.env(WORKER_FILTERS_ENV, filters);

    let token = create_worker_token()?;
    command.env(WORKER_TOKEN_ENV, token.path());
    let status = wait_for_worker(mode, command, options.timeout);
    let cleanup = if token.path().exists() {
        fs::remove_dir(token.path()).map_err(Error::ConsumeWorkerToken)
    } else {
        Ok(())
    };
    match (status, cleanup) {
        (Ok(status), Ok(())) => Ok(status),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(worker), Err(cleanup)) => Err(Error::MultipleModeFailures(vec![worker.to_string(), cleanup.to_string()])),
    }
}

fn wait_for_worker(mode: Mode, command: &mut Command, timeout: Option<Duration>) -> Result<ExitStatus, Error> {
    let mut child = command.group_spawn().map_err(|source| Error::Spawn {
        mode: mode.as_str(),
        source,
    })?;
    let mut clock = SystemPollClock { started: Instant::now() };
    wait_for_child(mode, &mut child, timeout, &mut clock)
}

trait WorkerChild {
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill_tree(&mut self) -> std::io::Result<()>;
}

impl WorkerChild for GroupChild {
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Self::wait(self)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Self::try_wait(self)
    }

    fn kill_tree(&mut self) -> std::io::Result<()> {
        Self::kill(self)
    }
}

trait PollClock {
    fn elapsed(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
}

struct SystemPollClock {
    started: Instant,
}

impl PollClock for SystemPollClock {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

fn wait_for_child(
    mode: Mode,
    child: &mut impl WorkerChild,
    timeout: Option<Duration>,
    clock: &mut impl PollClock,
) -> Result<ExitStatus, Error> {
    let Some(timeout) = timeout else {
        return child.wait().map_err(|source| Error::Spawn {
            mode: mode.as_str(),
            source,
        });
    };
    loop {
        if let Some(status) = child.try_wait().map_err(|source| Error::Spawn {
            mode: mode.as_str(),
            source,
        })? {
            return Ok(status);
        }
        if clock.elapsed() >= timeout {
            let kill = child.kill_tree();
            let wait = child.wait();
            return match (kill, wait) {
                (Ok(()), Ok(_)) => Err(Error::WorkerTimedOut {
                    mode: mode.as_str(),
                    timeout,
                }),
                (Err(source), Ok(_)) | (Ok(()), Err(source)) => Err(Error::Spawn {
                    mode: mode.as_str(),
                    source,
                }),
                (Err(kill), Err(wait)) => Err(Error::MultipleModeFailures(vec![
                    Error::Spawn {
                        mode: mode.as_str(),
                        source: kill,
                    }
                    .to_string(),
                    Error::Spawn {
                        mode: mode.as_str(),
                        source: wait,
                    }
                    .to_string(),
                ])),
            };
        }
        clock.sleep(Duration::from_millis(25));
    }
}

#[cfg(all(target_os = "linux", not(miri)))]
fn launch_perf_worker(
    executable: &std::path::Path,
    passthrough: &[OsString],
    benchmark: Option<&str>,
    options: WorkerOptions<'_>,
) -> Result<ExitStatus, Error> {
    use nix::sys::stat::Mode as FileMode;
    use nix::unistd::mkfifo;

    let fifo_directory = tempfile::Builder::new()
        .prefix("metabench-perf-")
        .tempdir()
        .map_err(Error::PerfControl)?;
    let control = fifo_directory.path().join("control");
    let acknowledgement = fifo_directory.path().join("ack");
    let control_text = control.to_str().ok_or_else(|| {
        Error::PerfControl(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "perf control FIFO path is not UTF-8",
        ))
    })?;
    let acknowledgement_text = acknowledgement.to_str().ok_or_else(|| {
        Error::PerfControl(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "perf acknowledgement FIFO path is not UTF-8",
        ))
    })?;
    if control_text.contains(',') || acknowledgement_text.contains(',') {
        return Err(Error::PerfControl(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "perf FIFO paths cannot contain commas",
        )));
    }
    let control_argument = format!("fifo:{control_text},{acknowledgement_text}");

    mkfifo(&control, FileMode::S_IRUSR | FileMode::S_IWUSR).map_err(|error| Error::PerfControl(error.into()))?;
    if let Err(error) = mkfifo(&acknowledgement, FileMode::S_IRUSR | FileMode::S_IWUSR) {
        let _ignored = fs::remove_file(&control);
        return Err(Error::PerfControl(error.into()));
    }

    let open_guard = |path: &std::path::Path| OpenOptions::new().read(true).write(true).open(path);
    let control_guard = match open_guard(&control) {
        Ok(file) => file,
        Err(error) => {
            let _control_cleanup = fs::remove_file(&control);
            let _acknowledgement_cleanup = fs::remove_file(&acknowledgement);
            return Err(Error::PerfControl(error));
        }
    };
    let acknowledgement_guard = match open_guard(&acknowledgement) {
        Ok(file) => file,
        Err(error) => {
            drop(control_guard);
            let _control_cleanup = fs::remove_file(&control);
            let _acknowledgement_cleanup = fs::remove_file(&acknowledgement);
            return Err(Error::PerfControl(error));
        }
    };

    let mut command = Command::new("perf");
    command.arg("stat").args(passthrough).args([
        "--delay=-1",
        "--event=instructions:u",
        "--event=cycles:u",
        "--event=branch-misses:u",
        "--event=cache-misses:u",
        "--control",
        &control_argument,
    ]);
    if let (Some(root), Some(benchmark)) = (options.artifact_root, benchmark) {
        let output = perf_path(root, benchmark);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
                path: parent.to_owned(),
                source,
            })?;
        }
        command.arg("--json-output").arg("--output").arg(output);
    }
    command
        .arg("--")
        .arg(executable)
        .env(PERF_CONTROL_ENV, &control)
        .env(PERF_ACK_ENV, &acknowledgement);

    let status = execute_worker_command(Mode::Perf, &mut command, &[], benchmark, options);
    drop(control_guard);
    drop(acknowledgement_guard);
    let control_cleanup = fs::remove_file(&control);
    let acknowledgement_cleanup = fs::remove_file(&acknowledgement);
    match status {
        Err(error) => Err(error),
        Ok(status) => {
            if let Some(error) = control_cleanup.err().or_else(|| acknowledgement_cleanup.err()) {
                Err(Error::PerfControl(error))
            } else {
                Ok(status)
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn launch_perf_worker(
    _executable: &std::path::Path,
    _passthrough: &[OsString],
    _benchmark: Option<&str>,
    _options: WorkerOptions<'_>,
) -> Result<ExitStatus, Error> {
    Err(Error::UnsupportedMode {
        mode: Mode::Perf.as_str(),
        reason: "Linux perf is supported only on Linux",
    })
}

struct WorkerToken {
    directory: tempfile::TempDir,
}

impl WorkerToken {
    fn path(&self) -> &std::path::Path {
        self.directory.path()
    }
}

fn create_worker_token() -> Result<WorkerToken, Error> {
    let directory = tempfile::Builder::new()
        .prefix("metabench-worker-")
        .tempdir()
        .map_err(Error::CreateWorkerToken)?;
    Ok(WorkerToken { directory })
}

fn consume_worker_token() -> Result<(), Error> {
    let Some(token) = env::var_os(WORKER_TOKEN_ENV) else {
        return Err(Error::StaleWorkerMarker);
    };
    let token = PathBuf::from(token);
    if !is_worker_token_path(&token, &env::temp_dir()) {
        return Err(Error::StaleWorkerMarker);
    }
    match fs::symlink_metadata(&token) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(Error::StaleWorkerMarker),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Error::StaleWorkerMarker),
        Err(error) => return Err(Error::ConsumeWorkerToken(error)),
    }
    match fs::remove_dir(token) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Error::StaleWorkerMarker),
        Err(error) => Err(Error::ConsumeWorkerToken(error)),
    }
}

fn is_worker_token_path(token: &std::path::Path, temp_root: &std::path::Path) -> bool {
    token.parent() == Some(temp_root)
        && token
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with("metabench-worker-") && name.len() > "metabench-worker-".len())
}

fn ensure_success(mode: Mode, status: ExitStatus) -> Result<(), Error> {
    if status.success() {
        Ok(())
    } else {
        Err(Error::WorkerFailed {
            mode: mode.as_str(),
            code: status.code(),
        })
    }
}

fn run_worker(mode: Mode, register: fn(&mut BenchmarkSuite)) -> Result<(), Error> {
    let mut benchmarks = BenchmarkSuite::register(register)?;
    benchmarks.retain_mode(mode);
    if let Some(selected) = env::var_os(WORKER_BENCHMARK_ENV) {
        let selected = selected.to_string_lossy();
        if !benchmarks.retain_benchmark(&selected) {
            return Err(Error::UnknownBenchmark(selected.into_owned()));
        }
    }

    match mode {
        Mode::Criterion => run_criterion(&benchmarks)?,
        Mode::Allocations => run_allocations(&benchmarks)?,
        Mode::Callgrind => return Err(Error::InvalidWorkerMode(mode.to_string())),
        Mode::Perf => {
            let mut control = PerfControl::connect()?;
            run_direct(&benchmarks, &mut control)?;
        }
    }

    Ok(())
}

fn run_criterion(benchmarks: &BenchmarkSuite) -> Result<(), Error> {
    let mut criterion = Criterion::default().without_plots().configure_from_args();
    for benchmark_group in &benchmarks.groups {
        let mut group = criterion.benchmark_group(&benchmark_group.name);
        for benchmark in &benchmark_group.benchmarks {
            let benchmark_id = benchmark.identity(&benchmark_group.name);
            let criterion_name = match &benchmark.case {
                Some(case) => format!("{}/{case}", benchmark.name),
                None => benchmark.name.clone(),
            };
            let mut benchmark_result = None;
            group.bench_function(&criterion_name, |criterion_bencher| {
                let mut bencher = Bencher::criterion(criterion_bencher);
                (benchmark.function)(&mut bencher);
                benchmark_result = Some(bencher.finish(&benchmark_id));
            });
            benchmark_result.unwrap_or(Err(Error::InvalidRunCount {
                benchmark: benchmark_id,
                count: 0,
            }))?;
        }
        group.finish();
    }
    criterion.final_summary();
    Ok(())
}

fn run_allocations(benchmarks: &BenchmarkSuite) -> Result<(), Error> {
    let artifact_root = env::var_os(ARTIFACT_DIR_ENV).map(std::path::PathBuf::from);
    let session = if artifact_root.is_some() {
        Session::new().no_stdout().no_file()
    } else {
        Session::new()
    };
    let mut operations = Vec::new();

    for (index, (group, benchmark)) in benchmarks.benchmarks().enumerate() {
        let benchmark_id = benchmark.identity(group);
        let operation_name = format!("{index:05}-{benchmark_id}");
        let operation = session.operation(&operation_name);
        let mut bencher = Bencher::allocations(&operation, ALLOCATION_SAMPLES);
        (benchmark.function)(&mut bencher);
        bencher.finish(&benchmark_id)?;
        operations.push((operation_name, benchmark_id));
    }

    if let Some(root) = artifact_root {
        write_allocations(&allocation_path(&root), &session.to_report(), &operations)?;
    }
    drop(session);
    Ok(())
}

fn run_direct(benchmarks: &BenchmarkSuite, control: &mut dyn MeasurementControl) -> Result<(), Error> {
    for (group, benchmark) in benchmarks.benchmarks() {
        let benchmark_id = benchmark.identity(group);
        let mut bencher = Bencher::direct(1, control);
        (benchmark.function)(&mut bencher);
        bencher.finish(&benchmark_id)?;
    }
    Ok(())
}

struct PerfControl {
    control: File,
    acknowledgement: File,
}

impl PerfControl {
    fn connect() -> Result<Self, Error> {
        let control = env::var_os(PERF_CONTROL_ENV)
            .ok_or_else(|| Error::PerfControl(std::io::Error::new(std::io::ErrorKind::NotFound, "missing perf control FIFO")))?;
        let acknowledgement = env::var_os(PERF_ACK_ENV).ok_or_else(|| {
            Error::PerfControl(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "missing perf acknowledgement FIFO",
            ))
        })?;

        let control = OpenOptions::new().write(true).open(control).map_err(Error::PerfControl)?;
        let acknowledgement = OpenOptions::new().read(true).open(acknowledgement).map_err(Error::PerfControl)?;
        Ok(Self { control, acknowledgement })
    }

    fn command(&mut self, command: &'static [u8]) -> Result<(), Error> {
        self.control.write_all(command).map_err(Error::PerfControl)?;
        self.control.flush().map_err(Error::PerfControl)?;

        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsFd;

            use nix::poll::{PollFd, PollFlags, poll};

            let mut descriptors = [PollFd::new(self.acknowledgement.as_fd(), PollFlags::POLLIN)];
            let ready = poll(&mut descriptors, 30_000_u16).map_err(|error| Error::PerfControl(error.into()))?;
            if ready == 0 {
                return Err(Error::PerfControl(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out waiting for perf acknowledgement",
                )));
            }
        }

        let mut acknowledgement = [0_u8; 4];
        self.acknowledgement.read_exact(&mut acknowledgement).map_err(Error::PerfControl)?;
        if acknowledgement == *b"ack\n" {
            Ok(())
        } else {
            Err(Error::UnexpectedPerfAcknowledgement(
                String::from_utf8_lossy(&acknowledgement).into_owned(),
            ))
        }
    }
}

impl MeasurementControl for PerfControl {
    fn start(&mut self) -> Result<(), Error> {
        self.command(b"enable\n")
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.command(b"disable\n")
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct FakeChild {
        polls: VecDeque<std::io::Result<Option<ExitStatus>>>,
        killed: bool,
        waited: bool,
    }

    impl WorkerChild for FakeChild {
        fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.waited = true;
            Ok(success_status())
        }

        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            self.polls.pop_front().unwrap_or(Ok(None))
        }

        fn kill_tree(&mut self) -> std::io::Result<()> {
            self.killed = true;
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeClock {
        elapsed: Duration,
        sleeps: Vec<Duration>,
    }

    impl PollClock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.elapsed
        }

        fn sleep(&mut self, duration: Duration) {
            self.sleeps.push(duration);
            self.elapsed += duration;
        }
    }

    #[cfg(unix)]
    fn success_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }

    #[cfg(windows)]
    fn success_status() -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }

    #[test]
    fn deterministic_wait_returns_ready_status_without_sleeping() {
        let mut child = FakeChild {
            polls: VecDeque::from([Ok(Some(success_status()))]),
            killed: false,
            waited: false,
        };
        let mut clock = FakeClock::default();

        let status = wait_for_child(Mode::Criterion, &mut child, Some(Duration::from_secs(1)), &mut clock).expect("worker completes");

        assert!(status.success());
        assert!(!child.killed);
        assert!(!child.waited);
        assert!(clock.sleeps.is_empty());
    }

    #[test]
    fn deterministic_timeout_kills_and_reaps_the_process_tree() {
        let mut child = FakeChild {
            polls: VecDeque::from([Ok(None), Ok(None), Ok(None)]),
            killed: false,
            waited: false,
        };
        let mut clock = FakeClock::default();

        let error =
            wait_for_child(Mode::Criterion, &mut child, Some(Duration::from_millis(50)), &mut clock).expect_err("worker should time out");

        assert!(matches!(error, Error::WorkerTimedOut { .. }));
        assert!(child.killed);
        assert!(child.waited);
        assert_eq!(clock.sleeps, [Duration::from_millis(25); 2]);
    }

    #[test]
    fn keep_going_aggregates_only_multi_engine_failures() {
        let failure = || Error::WorkerFailed {
            mode: Mode::Criterion.as_str(),
            code: Some(1),
        };
        let mut completed = Vec::new();
        let mut failures = Vec::new();

        record_mode_result(
            Mode::Criterion,
            Err(failure()),
            true,
            FailureMode::KeepGoing,
            &mut completed,
            &mut failures,
        )
        .expect("keep-going records failure");
        record_mode_result(
            Mode::Allocations,
            Ok(()),
            true,
            FailureMode::KeepGoing,
            &mut completed,
            &mut failures,
        )
        .expect("success is recorded");

        assert_eq!(completed, [Mode::Allocations]);
        assert_eq!(failures.len(), 1);
        assert!(
            record_mode_result(
                Mode::Criterion,
                Err(failure()),
                true,
                FailureMode::FailFast,
                &mut completed,
                &mut failures,
            )
            .is_err()
        );
        assert!(
            record_mode_result(
                Mode::Criterion,
                Err(failure()),
                false,
                FailureMode::KeepGoing,
                &mut completed,
                &mut failures,
            )
            .is_err()
        );
    }

    #[test]
    fn only_native_perf_is_linux_specific() {
        for mode in [Mode::Criterion, Mode::Callgrind, Mode::Allocations] {
            assert!(mode_supported(mode, false));
            assert!(mode_supported(mode, true));
        }
        assert!(!mode_supported(Mode::Perf, false));
        assert!(mode_supported(Mode::Perf, true));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn timeout_terminates_worker_descendants() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let pid_path = directory.path().join("descendant.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$1\"; wait")
            .arg("metabench-timeout-test")
            .arg(&pid_path);

        let error = wait_for_worker(Mode::Criterion, &mut command, Some(Duration::from_secs(1))).expect_err("worker should time out");
        assert!(matches!(error, Error::WorkerTimedOut { .. }));

        let pid = fs::read_to_string(&pid_path).expect("descendant PID");
        let status_path = PathBuf::from(format!("/proc/{}/stat", pid.trim()));
        let mut terminated = false;
        for _ in 0..40 {
            let no_live_process = fs::read_to_string(&status_path).map_or(true, |status| {
                status.split_ascii_whitespace().nth(2).is_some_and(|state| state == "Z")
            });
            if no_live_process {
                terminated = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(terminated, "descendant process survived worker timeout");
    }
}
