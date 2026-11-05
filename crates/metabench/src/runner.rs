// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use std::env::args_os as runner_arguments_os;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::{self, Write as _};
#[cfg(test)]
use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};
use std::{env, fs, thread};

use command_group::{CommandGroup, GroupChild};
#[cfg(target_os = "linux")]
use nix::sys::stat::Mode as FileMode;
#[cfg(target_os = "linux")]
use nix::unistd::mkfifo;

use crate::arguments::{Arguments, FailureMode, WORKER_MODE_ENV, WORKER_TOKEN_ENV, paths_alias};
use crate::artifact::{self, ArtifactDirectory};
use crate::error::Error;
use crate::identity::BenchmarkIdentity;
use crate::mode::Mode;
use crate::probe::ProbeTarget;
use crate::report::BenchmarkReport;
use crate::worker_log::WorkerLog;
use crate::{allocation, perf, vtune};

const HELP: &str = "\
Usage: BENCHMARK [METABENCH OPTIONS] [ENGINE OPTIONS]

Engines:
  --criterion              Run Criterion benchmarks
  --gungraun               Run Gungraun benchmarks
  --perf                   Measure Linux hardware performance counters
  --vtune                  Measure hardware events with Intel VTune
  --allocations            Measure allocations with alloc_tracker
  --all-engines            Run every engine compiled into this target

Native engine options (repeat once per argument):
  --criterion-arg ARG
  --gungraun-arg ARG
  --perf-arg ARG
  --vtune-arg ARG

Metabench options:
  --export-md PATH          Override the default Markdown report path
  --export-json PATH        Override the default JSON report path
  --output PATH             Override both paths with PATH.md and PATH.json
  --no-output               Do not write report files
  --baseline PATH           Override the previous report used as the baseline
  --no-baseline             Do not compare against an existing report
  --list                    Use each selected engine's native listing mode
                            (cross-engine canonical filtering is unavailable)
  --test                    Run native smoke checks without publishing reports
  --timeout DURATION        Limit each engine worker (for example 30s or 5m)
  --fail-fast               Stop after the first engine failure
  --keep-going              Continue through engine failures (default)
  --regression-threshold PERCENT
  --show-engine-output      Show raw child-engine stdout and stderr
  -h, --help

Arguments after -- are forwarded when exactly one engine is selected.
No arguments also selects smoke mode (Cargo's harness-free test invocation).
Forwarded Gungraun --test=true selects smoke mode.
Forwarded Gungraun --test=false selects measurements unless a smoke request is present.
Top-level --test takes priority.
BENCH_ENGINE may select criterion, gungraun, perf, vtune, or allocations when no selector is present.
CRITERION_HOME and GUNGRAUN_HOME retain the 20 most recent metabench runs
under each home's .metabench-runs directory.
";
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
const TERMINATION_REAP_GRACE: Duration = Duration::from_secs(1);

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestRunnerSources {
    argument: fn() -> Option<OsString>,
    environment: fn(&str) -> Option<OsString>,
    consume: fn() -> Result<(), Error>,
    prime: fn(Mode),
    finish: fn(Mode) -> Result<(), Error>,
    parse: fn() -> Result<Arguments, Error>,
    write: fn(&[u8]) -> io::Result<usize>,
    flush: fn() -> io::Result<()>,
    executable: fn() -> io::Result<PathBuf>,
}

#[cfg(test)]
thread_local! {
    static TEST_RUNNER_SOURCES: std::cell::Cell<Option<TestRunnerSources>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestChildSources {
    spawn: fn(&mut Command) -> io::Result<Box<dyn WorkerProcess>>,
    capture: fn(&mut Command, &Path, bool) -> Result<Option<WorkerLog>, Error>,
    token: fn(&str) -> io::Result<PathBuf>,
    exists: fn(&Path) -> io::Result<bool>,
    remove: fn(&Path) -> io::Result<()>,
    time: fn() -> Duration,
    sleep: fn(Duration),
    discovery: fn(&str, &Path) -> io::Result<PathBuf>,
    create_directory: fn(&Path) -> io::Result<()>,
}

#[cfg(test)]
thread_local! {
    static TEST_CHILD_SOURCES: std::cell::Cell<Option<TestChildSources>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
struct PerfWorkerRequest<'a> {
    executable: &'a Path,
    native_arguments: &'a [OsString],
    collector_arguments: &'a [OsString],
    artifact: &'a Path,
    home: &'a Path,
    show: bool,
    timeout: Option<Duration>,
}

#[cfg(test)]
type TestPerfLaunch = fn(PerfWorkerRequest<'_>) -> Result<ExitStatus, Error>;

#[cfg(test)]
thread_local! {
    static TEST_PERF_LAUNCH: std::cell::Cell<Option<TestPerfLaunch>> = const { std::cell::Cell::new(None) };
}

enum RunnerOutput<N = io::StdoutLock<'static>> {
    Native(N),
    #[cfg(test)]
    Injected(fn(&[u8]) -> io::Result<usize>, fn() -> io::Result<()>),
}

impl<N: io::Write> io::Write for RunnerOutput<N> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Native(output) => output.write(buf),
            #[cfg(test)]
            Self::Injected(write, _) => write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Native(output) => output.flush(),
            #[cfg(test)]
            Self::Injected(_, flush) => flush(),
        }
    }
}

fn runner_output() -> RunnerOutput {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return RunnerOutput::Injected(sources.write, sources.flush);
    }
    RunnerOutput::Native(io::stdout().lock())
}

fn runner_diagnostics() -> RunnerOutput<io::Stderr> {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return RunnerOutput::Injected(sources.write, sources.flush);
    }
    RunnerOutput::Native(io::stderr())
}

fn runner_argument() -> Option<OsString> {
    first_argument(runner_arguments_os())
}

#[cfg(test)]
fn runner_arguments_os() -> impl Iterator<Item = OsString> {
    let argument = TEST_RUNNER_SOURCES
        .get()
        .map_or_else(|| env::args_os().nth(1), |sources| (sources.argument)());
    std::iter::once(OsString::from("scripted-worker")).chain(argument)
}

fn runner_environment(name: &str) -> Option<OsString> {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return (sources.environment)(name);
    }
    env::var_os(name)
}

fn runner_executable() -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return (sources.executable)();
    }
    env::current_exe()
}

fn runner_arguments() -> Result<Arguments, Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return (sources.parse)();
    }
    Arguments::parse()
}

fn prime_probe(mode: Mode) {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return (sources.prime)(mode);
    }
    match mode {
        Mode::Allocations => allocation::prime(),
        Mode::Perf => perf::prime(),
        Mode::Vtune => vtune::prime(),
        Mode::Criterion | Mode::Gungraun => unreachable!("only probe workers are primed"),
    }
}

fn finish_probe(mode: Mode) -> Result<(), Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return (sources.finish)(mode);
    }
    match mode {
        Mode::Allocations => allocation::write_worker_artifact(),
        Mode::Perf => perf::finish_worker(),
        Mode::Vtune => vtune::finish_worker(),
        Mode::Criterion | Mode::Gungraun => unreachable!("only probe workers publish probe artifacts"),
    }
}

/// Native engine entry points compiled into a benchmark target.
#[derive(Clone, Copy, Debug)]
pub struct EngineSet {
    criterion: fn(),
    criterion_unit: Option<&'static str>,
    gungraun: fn(),
    probe: fn() -> Result<(), String>,
}

impl EngineSet {
    /// Creates a target containing both native engines and Criterion unit metadata.
    #[must_use]
    pub const fn both_with_unit(
        criterion: fn(),
        gungraun: fn(),
        criterion_unit: Option<&'static str>,
        probe: fn() -> Result<(), String>,
    ) -> Self {
        Self {
            criterion,
            criterion_unit,
            gungraun,
            probe,
        }
    }

    fn run(self, mode: Mode) {
        let entry = match mode {
            Mode::Criterion | Mode::Perf | Mode::Vtune | Mode::Allocations => self.criterion,
            Mode::Gungraun => self.gungraun,
        };
        entry();
    }
}

/// Runs a benchmark target generated by [`crate::main!`].
///
/// # Errors
///
/// Returns an error when orchestration, a native engine, artifact parsing, or
/// report generation fails.
#[doc(hidden)]
pub fn run(engines: EngineSet, identities: &'static [BenchmarkIdentity], benchmark_target: &'static str) -> Result<(), String> {
    run_inner(engines, identities, benchmark_target).map_err(|error| error.to_string())
}

fn run_inner(engines: EngineSet, identities: &'static [BenchmarkIdentity], benchmark_target: &'static str) -> Result<(), Error> {
    match entry_route(runner_argument, runner_environment, consume_worker_token, || {
        engines.run(Mode::Gungraun);
    })? {
        EntryRoute::Complete => Ok(()),
        EntryRoute::Worker(mode) => run_worker_mode(
            mode,
            prime_probe,
            |mode| engines.run(mode),
            engines.probe,
            runner_argument,
            (
                || finish_probe(Mode::Allocations),
                || finish_probe(Mode::Perf),
                || finish_probe(Mode::Vtune),
            ),
        ),
        EntryRoute::Parent => parent_entry(
            runner_arguments,
            |help| runner_output().write_all(help),
            |arguments| run_parent(arguments, engines, identities, benchmark_target),
        ),
    }
}

enum EntryRoute {
    Complete,
    Worker(Mode),
    Parent,
}

fn first_argument(mut arguments: impl Iterator<Item = OsString>) -> Option<OsString> {
    arguments.nth(1)
}

fn entry_route(
    argument: impl FnOnce() -> Option<OsString>,
    environment: impl FnOnce(&str) -> Option<OsString>,
    consume: impl FnOnce() -> Result<(), Error>,
    gungraun: impl FnOnce(),
) -> Result<EntryRoute, Error> {
    if argument().as_deref() == Some(OsStr::new("--gungraun-run")) {
        gungraun();
        return Ok(EntryRoute::Complete);
    }
    if let Some(worker_mode) = environment(WORKER_MODE_ENV) {
        consume()?;
        let mode = Mode::from_os_str(&worker_mode)?;
        return Ok(EntryRoute::Worker(mode));
    }
    Ok(EntryRoute::Parent)
}

fn run_worker_mode(
    mode: Mode,
    prime: impl FnOnce(Mode),
    native: impl FnOnce(Mode),
    probe: impl FnOnce() -> Result<(), String>,
    argument: impl FnOnce() -> Option<OsString>,
    finish: (
        impl FnOnce() -> Result<(), Error>,
        impl FnOnce() -> Result<(), Error>,
        impl FnOnce() -> Result<(), Error>,
    ),
) -> Result<(), Error> {
    match mode {
        Mode::Allocations | Mode::Perf | Mode::Vtune => prime(mode),
        Mode::Criterion | Mode::Gungraun => {}
    }
    if matches!(mode, Mode::Allocations | Mode::Perf | Mode::Vtune) {
        probe().map_err(Error::Probe)?;
        if argument().as_deref() == Some(OsStr::new("--metabench-probe-list")) {
            return Ok(());
        }
    } else {
        native(mode);
    }
    if mode == Mode::Allocations {
        finish.0()?;
    } else if mode == Mode::Perf {
        finish.1()?;
    } else if mode == Mode::Vtune {
        finish.2()?;
    }
    Ok(())
}

fn parent_entry(
    parse: impl FnOnce() -> Result<Arguments, Error>,
    help: impl FnOnce(&[u8]) -> io::Result<()>,
    parent: impl FnOnce(&mut Arguments) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut arguments = parse()?;
    if arguments.help {
        help(HELP.as_bytes()).map_err(Error::HelpOutput)?;
        return Ok(());
    }
    parent(&mut arguments)
}

fn run_parent(
    arguments: &mut Arguments,
    engines: EngineSet,
    identities: &[BenchmarkIdentity],
    benchmark_target: &str,
) -> Result<(), Error> {
    let (modes, executable) = prepare_parent(arguments, || current_executable(runner_executable))?;
    if let Some(smoke_modes) = select_smoke_modes(&modes, arguments.smoke(), arguments.list) {
        return run_parent_smoke(&smoke_modes, arguments, |mode, argv, show, timeout| {
            // #[gamma::skip(bool_expr.negate, tag = "equivalent", reason = "smoke workers pass no artifact homes, so launch_worker never configures output capture and either show value leaves command and output behavior unchanged")]
            launch_worker(mode, &executable, argv, None, show, timeout)
        });
    }
    let report_root = report_root(&executable, benchmark_target, |executable| {
        effective_target_directory(executable, runner_environment)
    })?;
    let has_probes = has_probe_modes(&modes);
    let artifacts = create_parent_artifacts(&report_root, arguments, &modes, create_run_artifacts)?;
    let native_modes = parent_native_modes(&modes);
    let summary = run_parent_native_modes(
        &native_modes,
        arguments,
        artifacts.as_ref(),
        print_progress,
        |mode, argv, artifacts, show, timeout| launch_worker(mode, &executable, argv, artifacts, show, timeout),
        |artifacts, mode, argv, status| {
            artifact::write_run_manifest(artifacts.path(), artifacts.root(), benchmark_target, mode.as_str(), argv, status)
        },
    )?;
    finish_parent_modes(
        arguments.list,
        artifacts.as_ref(),
        summary,
        |failures| {
            list_probe_cases(
                has_probes,
                artifacts.as_ref(),
                failures,
                |artifacts| discover_native_probe_targets(&executable, artifacts, arguments),
                print_probe_targets,
            )
        },
        |summary, failures| {
            run_probe_modes(
                &modes,
                &executable,
                arguments,
                artifacts.as_ref().expect("non-list runs always create an artifact directory"),
                summary,
                failures,
            )
        },
        |artifacts, completed, probe_results, complete_run| {
            build_report(
                artifacts,
                completed,
                identities,
                arguments,
                benchmark_target,
                engines.criterion_unit,
                probe_results,
                complete_run,
            )
            .and_then(|(report, json_path, markdown_path)| write_console_report(&report, json_path.as_deref(), markdown_path.as_deref()))
        },
    )
}

fn prepare_parent(arguments: &mut Arguments, executable: impl FnOnce() -> Result<PathBuf, Error>) -> Result<(Vec<Mode>, PathBuf), Error> {
    let modes = arguments.modes.clone();
    arguments.finalize_native_args(&modes)?;
    Ok((modes, executable()?))
}

fn select_smoke_modes(modes: &[Mode], smoke: bool, list: bool) -> Option<Vec<Mode>> {
    (smoke && !list).then(|| {
        let mut selected = modes
            .iter()
            .copied()
            .filter(|mode| matches!(mode, Mode::Criterion | Mode::Gungraun))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            selected.push(Mode::Criterion);
        }
        selected
    })
}

fn has_probe_modes(modes: &[Mode]) -> bool {
    modes
        .iter()
        .any(|mode| matches!(mode, Mode::Allocations | Mode::Perf | Mode::Vtune))
}

fn parent_native_modes(modes: &[Mode]) -> Vec<Mode> {
    modes
        .iter()
        .copied()
        .filter(|mode| !matches!(mode, Mode::Allocations | Mode::Perf | Mode::Vtune))
        .collect()
}

fn report_root(executable: &Path, target: &str, directory: impl FnOnce(&Path) -> Result<PathBuf, Error>) -> Result<PathBuf, Error> {
    Ok(directory(executable)?.join("metabench").join(target))
}

fn run_parent_smoke(
    modes: &[Mode],
    arguments: &Arguments,
    mut launch: impl FnMut(Mode, &[OsString], bool, Option<Duration>) -> Result<ExitStatus, Error>,
) -> Result<(), Error> {
    let summary = orchestrate_modes(modes, arguments.failure_mode, |mode| {
        let status = launch(mode, arguments.args_for(mode), true, arguments.timeout)?;
        ensure_success(mode, status)
    })?;
    finish_failures(summary.failures)
}

fn run_parent_native_modes(
    modes: &[Mode],
    arguments: &Arguments,
    artifacts: Option<&ArtifactDirectory>,
    mut progress: impl FnMut(Mode) -> Result<(), Error>,
    mut launch: impl FnMut(Mode, &[OsString], Option<&ArtifactDirectory>, bool, Option<Duration>) -> Result<ExitStatus, Error>,
    mut manifest: impl FnMut(&ArtifactDirectory, Mode, &[OsString], Option<ExitStatus>) -> Result<(), Error>,
) -> Result<EngineRunSummary, Error> {
    orchestrate_modes(modes, arguments.failure_mode, |mode| {
        if !arguments.list {
            progress(mode)?;
        }
        let launched = launch(
            mode,
            arguments.args_for(mode),
            artifacts,
            arguments.show_engine_output || arguments.list,
            arguments.timeout,
        );
        let manifest = if let Some(artifacts) = artifacts {
            manifest(artifacts, mode, arguments.args_for(mode), launched.as_ref().ok().copied())
        } else {
            Ok(())
        };
        combine_results(launched.and_then(|status| ensure_success(mode, status)), manifest)
    })
}

fn finish_parent_modes(
    list: bool,
    artifacts: Option<&ArtifactDirectory>,
    mut summary: EngineRunSummary,
    list_cases: impl FnOnce(&mut Vec<String>) -> Result<(), Error>,
    probes: impl FnOnce(&mut EngineRunSummary, &mut Vec<String>) -> Result<Vec<crate::report::NativeResult>, Error>,
    report: impl FnOnce(&ArtifactDirectory, &[Mode], Vec<crate::report::NativeResult>, bool) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut failures = std::mem::take(&mut summary.failures);

    if list {
        list_cases(&mut failures)?;
        return finish_failures(failures);
    }

    let probe_results = probes(&mut summary, &mut failures)?;

    if let Some(artifacts) = artifacts
        && !summary.completed.is_empty()
    {
        let complete_run = failures.is_empty();
        if let Err(error) = report(artifacts, &summary.completed, probe_results, complete_run) {
            // #[gamma::skip(call_result.default, reason = "append_failure returns unit and default_after preserves the appended failure")]
            append_failure(&mut failures, error);
        }
    }
    finish_failures(failures)
}

fn list_probe_cases(
    has_probes: bool,
    artifacts: Option<&ArtifactDirectory>,
    failures: &mut Vec<String>,
    discover: impl FnOnce(&ArtifactDirectory) -> Result<Vec<ProbeTarget>, Error>,
    print: impl FnOnce(&[ProbeTarget]) -> Result<(), Error>,
) -> Result<(), Error> {
    if has_probes {
        let artifacts = artifacts.expect("probe listing creates an artifact directory above");
        match discover(artifacts) {
            Ok(targets) => print(&targets)?,
            // #[gamma::skip(call_result.default, reason = "append_failure returns unit and default_after preserves the appended failure")]
            Err(error) => append_failure(failures, error),
        }
    }
    Ok(())
}

fn create_run_artifacts(report_root: &Path, list: bool, has_probes: bool) -> Result<Option<ArtifactDirectory>, Error> {
    create_run_artifacts_with(report_root, list, has_probes, ArtifactDirectory::create)
}

fn create_parent_artifacts(
    report_root: &Path,
    arguments: &Arguments,
    modes: &[Mode],
    create: impl FnOnce(&Path, bool, bool) -> Result<Option<ArtifactDirectory>, Error>,
) -> Result<Option<ArtifactDirectory>, Error> {
    create(report_root, arguments.list, has_probe_modes(modes))
}

fn create_run_artifacts_with(
    report_root: &Path,
    list: bool,
    has_probes: bool,
    create: impl FnOnce(&Path) -> Result<ArtifactDirectory, Error>,
) -> Result<Option<ArtifactDirectory>, Error> {
    (!list || has_probes).then(|| create(report_root)).transpose()
}

fn print_probe_targets(targets: &[ProbeTarget]) -> Result<(), Error> {
    print_probe_targets_with(targets, runner_output)
}

fn print_probe_targets_with<W: io::Write>(targets: &[ProbeTarget], output: impl FnOnce() -> W) -> Result<(), Error> {
    let mut stdout = output();
    write_probe_targets(&mut stdout, targets)
}

fn write_probe_targets(stdout: &mut impl io::Write, targets: &[ProbeTarget]) -> Result<(), Error> {
    for target in targets {
        writeln!(stdout, "{}: benchmark", terminal_text(&target.identity)).map_err(Error::ConsoleOutput)?;
    }
    Ok(())
}

fn run_probe_modes(
    modes: &[Mode],
    executable: &Path,
    arguments: &Arguments,
    artifacts: &ArtifactDirectory,
    summary: &mut EngineRunSummary,
    failures: &mut Vec<String>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    run_probe_modes_with(
        modes,
        arguments.failure_mode,
        summary,
        failures,
        print_progress,
        |mode| match mode {
            Mode::Allocations => run_allocations(executable, arguments, artifacts),
            Mode::Perf => run_perf(executable, arguments, artifacts),
            Mode::Vtune => run_vtune(executable, arguments, artifacts),
            Mode::Criterion | Mode::Gungraun => unreachable!("the probe mode list is fixed"),
        },
    )
}

fn run_probe_modes_with(
    modes: &[Mode],
    failure_mode: FailureMode,
    summary: &mut EngineRunSummary,
    failures: &mut Vec<String>,
    mut progress: impl FnMut(Mode) -> Result<(), Error>,
    mut measure: impl FnMut(Mode) -> Result<Vec<crate::report::NativeResult>, Error>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    let mut results = Vec::new();
    for &mode in modes {
        if !matches!(mode, Mode::Allocations | Mode::Perf | Mode::Vtune) {
            continue;
        }

        progress(mode)?;
        let measured = measure(mode);
        match measured {
            Ok(measured) => {
                summary.completed.push(mode);
                results.extend(measured);
            }
            Err(error) if failure_mode == FailureMode::KeepGoing => {
                // #[gamma::skip(call_result.default, tag = "equivalent", reason = "append_failure returns unit; default_after retains the append and replaces unit with unit")]
                append_failure(failures, error);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(results)
}

#[derive(Debug)]
struct EngineRunSummary {
    completed: Vec<Mode>,
    failures: Vec<String>,
}

fn orchestrate_modes(
    modes: &[Mode],
    failure_mode: FailureMode,
    mut execute: impl FnMut(Mode) -> Result<(), Error>,
) -> Result<EngineRunSummary, Error> {
    let mut completed = Vec::new();
    let mut failures = Vec::new();
    for &mode in modes {
        match execute(mode) {
            Ok(()) => completed.push(mode),
            Err(error) if failure_mode == FailureMode::KeepGoing => {
                // #[gamma::skip(call_result.default, tag = "equivalent", reason = "append_failure returns unit; default_after retains the append and replaces unit with unit")]
                append_failure(&mut failures, error);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(EngineRunSummary { completed, failures })
}

fn append_failure(failures: &mut Vec<String>, error: Error) {
    match error {
        Error::MultipleModeFailures(nested) => failures.extend(nested),
        error => failures.push(error.to_string()),
    }
}

fn combine_results<T>(primary: Result<T, Error>, secondary: Result<(), Error>) -> Result<T, Error> {
    match (primary, secondary) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => Err(multiple_failures([primary, secondary])),
    }
}

fn finish_failures(failures: Vec<String>) -> Result<(), Error> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::MultipleModeFailures(failures))
    }
}

fn multiple_failures<const N: usize>(errors: [Error; N]) -> Error {
    let mut failures = Vec::new();
    for error in errors {
        match error {
            Error::MultipleModeFailures(nested) => failures.extend(nested),
            error => failures.push(error.to_string()),
        }
    }
    Error::MultipleModeFailures(failures)
}

#[expect(
    clippy::too_many_arguments,
    reason = "report assembly is explicit about every input that shapes the final report"
)]
fn build_report(
    artifacts: &ArtifactDirectory,
    completed: &[Mode],
    identities: &[BenchmarkIdentity],
    arguments: &Arguments,
    benchmark_target: &str,
    criterion_unit: Option<&str>,
    probe_results: Vec<crate::report::NativeResult>,
    complete_run: bool,
) -> Result<(BenchmarkReport, Option<PathBuf>, Option<PathBuf>), Error> {
    let results = collect_report_results(completed, probe_results, criterion_unit, |mode, unit| match mode {
        Mode::Criterion => {
            let path = artifacts.criterion_path();
            load_report_backend(
                mode,
                &path,
                || artifact::parse_criterion(&path, artifacts.root(), identities, unit),
                |results| artifact::record_discovered_identities(artifacts.path(), Mode::Criterion.as_str(), results),
            )
        }
        Mode::Gungraun => {
            let path = artifacts.gungraun_path();
            load_report_backend(
                mode,
                &path,
                || artifact::parse_gungraun(&path, artifacts.root(), identities),
                |results| artifact::record_discovered_identities(artifacts.path(), Mode::Gungraun.as_str(), results),
            )
        }
        Mode::Allocations | Mode::Perf | Mode::Vtune => unreachable!("only native report backends are loaded"),
    })?;
    let report = assemble_report(results, completed, artifacts.root(), BenchmarkReport::from_results)?;
    require_report_entries(&report, artifacts.path())?;
    publish_report_with(
        report,
        arguments,
        benchmark_target,
        complete_run,
        report_paths,
        |report, arguments, json_path| {
            let baseline = select_baseline(arguments, json_path, |path| {
                baseline_available(path, crate::report::report_file_exists, BenchmarkReport::publication_incomplete)
            });
            if let Some((path, baseline)) = read_selected_baseline(baseline, read_baseline)? {
                apply_report_baseline(report, path, &baseline, arguments.regression_threshold, &mut runner_diagnostics())?;
            }
            Ok(())
        },
        BenchmarkReport::write_reports,
    )
}

type ReportOutput = (BenchmarkReport, Option<PathBuf>, Option<PathBuf>);

fn publish_report_with(
    mut report: BenchmarkReport,
    arguments: &Arguments,
    benchmark_target: &str,
    complete_run: bool,
    paths: impl FnOnce(&Arguments, &str) -> Result<(Option<PathBuf>, Option<PathBuf>), Error>,
    baseline: impl FnOnce(&mut BenchmarkReport, &Arguments, Option<&Path>) -> Result<(), Error>,
    write: impl FnOnce(&BenchmarkReport, Option<&Path>, Option<&Path>) -> Result<(), Error>,
) -> Result<ReportOutput, Error> {
    let (json_path, markdown_path) = paths(arguments, benchmark_target)?;
    baseline(&mut report, arguments, json_path.as_deref())?;
    // A `--keep-going` run where some selected engine failed must not overwrite the
    // rolling automatic-baseline file with results from only the engines that
    // completed: a later, fully successful run would then read that partial file
    // back as its baseline and silently lose comparisons for the engine that failed
    // here. Route such partial runs to sibling `.partial` paths instead, so the
    // diagnostics remain visible without corrupting the rolling baseline; only a
    // run where every selected engine succeeded is allowed to promote the baseline.
    let (json_path, markdown_path) = report_destinations(json_path, markdown_path, complete_run);
    write(&report, json_path.as_deref(), markdown_path.as_deref())?;
    Ok((report, json_path, markdown_path))
}

fn collect_report_results(
    completed: &[Mode],
    probe_results: Vec<crate::report::NativeResult>,
    criterion_unit: Option<&str>,
    mut load: impl FnMut(Mode, Option<&str>) -> Result<Vec<crate::report::NativeResult>, Error>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    let mut results = probe_results;
    if completed.contains(&Mode::Criterion) {
        results.extend(load(Mode::Criterion, criterion_unit)?);
    }
    if completed.contains(&Mode::Gungraun) {
        results.extend(load(Mode::Gungraun, None)?);
    }
    Ok(results)
}

fn load_report_backend(
    mode: Mode,
    path: &Path,
    parse: impl FnOnce() -> Result<Vec<crate::report::NativeResult>, Error>,
    record: impl FnOnce(&[crate::report::NativeResult]) -> Result<(), Error>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    let results = parse()?;
    require_backend_results(&results, mode, path)?;
    record(&results)?;
    Ok(results)
}

fn assemble_report(
    results: Vec<crate::report::NativeResult>,
    completed: &[Mode],
    root: &Path,
    assemble: impl FnOnce(Vec<crate::report::NativeResult>, Vec<String>, String) -> Result<BenchmarkReport, Error>,
) -> Result<BenchmarkReport, Error> {
    assemble(
        results,
        completed.iter().map(|mode| mode.as_str().to_owned()).collect(),
        root.to_string_lossy().into_owned(),
    )
}

fn require_report_entries(report: &BenchmarkReport, path: &Path) -> Result<(), Error> {
    if report.entries.is_empty() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "no benchmark cases matched; existing reports were preserved".to_owned(),
        });
    }
    Ok(())
}

fn select_baseline<'a>(
    arguments: &'a Arguments,
    json_path: Option<&'a Path>,
    available: impl FnOnce(&Path) -> bool,
) -> Option<(&'a Path, bool)> {
    if let Some(path) = arguments.baseline.as_deref() {
        Some((path, false))
    } else if arguments.automatic_baseline {
        json_path.filter(|path| available(path)).map(|path| (path, true))
    } else {
        None
    }
}

fn baseline_available(path: &Path, is_file: impl FnOnce(&Path) -> bool, incomplete: impl FnOnce(&Path) -> bool) -> bool {
    is_file(path) && !incomplete(path)
}

fn read_selected_baseline(
    selected: Option<(&Path, bool)>,
    read: impl FnOnce(&Path, bool) -> Result<Option<BenchmarkReport>, Error>,
) -> Result<Option<(&Path, BenchmarkReport)>, Error> {
    if let Some((path, automatic)) = selected
        && let Some(baseline) = read(path, automatic)?
    {
        Ok(Some((path, baseline)))
    } else {
        Ok(None)
    }
}

fn apply_report_baseline(
    report: &mut BenchmarkReport,
    path: &Path,
    baseline: &BenchmarkReport,
    threshold: f64,
    warning: &mut impl io::Write,
) -> Result<(), Error> {
    if report.metadata.comparable_to(&baseline.metadata) {
        report.apply_baseline(baseline, threshold)?;
    } else {
        write_baseline_warning(
            warning,
            &format!(
                "skipping incompatible baseline {} (target, compiler, CPU, profile, or flags differ)",
                path.display()
            ),
        )?;
    }
    Ok(())
}

fn report_destinations(
    json_path: Option<PathBuf>,
    markdown_path: Option<PathBuf>,
    complete_run: bool,
) -> (Option<PathBuf>, Option<PathBuf>) {
    if complete_run {
        (json_path, markdown_path)
    } else {
        (
            json_path.as_deref().map(partial_report_path),
            markdown_path.as_deref().map(partial_report_path),
        )
    }
}

fn require_backend_results(results: &[crate::report::NativeResult], mode: Mode, path: &Path) -> Result<(), Error> {
    if results.is_empty() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!(
                "{} produced no benchmark measurements; existing reports were preserved",
                mode.as_str()
            ),
        });
    }
    Ok(())
}

/// Derives a sibling path for partial-run diagnostics, e.g. `report.json` becomes
/// `report.partial.json`, keeping such output clearly separate from the rolling
/// baseline file at the original path.
fn partial_report_path(path: &Path) -> PathBuf {
    match path.extension().and_then(OsStr::to_str) {
        Some(extension) => path.with_extension(format!("partial.{extension}")),
        None => path.with_extension("partial"),
    }
}

fn read_baseline(path: &Path, automatic: bool) -> Result<Option<BenchmarkReport>, Error> {
    read_baseline_with_output(path, automatic, &mut runner_diagnostics())
}

fn write_baseline_warning(output: &mut impl io::Write, detail: &str) -> Result<(), Error> {
    writeln!(output, "metabench: {}", terminal_text(detail)).map_err(Error::ConsoleOutput)
}

fn read_baseline_with_output(path: &Path, automatic: bool, output: &mut impl io::Write) -> Result<Option<BenchmarkReport>, Error> {
    handle_baseline_result(BenchmarkReport::read_json(path), automatic, output)
}

fn handle_baseline_result(
    result: Result<BenchmarkReport, Error>,
    automatic: bool,
    output: &mut impl io::Write,
) -> Result<Option<BenchmarkReport>, Error> {
    match result {
        Ok(report) => Ok(Some(report)),
        Err(error @ Error::UnsupportedReportSchema { .. }) if automatic => {
            write_baseline_warning(output, &format!("skipping incompatible automatic baseline: {error}"))?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn probe_selection_arguments(criterion_arguments: &[OsString]) -> Result<Vec<OsString>, Error> {
    let mut probe_arguments = Vec::new();
    let mut value_expected = false;
    let mut discard_value = false;
    for argument in criterion_arguments {
        if discard_value {
            // #[gamma::skip(assign_value.default, reason = "bool Default is exactly the assigned false value")]
            discard_value = false;
            continue;
        }
        if value_expected {
            probe_arguments.push(argument.clone());
            // #[gamma::skip(assign_value.default, reason = "bool Default is exactly the assigned false value")]
            value_expected = false;
            continue;
        }
        let text = argument.to_string_lossy();
        let option = text.split('=').next().unwrap_or(&text);
        if criterion_measurement_only_option(option) {
            discard_value = !text.contains('=');
            continue;
        }
        if matches!(text.as_ref(), "--noplot" | "-n") {
            continue;
        }
        if matches!(text.as_ref(), "--bench" | "--test" | "--list" | "--quick") {
            continue;
        }
        // #[gamma::skip(cond.always_true, reason = "value_expected is false on entry here and criterion_option_takes_value returns false for every non-option string")]
        if text.starts_with('-') {
            value_expected = criterion_option_takes_value(&text);
        }
        probe_arguments.push(argument.clone());
    }
    if value_expected || discard_value {
        return Err(Error::Probe("missing a value for a forwarded Criterion option".to_owned()));
    }
    Ok(probe_arguments)
}

fn criterion_measurement_only_option(option: &str) -> bool {
    matches!(
        option,
        "--save-baseline" | "-s" | "--baseline" | "-b" | "--load-baseline" | "--baseline-lenient" | "--profile-time"
    )
}

fn criterion_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "--sample-size"
            | "--warm-up-time"
            | "--measurement-time"
            | "--nresamples"
            | "--noise-threshold"
            | "--confidence-level"
            | "--significance-level"
            | "--plotting-backend"
            | "--output-format"
            | "--format"
            | "--color"
            | "-c"
    )
}

fn run_allocations(
    executable: &Path,
    arguments: &Arguments,
    artifacts: &ArtifactDirectory,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    collect_discovered_targets(
        || discover_native_probe_targets(executable, artifacts, arguments),
        |targets| {
            run_allocation_targets(
                executable,
                arguments,
                artifacts,
                targets,
                |mode, executable, argv, artifacts, show, timeout, (home, discovery, allocation)| {
                    launch_worker_with(mode, executable, argv, artifacts, show, timeout, home, discovery, allocation)
                },
                allocation::read_artifact,
            )
        },
    )
}

type AllocationLaunchOptions<'a> = (Option<&'a Path>, Option<&'a Path>, Option<(&'a str, &'a Path)>);

fn collect_discovered_targets(
    discover: impl FnOnce() -> Result<Vec<ProbeTarget>, Error>,
    measure: impl FnOnce(Vec<ProbeTarget>) -> Result<Vec<crate::report::NativeResult>, Error>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    let targets = discover()?;
    measure(targets)
}

fn run_allocation_targets(
    executable: &Path,
    arguments: &Arguments,
    artifacts: &ArtifactDirectory,
    targets: Vec<ProbeTarget>,
    mut launch: impl FnMut(
        Mode,
        &Path,
        &[OsString],
        Option<&ArtifactDirectory>,
        bool,
        Option<Duration>,
        AllocationLaunchOptions<'_>,
    ) -> Result<ExitStatus, Error>,
    mut read: impl FnMut(&Path, String, String) -> Result<crate::report::NativeResult, Error>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    let mut results = Vec::with_capacity(targets.len());
    for (index, target) in targets.into_iter().enumerate() {
        let artifact_path = artifacts.path().join("allocations").join(format!("{index}.json"));
        let criterion_home = artifacts.path().join("allocation-probes").join(index.to_string());
        let native_arguments = target.arguments();
        let status = launch(
            Mode::Allocations,
            executable,
            &native_arguments,
            Some(artifacts),
            arguments.show_engine_output,
            arguments.timeout,
            (Some(&criterion_home), None, Some((&target.identity, &artifact_path))),
        )?;
        ensure_success(Mode::Allocations, status)?;
        let relative_path = artifact_path
            .strip_prefix(artifacts.root())
            .unwrap_or(&artifact_path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        results.push(read(&artifact_path, target.native_identity, relative_path)?);
    }
    Ok(results)
}

fn discover_native_probe_targets(
    executable: &Path,
    artifacts: &ArtifactDirectory,
    arguments: &Arguments,
) -> Result<Vec<ProbeTarget>, Error> {
    let home = native_discovery_home(artifacts, create_discovery_home)?;
    discover_native_probe_targets_with(
        arguments,
        artifacts,
        &home,
        |mode, native_arguments, artifacts, show_output, timeout, home, output| {
            launch_worker_with(
                mode,
                executable,
                native_arguments,
                artifacts,
                show_output,
                timeout,
                home,
                output,
                None,
            )
        },
        read_probe_targets,
    )
}

fn create_discovery_home(prefix: &str, parent: &Path) -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(sources) = TEST_CHILD_SOURCES.get() {
        return (sources.discovery)(prefix, parent);
    }
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .map(tempfile::TempDir::keep)
}

fn native_discovery_home(artifacts: &ArtifactDirectory, create: impl FnOnce(&str, &Path) -> io::Result<PathBuf>) -> Result<PathBuf, Error> {
    create("native-probe-discovery-", artifacts.path()).map_err(|source| Error::ArtifactIo {
        path: artifacts.path().to_owned(),
        source,
    })
}

fn discover_native_probe_targets_with(
    arguments: &Arguments,
    artifacts: &ArtifactDirectory,
    home: &Path,
    launch: impl FnOnce(
        Mode,
        &[OsString],
        Option<&ArtifactDirectory>,
        bool,
        Option<Duration>,
        Option<&Path>,
        Option<&Path>,
    ) -> Result<ExitStatus, Error>,
    read: impl FnOnce(&Path) -> Result<Vec<ProbeTarget>, Error>,
) -> Result<Vec<ProbeTarget>, Error> {
    let output_path = home.join("cases.json");
    let status = launch(
        Mode::Allocations,
        &["--metabench-probe-list".into()],
        Some(artifacts),
        false,
        arguments.timeout,
        Some(home),
        Some(&output_path),
    )?;
    ensure_success(Mode::Allocations, status)?;
    let targets = read(&output_path)?;
    let targets = select_native_probe_targets(targets, arguments.criterion_args())?;
    if targets.is_empty() {
        return Err(Error::ArtifactFormat {
            path: output_path,
            message: "no native benchmark cases matched; existing reports were preserved".to_owned(),
        });
    }
    Ok(targets)
}

fn read_probe_targets(path: &Path) -> Result<Vec<ProbeTarget>, Error> {
    read_probe_targets_with(path, artifact::read_json)
}

fn read_probe_targets_with(path: &Path, read: impl FnOnce(&Path) -> Result<Vec<ProbeTarget>, Error>) -> Result<Vec<ProbeTarget>, Error> {
    let targets = read(path)?;
    if targets.len() > crate::probe::MAX_PROBE_CASES {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("native discovery exceeds the {}-case limit", crate::probe::MAX_PROBE_CASES),
        });
    }
    Ok(targets)
}

fn select_native_probe_targets(targets: Vec<ProbeTarget>, arguments: &[OsString]) -> Result<Vec<ProbeTarget>, Error> {
    let selection = probe_selection_arguments(arguments)?;
    let mut filter = None;
    let mut exact = false;
    let mut value_expected = false;
    for argument in &selection {
        if value_expected {
            // #[gamma::skip(assign_value.default, reason = "bool Default is exactly the assigned false value")]
            value_expected = false;
            continue;
        }
        let text = argument.to_string_lossy();
        if text == "--exact" {
            exact = true;
        } else if text.starts_with('-') {
            let option = text.split('=').next().unwrap_or(&text);
            if criterion_option_takes_value(option) {
                value_expected = !text.contains('=');
            } else if !matches!(option, "--verbose" | "--quiet" | "--debug" | "--discard-baseline" | "-v") {
                return Err(Error::Probe(format!(
                    "unsupported Criterion option for native probe selection: {text}"
                )));
            }
        } else if filter.replace(text.into_owned()).is_some() {
            return Err(Error::Probe("only one case selection filter is supported".to_owned()));
        }
    }
    let pattern = filter
        .as_deref()
        .filter(|_| !exact)
        .map(regex::Regex::new)
        .transpose()
        .map_err(|error| Error::Probe(format!("invalid case filter: {error}")))?;
    Ok(targets
        .into_iter()
        .filter(|target| {
            filter.as_deref().is_none_or(|filter| {
                if exact {
                    target.identity == filter
                } else {
                    pattern.as_ref().is_some_and(|pattern| pattern.is_match(&target.identity))
                }
            })
        })
        .collect())
}

fn run_perf(executable: &Path, arguments: &Arguments, artifacts: &ArtifactDirectory) -> Result<Vec<crate::report::NativeResult>, Error> {
    require_perf_support(cfg!(target_os = "linux"))?;
    collect_discovered_targets(
        || discover_native_probe_targets(executable, artifacts, arguments),
        |targets| {
            run_counter_targets(
                Mode::Perf,
                executable,
                arguments,
                artifacts,
                targets,
                launch_counter_worker,
                parse_counter_artifact,
            )
        },
    )
}

fn require_perf_support(supported: bool) -> Result<(), Error> {
    if !supported {
        return Err(Error::UnsupportedMode {
            mode: Mode::Perf.as_str(),
            reason: "Linux perf is supported only on Linux",
        });
    }
    Ok(())
}

type CounterPaths<'a> = (&'a Path, &'a Path, Option<&'a Path>);

fn run_counter_targets(
    mode: Mode,
    executable: &Path,
    arguments: &Arguments,
    artifacts: &ArtifactDirectory,
    targets: Vec<ProbeTarget>,
    mut launch: impl FnMut(Mode, &Path, &[OsString], &[OsString], CounterPaths<'_>, bool, Option<Duration>) -> Result<ExitStatus, Error>,
    mut parse: impl FnMut(Mode, &Path, &Path, String, String) -> Result<crate::report::NativeResult, Error>,
) -> Result<Vec<crate::report::NativeResult>, Error> {
    let mut results = Vec::with_capacity(targets.len());
    for (index, target) in targets.into_iter().enumerate() {
        let (artifact_path, criterion_home, result_dir) = match mode {
            Mode::Perf => (
                artifacts.perf_path(index),
                artifacts.path().join("perf-probes").join(index.to_string()),
                None,
            ),
            Mode::Vtune => (
                artifacts.vtune_path(index),
                artifacts.path().join("vtune-probes").join(index.to_string()),
                Some(artifacts.vtune_result_path(index)),
            ),
            Mode::Criterion | Mode::Gungraun | Mode::Allocations => unreachable!("only counter collectors have counter artifacts"),
        };
        let native_arguments = target.arguments();
        let status = launch(
            mode,
            executable,
            &native_arguments,
            arguments.args_for(mode),
            (&artifact_path, &criterion_home, result_dir.as_deref()),
            arguments.show_engine_output,
            arguments.timeout,
        )?;
        ensure_success(mode, status)?;
        results.push(parse(
            mode,
            &artifact_path,
            artifacts.root(),
            target.identity,
            target.native_identity,
        )?);
    }
    Ok(results)
}

fn launch_counter_worker(
    mode: Mode,
    executable: &Path,
    argv: &[OsString],
    collector_arguments: &[OsString],
    (artifact, home, result): CounterPaths<'_>,
    show: bool,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    match mode {
        Mode::Perf => launch_perf_worker(executable, argv, collector_arguments, artifact, home, show, timeout),
        Mode::Vtune => launch_vtune_worker(
            executable,
            argv,
            collector_arguments,
            artifact,
            result.expect("VTune requests always include their result directory"),
            home,
            show,
            timeout,
        ),
        Mode::Criterion | Mode::Gungraun | Mode::Allocations => unreachable!("only counter collectors launch counter workers"),
    }
}

fn parse_counter_artifact(
    mode: Mode,
    artifact: &Path,
    root: &Path,
    identity: String,
    native: String,
) -> Result<crate::report::NativeResult, Error> {
    match mode {
        Mode::Perf => artifact::parse_perf(artifact, root, identity, native),
        Mode::Vtune => artifact::parse_vtune(artifact, root, identity, native),
        Mode::Criterion | Mode::Gungraun | Mode::Allocations => unreachable!("only counter collectors have counter artifacts"),
    }
}

fn launch_perf_worker(
    executable: &Path,
    native_arguments: &[OsString],
    perf_arguments: &[OsString],
    artifact_path: &Path,
    criterion_home: &Path,
    show_output: bool,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    #[cfg(test)]
    use self::scripted_perf_worker as launch_perf_worker_native;
    launch_perf_worker_native(
        executable,
        native_arguments,
        perf_arguments,
        artifact_path,
        criterion_home,
        show_output,
        timeout,
    )
}

#[cfg(test)]
fn scripted_perf_worker(
    executable: &Path,
    native_arguments: &[OsString],
    perf_arguments: &[OsString],
    artifact_path: &Path,
    criterion_home: &Path,
    show_output: bool,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    if let Some(launch) = TEST_PERF_LAUNCH.get() {
        return launch(PerfWorkerRequest {
            executable,
            native_arguments,
            collector_arguments: perf_arguments,
            artifact: artifact_path,
            home: criterion_home,
            show: show_output,
            timeout,
        });
    }
    launch_perf_worker_native(
        executable,
        native_arguments,
        perf_arguments,
        artifact_path,
        criterion_home,
        show_output,
        timeout,
    )
}

#[cfg(target_os = "linux")]
fn launch_perf_worker_native(
    executable: &Path,
    native_arguments: &[OsString],
    perf_arguments: &[OsString],
    artifact_path: &Path,
    criterion_home: &Path,
    show_output: bool,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    validate_perf_arguments(perf_arguments)?;
    let fifo_directory = tempfile::Builder::new()
        .prefix("metabench-perf-")
        .tempdir()
        .map_err(Error::PerfControl)?;
    let control = fifo_directory.path().join("control");
    let acknowledgement = fifo_directory.path().join("ack");
    let control_text = control
        .to_str()
        .ok_or_else(|| Error::PerfControl(io::Error::new(io::ErrorKind::InvalidInput, "perf control FIFO path is not UTF-8")))?;
    let acknowledgement_text = acknowledgement.to_str().ok_or_else(|| {
        Error::PerfControl(io::Error::new(
            io::ErrorKind::InvalidInput,
            "perf acknowledgement FIFO path is not UTF-8",
        ))
    })?;
    if control_text.contains(',') || acknowledgement_text.contains(',') {
        return Err(Error::PerfControl(io::Error::new(
            io::ErrorKind::InvalidInput,
            "perf control FIFO paths cannot contain commas",
        )));
    }
    mkfifo(&control, FileMode::S_IRUSR | FileMode::S_IWUSR).map_err(|error| Error::PerfControl(error.into()))?;
    mkfifo(&acknowledgement, FileMode::S_IRUSR | FileMode::S_IWUSR).map_err(|error| Error::PerfControl(error.into()))?;
    let control_guard = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&control)
        .map_err(Error::PerfControl)?;
    let acknowledgement_guard = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&acknowledgement)
        .map_err(Error::PerfControl)?;
    if let Some(parent) = artifact_path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }

    let control_argument = format!("fifo:{control_text},{acknowledgement_text}");
    let mut command = perf_command(
        executable,
        native_arguments,
        perf_arguments,
        artifact_path,
        criterion_home,
        &control_argument,
        (&control, &acknowledgement),
    );
    let log_path = collector_log_path(artifact_path, false);
    let log = capture_worker_output(&mut command, &log_path, show_output)?;

    let token = create_worker_token()?;
    authorize_worker(&mut command, token.path());
    let status = wait_for_logged_worker(Mode::Perf, &mut command, timeout, &log_path, log);
    drop(control_guard);
    drop(acknowledgement_guard);
    let cleanup = match worker_token_exists(token.path()) {
        Ok(true) => remove_worker_token(token.path()).map_err(Error::ConsumeWorkerToken),
        Ok(false) => Ok(()),
        Err(error) => Err(Error::ConsumeWorkerToken(error)),
    };
    combine_results(status, cleanup)
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(test, mutants::skip)] // `run_perf` rejects non-Linux hosts before this fallback can be called.
// #[gamma::skip(parameter.default_shadow, reason = "this non-Linux fallback ignores every parameter and returns the same fixed UnsupportedMode error, so defaulting show_output or timeout cannot affect its result or effects")]
fn launch_perf_worker_native(
    _executable: &Path,
    _native_arguments: &[OsString],
    _perf_arguments: &[OsString],
    _artifact_path: &Path,
    _criterion_home: &Path,
    _show_output: bool,
    _timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    Err(Error::UnsupportedMode {
        mode: Mode::Perf.as_str(),
        reason: "Linux perf is supported only on Linux",
    })
}

#[cfg(any(target_os = "linux", test))]
fn validate_perf_arguments(arguments: &[OsString]) -> Result<(), Error> {
    for argument in arguments {
        let value = argument.to_string_lossy();
        // #[gamma::skip(collection.reverse_array, reason = "any checks pure reserved-option predicates and always returns the same argument error independent of lookup order")]
        // #[gamma::skip(logical.and_remove_right, relational.gt_to_ge, literal.int_decrement, literal.int_to_zero, reason = "the only length predicates here follow two-byte prefixes; their length-two cases were already rejected by the exact reserved-option lookup, so weakening these three predicates cannot change the result or first error")]
        if [
            "--",
            "-o",
            "-r",
            "-I",
            "--output",
            "--json-output",
            "--control",
            "--delay",
            "--repeat",
            "--interval-print",
        ]
        .iter()
        .any(|reserved| value == *reserved || value.starts_with(&format!("{reserved}=")))
            || value.starts_with("-o") && value.len() > 2
            || value.starts_with("-r") && value.len() > 2
            || value.starts_with("-I") && value.len() > 2
        {
            return Err(Error::UnsupportedPerfArgument(argument.clone()));
        }
    }
    Ok(())
}

fn run_vtune(executable: &Path, arguments: &Arguments, artifacts: &ArtifactDirectory) -> Result<Vec<crate::report::NativeResult>, Error> {
    collect_discovered_targets(
        || discover_native_probe_targets(executable, artifacts, arguments),
        |targets| {
            run_counter_targets(
                Mode::Vtune,
                executable,
                arguments,
                artifacts,
                targets,
                launch_counter_worker,
                parse_counter_artifact,
            )
        },
    )
}

/// Launches a benchmark worker under a paused `VTune` collection and, once it
/// exits, extracts a CSV hardware-event report from the collection result.
///
/// This mirrors `launch_perf_worker`'s shape, but `VTune`'s own CLI takes the
/// place of the FIFO-based `perf --control` protocol: the collection starts
/// paused (`--start-paused`) and is resumed/paused around the annotated
/// workload by [`crate::vtune::begin`]/[`crate::vtune::Guard::drop`] via
/// `vtune -command resume|pause -r <result-dir>`, run from inside the worker
/// process. The exact `vtune` invocation below (analysis type, knobs, and
/// report format) is our best documented understanding of the `VTune`
/// command-line interface and has not been verified against a live
/// installation in this repository's environment; adjust it here if a real
/// `VTune` version's CLI differs. See [`crate::vtune`] for the control-protocol
/// references.
#[expect(
    clippy::too_many_arguments,
    reason = "worker process configuration is explicit at the process boundary"
)]
fn launch_vtune_worker(
    executable: &Path,
    native_arguments: &[OsString],
    vtune_arguments: &[OsString],
    artifact_path: &Path,
    result_dir: &Path,
    criterion_home: &Path,
    show_output: bool,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    validate_vtune_arguments(vtune_arguments)?;
    launch_vtune_commands_with(
        result_dir,
        artifact_path,
        (show_output, timeout),
        worker_create_directory,
        || vtune_collect_command(executable, native_arguments, vtune_arguments, result_dir, criterion_home),
        |command, log, show, timeout| {
            run_worker_command_with(
                command,
                Some(log),
                (show, timeout),
                capture_worker_output,
                || {
                    create_worker_token().map(|token| {
                        let path = token.path().to_owned();
                        (token, path)
                    })
                },
                |command, log, captured, token, timeout| execute_authorized_worker(Mode::Vtune, command, log, captured, &token, timeout),
            )
        },
        |command, path, show, timeout| {
            run_report_command_with(
                command,
                path,
                (show, timeout),
                capture_worker_output,
                |command, timeout, path, log| wait_for_logged_worker(Mode::Vtune, command, timeout, path, log),
            )
        },
    )
}

fn launch_vtune_commands_with(
    result_dir: &Path,
    artifact_path: &Path,
    (show_output, timeout): (bool, Option<Duration>),
    mut create_parent: impl FnMut(&Path) -> io::Result<()>,
    collect_command: impl FnOnce() -> Command,
    collect: impl FnOnce(&mut Command, &Path, bool, Option<Duration>) -> Result<ExitStatus, Error>,
    report: impl FnOnce(&mut Command, &Path, bool, Option<Duration>) -> Result<ExitStatus, Error>,
) -> Result<ExitStatus, Error> {
    if let Some(parent) = result_dir.parent() {
        create_parent(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }
    if let Some(parent) = artifact_path.parent() {
        create_parent(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }

    let mut command = collect_command();
    let log_path = collector_log_path(artifact_path, false);
    let status = collect(&mut command, &log_path, show_output, timeout);
    finish_vtune_collection(status, Ok(()), || {
        let mut report_command = vtune_report_command(result_dir, artifact_path);
        let report_log = collector_log_path(artifact_path, true);
        report(&mut report_command, &report_log, show_output, timeout)
    })
}

fn run_report_command_with<L>(
    command: &mut Command,
    path: &Path,
    (show, timeout): (bool, Option<Duration>),
    capture: impl FnOnce(&mut Command, &Path, bool) -> Result<Option<L>, Error>,
    wait: impl FnOnce(&mut Command, Option<Duration>, &Path, Option<L>) -> Result<ExitStatus, Error>,
) -> Result<ExitStatus, Error> {
    let log = capture(command, path, show)?;
    wait(command, timeout, path, log)
}

fn finish_vtune_collection(
    status: Result<ExitStatus, Error>,
    cleanup: Result<(), Error>,
    report: impl FnOnce() -> Result<ExitStatus, Error>,
) -> Result<ExitStatus, Error> {
    let status = combine_results(status, cleanup)?;
    if !status.success() {
        return Ok(status);
    }
    report()
}

fn validate_vtune_arguments(arguments: &[OsString]) -> Result<(), Error> {
    for argument in arguments {
        let value = argument.to_string_lossy();
        // #[gamma::skip(collection.reverse_array, reason = "any tests pure reserved-option predicates and returns the same error containing the original argument independent of reserved-name lookup order")]
        if [
            "--",
            "-r",
            "-collect",
            "-c",
            "-collect-with",
            "-result-dir",
            "-start-paused",
            "--start-paused",
            "-report",
            "-R",
            "-report-output",
            "-format",
            "-command",
            "-C",
        ]
        .iter()
        .any(|reserved| value == *reserved || value.starts_with(&format!("{reserved}=")))
        {
            return Err(Error::UnsupportedVtuneArgument(argument.clone()));
        }
    }
    Ok(())
}

fn launch_worker(
    mode: Mode,
    executable: &Path,
    native_arguments: &[OsString],
    artifacts: Option<&ArtifactDirectory>,
    show_output: bool,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    launch_worker_request(
        mode,
        artifacts,
        show_output,
        timeout,
        cfg!(target_os = "linux"),
        |mode, artifacts, show, timeout| launch_worker_with(mode, executable, native_arguments, artifacts, show, timeout, None, None, None),
    )
}

fn launch_worker_request(
    mode: Mode,
    artifacts: Option<&ArtifactDirectory>,
    show_output: bool,
    timeout: Option<Duration>,
    linux: bool,
    launch: impl FnOnce(Mode, Option<&ArtifactDirectory>, bool, Option<Duration>) -> Result<ExitStatus, Error>,
) -> Result<ExitStatus, Error> {
    // Gungraun measures via Valgrind/Callgrind, which is only available on Linux. An
    // explicit `--gungraun` request off Linux must fail clearly here rather than
    // spawning a worker that cannot actually measure anything.
    if mode == Mode::Gungraun && !linux {
        return Err(Error::UnsupportedMode {
            mode: Mode::Gungraun.as_str(),
            reason: "Gungraun is supported only on Linux",
        });
    }
    launch(mode, artifacts, show_output, timeout)
}

#[expect(
    clippy::too_many_arguments,
    reason = "worker process configuration is explicit at the process boundary"
)]
fn launch_worker_with(
    mode: Mode,
    executable: &Path,
    native_arguments: &[OsString],
    artifacts: Option<&ArtifactDirectory>,
    show_output: bool,
    timeout: Option<Duration>,
    criterion_home: Option<&Path>,
    discovery_output: Option<&Path>,
    allocation_target: Option<(&str, &Path)>,
) -> Result<ExitStatus, Error> {
    let mut command = native_worker_command(
        mode,
        executable,
        native_arguments,
        artifacts,
        criterion_home,
        discovery_output,
        allocation_target,
    );
    let log_path = worker_log_path(mode, criterion_home, artifacts);
    run_worker_command_with(
        &mut command,
        log_path.as_deref(),
        (show_output, timeout),
        capture_worker_output,
        || {
            create_worker_token().map(|token| {
                let path = token.path().to_owned();
                (token, path)
            })
        },
        |command, log_path, log, token, timeout| execute_authorized_worker(mode, command, log_path, log, &token, timeout),
    )
}

fn execute_authorized_worker(
    mode: Mode,
    command: &mut Command,
    log_path: Option<&Path>,
    log: Option<WorkerLog>,
    token: &WorkerToken,
    timeout: Option<Duration>,
) -> Result<ExitStatus, Error> {
    let status = match log_path {
        Some(path) => wait_for_logged_worker(mode, command, timeout, path, log),
        None => wait_for_worker(mode, command, timeout),
    };
    let cleanup = match worker_token_exists(token.path()) {
        Ok(true) => remove_worker_token(token.path()).map_err(Error::ConsumeWorkerToken),
        Ok(false) => Ok(()),
        Err(error) => Err(Error::ConsumeWorkerToken(error)),
    };
    combine_results(status, cleanup)
}

fn run_worker_command_with<T, L>(
    command: &mut Command,
    log_path: Option<&Path>,
    (show_output, timeout): (bool, Option<Duration>),
    capture: impl FnOnce(&mut Command, &Path, bool) -> Result<Option<L>, Error>,
    token: impl FnOnce() -> Result<(T, PathBuf), Error>,
    execute: impl FnOnce(&mut Command, Option<&Path>, Option<L>, T, Option<Duration>) -> Result<ExitStatus, Error>,
) -> Result<ExitStatus, Error> {
    let log = log_path.map(|path| capture(command, path, show_output)).transpose()?.flatten();
    let (token, token_path) = token()?;
    // #[gamma::skip(call_result.default, reason = "authorize_worker returns unit and default_after preserves the command environment update")]
    authorize_worker(command, &token_path);
    execute(command, log_path, log, token, timeout)
}

fn clear_collector_environment(command: &mut Command) {
    // #[gamma::skip(collection.reverse_array, reason = "distinct Command environment removals commute and get_envs exposes them in sorted order")]
    for name in [
        allocation::ARTIFACT_ENV,
        allocation::IDENTITY_ENV,
        perf::CONTROL_ENV,
        perf::ACK_ENV,
        vtune::RESULT_DIR_ENV,
        crate::probe::DISCOVERY_ENV,
    ] {
        command.env_remove(name);
    }
}

fn authorize_worker(command: &mut Command, token: &Path) {
    command.env(WORKER_TOKEN_ENV, token);
}

fn collector_log_path(artifact: &Path, report: bool) -> PathBuf {
    artifact.with_extension(if report { "report.log" } else { "log" })
}

fn worker_log_path(mode: Mode, criterion_home: Option<&Path>, artifacts: Option<&ArtifactDirectory>) -> Option<PathBuf> {
    let log_root = criterion_home
        .map(Path::to_owned)
        .or_else(|| artifacts.map(|artifacts| artifacts.path().join("logs")));
    log_root.map(|root| root.join(format!("{}.log", mode.as_str())))
}

fn native_worker_command(
    mode: Mode,
    executable: &Path,
    native_arguments: &[OsString],
    artifacts: Option<&ArtifactDirectory>,
    criterion_home: Option<&Path>,
    discovery_output: Option<&Path>,
    allocation_target: Option<(&str, &Path)>,
) -> Command {
    let mut command = Command::new(executable);
    // #[gamma::skip(call_result.default, reason = "clear_collector_environment returns unit and default_after preserves every env_remove")]
    clear_collector_environment(&mut command);
    command.args(native_arguments).env(WORKER_MODE_ENV, mode.as_str());
    if let Some(artifacts) = artifacts {
        match mode {
            Mode::Criterion | Mode::Allocations => {
                command
                    .env(
                        "CRITERION_HOME",
                        criterion_home.map_or_else(|| artifacts.criterion_path(), Path::to_owned),
                    )
                    .env_remove("CARGO_CRITERION_PORT");
            }
            Mode::Gungraun => {
                command
                    .env("GUNGRAUN_HOME", artifacts.gungraun_path())
                    .env("GUNGRAUN_SAVE_SUMMARY", "json");
            }
            Mode::Perf => unreachable!("perf workers use launch_perf_worker"),
            Mode::Vtune => unreachable!("vtune workers use launch_vtune_worker"),
        }
    }
    if let Some((identity, path)) = allocation_target {
        command.env(allocation::IDENTITY_ENV, identity).env(allocation::ARTIFACT_ENV, path);
    }
    if let Some(path) = discovery_output {
        command.env(crate::probe::DISCOVERY_ENV, path);
    }
    command
}

fn vtune_collect_command(
    executable: &Path,
    native_arguments: &[OsString],
    vtune_arguments: &[OsString],
    result_dir: &Path,
    criterion_home: &Path,
) -> Command {
    let mut command = Command::new("vtune");
    // #[gamma::skip(call_result.default, reason = "clear_collector_environment returns unit and default_after preserves every env_remove")]
    clear_collector_environment(&mut command);
    command
        .arg("-collect-with")
        .arg("runsa")
        .args(vtune_arguments)
        .arg("--start-paused")
        .arg("-result-dir")
        .arg(result_dir)
        .env(WORKER_MODE_ENV, Mode::Vtune.as_str())
        .env("CRITERION_HOME", criterion_home)
        .env_remove("CARGO_CRITERION_PORT")
        .env(vtune::RESULT_DIR_ENV, result_dir)
        .arg("--")
        .arg(executable)
        .args(native_arguments);
    command
}

fn vtune_report_command(result_dir: &Path, artifact_path: &Path) -> Command {
    let mut command = Command::new("vtune");
    // #[gamma::skip(call_result.default, reason = "clear_collector_environment returns unit and default_after preserves every env_remove")]
    clear_collector_environment(&mut command);
    command
        .args(["-report", "hw-events", "-r"])
        .arg(result_dir)
        .args(["-format", "csv", "-report-output"])
        .arg(artifact_path);
    command
}

#[cfg(any(target_os = "linux", test))]
fn perf_command(
    executable: &Path,
    native_arguments: &[OsString],
    perf_arguments: &[OsString],
    artifact_path: &Path,
    criterion_home: &Path,
    control_argument: &str,
    channels: (&Path, &Path),
) -> Command {
    let mut command = Command::new("perf");
    // #[gamma::skip(call_result.default, reason = "clear_collector_environment returns unit and default_after preserves every env_remove")]
    clear_collector_environment(&mut command);
    command
        .arg("stat")
        .args(perf_arguments)
        .args([
            "--event=instructions:u",
            "--event=cycles:u",
            "--event=branches:u",
            "--event=branch-misses:u",
            "--event=cache-references:u",
            "--event=cache-misses:u",
            "--delay=-1",
            "--json-output",
            "--output",
        ])
        .arg(artifact_path)
        .arg("--control")
        .arg(control_argument)
        .arg("--")
        .arg(executable)
        .args(native_arguments)
        .env(WORKER_MODE_ENV, Mode::Perf.as_str())
        .env("CRITERION_HOME", criterion_home)
        .env_remove("CARGO_CRITERION_PORT")
        .env(perf::CONTROL_ENV, channels.0)
        .env(perf::ACK_ENV, channels.1);
    command
}

pub(crate) fn terminal_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control()
            || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output
}

fn capture_worker_output(command: &mut Command, path: &Path, show_output: bool) -> Result<Option<WorkerLog>, Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_CHILD_SOURCES.get() {
        return (sources.capture)(command, path, show_output);
    }
    capture_worker_output_with(
        command,
        path,
        show_output,
        |parent| fs::create_dir_all(parent),
        |path, command| {
            let log = fs::OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
            WorkerLog::new(log, command)
        },
    )
}

fn capture_worker_output_with<L>(
    command: &mut Command,
    path: &Path,
    show_output: bool,
    create_parent: impl FnOnce(&Path) -> io::Result<()>,
    capture: impl FnOnce(&Path, &mut Command) -> io::Result<L>,
) -> Result<Option<L>, Error> {
    if show_output {
        return Ok(None);
    }
    if let Some(parent) = path.parent() {
        create_parent(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }
    capture(path, command).map(Some).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })
}

fn wait_for_logged_worker(
    mode: Mode,
    command: &mut Command,
    timeout: Option<Duration>,
    path: &Path,
    log: Option<WorkerLog>,
) -> Result<ExitStatus, Error> {
    wait_for_logged_worker_with_output(mode, command, timeout, path, log, &mut runner_diagnostics())
}

fn wait_for_logged_worker_with_output(
    mode: Mode,
    command: &mut Command,
    timeout: Option<Duration>,
    path: &Path,
    log: Option<WorkerLog>,
    output: &mut impl io::Write,
) -> Result<ExitStatus, Error> {
    let Some(mut log) = log else {
        return wait_for_worker(mode, command, timeout);
    };
    let result = wait_for_captured_worker(mode, command, timeout, path, &mut log);
    let persisted = log.persist().map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    });
    let result = combine_worker_results(mode, result, persisted);
    if !result.as_ref().is_ok_and(ExitStatus::success) {
        let diagnostics = write_worker_excerpt(output, mode, path, &log.excerpt(), log.truncated()).map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        });
        return combine_worker_results(mode, result, diagnostics);
    }
    result
}

fn combine_worker_results(mode: Mode, result: Result<ExitStatus, Error>, supplemental: Result<(), Error>) -> Result<ExitStatus, Error> {
    let result = if supplemental.is_err() {
        result.and_then(|status| ensure_success(mode, status).map(|()| status))
    } else {
        result
    };
    combine_results(result, supplemental)
}

#[cfg(test)]
fn write_worker_log(output: &mut impl io::Write, mode: Mode, path: &Path, mut log: &fs::File, truncated: bool) -> io::Result<()> {
    let length = log.metadata()?.len();
    log.seek(io::SeekFrom::Start(length.saturating_sub(8_192)))?;
    let mut bytes = Vec::new();
    log.take(8_192).read_to_end(&mut bytes)?;
    write_worker_excerpt(output, mode, path, &bytes, truncated)
}

fn write_worker_excerpt(output: &mut impl io::Write, mode: Mode, path: &Path, bytes: &[u8], truncated: bool) -> io::Result<()> {
    writeln!(
        output,
        "metabench: {} worker log: {}{}\n{}",
        mode.as_str(),
        terminal_text(&path.to_string_lossy()),
        if truncated { " (output truncated; retained tail)" } else { "" },
        terminal_text(&String::from_utf8_lossy(bytes))
    )
}

fn wait_for_captured_worker(
    mode: Mode,
    command: &mut Command,
    timeout: Option<Duration>,
    path: &Path,
    log: &mut WorkerLog,
) -> Result<ExitStatus, Error> {
    let spawned = spawn_worker(command).map_err(|source| spawn_error(mode, source));
    WorkerLog::release_command_writers(command);
    let mut child = spawned?;
    wait_for_captured_process(mode, &mut child, timeout, path, || log.drain(), &SystemClock)
}

/// Waits for worker completion and output EOF, bounding inherited writers' final drain.
///
/// Read errors terminate and reap immediately instead of entering the final-drain state.
fn wait_for_captured_process(
    mode: Mode,
    child: &mut impl WorkerProcess,
    timeout: Option<Duration>,
    path: &Path,
    mut drain: impl FnMut() -> io::Result<(bool, bool)>,
    clock: &impl Clock,
) -> Result<ExitStatus, Error> {
    let started = clock.mark();
    let mut completed = None;
    let mut drain_started = None;
    loop {
        let (eof, had_output) = match drain() {
            Ok(state) => state,
            Err(source) => {
                let error = Error::ArtifactIo {
                    path: path.to_owned(),
                    source,
                };
                let cleanup = kill_and_reap_worker(mode, child, clock);
                return combine_results(Err(error), cleanup);
            }
        };
        if completed.is_none() {
            completed = match child.try_wait() {
                Ok(Some(status)) => Some(Ok(status)),
                Ok(None) => match timeout {
                    Some(timeout) if clock.elapsed(&started) >= timeout => {
                        // Termination and reaping consume the same grace as the remaining output drain.
                        drain_started = Some(clock.mark());
                        Some(terminate_timed_out_worker(mode, child, timeout, clock))
                    }
                    _ => None,
                },
                Err(source) => {
                    let error = spawn_error(mode, source);
                    drain_started = Some(clock.mark());
                    let cleanup = kill_and_reap_worker(mode, child, clock);
                    // #[gamma::skip(option.some_to_none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                    Some(combine_results(Err(error), cleanup))
                }
            };
            if completed.is_some() && drain_started.is_none() {
                drain_started = Some(clock.mark());
            }
        }
        if (eof
            || drain_started
                .as_ref()
                .is_some_and(|mark| clock.elapsed(mark) >= TERMINATION_REAP_GRACE))
            && let Some(result) = completed.take()
        {
            if eof {
                return result;
            }
            let error = Error::ArtifactIo {
                path: path.to_owned(),
                source: io::Error::new(io::ErrorKind::TimedOut, "worker output pipe remained open after the drain deadline"),
            };
            let cleanup = kill_and_reap_worker(mode, child, clock);
            return combine_results(combine_results(result, Err(error)), cleanup);
        }
        clock.sleep(if had_output {
            Duration::from_millis(1)
        } else {
            WORKER_POLL_INTERVAL
        });
    }
}

fn kill_and_reap_worker(mode: Mode, child: &mut impl WorkerProcess, clock: &impl Clock) -> Result<(), Error> {
    child.kill().map_err(|source| termination_error(mode, source))?;
    let started = clock.mark();
    loop {
        match child.try_wait().map_err(|source| termination_error(mode, source))? {
            Some(_) => return Ok(()),
            None if clock.elapsed(&started) < TERMINATION_REAP_GRACE => clock.sleep(WORKER_POLL_INTERVAL),
            None => {
                return Err(termination_error(
                    mode,
                    io::Error::new(io::ErrorKind::TimedOut, "worker did not exit before the termination reap deadline"),
                ));
            }
        }
    }
}

fn wait_for_worker(mode: Mode, command: &mut Command, timeout: Option<Duration>) -> Result<ExitStatus, Error> {
    let mut child = spawn_worker(command).map_err(|source| Error::Spawn {
        mode: mode.as_str(),
        source,
    })?;
    wait_for_process(mode, &mut child, timeout, &SystemClock)
}

enum WorkerChild {
    Native(GroupChild),
    #[cfg(test)]
    Injected(Box<dyn WorkerProcess>),
}

fn spawn_worker(command: &mut Command) -> io::Result<WorkerChild> {
    #[cfg(test)]
    if let Some(sources) = TEST_CHILD_SOURCES.get() {
        return (sources.spawn)(command).map(WorkerChild::Injected);
    }
    command.group_spawn().map(WorkerChild::Native)
}

impl WorkerProcess for WorkerChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self {
            Self::Native(child) => child.try_wait(),
            #[cfg(test)]
            Self::Injected(child) => child.try_wait(),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        match self {
            Self::Native(child) => child.kill(),
            #[cfg(test)]
            Self::Injected(child) => child.kill(),
        }
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        match self {
            Self::Native(child) => child.wait(),
            #[cfg(test)]
            Self::Injected(child) => child.wait(),
        }
    }
}

trait WorkerProcess {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
}

impl WorkerProcess for GroupChild {
    // #[gamma::skip(fn_value.ok, reason = "this adapter polls a native process group; deterministic WorkerProcess tests cover pending, completed and failed polls without launching a process")]
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.try_wait()
    }

    // #[gamma::skip(fn_value.ok, reason = "native process-group termination cannot be observed without a prohibited live child process; deterministic actual wait policies separately assert one kill and exact reap/error order")]
    fn kill(&mut self) -> io::Result<()> {
        self.kill()
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait()
    }
}

trait Clock {
    type Mark;

    fn mark(&self) -> Self::Mark;
    fn elapsed(&self, mark: &Self::Mark) -> Duration;
    fn sleep(&self, duration: Duration);
}

struct SystemClock;

enum ClockMark {
    Native(Instant),
    #[cfg(test)]
    Injected(Duration),
}

impl Clock for SystemClock {
    type Mark = ClockMark;

    fn mark(&self) -> Self::Mark {
        #[cfg(test)]
        if let Some(sources) = TEST_CHILD_SOURCES.get() {
            return ClockMark::Injected((sources.time)());
        }
        ClockMark::Native(Instant::now())
    }

    fn elapsed(&self, mark: &Self::Mark) -> Duration {
        match mark {
            ClockMark::Native(mark) => mark.elapsed(),
            #[cfg(test)]
            ClockMark::Injected(mark) => {
                let sources = TEST_CHILD_SOURCES
                    .get()
                    .expect("injected clock marks are consumed within their scoped test source");
                (sources.time)().saturating_sub(*mark)
            }
        }
    }

    fn sleep(&self, duration: Duration) {
        #[cfg(test)]
        if let Some(sources) = TEST_CHILD_SOURCES.get() {
            return (sources.sleep)(duration);
        }
        // #[gamma::skip(stmt.delete_call, reason = "the scheduler handoff itself requires prohibited real waiting to observe; actual SystemClock and captured/uncaptured deadline policies are tested with exact injected sleep durations")]
        thread::sleep(duration);
    }
}

fn wait_for_process(
    mode: Mode,
    child: &mut impl WorkerProcess,
    timeout: Option<Duration>,
    clock: &impl Clock,
) -> Result<ExitStatus, Error> {
    let Some(timeout) = timeout else {
        return child.wait().map_err(|source| spawn_error(mode, source));
    };
    let started = clock.mark();
    loop {
        if let Some(status) = child.try_wait().map_err(|source| spawn_error(mode, source))? {
            return Ok(status);
        }
        if clock.elapsed(&started) >= timeout {
            return terminate_timed_out_worker(mode, child, timeout, clock);
        }
        clock.sleep(WORKER_POLL_INTERVAL);
    }
}

fn terminate_timed_out_worker(
    mode: Mode,
    child: &mut impl WorkerProcess,
    timeout: Duration,
    clock: &impl Clock,
) -> Result<ExitStatus, Error> {
    let timed_out = || Error::WorkerTimedOut {
        mode: mode.as_str(),
        timeout,
    };
    if let Err(kill) = child.kill() {
        return match child.try_wait() {
            Ok(Some(status)) => Ok(status),
            Ok(None) => Err(termination_error(mode, kill)),
            Err(wait) => Err(multiple_failures([termination_error(mode, kill), termination_error(mode, wait)])),
        };
    }

    let termination_started = clock.mark();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return Err(timed_out()),
            Ok(None) if
            // #[gamma::skip(match_guard.always_true, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            clock.elapsed(&termination_started) < TERMINATION_REAP_GRACE => {
                // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                clock.sleep(WORKER_POLL_INTERVAL);
            }
            Ok(None) => return Err(timed_out()),
            Err(wait) => return Err(multiple_failures([timed_out(), termination_error(mode, wait)])),
        }
    }
}

fn spawn_error(mode: Mode, source: io::Error) -> Error {
    Error::Spawn {
        mode: mode.as_str(),
        source,
    }
}

fn termination_error(mode: Mode, source: io::Error) -> Error {
    Error::WorkerTermination {
        mode: mode.as_str(),
        source,
    }
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

fn print_progress(mode: Mode) -> Result<(), Error> {
    print_progress_with(mode, runner_output)
}

fn print_progress_with<W: io::Write>(mode: Mode, output: impl FnOnce() -> W) -> Result<(), Error> {
    let mut stdout = output();
    write_progress(&mut stdout, mode)
}

fn write_progress(mut output: impl io::Write, mode: Mode) -> Result<(), Error> {
    writeln!(output, "Running {} benchmarks...", mode.as_str()).map_err(Error::ConsoleOutput)?;
    output.flush().map_err(Error::ConsoleOutput)
}

fn write_console_report(report: &BenchmarkReport, json_path: Option<&Path>, markdown_path: Option<&Path>) -> Result<(), Error> {
    write_console_report_with(
        report,
        json_path,
        markdown_path,
        BenchmarkReport::render_console_table,
        runner_output(),
    )
}

fn write_console_report_with(
    report: &BenchmarkReport,
    json_path: Option<&Path>,
    markdown_path: Option<&Path>,
    render: impl FnOnce(&BenchmarkReport) -> String,
    destination: impl io::Write,
) -> Result<(), Error> {
    let mut output = String::from("\n");
    output.push_str(&render(report));
    output.push('\n');
    if let (Some(json_path), Some(markdown_path)) = (json_path, markdown_path) {
        let _ = writeln!(output, "Generated {}", json_path.display());
        let _ = writeln!(output, "Generated {}", markdown_path.display());
    }
    write_console_output(destination, &output)
}

fn write_console_output(mut destination: impl io::Write, output: &str) -> Result<(), Error> {
    destination.write_all(output.as_bytes()).map_err(Error::ConsoleOutput)
}

fn report_paths(arguments: &Arguments, benchmark_target: &str) -> Result<(Option<PathBuf>, Option<PathBuf>), Error> {
    report_paths_with_environment(
        arguments,
        benchmark_target,
        || current_executable(runner_executable),
        |executable| effective_target_directory(executable, runner_environment),
        report_paths_with_root,
    )
}

fn report_paths_with_environment(
    arguments: &Arguments,
    benchmark_target: &str,
    executable: impl FnOnce() -> Result<PathBuf, Error>,
    directory: impl FnOnce(&Path) -> Result<PathBuf, Error>,
    paths: impl FnOnce(&Arguments, &Path) -> Result<(Option<PathBuf>, Option<PathBuf>), Error>,
) -> Result<(Option<PathBuf>, Option<PathBuf>), Error> {
    let root = directory(&executable()?)?.join("metabench").join(benchmark_target);
    paths(arguments, &root)
}

fn report_paths_with_root(arguments: &Arguments, root: &Path) -> Result<(Option<PathBuf>, Option<PathBuf>), Error> {
    if arguments.no_output {
        return Ok((None, None));
    }
    let paths = (
        arguments.export_json.clone().or_else(|| Some(root.join("report.json"))),
        arguments.export_markdown.clone().or_else(|| Some(root.join("report.md"))),
    );
    if let (Some(json), Some(markdown)) = &paths
        && paths_alias(json, markdown)
    {
        return Err(Error::ConflictingOutputOptions);
    }
    Ok(paths)
}

fn effective_target_directory(executable: &Path, var_os: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, Error> {
    effective_target_directory_with(executable, var_os, Path::is_dir, paths_alias)
}

fn effective_target_directory_with(
    executable: &Path,
    var_os: impl Fn(&str) -> Option<OsString>,
    directory_exists: impl Fn(&Path) -> bool,
    aliases: impl Fn(&Path, &Path) -> bool,
) -> Result<PathBuf, Error> {
    let configured = var_os("CARGO_TARGET_DIR").map(PathBuf::from);
    let parent = executable.parent();
    let profile = parent.and_then(|parent| {
        if matches!(parent.file_name().and_then(OsStr::to_str), Some("deps" | "examples")) {
            parent.parent()
        } else {
            Some(parent)
        }
    });
    if let Some(profile) = profile
        && (matches!(profile.file_name().and_then(OsStr::to_str), Some("debug" | "release"))
            || directory_exists(&profile.join(".fingerprint")))
        && let Some(root) = profile.parent()
    {
        return Ok(
            if root.file_name() == Some(OsStr::new(env!("METABENCH_TARGET")))
                && !configured.as_deref().is_some_and(|configured| aliases(configured, root))
            {
                root.parent().unwrap_or(root).to_owned()
            } else {
                root.to_owned()
            },
        );
    }
    if let Some(directory) = configured {
        return Ok(directory);
    }
    Err(Error::ArtifactFormat {
        path: executable.to_owned(),
        message: "cannot infer the Cargo target directory from this executable; set CARGO_TARGET_DIR for a relocated binary".to_owned(),
    })
}

fn current_executable(current_exe: impl FnOnce() -> io::Result<PathBuf>) -> Result<PathBuf, Error> {
    current_exe().map_err(Error::CurrentExecutable)
}

enum WorkerToken {
    Native(tempfile::TempDir),
    #[cfg(test)]
    Injected(PathBuf),
}

impl WorkerToken {
    fn path(&self) -> &Path {
        match self {
            Self::Native(directory) => directory.path(),
            #[cfg(test)]
            Self::Injected(path) => path,
        }
    }
}

fn create_worker_token() -> Result<WorkerToken, Error> {
    create_worker_token_with(|prefix| {
        #[cfg(test)]
        if let Some(sources) = TEST_CHILD_SOURCES.get() {
            return (sources.token)(prefix).map(WorkerToken::Injected);
        }
        tempfile::Builder::new().prefix(prefix).tempdir().map(WorkerToken::Native)
    })
}

fn worker_token_exists(path: &Path) -> io::Result<bool> {
    #[cfg(test)]
    if let Some(sources) = TEST_CHILD_SOURCES.get() {
        return (sources.exists)(path);
    }
    path.try_exists()
}

fn worker_create_directory(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(sources) = TEST_CHILD_SOURCES.get() {
        return (sources.create_directory)(path);
    }
    fs::create_dir_all(path)
}

fn remove_worker_token(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(sources) = TEST_CHILD_SOURCES.get() {
        return (sources.remove)(path);
    }
    fs::remove_dir(path)
}

fn create_worker_token_with<T>(create: impl FnOnce(&str) -> io::Result<T>) -> Result<T, Error> {
    create("metabench-worker-").map_err(Error::CreateWorkerToken)
}

fn consume_worker_token() -> Result<(), Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_RUNNER_SOURCES.get() {
        return (sources.consume)();
    }
    let token = env::var_os(WORKER_TOKEN_ENV);
    consume_worker_token_from(token.as_deref(), &env::temp_dir())
}

fn consume_worker_token_from(token: Option<&OsStr>, temporary_directory: &Path) -> Result<(), Error> {
    let Some(token) = token else {
        return Err(Error::StaleWorkerMarker);
    };
    let token = PathBuf::from(token);
    if token.parent() != Some(temporary_directory)
        || token
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| name.strip_prefix("metabench-worker-"))
            .is_none_or(str::is_empty)
    {
        return Err(Error::StaleWorkerMarker);
    }
    fs::remove_dir(token).map_err(Error::ConsumeWorkerToken)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ChildFailure {
        Spawn,
        Exists,
        Remove,
        Discovery,
    }

    struct ObservedCommand {
        program: OsString,
        arguments: Vec<OsString>,
        environment: std::collections::BTreeMap<OsString, Option<OsString>>,
    }

    #[derive(Default)]
    struct ChildTrace {
        commands: Vec<ObservedCommand>,
        captures: Vec<(PathBuf, bool)>,
        operations: Vec<&'static str>,
        directories: Vec<PathBuf>,
        failure: Option<ChildFailure>,
        pending: usize,
        time: Duration,
        sleeps: Vec<Duration>,
        exit_code: i32,
        owned_log: Option<Vec<u8>>,
        log_files: Vec<std::rc::Rc<std::cell::RefCell<io::Cursor<Vec<u8>>>>>,
    }

    thread_local! {
        static CHILD_TRACE: std::cell::RefCell<ChildTrace> = std::cell::RefCell::new(ChildTrace::default());
    }

    struct ObservedChild {
        pending: usize,
        terminated: bool,
        exit_code: i32,
    }

    impl WorkerProcess for ObservedChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            CHILD_TRACE.with_borrow_mut(|trace| trace.operations.push("try_wait"));
            if self.pending != 0 && !self.terminated {
                self.pending -= 1;
                Ok(None)
            } else {
                Ok(Some(exit_status(self.exit_code)))
            }
        }

        fn kill(&mut self) -> io::Result<()> {
            CHILD_TRACE.with_borrow_mut(|trace| trace.operations.push("kill"));
            self.terminated = true;
            Ok(())
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            CHILD_TRACE.with_borrow_mut(|trace| trace.operations.push("wait"));
            Ok(exit_status(self.exit_code))
        }
    }

    struct ChildScope(Option<TestChildSources>, ChildTrace);

    impl ChildScope {
        fn new(trace: ChildTrace) -> Self {
            let previous_trace = CHILD_TRACE.replace(trace);
            let previous_sources = TEST_CHILD_SOURCES.replace(Some(TestChildSources {
                spawn: |command| {
                    CHILD_TRACE.with_borrow_mut(|trace| {
                        trace.commands.push(ObservedCommand {
                            program: command.get_program().to_owned(),
                            arguments: command.get_args().map(OsStr::to_owned).collect(),
                            environment: command
                                .get_envs()
                                .map(|(key, value)| (key.to_owned(), value.map(OsStr::to_owned)))
                                .collect(),
                        });
                        if trace.failure == Some(ChildFailure::Spawn) {
                            Err(io::Error::from_raw_os_error(5))
                        } else {
                            Ok(Box::new(ObservedChild {
                                pending: trace.pending,
                                terminated: false,
                                exit_code: trace.exit_code,
                            }) as Box<dyn WorkerProcess>)
                        }
                    })
                },
                capture: |_, path, show| {
                    CHILD_TRACE.with_borrow_mut(|trace| {
                        trace.captures.push((path.to_owned(), show));
                        Ok(trace.owned_log.as_ref().map(|bytes| {
                            let file = std::rc::Rc::new(std::cell::RefCell::new(io::Cursor::new(Vec::new())));
                            trace.log_files.push(std::rc::Rc::clone(&file));
                            WorkerLog::in_memory(bytes.clone(), file)
                        }))
                    })
                },
                token: |prefix| {
                    assert_eq!(prefix, "metabench-worker-");
                    Ok(PathBuf::from("in-memory-worker-token"))
                },
                exists: |path| {
                    assert_eq!(path, Path::new("in-memory-worker-token"));
                    CHILD_TRACE.with_borrow_mut(|trace| {
                        trace.operations.push("exists");
                        if trace.failure == Some(ChildFailure::Exists) {
                            Err(io::Error::from_raw_os_error(6))
                        } else {
                            Ok(trace.failure == Some(ChildFailure::Remove))
                        }
                    })
                },
                remove: |path| {
                    assert_eq!(path, Path::new("in-memory-worker-token"));
                    CHILD_TRACE.with_borrow_mut(|trace| trace.operations.push("remove"));
                    Err(io::Error::from_raw_os_error(7))
                },
                time: || CHILD_TRACE.with_borrow(|trace| trace.time),
                sleep: |duration| {
                    CHILD_TRACE.with_borrow_mut(|trace| {
                        trace.sleeps.push(duration);
                        trace.time += duration;
                    });
                },
                discovery: |prefix, parent| {
                    assert_eq!(prefix, "native-probe-discovery-");
                    if CHILD_TRACE.with_borrow(|trace| trace.failure == Some(ChildFailure::Discovery)) {
                        Err(io::Error::from_raw_os_error(8))
                    } else {
                        Ok(parent.join("in-memory-discovery"))
                    }
                },
                create_directory: |path| {
                    CHILD_TRACE.with_borrow_mut(|trace| trace.directories.push(path.to_owned()));
                    Ok(())
                },
            }));
            Self(previous_sources, previous_trace)
        }
    }

    impl Drop for ChildScope {
        fn drop(&mut self) {
            TEST_CHILD_SOURCES.set(self.0);
            CHILD_TRACE.set(std::mem::take(&mut self.1));
        }
    }

    fn in_memory_probe_bytes(path: &Path) -> Vec<u8> {
        if path.file_name() == Some(OsStr::new("cases.json")) {
            br#"[{"identity":"canonical","native_identity":"native-id","group":0,"benchmark":0,"case":0,"iteration":null}]"#.to_vec()
        } else if path.extension() == Some(OsStr::new("csv")) {
            b"Hardware Event Type,Hardware Event Count:Self\ncycles,17\n".to_vec()
        } else {
            br#"{"identity":"canonical","allocated_bytes":17,"allocation_count":9}"#.to_vec()
        }
    }

    #[test]
    fn november_actual_captured_worker_ownership_persists_exact_bytes_and_emits_only_failed_worker_diagnostics_without_native_resources() {
        let _runner = RunnerTestScope::new(RunnerTrace::default());
        let path = Path::new("declared.log");
        for exit_code in [0, 3] {
            let _scope = ChildScope::new(ChildTrace {
                exit_code,
                ..ChildTrace::default()
            });
            let file = std::rc::Rc::new(std::cell::RefCell::new(io::Cursor::new(Vec::new())));
            let bytes = if exit_code == 0 {
                b"successful retained bytes".to_vec()
            } else {
                let mut bytes = vec![b'x'; crate::worker_log::LOG_LIMIT + 3];
                bytes.extend_from_slice(b"final");
                bytes
            };
            let log = WorkerLog::in_memory(bytes.clone(), std::rc::Rc::clone(&file));
            let mut output = Vec::new();
            let status = wait_for_logged_worker_with_output(
                Mode::Criterion,
                &mut Command::new("unused"),
                Some(Duration::from_secs(1)),
                path,
                Some(log),
                &mut output,
            )
            .unwrap();
            assert_eq!(status.code(), Some(exit_code));
            if exit_code == 0 {
                assert_eq!(file.borrow().get_ref(), &bytes);
                assert!(output.is_empty(), "a successful worker must not print a failure excerpt");
            } else {
                let file = file.borrow();
                assert_eq!(file.get_ref().len(), crate::worker_log::LOG_LIMIT);
                assert!(
                    file.get_ref()
                        .starts_with(b"[metabench: output truncated; only the final bytes are retained]\n")
                );
                assert!(file.get_ref().ends_with(b"final"));
                assert_eq!(
                    String::from_utf8(output).unwrap(),
                    format!(
                        "metabench: criterion worker log: declared.log (output truncated; retained tail)\n{}final\n",
                        "x".repeat(8_187),
                    )
                );
            }
        }
        {
            let _scope = ChildScope::new(ChildTrace::default());
            let file = std::rc::Rc::new(std::cell::RefCell::new(io::Cursor::new(Vec::new())));
            let log = WorkerLog::in_memory(b"outer retained".to_vec(), std::rc::Rc::clone(&file));
            wait_for_logged_worker(
                Mode::Criterion,
                &mut Command::new("unused"),
                Some(Duration::from_secs(1)),
                path,
                Some(log),
            )
            .unwrap();
            assert_eq!(file.borrow().get_ref(), b"outer retained");
            assert!(RUNNER_TRACE.with_borrow(|trace| trace.output.is_empty()));
        }
        {
            let _scope = ChildScope::new(ChildTrace {
                owned_log: Some(b"authorized retained".to_vec()),
                ..ChildTrace::default()
            });
            let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
            launch_worker(
                Mode::Criterion,
                Path::new("unused"),
                &[],
                Some(&artifacts),
                false,
                Some(Duration::from_secs(1)),
            )
            .unwrap();
            CHILD_TRACE.with_borrow(|trace| {
                assert_eq!(trace.captures, [(artifacts.path().join("logs").join("criterion.log"), false)]);
                assert_eq!(trace.log_files.len(), 1);
                assert_eq!(trace.log_files[0].borrow().get_ref(), b"authorized retained");
                assert_eq!(trace.operations, ["try_wait", "exists"]);
            });
        }
    }

    #[test]
    fn november_actual_captured_worker_preserves_spawn_and_timeout_failures_and_kill_reap_order_without_native_resources() {
        for failure in [Some(ChildFailure::Spawn), None] {
            let _scope = ChildScope::new(ChildTrace {
                failure,
                pending: 2,
                ..ChildTrace::default()
            });
            let file = std::rc::Rc::new(std::cell::RefCell::new(io::Cursor::new(Vec::new())));
            let mut log = WorkerLog::in_memory(b"retained".to_vec(), file);
            let error = wait_for_captured_worker(
                Mode::Criterion,
                &mut Command::new("unused"),
                Some(Duration::from_millis(1)),
                Path::new("declared.log"),
                &mut log,
            )
            .unwrap_err();
            if failure.is_some() {
                assert!(matches!(error, Error::Spawn { source, .. } if source.raw_os_error() == Some(5)));
                CHILD_TRACE.with_borrow(|trace| assert!(trace.operations.is_empty()));
            } else {
                assert!(matches!(error, Error::WorkerTimedOut { .. }));
                CHILD_TRACE.with_borrow(|trace| {
                    assert_eq!(trace.operations, ["try_wait", "try_wait", "kill", "try_wait"]);
                    assert_eq!(trace.sleeps, [Duration::from_millis(1)]);
                });
            }
        }
    }

    #[test]
    fn november_actual_worker_launchers_preserve_every_command_environment_log_and_timeout_input_without_native_resources() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let executable = Path::new("declared-worker");
        let arguments = [OsString::from("--declared")];
        let timeout = Some(Duration::from_secs(3));
        for show in [false, true] {
            let _scope = ChildScope::new(ChildTrace::default());
            launch_worker(Mode::Criterion, executable, &arguments, Some(&artifacts), show, timeout).unwrap();
            CHILD_TRACE.with_borrow(|trace| {
                assert_eq!(trace.commands.len(), 1);
                let command = &trace.commands[0];
                assert_eq!(command.program, executable);
                assert_eq!(command.arguments, arguments);
                assert_eq!(
                    command.environment.get(OsStr::new("CRITERION_HOME")),
                    Some(&Some(artifacts.criterion_path().into_os_string()))
                );
                assert_eq!(
                    command.environment.get(OsStr::new(WORKER_TOKEN_ENV)),
                    Some(&Some("in-memory-worker-token".into()))
                );
                assert_eq!(trace.captures, [(artifacts.path().join("logs").join("criterion.log"), show)]);
                assert_eq!(trace.operations, ["try_wait", "exists"]);
            });
        }
        let _scope = ChildScope::new(ChildTrace::default());
        launch_worker_with(
            Mode::Allocations,
            executable,
            &arguments,
            Some(&artifacts),
            true,
            timeout,
            Some(Path::new("alternate-home")),
            Some(Path::new("discovery.json")),
            Some(("declared-id", Path::new("allocation.json"))),
        )
        .unwrap();
        CHILD_TRACE.with_borrow(|trace| {
            let command = &trace.commands[0];
            assert_eq!(command.program, executable);
            assert_eq!(command.arguments, arguments);
            for (name, value) in [
                (WORKER_MODE_ENV, "allocations"),
                ("CRITERION_HOME", "alternate-home"),
                (crate::probe::DISCOVERY_ENV, "discovery.json"),
                (allocation::IDENTITY_ENV, "declared-id"),
                (allocation::ARTIFACT_ENV, "allocation.json"),
            ] {
                assert_eq!(command.environment.get(OsStr::new(name)), Some(&Some(value.into())));
            }
            assert_eq!(trace.captures, [(Path::new("alternate-home").join("allocations.log"), true)]);
        });
    }

    #[test]
    fn november_actual_wait_and_authorized_cleanup_propagate_spawn_deadline_and_token_failures_without_clocks_or_processes() {
        {
            let _scope = ChildScope::new(ChildTrace {
                failure: Some(ChildFailure::Spawn),
                ..ChildTrace::default()
            });
            let error = wait_for_worker(Mode::Allocations, &mut Command::new("unused"), Some(Duration::from_millis(1))).unwrap_err();
            assert!(matches!(error, Error::Spawn { mode: "allocations", source } if source.raw_os_error() == Some(5)));
        }
        {
            let _scope = ChildScope::new(ChildTrace {
                pending: 2,
                ..ChildTrace::default()
            });
            let error = wait_for_worker(Mode::Allocations, &mut Command::new("unused"), Some(Duration::from_millis(1))).unwrap_err();
            assert!(matches!(error, Error::WorkerTimedOut { mode: "allocations", timeout } if timeout == Duration::from_millis(1)));
            CHILD_TRACE.with_borrow(|trace| {
                assert_eq!(trace.sleeps, [WORKER_POLL_INTERVAL]);
                assert_eq!(trace.operations, ["try_wait", "try_wait", "kill", "try_wait"]);
            });
        }
        for (failure, code) in [(ChildFailure::Exists, 6), (ChildFailure::Remove, 7)] {
            let _scope = ChildScope::new(ChildTrace {
                failure: Some(failure),
                ..ChildTrace::default()
            });
            let token = WorkerToken::Injected("in-memory-worker-token".into());
            let error = execute_authorized_worker(Mode::Allocations, &mut Command::new("unused"), None, None, &token, None).unwrap_err();
            assert!(matches!(error, Error::ConsumeWorkerToken(source) if source.raw_os_error() == Some(code)));
            CHILD_TRACE.with_borrow(|trace| {
                let expected = if failure == ChildFailure::Remove {
                    &["wait", "exists", "remove"][..]
                } else {
                    &["wait", "exists"][..]
                };
                assert_eq!(trace.operations, expected);
            });
        }
    }

    #[test]
    fn november_actual_discovery_and_probe_collectors_preserve_each_identity_and_native_error_without_files_or_processes() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let arguments = Arguments::parse_from(["--allocations", "--timeout=3s"].map(OsString::from), None).unwrap();
        let _scope = ChildScope::new(ChildTrace::default());
        artifact::with_in_memory_artifact_source(in_memory_probe_bytes, || {
            let targets = discover_native_probe_targets(Path::new("worker"), &artifacts, &arguments).unwrap();
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].identity, "canonical");
            let allocations = run_allocations(Path::new("worker"), &arguments, &artifacts).unwrap();
            assert_eq!(allocations.len(), 1);
            assert_eq!(allocations[0].identity, "canonical");
            assert_eq!(allocations[0].native_identity, "native-id");
            assert_eq!(allocations[0].metrics["Allocations"].value.as_f64().to_bits(), 9.0_f64.to_bits());
            let vtune = run_vtune(Path::new("worker"), &arguments, &artifacts).unwrap();
            assert_eq!(vtune.len(), 1);
            assert_eq!(vtune[0].identity, "canonical");
            assert_eq!(vtune[0].native_identity, "native-id");
            assert_eq!(vtune[0].metrics["cycles"].value.as_f64().to_bits(), 17.0_f64.to_bits());
        });
        CHILD_TRACE.with_borrow(|trace| {
            assert_eq!(trace.commands.len(), 6);
            assert!(
                !trace.operations.contains(&"wait"),
                "every configured deadline must use the polling policy"
            );
            assert!(
                trace
                    .captures
                    .iter()
                    .filter(|(path, _)| path.file_name() == Some(OsStr::new("allocations.log")))
                    .all(|(_, show)| !show)
            );
        });
        CHILD_TRACE.with_borrow_mut(|trace| trace.failure = Some(ChildFailure::Discovery));
        let error = discover_native_probe_targets(Path::new("worker"), &artifacts, &arguments).unwrap_err();
        assert!(matches!(error, Error::ArtifactIo { source, .. } if source.raw_os_error() == Some(8)));
    }

    #[test]
    fn november_actual_vtune_dispatch_forwards_output_deadline_and_both_log_paths_without_a_collector() {
        let _scope = ChildScope::new(ChildTrace::default());
        let artifact = Path::new("artifacts").join("17.csv");
        let result = Path::new("results").join("vtune");
        launch_counter_worker(
            Mode::Vtune,
            Path::new("declared-worker"),
            &["--native-case".into()],
            &["-knob".into(), "x=y".into()],
            (&artifact, Path::new("criterion-home"), Some(&result)),
            true,
            Some(Duration::from_secs(3)),
        )
        .unwrap();
        CHILD_TRACE.with_borrow(|trace| {
            assert_eq!(trace.commands.len(), 2);
            assert_eq!(trace.commands[0].program, "vtune");
            let expected: Vec<OsString> = [
                OsString::from("-collect-with"),
                "runsa".into(),
                "-knob".into(),
                "x=y".into(),
                "--start-paused".into(),
                "-result-dir".into(),
                result.into_os_string(),
                "--".into(),
                "declared-worker".into(),
                "--native-case".into(),
            ]
            .into();
            assert_eq!(trace.commands[0].arguments, expected);
            assert_eq!(trace.commands[1].arguments.last(), Some(&artifact.clone().into_os_string()));
            assert_eq!(
                trace.captures,
                [
                    (collector_log_path(&artifact, false), true),
                    (collector_log_path(&artifact, true), true)
                ]
            );
            assert_eq!(trace.operations, ["try_wait", "exists", "try_wait"]);
            assert_eq!(trace.directories, [PathBuf::from("results"), PathBuf::from("artifacts")]);
        });
    }

    #[test]
    fn november_actual_perf_dispatch_retains_both_output_choices_and_every_native_collector_input_without_linux_resources() {
        struct Reset(Option<TestPerfLaunch>);
        impl Drop for Reset {
            fn drop(&mut self) {
                TEST_PERF_LAUNCH.set(self.0);
            }
        }
        let reset = Reset(TEST_PERF_LAUNCH.replace(Some(|request| {
            assert_eq!(request.executable, Path::new("declared-worker"));
            assert_eq!(request.native_arguments, [OsString::from("native-case")]);
            assert_eq!(request.collector_arguments, [OsString::from("--event=cycles")]);
            assert_eq!(request.artifact, Path::new("declared-perf.json"));
            assert_eq!(request.home, Path::new("declared-home"));
            assert_eq!(request.timeout, Some(Duration::from_secs(3)));
            CHILD_TRACE.with_borrow_mut(|trace| trace.captures.push((request.artifact.to_owned(), request.show)));
            Ok(exit_status(0))
        })));
        for show in [false, true] {
            let _scope = ChildScope::new(ChildTrace::default());
            launch_counter_worker(
                Mode::Perf,
                Path::new("declared-worker"),
                &[OsString::from("native-case")],
                &[OsString::from("--event=cycles")],
                (Path::new("declared-perf.json"), Path::new("declared-home"), None),
                show,
                Some(Duration::from_secs(3)),
            )
            .unwrap();
            CHILD_TRACE.with_borrow(|trace| assert_eq!(trace.captures, [(PathBuf::from("declared-perf.json"), show)]));
        }
        drop(reset);
    }

    #[test]
    fn november_actual_smoke_worker_output_choice_has_no_capture_or_command_effect_without_artifact_homes() {
        let mut commands = Vec::new();
        for show in [false, true] {
            let _scope = ChildScope::new(ChildTrace::default());
            launch_worker(
                Mode::Criterion,
                Path::new("declared-worker"),
                &[OsString::from("--test")],
                None,
                show,
                Some(Duration::from_secs(3)),
            )
            .unwrap();
            CHILD_TRACE.with_borrow(|trace| {
                assert!(trace.captures.is_empty());
                assert_eq!(trace.commands.len(), 1);
                assert_eq!(trace.operations, ["try_wait", "exists"]);
                let command = &trace.commands[0];
                commands.push((command.program.clone(), command.arguments.clone(), command.environment.clone()));
            });
        }
        assert_eq!(commands[0], commands[1]);
    }

    #[test]
    fn november_actual_parent_smoke_native_listing_and_probe_listing_preserve_routing_and_original_failures_without_native_observation() {
        let _runner = RunnerTestScope::new(RunnerTrace::default());
        for options in [
            &["--criterion", "--test", "--timeout=3s", "--no-output", "--no-baseline"][..],
            &["--criterion", "--list", "--timeout=3s", "--no-output", "--no-baseline"][..],
            &["--allocations", "--list", "--timeout=3s", "--no-output", "--no-baseline"][..],
            &["--allocations", "--timeout=3s", "--no-output", "--no-baseline"][..],
        ] {
            let _child = ChildScope::new(ChildTrace::default());
            let mut arguments = Arguments::parse_from(options.iter().map(OsString::from), None).unwrap();
            artifact::with_in_memory_artifact_run(|| {
                artifact::with_in_memory_artifact_source(in_memory_probe_bytes, || {
                    crate::report::with_in_memory_metadata(|| {
                        run_parent(&mut arguments, in_memory_engines(), &[], "declared-target").unwrap();
                    });
                });
            });
            CHILD_TRACE.with_borrow(|trace| {
                let expected_commands = if options[0] == "--allocations" && options[1] != "--list" {
                    2
                } else {
                    1
                };
                assert_eq!(trace.commands.len(), expected_commands);
                let expected = if options[0] == "--allocations" {
                    "allocations"
                } else {
                    "criterion"
                };
                assert_eq!(
                    trace.commands[0].environment.get(OsStr::new(WORKER_MODE_ENV)),
                    Some(&Some(expected.into()))
                );
                assert_eq!(trace.operations, ["try_wait", "exists"].repeat(expected_commands));
            });
        }
        let _child = ChildScope::new(ChildTrace {
            failure: Some(ChildFailure::Spawn),
            ..ChildTrace::default()
        });
        let mut arguments = Arguments::parse_from(["--criterion", "--fail-fast"].map(OsString::from), None).unwrap();
        let error =
            artifact::with_in_memory_artifact_run(|| run_parent(&mut arguments, in_memory_engines(), &[], "declared-target")).unwrap_err();
        assert!(matches!(error, Error::Spawn { mode: "criterion", source } if source.raw_os_error() == Some(5)));
        CHILD_TRACE.with_borrow(|trace| {
            assert_eq!(trace.captures.len(), 1);
            assert!(!trace.captures[0].1);
        });
        let mut arguments = Arguments::parse_from(["--criterion", "--list"].map(OsString::from), None).unwrap();
        let mut sources = TEST_RUNNER_SOURCES.get().unwrap();
        sources.executable = || Ok("relocated-worker-without-target-layout".into());
        TEST_RUNNER_SOURCES.set(Some(sources));
        assert!(matches!(
            run_parent(&mut arguments, in_memory_engines(), &[], "declared-target"),
            Err(Error::ArtifactFormat { .. })
        ));
        sources.executable = || Ok(Path::new("imaginary-target").join("debug").join("worker.exe"));
        TEST_RUNNER_SOURCES.set(Some(sources));
        let mut arguments = Arguments::parse_from(["--criterion", "--fail-fast"].map(OsString::from), None).unwrap();
        let error =
            artifact::with_in_memory_artifact_creation_failure(|| run_parent(&mut arguments, in_memory_engines(), &[], "declared-target"))
                .unwrap_err();
        assert!(matches!(error, Error::Metadata(message) if message == "injected artifact creation failure"));
    }

    #[test]
    fn november_actual_counter_artifact_dispatch_preserves_canonical_and_native_identities_without_file_io() {
        for (mode, bytes) in [
            (Mode::Perf, br#"{"counter-value":"17","event":"cycles"}"#.as_slice()),
            (
                Mode::Vtune,
                b"Hardware Event Type,Hardware Event Count:Self\ncycles,17\n".as_slice(),
            ),
        ] {
            artifact::with_in_memory_artifact(bytes, || {
                let result = parse_counter_artifact(
                    mode,
                    Path::new("root").join("counter.data").as_path(),
                    Path::new("root"),
                    "canonical".into(),
                    "native-original".into(),
                )
                .unwrap();
                assert_eq!(result.identity, "canonical");
                assert_eq!(result.native_identity, "native-original");
                assert_eq!(result.source, mode.as_str());
                assert_eq!(result.raw_artifacts, ["counter.data"]);
                assert_eq!(result.metrics.len(), 1);
                assert_eq!(result.metrics["cycles"].value.as_f64().to_bits(), 17.0_f64.to_bits());
            });
        }
    }

    #[test]
    fn november_actual_report_build_preserves_probe_rows_and_returns_assembly_or_empty_report_errors_without_native_observation() {
        let _scope = RunnerTestScope::new(RunnerTrace::default());
        crate::report::with_in_memory_metadata(|| {
            let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
            let mut arguments = Arguments::parse_from(["--perf", "--no-output", "--no-baseline"].map(OsString::from), None).unwrap();
            arguments.automatic_baseline = false;
            let result = build_report(
                &artifacts,
                &[Mode::Perf],
                &[],
                &arguments,
                "memory-target",
                None,
                vec![native_result("perf", "case")],
                true,
            )
            .unwrap();
            assert!(result.1.is_none());
            assert!(result.2.is_none());
            assert_eq!(result.0.entries.len(), 1);
            assert_eq!(result.0.entries[0].identity, "case");
            assert_eq!(
                result.0.entries[0].results["perf"].metrics["cycles"].value.as_f64().to_bits(),
                17.0_f64.to_bits()
            );
            assert_eq!(result.0.metadata.timestamp_utc, "2000-01-02T03:04:05Z");
            let error = build_report(&artifacts, &[Mode::Perf], &[], &arguments, "memory-target", None, vec![], true).unwrap_err();
            assert!(
                matches!(error, Error::ArtifactFormat { message, .. } if message == "no benchmark cases matched; existing reports were preserved")
            );
            let error = build_report(
                &artifacts,
                &[Mode::Perf],
                &[],
                &arguments,
                "memory-target",
                None,
                vec![native_result("perf", "case"), native_result("perf", "case")],
                true,
            )
            .unwrap_err();
            assert!(matches!(error, Error::DuplicateReportBenchmark(identity) if identity == "case (perf)"));
        });
    }

    fn in_memory_report_or_probe_bytes(path: &Path) -> Vec<u8> {
        crate::report::in_memory_report_file(path).unwrap_or_else(|| in_memory_probe_bytes(path))
    }

    #[test]
    fn november_actual_report_build_selects_explicit_and_automatic_baselines_and_publishes_only_the_exact_complete_or_partial_paths() {
        let _scope = RunnerTestScope::new(RunnerTrace::default());
        crate::arguments::with_in_memory_paths(|| {
            crate::report::with_in_memory_metadata(|| {
                let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
                let mut arguments = Arguments::parse_from(
                    ["--perf", "--export-json", "selected.json", "--export-md", "selected.md"].map(OsString::from),
                    None,
                )
                .unwrap();
                let baseline =
                    BenchmarkReport::from_results(vec![native_result("perf", "case")], vec!["perf".into()], "artifacts".into()).unwrap();
                let previous = serde_json::to_vec(&baseline).unwrap();
                for automatic in [false, true] {
                    for complete in [false, true] {
                        arguments.automatic_baseline = automatic;
                        arguments.baseline = (!automatic).then(|| PathBuf::from("explicit.json"));
                        let baseline_path = if automatic {
                            PathBuf::from("selected.json")
                        } else {
                            PathBuf::from("explicit.json")
                        };
                        let files = std::collections::BTreeMap::from([(baseline_path.clone(), previous.clone())]);
                        let ((report, json, markdown), files) = crate::report::with_in_memory_report_files(files, || {
                            artifact::with_in_memory_artifact_source(in_memory_report_or_probe_bytes, || {
                                let mut result = native_result("perf", "case");
                                result.metrics.insert(
                                    "cycles".into(),
                                    crate::report::Metric::float("perf", "cycles", 34.0, crate::report::MetricDirection::LowerIsBetter),
                                );
                                build_report(
                                    &artifacts,
                                    &[Mode::Perf],
                                    &[],
                                    &arguments,
                                    "declared-target",
                                    None,
                                    vec![result],
                                    complete,
                                )
                                .unwrap()
                            })
                        });
                        let expected_json = PathBuf::from(if complete { "selected.json" } else { "selected.partial.json" });
                        let expected_markdown = PathBuf::from(if complete { "selected.md" } else { "selected.partial.md" });
                        assert_eq!(json, Some(expected_json.clone()));
                        assert_eq!(markdown, Some(expected_markdown.clone()));
                        assert_eq!(serde_json::to_value(&report).unwrap()["entries"][0]["status"], "regressed");
                        assert_eq!(
                            report.entries[0].prior.as_ref().unwrap()["perf"].metrics["cycles"]
                                .value
                                .as_f64()
                                .to_bits(),
                            17.0_f64.to_bits()
                        );
                        let emitted: serde_json::Value = serde_json::from_slice(&files[&expected_json]).unwrap();
                        assert_eq!(emitted, serde_json::to_value(&report).unwrap());
                        assert!(files[&expected_markdown].starts_with(b"# Benchmark Report"));
                        assert!(!files.contains_key(&expected_json.with_extension("json.metabench-incomplete")));
                        if !complete || !automatic {
                            assert_eq!(files[&baseline_path], previous, "partial output must never overwrite its baseline");
                        }
                    }
                }
            });
        });
    }

    #[test]
    fn november_actual_report_build_propagates_selected_baseline_parse_and_warning_output_errors_without_publication() {
        let _scope = RunnerTestScope::new(RunnerTrace::default());
        crate::arguments::with_in_memory_paths(|| {
            crate::report::with_in_memory_metadata(|| {
                let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
                let mut arguments = Arguments::parse_from(
                    [
                        "--perf",
                        "--baseline",
                        "explicit.json",
                        "--export-json",
                        "selected.json",
                        "--export-md",
                        "selected.md",
                    ]
                    .map(OsString::from),
                    None,
                )
                .unwrap();
                arguments.automatic_baseline = false;
                for malformed in [true, false] {
                    let bytes = if malformed {
                        b"{ invalid baseline".to_vec()
                    } else {
                        let mut baseline =
                            BenchmarkReport::from_results(vec![native_result("perf", "case")], vec!["perf".into()], "artifacts".into())
                                .unwrap();
                        baseline.metadata.target_triple = "incompatible-native-target".into();
                        serde_json::to_vec(&baseline).unwrap()
                    };
                    RUNNER_TRACE.with_borrow_mut(|trace| trace.failure = (!malformed).then_some(RunnerFailure::Output));
                    let (error, files) = crate::report::with_in_memory_report_files(
                        std::collections::BTreeMap::from([(PathBuf::from("explicit.json"), bytes)]),
                        || {
                            artifact::with_in_memory_artifact_source(in_memory_report_or_probe_bytes, || {
                                build_report(
                                    &artifacts,
                                    &[Mode::Perf],
                                    &[],
                                    &arguments,
                                    "declared-target",
                                    None,
                                    vec![native_result("perf", "case")],
                                    true,
                                )
                                .unwrap_err()
                            })
                        },
                    );
                    if malformed {
                        assert!(matches!(error, Error::ReportJson { path, .. } if path == Path::new("explicit.json")));
                    } else {
                        assert!(
                            matches!(error, Error::ConsoleOutput(source) if source.kind() == io::ErrorKind::Other && source.to_string() == "console refused")
                        );
                    }
                    assert_eq!(files.len(), 1);
                    assert!(!files.contains_key(Path::new("selected.json")));
                    assert!(!files.contains_key(Path::new("selected.md")));
                }
            });
        });
    }

    #[test]
    fn november_actual_parent_promotes_a_successful_measured_run_to_complete_reports_without_native_observation() {
        let _runner = RunnerTestScope::new(RunnerTrace::default());
        let _child = ChildScope::new(ChildTrace::default());
        crate::arguments::with_in_memory_paths(|| {
            let mut arguments = Arguments::parse_from(
                [
                    "--allocations",
                    "--timeout=3s",
                    "--no-baseline",
                    "--export-json",
                    "selected.json",
                    "--export-md",
                    "selected.md",
                ]
                .map(OsString::from),
                None,
            )
            .unwrap();
            let ((), files) = crate::report::with_in_memory_report_files(std::collections::BTreeMap::new(), || {
                artifact::with_in_memory_artifact_run(|| {
                    artifact::with_in_memory_artifact_source(in_memory_report_or_probe_bytes, || {
                        crate::report::with_in_memory_metadata(|| {
                            run_parent(&mut arguments, in_memory_engines(), &[], "declared-target").unwrap();
                        });
                    });
                });
            });
            assert_eq!(
                files.keys().cloned().collect::<Vec<_>>(),
                [PathBuf::from("selected.json"), PathBuf::from("selected.md")]
            );
            let report: serde_json::Value = serde_json::from_slice(&files[Path::new("selected.json")]).unwrap();
            assert_eq!(report["entries"][0]["identity"], "canonical");
            assert_eq!(
                report["entries"][0]["results"]["alloc_tracker"]["metrics"]["Allocations"]["value"],
                9
            );
            assert_eq!(
                report["entries"][0]["results"]["alloc_tracker"]["metrics"]["Allocated bytes"]["value"],
                17
            );
        });
    }

    #[test]
    fn november_actual_criterion_report_build_preserves_the_explicit_measurement_unit_through_native_artifact_parsing() {
        let _runner = RunnerTestScope::new(RunnerTrace::default());
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let arguments = Arguments::parse_from(["--criterion", "--no-output", "--no-baseline"].map(OsString::from), None).unwrap();
        artifact::with_in_memory_artifact_listing(
            |root| Ok(vec![root.join("group").join("new").join("benchmark.json")]),
            || {
                artifact::with_in_memory_artifact_source(
                    |path| {
                        match path.file_name().and_then(OsStr::to_str).unwrap() {
                    "benchmark.json" => br#"{"group_id":"group","function_id":null,"value_str":null,"throughput":null}"#.to_vec(),
                    "estimates.json" => br#"{"slope":null,"mean":{"point_estimate":17.0,"confidence_interval":{"confidence_level":0.95,"lower_bound":16.0,"upper_bound":18.0}}}"#.to_vec(),
                    "criterion.json" => br#"{"target":"declared-target","engine":"criterion","command_line":[],"engine_version":null,"artifact_root":"artifacts","success":true,"exit_code":0,"discovered_native_identities":[]}"#.to_vec(),
                    name => panic!("unexpected artifact read: {name}"),
                }
                    },
                    || {
                        crate::report::with_in_memory_metadata(|| {
                            let (report, _, _) = build_report(
                                &artifacts,
                                &[Mode::Criterion],
                                &[],
                                &arguments,
                                "declared-target",
                                Some("ticks"),
                                vec![],
                                true,
                            )
                            .unwrap();
                            assert_eq!(report.entries[0].identity, "group");
                            let metric = &report.entries[0].results["criterion"].metrics["time"];
                            assert_eq!(metric.unit.as_deref(), Some("ticks"));
                            assert_eq!(metric.value.as_f64().to_bits(), 17.0_f64.to_bits());
                            assert_eq!(metric.statistic.as_deref(), Some("mean"));
                        });
                    },
                );
            },
        );
    }

    #[test]
    fn november_worker_token_request_preserves_exact_prefix_and_creation_error_without_allocating_a_directory() {
        assert_eq!(
            create_worker_token_with(|prefix| {
                assert_eq!(prefix, "metabench-worker-");
                Ok(17)
            })
            .unwrap(),
            17
        );
        let error = create_worker_token_with::<()>(|prefix| {
            assert_eq!(prefix, "metabench-worker-");
            Err(io::Error::from_raw_os_error(5))
        })
        .unwrap_err();
        assert!(matches!(error, Error::CreateWorkerToken(source) if source.raw_os_error() == Some(5)));
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RunnerFailure {
        Consume,
        Probe,
        Finish,
        Parse,
        Output,
    }

    #[derive(Default)]
    struct RunnerTrace {
        argument: Option<OsString>,
        worker: Option<OsString>,
        failure: Option<RunnerFailure>,
        events: Vec<String>,
        output: Vec<u8>,
    }

    thread_local! {
        static RUNNER_TRACE: std::cell::RefCell<RunnerTrace> = std::cell::RefCell::new(RunnerTrace::default());
    }

    struct RunnerTestScope(Option<TestRunnerSources>, RunnerTrace);

    #[test]
    fn november6_actual_runner_argument_preserves_the_first_nonprogram_argument() {
        for argument in [None, Some(OsString::from("--gungraun-run")), Some(OsString::from("--measure"))] {
            let _scope = RunnerTestScope::new(RunnerTrace {
                argument: argument.clone(),
                ..RunnerTrace::default()
            });
            assert_eq!(runner_argument(), argument);
        }
    }

    impl RunnerTestScope {
        fn new(trace: RunnerTrace) -> Self {
            let previous_trace = RUNNER_TRACE.replace(trace);
            let previous = TEST_RUNNER_SOURCES.replace(Some(TestRunnerSources {
                argument: || {
                    RUNNER_TRACE.with_borrow_mut(|trace| {
                        trace.events.push("argument".into());
                        trace.argument.clone()
                    })
                },
                environment: |name| {
                    RUNNER_TRACE.with_borrow_mut(|trace| {
                        if name == WORKER_MODE_ENV {
                            trace.events.push("environment".into());
                            trace.worker.clone()
                        } else {
                            assert_eq!(name, "CARGO_TARGET_DIR");
                            None
                        }
                    })
                },
                consume: || {
                    RUNNER_TRACE.with_borrow_mut(|trace| {
                        trace.events.push("consume".into());
                        if trace.failure == Some(RunnerFailure::Consume) {
                            Err(Error::Probe("token refused".into()))
                        } else {
                            Ok(())
                        }
                    })
                },
                prime: |mode| RUNNER_TRACE.with_borrow_mut(|trace| trace.events.push(format!("prime:{}", mode.as_str()))),
                finish: |mode| {
                    RUNNER_TRACE.with_borrow_mut(|trace| {
                        trace.events.push(format!("finish:{}", mode.as_str()));
                        if trace.failure == Some(RunnerFailure::Finish) {
                            Err(Error::Probe("finish refused".into()))
                        } else {
                            Ok(())
                        }
                    })
                },
                parse: || {
                    let fail = RUNNER_TRACE.with_borrow_mut(|trace| {
                        trace.events.push("parse".into());
                        trace.failure == Some(RunnerFailure::Parse)
                    });
                    if fail {
                        Err(Error::Probe("parse refused".into()))
                    } else {
                        Arguments::parse_from(["--help"].map(OsString::from), None)
                    }
                },
                write: |bytes| {
                    RUNNER_TRACE.with_borrow_mut(|trace| {
                        if trace.failure == Some(RunnerFailure::Output) {
                            Err(io::Error::other("console refused"))
                        } else {
                            trace.output.extend_from_slice(bytes);
                            Ok(bytes.len())
                        }
                    })
                },
                flush: || {
                    RUNNER_TRACE.with_borrow_mut(|trace| {
                        trace.events.push("flush".into());
                        Ok(())
                    })
                },
                executable: || {
                    RUNNER_TRACE.with_borrow_mut(|trace| trace.events.push("executable".into()));
                    Ok(PathBuf::from("imaginary-target").join("debug").join("worker.exe"))
                },
            }));
            Self(previous, previous_trace)
        }
    }

    impl Drop for RunnerTestScope {
        fn drop(&mut self) {
            TEST_RUNNER_SOURCES.set(self.0);
            RUNNER_TRACE.set(std::mem::take(&mut self.1));
        }
    }

    fn in_memory_engines() -> EngineSet {
        EngineSet::both_with_unit(
            || RUNNER_TRACE.with_borrow_mut(|trace| trace.events.push("criterion".into())),
            || RUNNER_TRACE.with_borrow_mut(|trace| trace.events.push("gungraun".into())),
            Some("ns"),
            || {
                RUNNER_TRACE.with_borrow_mut(|trace| {
                    trace.events.push("probe".into());
                    if trace.failure == Some(RunnerFailure::Probe) {
                        Err("probe refused".into())
                    } else {
                        Ok(())
                    }
                })
            },
        )
    }

    #[test]
    fn november_actual_runner_routes_arguments_tokens_and_native_engines_without_process_observations() {
        {
            let _scope = RunnerTestScope::new(RunnerTrace {
                argument: Some("--gungraun-run".into()),
                worker: Some("criterion".into()),
                ..RunnerTrace::default()
            });
            run_inner(in_memory_engines(), &[], "in-memory").unwrap();
            RUNNER_TRACE.with_borrow(|trace| assert_eq!(trace.events, ["argument", "gungraun"]));
        }
        for consume_error in [false, true] {
            let _scope = RunnerTestScope::new(RunnerTrace {
                worker: Some("criterion".into()),
                failure: consume_error.then_some(RunnerFailure::Consume),
                ..RunnerTrace::default()
            });
            let result = run_inner(in_memory_engines(), &[], "in-memory");
            if consume_error {
                assert!(matches!(result, Err(Error::Probe(message)) if message == "token refused"));
            } else {
                result.unwrap();
            }
            RUNNER_TRACE.with_borrow(|trace| {
                assert_eq!(
                    trace.events,
                    if consume_error {
                        vec!["argument", "environment", "consume"]
                    } else {
                        vec!["argument", "environment", "consume", "criterion"]
                    }
                );
                assert!(trace.output.is_empty());
            });
        }
    }

    #[test]
    fn november_actual_runner_probe_listing_and_measurement_forward_exact_finish_and_failure_order() {
        for mode in ["allocations", "perf", "vtune"] {
            for listing in [false, true] {
                for failure in ["none", "probe", "finish"] {
                    let _scope = RunnerTestScope::new(RunnerTrace {
                        argument: Some(if listing { "--metabench-probe-list" } else { "--measure" }.into()),
                        worker: Some(mode.into()),
                        failure: match failure {
                            "probe" => Some(RunnerFailure::Probe),
                            "finish" => Some(RunnerFailure::Finish),
                            _ => None,
                        },
                        ..RunnerTrace::default()
                    });
                    let result = run_inner(in_memory_engines(), &[], "in-memory");
                    if failure == "probe" {
                        assert!(matches!(result, Err(Error::Probe(message)) if message == "probe refused"));
                    } else if failure == "finish" && !listing {
                        assert!(matches!(result, Err(Error::Probe(message)) if message == "finish refused"));
                    } else {
                        result.unwrap();
                    }
                    let mut expected = vec![
                        "argument".into(),
                        "environment".into(),
                        "consume".into(),
                        format!("prime:{mode}"),
                        "probe".into(),
                    ];
                    if failure != "probe" {
                        expected.push("argument".into());
                        if !listing {
                            expected.push(format!("finish:{mode}"));
                        }
                    }
                    RUNNER_TRACE.with_borrow(|trace| {
                        assert_eq!(trace.events, expected);
                        assert!(trace.output.is_empty());
                    });
                }
            }
        }
    }

    #[test]
    fn november_actual_runner_parent_help_preserves_exact_output_and_parse_or_console_errors() {
        for failure in ["none", "parse", "output"] {
            let _scope = RunnerTestScope::new(RunnerTrace {
                failure: match failure {
                    "parse" => Some(RunnerFailure::Parse),
                    "output" => Some(RunnerFailure::Output),
                    _ => None,
                },
                ..RunnerTrace::default()
            });
            let result = run_inner(in_memory_engines(), &[], "in-memory");
            match failure {
                "parse" => assert!(matches!(result, Err(Error::Probe(message)) if message == "parse refused")),
                "output" => assert!(matches!(result, Err(Error::HelpOutput(error)) if error.to_string() == "console refused")),
                _ => result.unwrap(),
            }
            RUNNER_TRACE.with_borrow(|trace| {
                assert_eq!(trace.events, ["argument", "environment", "parse"]);
                assert_eq!(trace.output, if failure == "none" { HELP.as_bytes() } else { b"" });
            });
        }
    }

    #[test]
    fn november_actual_runner_console_wrappers_preserve_exact_payloads_paths_flushes_and_write_failures() {
        for fail in [false, true] {
            let _scope = RunnerTestScope::new(RunnerTrace {
                failure: fail.then_some(RunnerFailure::Output),
                ..RunnerTrace::default()
            });
            let targets = [probe_target("first", 0), probe_target("second", 1)];
            let result = print_probe_targets(&targets);
            if fail {
                assert!(matches!(result, Err(Error::ConsoleOutput(error)) if error.to_string() == "console refused"));
            } else {
                result.unwrap();
                RUNNER_TRACE.with_borrow_mut(|trace| {
                    assert_eq!(trace.output, b"first: benchmark\nsecond: benchmark\n");
                    trace.output.clear();
                });
            }
            let result = print_progress(Mode::Criterion);
            if fail {
                assert!(matches!(result, Err(Error::ConsoleOutput(error)) if error.to_string() == "console refused"));
            } else {
                result.unwrap();
                RUNNER_TRACE.with_borrow_mut(|trace| {
                    assert_eq!(trace.output, b"Running criterion benchmarks...\n");
                    assert_eq!(trace.events, ["flush"]);
                    trace.output.clear();
                });
            }
            let report = report_fixture(17);
            let result = write_console_report(&report, Some(Path::new("one.json")), Some(Path::new("two.md")));
            if fail {
                assert!(matches!(result, Err(Error::ConsoleOutput(error)) if error.to_string() == "console refused"));
            } else {
                result.unwrap();
                let expected = format!("\n{}\nGenerated one.json\nGenerated two.md\n", report.render_console_table());
                RUNNER_TRACE.with_borrow(|trace| assert_eq!(trace.output, expected.as_bytes()));
            }
        }
    }

    #[test]
    fn november_actual_report_paths_propagate_alias_rejection_using_only_injected_process_inputs() {
        let _scope = RunnerTestScope::new(RunnerTrace::default());
        let mut arguments = Arguments::parse_from(["--criterion"].map(OsString::from), None).unwrap();
        arguments.export_json = Some("same-path".into());
        arguments.export_markdown = Some("same-path".into());
        assert!(matches!(report_paths(&arguments, "target"), Err(Error::ConflictingOutputOptions)));
        RUNNER_TRACE.with_borrow(|trace| {
            assert_eq!(trace.events, ["executable"]);
            assert!(trace.output.is_empty());
        });
    }

    #[test]
    fn gamma_round2_allocation_requests_preserve_indexed_paths_payloads_and_launch_options() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let executable = Path::new("worker.exe");
        for show in [false, true] {
            let mut arguments = Arguments::parse_from(["--allocations"].map(OsString::from), None).unwrap();
            arguments.show_engine_output = show;
            arguments.timeout = Some(Duration::from_secs(23));
            let launched = Cell::new(0);
            let read = Cell::new(0);
            let results = run_allocation_targets(
                executable,
                &arguments,
                &artifacts,
                vec![probe_target("case-0", 0), probe_target("case-1", 1)],
                |mode, program, argv, included, visible, timeout, (home, discovery, allocation)| {
                    let index = launched.get();
                    assert_eq!(index, read.get());
                    launched.set(index + 1);
                    assert_eq!(mode, Mode::Allocations);
                    assert_eq!(program, executable);
                    assert_eq!(
                        argv,
                        [
                            "--metabench-probe".to_owned(),
                            "2".into(),
                            "3".into(),
                            (index + 7).to_string(),
                            (index + 20).to_string()
                        ]
                        .map(OsString::from)
                    );
                    assert_eq!(included.unwrap().path(), artifacts.path());
                    assert_eq!(visible, show);
                    assert_eq!(timeout, Some(Duration::from_secs(23)));
                    assert_eq!(
                        home,
                        Some(artifacts.path().join("allocation-probes").join(index.to_string()).as_path())
                    );
                    assert!(discovery.is_none());
                    let (identity, path) = allocation.unwrap();
                    assert_eq!(identity, format!("case-{index}"));
                    assert_eq!(path, artifacts.path().join("allocations").join(format!("{index}.json")));
                    Ok(ExitStatus::from_raw(0))
                },
                |path, native, relative| {
                    let index = read.get();
                    assert_eq!(launched.get(), index + 1);
                    read.set(index + 1);
                    assert_eq!(path, artifacts.path().join("allocations").join(format!("{index}.json")));
                    assert_eq!(native, format!("native/case-{index}"));
                    assert_eq!(relative, format!("run/allocations/{index}.json"));
                    Ok(native_result("alloc_tracker", &format!("result-{index}")))
                },
            )
            .unwrap();
            assert_eq!(launched.get(), 2);
            assert_eq!(read.get(), 2);
            assert_eq!(
                results.iter().map(|result| result.identity.as_str()).collect::<Vec<_>>(),
                ["result-0", "result-1"]
            );
        }
    }

    #[test]
    fn gamma_round2_counter_requests_preserve_collector_paths_arguments_identity_and_results() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let executable = Path::new("worker.exe");
        for mode in [Mode::Perf, Mode::Vtune] {
            for show in [false, true] {
                let mut arguments = Arguments::parse_from(
                    ["--perf", "--vtune", "--perf-arg", "event=cycles", "--vtune-arg", "event=uops"].map(OsString::from),
                    None,
                )
                .unwrap();
                arguments.show_engine_output = show;
                arguments.timeout = Some(Duration::from_secs(29));
                let launched = Cell::new(0);
                let parsed = Cell::new(0);
                let results = run_counter_targets(
                    mode,
                    executable,
                    &arguments,
                    &artifacts,
                    vec![probe_target("case-0", 0), probe_target("case-1", 1)],
                    |selected, program, argv, collector_args, (artifact, home, result), visible, timeout| {
                        let index = launched.get();
                        assert_eq!(index, parsed.get());
                        launched.set(index + 1);
                        assert_eq!(selected, mode);
                        assert_eq!(program, executable);
                        assert_eq!(
                            argv,
                            [
                                "--metabench-probe".to_owned(),
                                "2".into(),
                                "3".into(),
                                (index + 7).to_string(),
                                (index + 20).to_string()
                            ]
                            .map(OsString::from)
                        );
                        assert_eq!(
                            collector_args,
                            [OsString::from(if mode == Mode::Perf { "event=cycles" } else { "event=uops" })]
                        );
                        assert_eq!(visible, show);
                        assert_eq!(timeout, Some(Duration::from_secs(29)));
                        if mode == Mode::Perf {
                            assert_eq!(artifact, artifacts.path().join("perf").join(format!("{index}.jsonl")));
                            assert_eq!(home, artifacts.path().join("perf-probes").join(index.to_string()));
                            assert!(result.is_none());
                        } else {
                            assert_eq!(artifact, artifacts.path().join("vtune").join(format!("{index}.csv")));
                            assert_eq!(home, artifacts.path().join("vtune-probes").join(index.to_string()));
                            assert_eq!(
                                result,
                                Some(artifacts.path().join("vtune-results").join(index.to_string()).as_path())
                            );
                        }
                        Ok(ExitStatus::from_raw(0))
                    },
                    |selected, path, root, identity, native| {
                        let index = parsed.get();
                        assert_eq!(launched.get(), index + 1);
                        parsed.set(index + 1);
                        assert_eq!(selected, mode);
                        assert_eq!(root, artifacts.root());
                        assert_eq!(identity, format!("case-{index}"));
                        assert_eq!(native, format!("native/case-{index}"));
                        assert_eq!(
                            path,
                            artifacts
                                .path()
                                .join(if mode == Mode::Perf { "perf" } else { "vtune" })
                                .join(format!("{index}.{}", if mode == Mode::Perf { "jsonl" } else { "csv" }))
                        );
                        Ok(native_result(mode.as_str(), &identity))
                    },
                )
                .unwrap();
                assert_eq!(launched.get(), 2);
                assert_eq!(parsed.get(), 2);
                assert_eq!(
                    results.iter().map(|result| result.identity.as_str()).collect::<Vec<_>>(),
                    ["case-0", "case-1"]
                );
            }
        }
        require_perf_support(true).unwrap();
        assert!(matches!(
            require_perf_support(false),
            Err(Error::UnsupportedMode {
                mode: "perf",
                reason: "Linux perf is supported only on Linux"
            })
        ));
    }

    #[test]
    fn gamma_round2_collector_discovery_launch_status_and_parser_errors_stop_in_order() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let arguments = Arguments::parse_from(["--allocations"].map(OsString::from), None).unwrap();
        assert!(
            matches!(collect_discovered_targets(|| Err(Error::Metadata("discovery refused".into())), |_| panic!("discovery failed")),
            Err(Error::Metadata(message)) if message == "discovery refused")
        );
        let results = collect_discovered_targets(
            || Ok(vec![probe_target("discovered", 0)]),
            |targets| {
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].identity, "discovered");
                Ok(vec![native_result("perf", "measured")])
            },
        )
        .unwrap();
        assert_eq!(results[0].identity, "measured");
        for mode in [Mode::Allocations, Mode::Perf, Mode::Vtune] {
            for phase in ["launch", "status", "parse"] {
                let launch = || {
                    if phase == "launch" {
                        Err(Error::Metadata("launch refused".into()))
                    } else {
                        Ok(ExitStatus::from_raw((phase == "status").into()))
                    }
                };
                let parse = || {
                    assert_eq!(phase, "parse");
                    Err(Error::Metadata("parse refused".into()))
                };
                let result = if mode == Mode::Allocations {
                    run_allocation_targets(
                        Path::new("worker"),
                        &arguments,
                        &artifacts,
                        vec![probe_target("case", 0)],
                        |_, _, _, _, _, _, _| launch(),
                        |_, _, _| parse(),
                    )
                } else {
                    run_counter_targets(
                        mode,
                        Path::new("worker"),
                        &arguments,
                        &artifacts,
                        vec![probe_target("case", 0)],
                        |_, _, _, _, _, _, _| launch(),
                        |_, _, _, _, _| parse(),
                    )
                };
                if phase == "status" {
                    assert!(matches!(result, Err(Error::WorkerFailed { mode: failed, .. }) if failed == mode.as_str()));
                } else {
                    assert!(matches!(result, Err(Error::Metadata(message)) if message == format!("{phase} refused")));
                }
            }
        }
    }

    #[test]
    fn gamma_round2_report_environment_and_custom_profiles_preserve_roots_and_errors() {
        let arguments = Arguments::parse_from(["--criterion"].map(OsString::from), None).unwrap();
        let paths = report_paths_with_environment(
            &arguments,
            "bench target",
            || Ok("worker.exe".into()),
            |executable| {
                assert_eq!(executable, Path::new("worker.exe"));
                Ok("cargo target".into())
            },
            |_, root| {
                assert_eq!(root, Path::new("cargo target").join("metabench").join("bench target"));
                Ok((Some(root.join("report.json")), Some(root.join("report.md"))))
            },
        )
        .unwrap();
        assert_eq!(
            paths.0,
            Some(Path::new("cargo target").join("metabench").join("bench target").join("report.json"))
        );
        assert!(
            matches!(report_paths_with_environment(&arguments, "bench", || Err(Error::Metadata("executable refused".into())),
            |_| panic!("no executable"), |_, _| panic!("no executable")), Err(Error::Metadata(message)) if message == "executable refused")
        );
        assert!(matches!(report_paths_with_environment(&arguments, "bench", || Ok("worker".into()),
            |_| Err(Error::Metadata("target refused".into())), |_, _| panic!("no target")), Err(Error::Metadata(message)) if message == "target refused"));
        let target = Path::new("workspace").join("target");
        let profile = target.join("custom-profile");
        let root = effective_target_directory_with(
            &profile.join("deps").join("worker.exe"),
            |name| {
                assert_eq!(name, "CARGO_TARGET_DIR");
                None
            },
            |path| {
                assert_eq!(path, profile.join(".fingerprint"));
                true
            },
            |_, _| panic!("not a target-triple directory"),
        )
        .unwrap();
        assert_eq!(root, target);
        assert!(
            matches!(effective_target_directory_with(Path::new("relocated").join("worker").as_path(), |_| None, |_| false, |_, _| false),
            Err(Error::ArtifactFormat { message, .. }) if message.contains("CARGO_TARGET_DIR"))
        );
    }

    #[test]
    fn gamma_round2_report_publication_propagates_path_baseline_and_write_errors() {
        let arguments = Arguments::parse_from(["--criterion"].map(OsString::from), None).unwrap();
        for complete in [false, true] {
            let events = std::cell::RefCell::new(Vec::new());
            let (report, json, markdown) = publish_report_with(
                report_fixture(17),
                &arguments,
                "bench",
                complete,
                |_, target| {
                    assert_eq!(target, "bench");
                    events.borrow_mut().push("paths");
                    Ok((Some("report.json".into()), Some("report.md".into())))
                },
                |report, _, path| {
                    assert_eq!(path, Some(Path::new("report.json")));
                    assert_eq!(report.entries.len(), 1);
                    events.borrow_mut().push("baseline");
                    Ok(())
                },
                |report, json, markdown| {
                    assert_eq!(report.entries.len(), 1);
                    assert_eq!(json, Some(Path::new(if complete { "report.json" } else { "report.partial.json" })));
                    assert_eq!(markdown, Some(Path::new(if complete { "report.md" } else { "report.partial.md" })));
                    events.borrow_mut().push("write");
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(report.entries.len(), 1);
            assert!(json.is_some());
            assert!(markdown.is_some());
            assert_eq!(*events.borrow(), ["paths", "baseline", "write"]);
        }
        assert!(matches!(publish_report_with(report_fixture(17), &arguments, "bench", true,
            |_, _| Err(Error::Metadata("paths refused".into())), |_, _, _| panic!("paths failed"), |_, _, _| panic!("paths failed")),
            Err(Error::Metadata(message)) if message == "paths refused"));
        assert!(matches!(publish_report_with(report_fixture(17), &arguments, "bench", true,
            |_, _| Ok((None, None)), |_, _, _| Err(Error::Metadata("baseline refused".into())), |_, _, _| panic!("baseline failed")),
            Err(Error::Metadata(message)) if message == "baseline refused"));
        assert!(matches!(publish_report_with(report_fixture(17), &arguments, "bench", true,
            |_, _| Ok((None, None)), |_, _, _| Ok(()), |_, json, markdown| {
                assert!(json.is_none()); assert!(markdown.is_none()); Err(Error::Metadata("publication refused".into()))
            }), Err(Error::Metadata(message)) if message == "publication refused"));
    }

    #[test]
    fn gamma_round2_native_worker_request_forwards_artifacts_output_timeout_and_platform_gate() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        for linux in [false, true] {
            for mode in [Mode::Criterion, Mode::Gungraun] {
                for show in [false, true] {
                    let called = Cell::new(false);
                    let result = launch_worker_request(
                        mode,
                        Some(&artifacts),
                        show,
                        Some(Duration::from_secs(37)),
                        linux,
                        |selected, included, visible, timeout| {
                            called.set(true);
                            assert_eq!(selected, mode);
                            assert_eq!(included.unwrap().path(), artifacts.path());
                            assert_eq!(visible, show);
                            assert_eq!(timeout, Some(Duration::from_secs(37)));
                            Ok(ExitStatus::from_raw(0))
                        },
                    );
                    if mode == Mode::Gungraun && !linux {
                        assert!(!called.get());
                        assert!(matches!(result, Err(Error::UnsupportedMode { mode: "gungraun", .. })));
                    } else {
                        assert!(result.unwrap().success());
                        assert!(called.get());
                    }
                }
            }
        }
        assert!(
            matches!(launch_worker_request(Mode::Criterion, None, false, None, false, |_, included, show, timeout| {
            assert!(included.is_none()); assert!(!show); assert!(timeout.is_none()); Err(Error::Metadata("launch refused".into()))
        }), Err(Error::Metadata(message)) if message == "launch refused")
        );
    }

    #[test]
    fn gamma_round2_authorized_command_execution_captures_then_tokens_and_forwards_timeout() {
        for logged in [false, true] {
            for show in [false, true] {
                let mut command = Command::new("worker.exe");
                command.arg("payload");
                let events = std::cell::RefCell::new(Vec::new());
                let path = Path::new("worker.log");
                let result = run_worker_command_with(
                    &mut command,
                    logged.then_some(path),
                    (show, Some(Duration::from_secs(41))),
                    |command, log, visible| {
                        assert_eq!(command.get_args().collect::<Vec<_>>(), [OsStr::new("payload")]);
                        assert_eq!(log, path);
                        assert_eq!(visible, show);
                        events.borrow_mut().push("capture");
                        Ok(Some(13))
                    },
                    || {
                        events.borrow_mut().push("token");
                        Ok((17, "token directory".into()))
                    },
                    |command, log_path, log, token, timeout| {
                        assert_eq!(log_path, logged.then_some(path));
                        assert_eq!(log, logged.then_some(13));
                        assert_eq!(token, 17);
                        assert_eq!(timeout, Some(Duration::from_secs(41)));
                        assert_eq!(
                            command.get_envs().find(|(name, _)| *name == WORKER_TOKEN_ENV).unwrap().1,
                            Some(OsStr::new("token directory"))
                        );
                        events.borrow_mut().push("execute");
                        Ok(ExitStatus::from_raw(0))
                    },
                )
                .unwrap();
                assert!(result.success());
                assert_eq!(
                    *events.borrow(),
                    if logged {
                        vec!["capture", "token", "execute"]
                    } else {
                        vec!["token", "execute"]
                    }
                );
            }
        }
        let mut command = Command::new("worker");
        assert!(
            matches!(run_worker_command_with::<u8, u8>(&mut command, Some(Path::new("worker.log")), (false, None),
            |_, _, _| Err(Error::Metadata("capture refused".into())), || panic!("capture failed"), |_, _, _, _, _| panic!("capture failed")),
            Err(Error::Metadata(message)) if message == "capture refused")
        );
        assert!(matches!(run_worker_command_with::<u8, u8>(&mut command, None, (true, None),
            |_, _, _| panic!("no log"), || Err(Error::Metadata("token refused".into())), |_, _, _, _, _| panic!("token failed")),
            Err(Error::Metadata(message)) if message == "token refused"));
    }

    #[test]
    fn gamma_round2_vtune_collection_preserves_directory_order_and_both_command_flags() {
        let result_dir = Path::new("results").join("case");
        let artifact = Path::new("artifacts").join("case.csv");
        for show in [false, true] {
            for failing in [
                None,
                Some("result parent"),
                Some("artifact parent"),
                Some("collect"),
                Some("report"),
            ] {
                let events = std::cell::RefCell::new(Vec::new());
                let result = launch_vtune_commands_with(
                    &result_dir,
                    &artifact,
                    (show, Some(Duration::from_secs(43))),
                    |path| {
                        let phase = if path == Path::new("results") {
                            "result parent"
                        } else {
                            assert_eq!(path, Path::new("artifacts"));
                            "artifact parent"
                        };
                        events.borrow_mut().push(phase);
                        if failing == Some(phase) {
                            Err(io::Error::new(io::ErrorKind::PermissionDenied, phase))
                        } else {
                            Ok(())
                        }
                    },
                    || {
                        events.borrow_mut().push("construct");
                        Command::new("collect.exe")
                    },
                    |command, log, visible, timeout| {
                        assert_eq!(command.get_program(), OsStr::new("collect.exe"));
                        assert_eq!(log, Path::new("artifacts").join("case.log"));
                        assert_eq!(visible, show);
                        assert_eq!(timeout, Some(Duration::from_secs(43)));
                        events.borrow_mut().push("collect");
                        if failing == Some("collect") {
                            Err(Error::Metadata("collect refused".into()))
                        } else {
                            Ok(ExitStatus::from_raw(0))
                        }
                    },
                    |command, log, visible, timeout| {
                        assert_eq!(command.get_program(), OsStr::new("vtune"));
                        assert_eq!(log, Path::new("artifacts").join("case.report.log"));
                        assert_eq!(visible, show);
                        assert_eq!(timeout, Some(Duration::from_secs(43)));
                        events.borrow_mut().push("report");
                        if failing == Some("report") {
                            Err(Error::Metadata("report refused".into()))
                        } else {
                            Ok(ExitStatus::from_raw(0))
                        }
                    },
                );
                let expected: &[&str] = match failing {
                    Some("result parent") => &["result parent"],
                    Some("artifact parent") => &["result parent", "artifact parent"],
                    Some("collect") => &["result parent", "artifact parent", "construct", "collect"],
                    None | Some("report") => &["result parent", "artifact parent", "construct", "collect", "report"],
                    _ => unreachable!(),
                };
                assert_eq!(*events.borrow(), expected);
                if let Some(phase) = failing {
                    if phase.ends_with("parent") {
                        assert!(matches!(result, Err(Error::ArtifactIo { source, .. }) if source.to_string() == phase));
                    } else {
                        assert!(matches!(result, Err(Error::Metadata(message)) if message == format!("{phase} refused")));
                    }
                } else {
                    assert!(result.unwrap().success());
                }
            }
        }
    }

    #[test]
    fn gamma_round2_vtune_failed_collection_skips_report_and_capture_errors_propagate() {
        let result_dir = Path::new("results").join("case");
        let artifact = Path::new("artifacts").join("case.csv");
        assert!(
            launch_vtune_commands_with(
                &result_dir,
                &artifact,
                (false, None),
                |_| Ok(()),
                || Command::new("collect"),
                |_, _, _, timeout| {
                    assert!(timeout.is_none());
                    Ok(ExitStatus::from_raw(1))
                },
                |_, _, _, _| panic!("collection failed")
            )
            .unwrap()
            .code()
                != Some(0)
        );
        let mut command = Command::new("report");
        assert!(
            matches!(run_report_command_with::<u8>(&mut command, &artifact, (true, Some(Duration::from_secs(47))),
            |_, _, _| Err(Error::Metadata("report capture refused".into())), |_, _, _, _| panic!("capture failed")),
            Err(Error::Metadata(message)) if message == "report capture refused")
        );
        assert!(
            run_report_command_with(
                &mut command,
                &artifact,
                (true, Some(Duration::from_secs(47))),
                |_, path, show| {
                    assert_eq!(path, artifact);
                    assert!(show);
                    Ok(Some(13))
                },
                |_, timeout, path, log| {
                    assert_eq!(timeout, Some(Duration::from_secs(47)));
                    assert_eq!(path, artifact);
                    assert_eq!(log, Some(13));
                    Ok(ExitStatus::from_raw(0))
                },
            )
            .unwrap()
            .success()
        );
    }

    #[test]
    fn gamma_round2_discovery_capture_and_console_seams_preserve_requests_and_errors() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        assert_eq!(
            native_discovery_home(&artifacts, |prefix, parent| {
                assert_eq!(prefix, "native-probe-discovery-");
                assert_eq!(parent, artifacts.path());
                Ok("discovery home".into())
            })
            .unwrap(),
            Path::new("discovery home")
        );
        assert!(
            matches!(native_discovery_home(&artifacts, |_, _| Err(io::Error::new(io::ErrorKind::PermissionDenied, "home refused"))),
            Err(Error::ArtifactIo { path, source }) if path == artifacts.path() && source.to_string() == "home refused")
        );
        assert!(matches!(read_probe_targets_with(Path::new("cases.json"), |path| {
            assert_eq!(path, Path::new("cases.json")); Err(Error::Metadata("read refused".into()))
        }), Err(Error::Metadata(message)) if message == "read refused"));
        assert_eq!(
            read_probe_targets_with(Path::new("cases.json"), |_| Ok(vec![probe_target("case", 0)])).unwrap()[0].identity,
            "case"
        );
        let mut command = Command::new("worker");
        let path = Path::new("logs").join("worker.log");
        assert!(
            capture_worker_output_with::<u8>(
                &mut command,
                &path,
                true,
                |_| panic!("output inherited"),
                |_, _| panic!("output inherited")
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(capture_worker_output_with::<u8>(&mut command, &path, false,
            |parent| { assert_eq!(parent, Path::new("logs")); Err(io::Error::new(io::ErrorKind::PermissionDenied, "parent refused")) }, |_, _| panic!("parent failed")),
            Err(Error::ArtifactIo { path: failed, source }) if failed == Path::new("logs") && source.to_string() == "parent refused"));
        assert_eq!(
            capture_worker_output_with(
                &mut command,
                &path,
                false,
                |_| Ok(()),
                |output, command| {
                    assert_eq!(output, path);
                    assert_eq!(command.get_program(), OsStr::new("worker"));
                    Ok(17)
                }
            )
            .unwrap(),
            Some(17)
        );
        let mut progress = Vec::new();
        print_progress_with(Mode::Vtune, || &mut progress).unwrap();
        assert_eq!(progress, b"Running vtune benchmarks...\n");
        let mut listing = Vec::new();
        print_probe_targets_with(&[probe_target("case", 0)], || &mut listing).unwrap();
        assert_eq!(listing, b"case: benchmark\n");
    }

    #[test]
    fn november_perf_command_explicitly_removes_unrelated_collector_environment() {
        let command = perf_command(
            Path::new("bench worker"),
            &["case".into(), "--exact".into()],
            &["--all-user".into()],
            Path::new("counter.json"),
            Path::new("criterion home"),
            "fifo:control,ack",
            (Path::new("control"), Path::new("ack")),
        );
        assert_eq!(command.get_program(), OsStr::new("perf"));
        let environment = command.get_envs().collect::<std::collections::BTreeMap<_, _>>();
        for removed in [
            allocation::ARTIFACT_ENV,
            allocation::IDENTITY_ENV,
            vtune::RESULT_DIR_ENV,
            crate::probe::DISCOVERY_ENV,
            "CARGO_CRITERION_PORT",
        ] {
            assert_eq!(environment.get(OsStr::new(removed)), Some(&None), "{removed}");
        }
        for (name, value) in [
            (WORKER_MODE_ENV, "perf"),
            ("CRITERION_HOME", "criterion home"),
            (perf::CONTROL_ENV, "control"),
            (perf::ACK_ENV, "ack"),
        ] {
            assert_eq!(environment.get(OsStr::new(name)), Some(&Some(OsStr::new(value))), "{name}");
        }
    }

    #[test]
    fn november_parent_listing_returns_the_original_listing_error_without_running_probes() {
        let result = finish_parent_modes(
            true,
            None,
            EngineRunSummary {
                completed: vec![Mode::Criterion],
                failures: vec!["prior failure".into()],
            },
            |failures| {
                assert_eq!(failures, &["prior failure"]);
                Err(Error::Metadata("listing refused".into()))
            },
            |_, _| panic!("listing failure must not launch probes"),
            |_, _, _, _| panic!("listing failure must not publish a report"),
        );
        assert!(matches!(result, Err(Error::Metadata(message)) if message == "listing refused"));
    }

    #[test]
    fn november_native_selection_separates_inline_values_from_filters() {
        for option in ["--sample-size", "--measurement-time", "--warm-up-time"] {
            for arguments in [
                vec![format!("{option}=11"), "exact-case".into(), "--exact".into()],
                vec![option.into(), "11".into(), "exact-case".into(), "--exact".into()],
            ] {
                let selected = select_native_probe_targets(
                    vec![
                        probe_target("other-case", 0),
                        probe_target("exact-case", 1),
                        probe_target("exact-case-suffix", 2),
                    ],
                    &arguments.into_iter().map(OsString::from).collect::<Vec<_>>(),
                )
                .unwrap();
                assert_eq!(selected.len(), 1);
                assert_eq!(selected[0].identity, "exact-case");
                assert_eq!(
                    selected[0].arguments(),
                    ["--metabench-probe", "2", "3", "8", "21"].map(OsString::from)
                );
            }
        }
        let selected = select_native_probe_targets(
            vec![
                probe_target("prefix-one", 0),
                probe_target("other", 1),
                probe_target("prefix-two", 2),
            ],
            &["^prefix-".into()],
        )
        .unwrap();
        assert_eq!(
            selected.iter().map(|target| target.identity.as_str()).collect::<Vec<_>>(),
            ["prefix-one", "prefix-two"]
        );
    }

    #[test]
    fn gamma_round2_entry_routes_use_first_argument_and_authorize_before_parsing() {
        let called = Cell::new(false);
        assert!(first_argument(["program"].map(OsString::from).into_iter()).is_none());
        assert!(matches!(
            entry_route(
                || first_argument(["program", "--gungraun-run", "other"].map(OsString::from).into_iter()),
                |_| panic!("native Gungraun bypasses worker environment"),
                || panic!("native Gungraun bypasses authorization"),
                || called.set(true),
            )
            .unwrap(),
            EntryRoute::Complete
        ));
        assert!(called.get());
        for argument in ["", "xyzzy", "ordinary"] {
            assert!(matches!(
                entry_route(
                    || Some(argument.into()),
                    |_| None,
                    || panic!("not a worker"),
                    || panic!("not Gungraun"),
                )
                .unwrap(),
                EntryRoute::Parent
            ));
        }
        for mode in [Mode::Criterion, Mode::Gungraun, Mode::Allocations, Mode::Perf, Mode::Vtune] {
            called.set(false);
            assert!(matches!(entry_route(
                || None,
                |name| { assert_eq!(name, "METABENCH_INTERNAL_WORKER_MODE"); Some(mode.as_str().into()) },
                || { called.set(true); Ok(()) },
                || panic!("not native Gungraun"),
            ).unwrap(), EntryRoute::Worker(selected) if selected == mode));
            assert!(called.get());
        }
        assert!(matches!(
            entry_route(
                || None,
                |_| Some("invalid".into()),
                || {
                    called.set(false);
                    Ok(())
                },
                || panic!("not Gungraun")
            ),
            Err(Error::InvalidMode(_))
        ));
        assert!(!called.get());
        assert!(matches!(entry_route(
            || None, |_| Some("invalid".into()), || Err(Error::Metadata("token refused".into())), || panic!("not Gungraun"),
        ), Err(Error::Metadata(message)) if message == "token refused"));
    }

    #[test]
    fn gamma_round2_worker_dispatch_primes_probes_lists_and_finishes_only_its_collector() {
        for mode in [Mode::Criterion, Mode::Gungraun, Mode::Allocations, Mode::Perf, Mode::Vtune] {
            for argument in [None, Some("ordinary"), Some(""), Some("xyzzy"), Some("--metabench-probe-list")] {
                let events = std::cell::RefCell::new(Vec::new());
                run_worker_mode(
                    mode,
                    |selected| {
                        assert_eq!(selected, mode);
                        events.borrow_mut().push("prime");
                    },
                    |selected| {
                        assert_eq!(selected, mode);
                        events.borrow_mut().push("native");
                    },
                    || {
                        events.borrow_mut().push("probe");
                        Ok(())
                    },
                    || {
                        events.borrow_mut().push("argument");
                        argument.map(OsString::from)
                    },
                    (
                        || {
                            events.borrow_mut().push("allocations");
                            Ok(())
                        },
                        || {
                            events.borrow_mut().push("perf");
                            Ok(())
                        },
                        || {
                            events.borrow_mut().push("vtune");
                            Ok(())
                        },
                    ),
                )
                .unwrap();
                let expected = match mode {
                    Mode::Criterion | Mode::Gungraun => vec!["native"],
                    Mode::Allocations | Mode::Perf | Mode::Vtune if argument == Some("--metabench-probe-list") => {
                        vec!["prime", "probe", "argument"]
                    }
                    Mode::Allocations => vec!["prime", "probe", "argument", "allocations"],
                    Mode::Perf => vec!["prime", "probe", "argument", "perf"],
                    Mode::Vtune => vec!["prime", "probe", "argument", "vtune"],
                };
                assert_eq!(*events.borrow(), expected);
            }
        }
    }

    #[test]
    fn gamma_round2_worker_errors_propagate_without_finishing_failed_probes() {
        for mode in [Mode::Allocations, Mode::Perf, Mode::Vtune] {
            assert!(
                matches!(run_worker_mode(mode, |_| {}, |_| panic!("probe worker"), || Err("probe refused".into()),
                || panic!("probe failed"), (|| panic!("probe failed"), || panic!("probe failed"), || panic!("probe failed"))),
                Err(Error::Probe(message)) if message == "probe refused")
            );
            let finish = || Err(Error::Metadata(format!("{} finish refused", mode.as_str())));
            assert!(
                matches!(run_worker_mode(mode, |_| {}, |_| panic!("probe worker"), || Ok(()), || None,
                (finish, finish, finish)), Err(Error::Metadata(message)) if message == format!("{} finish refused", mode.as_str()))
            );
        }
    }

    #[test]
    fn gamma_round2_parent_entry_help_and_errors_are_not_silently_discarded() {
        for help in [false, true] {
            let mut arguments = Arguments::parse_from(["--criterion"].map(OsString::from), None).unwrap();
            arguments.help = help;
            let called = Cell::new("");
            parent_entry(
                || Ok(arguments),
                |bytes| {
                    assert_eq!(bytes, HELP.as_bytes());
                    called.set("help");
                    Ok(())
                },
                |_| {
                    called.set("parent");
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(called.get(), if help { "help" } else { "parent" });
        }
        assert!(
            matches!(parent_entry(|| Err(Error::Metadata("parse refused".into())), |_| panic!("parse failed"), |_| panic!("parse failed")),
            Err(Error::Metadata(message)) if message == "parse refused")
        );
        assert!(matches!(parent_entry(
            || Arguments::parse_from(["--help"].map(OsString::from), None),
            |_| Err(io::Error::new(io::ErrorKind::BrokenPipe, "help refused")),
            |_| panic!("help requested"),
        ), Err(Error::HelpOutput(error)) if error.kind() == io::ErrorKind::BrokenPipe && error.to_string() == "help refused"));
        assert!(matches!(parent_entry(
            || Arguments::parse_from(["--criterion"].map(OsString::from), None),
            |_| panic!("not help"), |_| Err(Error::Metadata("parent refused".into())),
        ), Err(Error::Metadata(message)) if message == "parent refused"));
    }

    #[test]
    fn gamma_round2_parent_preparation_and_mode_selection_preserve_order_and_fallback() {
        let modes = [Mode::Vtune, Mode::Gungraun, Mode::Allocations, Mode::Criterion, Mode::Perf];
        assert_eq!(parent_native_modes(&modes), [Mode::Gungraun, Mode::Criterion]);
        assert!(has_probe_modes(&modes));
        assert!(!has_probe_modes(&[Mode::Criterion, Mode::Gungraun]));
        assert!(!has_probe_modes(&[]));
        for smoke in [false, true] {
            for list in [false, true] {
                assert_eq!(
                    select_smoke_modes(&modes, smoke, list),
                    (smoke && !list).then(|| vec![Mode::Gungraun, Mode::Criterion])
                );
            }
        }
        assert_eq!(select_smoke_modes(&[Mode::Perf], true, false), Some(vec![Mode::Criterion]));
        assert_eq!(select_smoke_modes(&[], true, false), Some(vec![Mode::Criterion]));
        let mut arguments = Arguments::parse_from(["--criterion", "--list"].map(OsString::from), None).unwrap();
        let (selected, executable) = prepare_parent(&mut arguments, || Ok("worker.exe".into())).unwrap();
        assert_eq!(selected, [Mode::Criterion]);
        assert_eq!(executable, Path::new("worker.exe"));
        assert!(arguments.args_for(Mode::Criterion).contains(&OsString::from("--list")));
        assert!(
            matches!(prepare_parent(&mut arguments, || Err(Error::Metadata("executable refused".into()))),
            Err(Error::Metadata(message)) if message == "executable refused")
        );
        let mut arguments = Arguments::parse_from(["--perf", "--perf-arg", "event"].map(OsString::from), None).unwrap();
        arguments.modes = vec![Mode::Criterion];
        assert!(matches!(
            prepare_parent(&mut arguments, || panic!("argument validation must happen first")),
            Err(Error::ArgumentsForUnselectedEngine("perf"))
        ));
        let root = report_root(Path::new("worker.exe"), "bench-target", |path| {
            assert_eq!(path, Path::new("worker.exe"));
            Ok(PathBuf::from("cargo target"))
        })
        .unwrap();
        assert_eq!(root, Path::new("cargo target").join("metabench").join("bench-target"));
        assert!(
            matches!(report_root(Path::new("worker"), "bench", |_| Err(Error::Metadata("root refused".into()))),
            Err(Error::Metadata(message)) if message == "root refused")
        );
        for list in [false, true] {
            for probes in [false, true] {
                let called = Cell::new(false);
                arguments.list = list;
                let modes = if probes {
                    vec![Mode::Criterion, Mode::Perf]
                } else {
                    vec![Mode::Criterion]
                };
                let result = create_parent_artifacts(Path::new("root"), &arguments, &modes, |root, listing, has_probes| {
                    assert_eq!(listing, list);
                    assert_eq!(has_probes, probes);
                    create_run_artifacts_with(root, listing, has_probes, |path| {
                        assert_eq!(path, Path::new("root"));
                        called.set(true);
                        Ok(ArtifactDirectory::gamma_round2_in_memory_directory())
                    })
                })
                .unwrap();
                assert_eq!(result.is_some(), !list || probes);
                assert_eq!(called.get(), !list || probes);
            }
        }
        assert!(
            matches!(create_run_artifacts_with(Path::new("root"), false, false, |_| Err(Error::Metadata("create refused".into()))),
            Err(Error::Metadata(message)) if message == "create refused")
        );
    }

    #[test]
    fn gamma_round2_smoke_launch_forces_output_and_propagates_failure_policy() {
        let mut arguments = Arguments::parse_from(["--criterion", "--gungraun"].map(OsString::from), None).unwrap();
        arguments.timeout = Some(Duration::from_secs(31));
        let seen = std::cell::RefCell::new(Vec::new());
        run_parent_smoke(&[Mode::Criterion, Mode::Gungraun], &arguments, |mode, argv, show, timeout| {
            assert_eq!(argv, arguments.args_for(mode));
            assert!(show);
            assert_eq!(timeout, Some(Duration::from_secs(31)));
            seen.borrow_mut().push(mode);
            Ok(ExitStatus::from_raw(0))
        })
        .unwrap();
        assert_eq!(*seen.borrow(), [Mode::Criterion, Mode::Gungraun]);
        arguments.failure_mode = FailureMode::FailFast;
        assert!(
            matches!(run_parent_smoke(&[Mode::Criterion], &arguments, |_, _, _, _| Err(Error::Metadata("launch refused".into()))),
            Err(Error::Metadata(message)) if message == "launch refused")
        );
        assert!(matches!(
            run_parent_smoke(&[Mode::Criterion], &arguments, |_, _, _, _| Ok(ExitStatus::from_raw(1))),
            Err(Error::WorkerFailed { mode: "criterion", .. })
        ));
    }

    #[test]
    fn gamma_round2_native_parent_requests_apply_list_show_truth_table_and_publish_manifests() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        for list in [false, true] {
            for show in [false, true] {
                let mut arguments = Arguments::parse_from(["--criterion"].map(OsString::from), None).unwrap();
                arguments.list = list;
                arguments.show_engine_output = show;
                arguments.timeout = Some(Duration::from_secs(19));
                let events = std::cell::RefCell::new(Vec::new());
                let summary = run_parent_native_modes(
                    &[Mode::Criterion],
                    &arguments,
                    Some(&artifacts),
                    |mode| {
                        assert_eq!(mode, Mode::Criterion);
                        events.borrow_mut().push("progress");
                        Ok(())
                    },
                    |mode, argv, included, visible, timeout| {
                        assert_eq!(mode, Mode::Criterion);
                        assert_eq!(argv, arguments.args_for(Mode::Criterion));
                        assert_eq!(included.unwrap().path(), artifacts.path());
                        assert_eq!(visible, show || list);
                        assert_eq!(timeout, Some(Duration::from_secs(19)));
                        events.borrow_mut().push("launch");
                        Ok(ExitStatus::from_raw(0))
                    },
                    |included, mode, argv, status| {
                        assert_eq!(included.path(), artifacts.path());
                        assert_eq!(mode, Mode::Criterion);
                        assert_eq!(argv, arguments.args_for(Mode::Criterion));
                        assert!(status.unwrap().success());
                        events.borrow_mut().push("manifest");
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(summary.completed, [Mode::Criterion]);
                assert!(summary.failures.is_empty());
                assert_eq!(
                    *events.borrow(),
                    if list {
                        vec!["launch", "manifest"]
                    } else {
                        vec!["progress", "launch", "manifest"]
                    }
                );
            }
        }
        let mut arguments = Arguments::parse_from(["--criterion"].map(OsString::from), None).unwrap();
        arguments.failure_mode = FailureMode::FailFast;
        assert!(matches!(run_parent_native_modes(&[Mode::Criterion], &arguments, Some(&artifacts),
            |_| Err(Error::Metadata("progress refused".into())), |_, _, _, _, _| panic!("progress failed"), |_, _, _, _| panic!("progress failed")),
            Err(Error::Metadata(message)) if message == "progress refused"));
        arguments.failure_mode = FailureMode::KeepGoing;
        let summary = run_parent_native_modes(
            &[Mode::Criterion],
            &arguments,
            Some(&artifacts),
            |_| Ok(()),
            |_, _, _, _, _| Err(Error::Metadata("launch refused".into())),
            |_, _, _, status| {
                assert!(status.is_none());
                Err(Error::MultipleModeFailures(vec!["manifest refused".into()]))
            },
        )
        .unwrap();
        assert!(summary.completed.is_empty());
        assert_eq!(
            summary.failures,
            [Error::Metadata("launch refused".into()).to_string(), "manifest refused".into()]
        );
        let summary = run_parent_native_modes(
            &[Mode::Criterion],
            &arguments,
            None,
            |_| Ok(()),
            |_, _, included, _, _| {
                assert!(included.is_none());
                Ok(ExitStatus::from_raw(0))
            },
            |_, _, _, _| panic!("no artifact directory"),
        )
        .unwrap();
        assert_eq!(summary.completed, [Mode::Criterion]);
    }

    #[test]
    fn gamma_round2_parent_tail_routes_listing_probes_reports_and_complete_state() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        for list in [false, true] {
            for has_artifacts in [false, true] {
                for completed in [false, true] {
                    for failed in [false, true] {
                        let events = std::cell::RefCell::new(Vec::new());
                        let result = finish_parent_modes(
                            list,
                            has_artifacts.then_some(&artifacts),
                            EngineRunSummary {
                                completed: if completed { vec![Mode::Criterion] } else { vec![] },
                                failures: if failed { vec!["earlier failure".into()] } else { vec![] },
                            },
                            |_| {
                                events.borrow_mut().push("list");
                                Ok(())
                            },
                            |_, _| {
                                events.borrow_mut().push("probes");
                                Ok(vec![native_result("perf", "probe")])
                            },
                            |included, modes, results, complete| {
                                assert_eq!(included.path(), artifacts.path());
                                assert_eq!(modes, [Mode::Criterion]);
                                assert_eq!(results[0].identity, "probe");
                                assert_eq!(complete, !failed);
                                events.borrow_mut().push("report");
                                Ok(())
                            },
                        );
                        if failed {
                            assert!(matches!(result, Err(Error::MultipleModeFailures(failures)) if failures == ["earlier failure"]));
                        } else {
                            result.unwrap();
                        }
                        assert_eq!(
                            *events.borrow(),
                            if list {
                                vec!["list"]
                            } else if has_artifacts && completed {
                                vec!["probes", "report"]
                            } else {
                                vec!["probes"]
                            }
                        );
                    }
                }
            }
        }
        let summary = || EngineRunSummary {
            completed: vec![Mode::Perf],
            failures: vec!["earlier".into()],
        };
        assert!(
            matches!(finish_parent_modes(false, Some(&artifacts), summary(), |_| panic!("not listing"),
            |_, _| Ok(vec![]), |_, _, _, _| Err(Error::MultipleModeFailures(vec!["report one".into(), "report two".into()]))),
            Err(Error::MultipleModeFailures(failures)) if failures == ["earlier", "report one", "report two"])
        );
        assert!(
            matches!(finish_parent_modes(false, Some(&artifacts), summary(), |_| panic!("not listing"),
            |_, _| Err(Error::Metadata("probe refused".into())), |_, _, _, _| panic!("probe failed")),
            Err(Error::Metadata(message)) if message == "probe refused")
        );
    }

    fn probe_target(identity: &str, index: usize) -> ProbeTarget {
        serde_json::from_value(serde_json::json!({
            "identity": identity, "native_identity": format!("native/{identity}"),
            "group": 2, "benchmark": 3, "case": index + 7, "iteration": index + 20
        }))
        .unwrap()
    }

    #[test]
    fn gamma_round2_probe_listing_preserves_discovery_and_output_errors() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let mut failures = vec![];
        list_probe_cases(false, None, &mut failures, |_| panic!("no probes"), |_| panic!("no probes")).unwrap();
        list_probe_cases(
            true,
            Some(&artifacts),
            &mut failures,
            |included| {
                assert_eq!(included.path(), artifacts.path());
                Ok(vec![probe_target("chosen", 0)])
            },
            |targets| {
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].identity, "chosen");
                Ok(())
            },
        )
        .unwrap();
        list_probe_cases(
            true,
            Some(&artifacts),
            &mut failures,
            |_| Err(Error::MultipleModeFailures(vec!["one".into(), "two".into()])),
            |_| panic!("discovery failed"),
        )
        .unwrap();
        assert_eq!(failures, ["one", "two"]);
        assert!(matches!(list_probe_cases(true, Some(&artifacts), &mut failures, |_| Ok(vec![]),
            |_| Err(Error::Metadata("print refused".into()))), Err(Error::Metadata(message)) if message == "print refused"));
    }

    #[test]
    fn gamma_round2_probe_loop_appends_completed_results_and_stops_on_progress_error() {
        let modes = [Mode::Criterion, Mode::Allocations, Mode::Gungraun, Mode::Perf, Mode::Vtune];
        let mut summary = EngineRunSummary {
            completed: vec![Mode::Criterion],
            failures: vec![],
        };
        let mut failures = vec![];
        let progress = std::cell::RefCell::new(Vec::new());
        let results = run_probe_modes_with(
            &modes,
            FailureMode::KeepGoing,
            &mut summary,
            &mut failures,
            |mode| {
                progress.borrow_mut().push(mode);
                Ok(())
            },
            |mode| {
                if mode == Mode::Perf {
                    Err(Error::MultipleModeFailures(vec!["perf refused".into()]))
                } else {
                    Ok(vec![native_result(mode.as_str(), mode.as_str())])
                }
            },
        )
        .unwrap();
        assert_eq!(*progress.borrow(), [Mode::Allocations, Mode::Perf, Mode::Vtune]);
        assert_eq!(summary.completed, [Mode::Criterion, Mode::Allocations, Mode::Vtune]);
        assert_eq!(
            results.iter().map(|result| result.identity.as_str()).collect::<Vec<_>>(),
            ["allocations", "vtune"]
        );
        assert_eq!(failures, ["perf refused"]);
        assert!(
            matches!(run_probe_modes_with(&[Mode::Perf], FailureMode::KeepGoing, &mut summary, &mut failures,
            |_| Err(Error::Metadata("progress refused".into())), |_| panic!("progress failed")), Err(Error::Metadata(message)) if message == "progress refused")
        );
        assert!(
            matches!(run_probe_modes_with(&[Mode::Perf], FailureMode::FailFast, &mut summary, &mut failures,
            |_| Ok(()), |_| Err(Error::Metadata("measurement refused".into()))), Err(Error::Metadata(message)) if message == "measurement refused")
        );
    }
    #[test]
    fn gamma_round2_vtune_report_runs_only_after_successful_collection_and_cleanup() {
        let successful = ExitStatus::from_raw(0);
        let failed = ExitStatus::from_raw(1);
        let result = finish_vtune_collection(Ok(successful), Ok(()), || Ok(failed)).unwrap();
        assert_eq!(result, failed);
        let result = finish_vtune_collection(Ok(failed), Ok(()), || panic!("failed collection must not report")).unwrap();
        assert_eq!(result, failed);
        assert!(
            matches!(finish_vtune_collection(Err(Error::Metadata("collection refused".into())), Ok(()), || panic!("failed collection")),
            Err(Error::Metadata(message)) if message == "collection refused")
        );
        assert!(
            matches!(finish_vtune_collection(Ok(successful), Err(Error::Metadata("cleanup refused".into())), || panic!("failed cleanup")),
            Err(Error::Metadata(message)) if message == "cleanup refused")
        );
        assert!(
            matches!(finish_vtune_collection(Ok(successful), Ok(()), || Err(Error::Metadata("report refused".into()))),
            Err(Error::Metadata(message)) if message == "report refused")
        );
    }

    #[test]
    fn gamma_round2_console_report_preserves_table_newlines_paths_and_output_errors() {
        struct Refused;
        impl io::Write for Refused {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "output refused"))
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("no flush required")
            }
        }
        let report = report_fixture(7);
        for (json, markdown, suffix) in [
            (None, None, ""),
            (Some(Path::new("report.json")), None, ""),
            (None, Some(Path::new("report.md")), ""),
            (
                Some(Path::new("report.json")),
                Some(Path::new("report.md")),
                "Generated report.json\nGenerated report.md\n",
            ),
        ] {
            let mut output = Vec::new();
            write_console_report_with(
                &report,
                json,
                markdown,
                |received| {
                    assert_eq!(received.entries[0].identity, report.entries[0].identity);
                    "fixed table".into()
                },
                &mut output,
            )
            .unwrap();
            assert_eq!(String::from_utf8(output).unwrap(), format!("\nfixed table\n{suffix}"));
        }
        assert!(
            matches!(write_console_report_with(&report, None, None, |_| "table".into(), Refused),
            Err(Error::ConsoleOutput(error)) if error.kind() == io::ErrorKind::BrokenPipe && error.to_string() == "output refused")
        );
    }

    #[test]
    fn gamma_round2_worker_cleanup_preserves_primary_success_failure_and_failure_order() {
        assert_eq!(combine_results(Ok(17), Ok(())).unwrap(), 17);
        for (primary, cleanup, expected) in [
            (Err(Error::Metadata("worker".into())), Ok(()), vec!["worker"]),
            (Ok(17), Err(Error::Metadata("cleanup".into())), vec!["cleanup"]),
        ] {
            let error = combine_results(primary, cleanup).unwrap_err();
            assert!(error.to_string().contains(expected[0]));
        }
        let error = combine_results(
            Err::<(), _>(Error::Metadata("worker".into())),
            Err(Error::Metadata("cleanup".into())),
        )
        .unwrap_err();
        let worker = Error::Metadata("worker".into()).to_string();
        let cleanup = Error::Metadata("cleanup".into()).to_string();
        assert!(matches!(error, Error::MultipleModeFailures(failures) if failures == [worker, cleanup]));
    }

    #[test]
    fn gamma_round2_probe_discovery_forwards_protocol_and_validates_selected_cases() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;
        let home = Path::new("discovery home");
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let mut arguments = Arguments::parse_from(
            ["--criterion-arg", "--exact", "--criterion-arg", "chosen"].map(OsString::from),
            None,
        )
        .unwrap();
        arguments.timeout = Some(Duration::from_secs(17));
        let target = |identity: &str| {
            serde_json::from_value::<ProbeTarget>(serde_json::json!({
            "identity": identity, "native_identity": format!("native/{identity}"), "group": 0, "benchmark": 0, "case": 0, "iteration": null
        })).unwrap()
        };
        let results = discover_native_probe_targets_with(
            &arguments,
            &artifacts,
            home,
            |mode, argv, included, show, timeout, work, output| {
                assert_eq!(mode, Mode::Allocations);
                assert_eq!(argv, [OsString::from("--metabench-probe-list")]);
                assert_eq!(included.unwrap().path(), artifacts.path());
                assert!(!show);
                assert_eq!(timeout, Some(Duration::from_secs(17)));
                assert_eq!(work, Some(home));
                assert_eq!(output, Some(home.join("cases.json").as_path()));
                Ok(ExitStatus::from_raw(0))
            },
            |output| {
                assert_eq!(output, home.join("cases.json"));
                Ok(vec![target("other"), target("chosen")])
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].identity, "chosen");
        assert!(
            matches!(discover_native_probe_targets_with(&arguments, &artifacts, home, |_, _, _, _, _, _, _| Ok(ExitStatus::from_raw(0)), |_| Ok(Vec::new())),
            Err(Error::ArtifactFormat { path, message }) if path == home.join("cases.json") && message == "no native benchmark cases matched; existing reports were preserved")
        );
        assert!(
            matches!(discover_native_probe_targets_with(&arguments, &artifacts, home, |_, _, _, _, _, _, _| Err(Error::Metadata("launch refused".into())), |_| panic!("failed launch")),
            Err(Error::Metadata(message)) if message == "launch refused")
        );
        assert!(
            matches!(discover_native_probe_targets_with(&arguments, &artifacts, home, |_, _, _, _, _, _, _| Ok(ExitStatus::from_raw(0)), |_| Err(Error::Metadata("discovery unreadable".into()))),
            Err(Error::Metadata(message)) if message == "discovery unreadable")
        );
        assert!(matches!(
            discover_native_probe_targets_with(
                &arguments,
                &artifacts,
                home,
                |_, _, _, _, _, _, _| Ok(ExitStatus::from_raw(1)),
                |_| panic!("failed worker")
            ),
            Err(Error::WorkerFailed { mode: "allocations", .. })
        ));
        let invalid = Arguments::parse_from(["--criterion-arg", "["].map(OsString::from), None).unwrap();
        assert!(
            matches!(discover_native_probe_targets_with(&invalid, &artifacts, home, |_, _, _, _, _, _, _| Ok(ExitStatus::from_raw(0)), |_| Ok(vec![target("chosen")])),
            Err(Error::Probe(message)) if message.starts_with("invalid case filter:"))
        );
    }

    fn command_env(command: &Command) -> std::collections::BTreeMap<OsString, Option<OsString>> {
        command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect()
    }

    fn native_result(source: &str, identity: &str) -> crate::report::NativeResult {
        crate::report::NativeResult {
            identity: identity.into(),
            native_identity: format!("native::{identity}"),
            source: source.into(),
            metrics: std::collections::BTreeMap::from([(
                "cycles".into(),
                crate::report::Metric::float(source, "cycles", 17.0, crate::report::MetricDirection::LowerIsBetter),
            )]),
            raw_artifacts: vec![format!("{identity}.json")],
        }
    }

    fn report_fixture(value: u64) -> BenchmarkReport {
        serde_json::from_value(serde_json::json!({
            "schema_version": 9,
            "metadata": {
                "timestamp_utc": "2026-01-02T03:04:05Z",
                "target_triple": "test-target", "rustc_version": "test-rustc",
                "host_cpu": "test-cpu", "profile": "release", "compiler_flags": null,
                "measured_backends": ["perf"], "artifact_root": "artifacts"
            },
            "entries": [{
                "identity": "case", "status": "uncompared",
                "results": { "perf": { "metrics": { "cycles": {
                    "value": value, "direction": "lower_is_better"
                } } } }
            }]
        }))
        .unwrap()
    }

    #[test]
    fn gamma_round2_native_commands_preserve_arguments_and_collector_isolation() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let executable = Path::new("worker with spaces.exe");
        let arguments = ["--test", "case with spaces", "x=y"].map(OsString::from);
        for mode in [Mode::Criterion, Mode::Allocations, Mode::Gungraun] {
            let command = native_worker_command(mode, executable, &arguments, Some(&artifacts), None, None, None);
            assert_eq!(command.get_program(), executable);
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                arguments.iter().map(OsString::as_os_str).collect::<Vec<_>>()
            );
            let env = command_env(&command);
            assert_eq!(env.get(OsStr::new(WORKER_MODE_ENV)), Some(&Some(mode.as_str().into())));
            for name in [
                allocation::ARTIFACT_ENV,
                allocation::IDENTITY_ENV,
                perf::CONTROL_ENV,
                perf::ACK_ENV,
                vtune::RESULT_DIR_ENV,
                crate::probe::DISCOVERY_ENV,
            ] {
                assert_eq!(env.get(OsStr::new(name)), Some(&None), "{mode:?}/{name}");
            }
            if mode == Mode::Gungraun {
                assert_eq!(
                    env.get(OsStr::new("GUNGRAUN_HOME")),
                    Some(&Some(artifacts.gungraun_path().into_os_string()))
                );
                assert_eq!(env.get(OsStr::new("GUNGRAUN_SAVE_SUMMARY")), Some(&Some("json".into())));
            } else {
                assert_eq!(
                    env.get(OsStr::new("CRITERION_HOME")),
                    Some(&Some(artifacts.criterion_path().into_os_string()))
                );
                assert_eq!(env.get(OsStr::new("CARGO_CRITERION_PORT")), Some(&None));
            }
        }
    }

    #[test]
    fn gamma_round2_native_command_optional_inputs_and_log_paths_are_independent() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let mut command = native_worker_command(
            Mode::Allocations,
            Path::new("worker.exe"),
            &[],
            Some(&artifacts),
            Some(Path::new("isolated-home")),
            Some(Path::new("discovery.json")),
            Some(("group/case", Path::new("allocation.json"))),
        );
        authorize_worker(&mut command, Path::new("authorization-token"));
        let env = command_env(&command);
        for (name, value) in [
            ("CRITERION_HOME", "isolated-home"),
            (allocation::IDENTITY_ENV, "group/case"),
            (allocation::ARTIFACT_ENV, "allocation.json"),
            (crate::probe::DISCOVERY_ENV, "discovery.json"),
            (WORKER_TOKEN_ENV, "authorization-token"),
        ] {
            assert_eq!(env.get(OsStr::new(name)), Some(&Some(value.into())), "{name}");
        }
        let bare = native_worker_command(Mode::Criterion, Path::new("worker.exe"), &[], None, None, None, None);
        assert_eq!(command_env(&bare).get(OsStr::new("CRITERION_HOME")), None);
        assert_eq!(
            worker_log_path(Mode::Allocations, Some(Path::new("isolated-home")), Some(&artifacts)),
            Some(Path::new("isolated-home").join("allocations.log"))
        );
        assert_eq!(
            worker_log_path(Mode::Criterion, None, Some(&artifacts)),
            Some(artifacts.path().join("logs").join("criterion.log"))
        );
        assert_eq!(worker_log_path(Mode::Criterion, None, None), None);
        assert_eq!(collector_log_path(Path::new("report.csv"), false), PathBuf::from("report.log"));
        assert_eq!(
            collector_log_path(Path::new("report.csv"), true),
            PathBuf::from("report.report.log")
        );
    }

    #[test]
    fn gamma_round2_vtune_commands_preserve_exact_argv_and_environment() {
        let arguments = ["--metabench-probe", "case with spaces"].map(OsString::from);
        let collect = vtune_collect_command(
            Path::new("worker.exe"),
            &arguments,
            &["-knob".into(), "collect=true".into()],
            Path::new("result directory"),
            Path::new("criterion-home"),
        );
        assert_eq!(collect.get_program(), "vtune");
        assert_eq!(
            collect.get_args().collect::<Vec<_>>(),
            [
                "-collect-with",
                "runsa",
                "-knob",
                "collect=true",
                "--start-paused",
                "-result-dir",
                "result directory",
                "--",
                "worker.exe",
                "--metabench-probe",
                "case with spaces",
            ]
            .map(OsStr::new)
        );
        let env = command_env(&collect);
        assert_eq!(env.get(OsStr::new(WORKER_MODE_ENV)), Some(&Some("vtune".into())));
        assert_eq!(env.get(OsStr::new("CRITERION_HOME")), Some(&Some("criterion-home".into())));
        assert_eq!(env.get(OsStr::new("CARGO_CRITERION_PORT")), Some(&None));
        assert_eq!(env.get(OsStr::new(vtune::RESULT_DIR_ENV)), Some(&Some("result directory".into())));
        for name in [
            allocation::ARTIFACT_ENV,
            allocation::IDENTITY_ENV,
            perf::CONTROL_ENV,
            perf::ACK_ENV,
            crate::probe::DISCOVERY_ENV,
        ] {
            assert_eq!(env.get(OsStr::new(name)), Some(&None));
        }
        let report = vtune_report_command(Path::new("result directory"), Path::new("report.csv"));
        assert_eq!(report.get_program(), "vtune");
        assert_eq!(
            report.get_args().collect::<Vec<_>>(),
            [
                "-report",
                "hw-events",
                "-r",
                "result directory",
                "-format",
                "csv",
                "-report-output",
                "report.csv"
            ]
            .map(OsStr::new)
        );
        for name in [
            allocation::ARTIFACT_ENV,
            allocation::IDENTITY_ENV,
            perf::CONTROL_ENV,
            perf::ACK_ENV,
            vtune::RESULT_DIR_ENV,
            crate::probe::DISCOVERY_ENV,
        ] {
            assert_eq!(command_env(&report).get(OsStr::new(name)), Some(&None));
        }
    }

    #[test]
    fn gamma_round2_perf_command_is_inspectable_without_linux_or_fifos() {
        let command = perf_command(
            Path::new("worker.exe"),
            &["case".into()],
            &["--user-option".into()],
            Path::new("perf.json"),
            Path::new("criterion-home"),
            "fifo:control,ack",
            (Path::new("control"), Path::new("ack")),
        );
        assert_eq!(command.get_program(), "perf");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "stat",
                "--user-option",
                "--event=instructions:u",
                "--event=cycles:u",
                "--event=branches:u",
                "--event=branch-misses:u",
                "--event=cache-references:u",
                "--event=cache-misses:u",
                "--delay=-1",
                "--json-output",
                "--output",
                "perf.json",
                "--control",
                "fifo:control,ack",
                "--",
                "worker.exe",
                "case",
            ]
            .map(OsStr::new)
        );
        let env = command_env(&command);
        for (name, value) in [
            (WORKER_MODE_ENV, "perf"),
            ("CRITERION_HOME", "criterion-home"),
            (perf::CONTROL_ENV, "control"),
            (perf::ACK_ENV, "ack"),
        ] {
            assert_eq!(env.get(OsStr::new(name)), Some(&Some(value.into())));
        }
        assert_eq!(env.get(OsStr::new("CARGO_CRITERION_PORT")), Some(&None));
    }

    #[test]
    fn gamma_round2_report_collection_loads_only_completed_backends_in_order() {
        let mut loaded = Vec::new();
        let results = collect_report_results(
            &[Mode::Gungraun, Mode::Criterion, Mode::Perf],
            vec![native_result("perf", "probe")],
            Some("ns"),
            |mode, unit| {
                loaded.push(mode);
                assert_eq!(unit, (mode == Mode::Criterion).then_some("ns"));
                Ok(vec![native_result(mode.as_str(), mode.as_str())])
            },
        )
        .unwrap();
        assert_eq!(loaded, [Mode::Criterion, Mode::Gungraun]);
        assert_eq!(
            results.iter().map(|r| r.identity.as_str()).collect::<Vec<_>>(),
            ["probe", "criterion", "gungraun"]
        );
        let results = collect_report_results(&[Mode::Perf], vec![native_result("perf", "probe")], None, |_, _| {
            panic!("no native backend completed")
        })
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].identity, "probe");
        assert!(
            matches!(collect_report_results(&[Mode::Criterion], Vec::new(), None, |_, _| Err(Error::Metadata("load failed".into()))), Err(Error::Metadata(message)) if message == "load failed")
        );
    }

    #[test]
    fn gamma_round2_report_backend_loading_preserves_errors_and_publication_order() {
        for mode in [Mode::Criterion, Mode::Gungraun] {
            let results = load_report_backend(
                mode,
                Path::new("backend"),
                || Ok(vec![native_result(mode.as_str(), "case")]),
                |results| {
                    assert_eq!(results.len(), 1);
                    assert_eq!(results[0].source, mode.as_str());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(results.len(), 1);
            assert!(
                matches!(load_report_backend(mode, Path::new("backend"), || Err(Error::Metadata("parse failed".into())), |_| panic!("cannot record failed parse")),
                Err(Error::Metadata(message)) if message == "parse failed")
            );
            assert!(
                matches!(load_report_backend(mode, Path::new("backend"), || Ok(Vec::new()), |_| panic!("cannot record empty backend")),
                Err(Error::ArtifactFormat { path, message }) if path == Path::new("backend")
                    && message == format!("{} produced no benchmark measurements; existing reports were preserved", mode.as_str()))
            );
            assert!(
                matches!(load_report_backend(mode, Path::new("backend"), || Ok(vec![native_result(mode.as_str(), "case")]), |_| Err(Error::Metadata("record failed".into()))),
                Err(Error::Metadata(message)) if message == "record failed")
            );
        }
        let report = assemble_report(
            vec![native_result("perf", "case")],
            &[Mode::Perf, Mode::Allocations],
            Path::new("root"),
            |results, backends, root| {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].identity, "case");
                assert_eq!(backends, ["perf", "allocations"]);
                assert_eq!(root, "root");
                Ok(report_fixture(17))
            },
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert!(
            matches!(assemble_report(Vec::new(), &[], Path::new("root"), |_, _, _| Err(Error::Metadata("assemble failed".into()))),
            Err(Error::Metadata(message)) if message == "assemble failed")
        );
    }

    #[test]
    fn gamma_round2_baseline_result_handling_preserves_success_policy_and_write_errors() {
        struct Refuse;
        impl io::Write for Refuse {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("write failed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = Vec::new();
        let report = handle_baseline_result(Ok(report_fixture(17)), true, &mut output).unwrap().unwrap();
        assert_eq!(report.entries.len(), 1);
        assert!(output.is_empty());
        let unsupported = || Error::UnsupportedReportSchema {
            path: "baseline.json".into(),
            found: 1,
            expected: 9,
        };
        assert!(handle_baseline_result(Err(unsupported()), true, &mut output).unwrap().is_none());
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("metabench: skipping incompatible automatic baseline: {}\n", unsupported())
        );
        assert!(matches!(
            handle_baseline_result(Err(unsupported()), false, &mut Vec::new()),
            Err(Error::UnsupportedReportSchema { found: 1, expected: 9, .. })
        ));
        assert!(
            matches!(handle_baseline_result(Err(Error::Metadata("read failed".into())), true, &mut Vec::new()),
            Err(Error::Metadata(message)) if message == "read failed")
        );
        assert!(matches!(handle_baseline_result(Err(unsupported()), true, &mut Refuse),
            Err(Error::ConsoleOutput(error)) if error.to_string() == "write failed"));
    }

    #[test]
    fn gamma_round2_report_nonempty_guard_and_partial_destinations() {
        let mut report = report_fixture(17);
        require_report_entries(&report, Path::new("run")).unwrap();
        report.entries.clear();
        assert!(
            matches!(require_report_entries(&report, Path::new("run")), Err(Error::ArtifactFormat { path, message })
            if path == Path::new("run") && message == "no benchmark cases matched; existing reports were preserved")
        );
        for complete in [true, false] {
            let json = PathBuf::from("report.json");
            let markdown = PathBuf::from("report.md");
            let actual = report_destinations(Some(json.clone()), Some(markdown.clone()), complete);
            assert_eq!(
                actual,
                if complete {
                    (Some(json), Some(markdown))
                } else {
                    (Some("report.partial.json".into()), Some("report.partial.md".into()))
                }
            );
            assert_eq!(report_destinations(None, None, complete), (None, None));
        }
    }

    #[test]
    fn gamma_round2_baseline_selection_respects_precedence_availability_and_policy() {
        let mut arguments = Arguments::parse_from([], None).unwrap();
        let json = Path::new("automatic.json");
        arguments.automatic_baseline = false;
        assert_eq!(
            select_baseline(&arguments, Some(json), |_| panic!("automatic baseline disabled")),
            None
        );
        arguments.automatic_baseline = true;
        assert_eq!(select_baseline(&arguments, Some(json), |_| true), Some((json, true)));
        assert_eq!(select_baseline(&arguments, Some(json), |_| false), None);
        assert_eq!(select_baseline(&arguments, None, |_| panic!("no output path")), None);
        arguments.baseline = Some("explicit.json".into());
        assert_eq!(
            select_baseline(&arguments, Some(json), |_| panic!("explicit baseline wins")),
            Some((Path::new("explicit.json"), false))
        );
        for is_file in [true, false] {
            for incomplete in [true, false] {
                assert_eq!(
                    baseline_available(
                        json,
                        |path| {
                            assert_eq!(path, json);
                            is_file
                        },
                        |path| {
                            assert!(is_file);
                            assert_eq!(path, json);
                            incomplete
                        }
                    ),
                    is_file && !incomplete
                );
            }
        }
        for automatic in [false, true] {
            let selected = read_selected_baseline(Some((json, automatic)), |path, policy| {
                assert_eq!(path, json);
                assert_eq!(policy, automatic);
                Ok(Some(report_fixture(17)))
            })
            .unwrap()
            .unwrap();
            assert_eq!(selected.0, json);
            assert_eq!(selected.1.entries.len(), 1);
        }
        assert!(
            read_selected_baseline(None, |_, _| panic!("no selected baseline"))
                .unwrap()
                .is_none()
        );
        assert!(read_selected_baseline(Some((json, true)), |_, _| Ok(None)).unwrap().is_none());
        assert!(
            matches!(read_selected_baseline(Some((json, false)), |_, _| Err(Error::Metadata("read failed".into()))), Err(Error::Metadata(message)) if message == "read failed")
        );
    }

    #[test]
    fn gamma_round2_comparable_baselines_apply_changes_and_incompatible_ones_warn() {
        struct Refuse;
        impl io::Write for Refuse {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("warning refused"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let baseline = report_fixture(100);
        let mut report = report_fixture(80);
        let mut warnings = Vec::new();
        apply_report_baseline(&mut report, Path::new("baseline.json"), &baseline, 5.0, &mut warnings).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(report.entries[0].status, crate::report::BenchmarkStatus::Improved);
        assert!(report.entries[0].prior.is_some());
        assert_eq!(report.entries[0].results["perf"].metrics["cycles"].change_percentage, Some(-20.0));
        let mut incompatible = report_fixture(80);
        incompatible.metadata.host_cpu = "different-cpu".into();
        apply_report_baseline(&mut incompatible, Path::new("baseline.json"), &baseline, 5.0, &mut warnings).unwrap();
        assert_eq!(incompatible.entries[0].status, crate::report::BenchmarkStatus::Uncompared);
        assert!(incompatible.entries[0].prior.is_none());
        assert_eq!(
            String::from_utf8(warnings).unwrap(),
            "metabench: skipping incompatible baseline baseline.json (target, compiler, CPU, profile, or flags differ)\n"
        );
        assert!(
            matches!(apply_report_baseline(&mut report_fixture(80), Path::new("baseline.json"), &baseline, -1.0, &mut Vec::new()), Err(Error::InvalidThreshold(value)) if value == "-1")
        );
        assert!(
            matches!(apply_report_baseline(&mut incompatible, Path::new("baseline.json"), &baseline, 5.0, &mut Refuse), Err(Error::ConsoleOutput(error)) if error.to_string() == "warning refused")
        );
    }

    use std::cell::Cell;
    use std::collections::VecDeque;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt as _;

    use super::*;

    #[test]
    fn gamma_round2_default_report_name_conflicts_are_detected_before_io() {
        let mut arguments = Arguments::parse_from([], None).unwrap();
        arguments.export_markdown = Some(PathBuf::from("report.json"));
        assert!(matches!(
            report_paths_with_root(&arguments, Path::new("")),
            Err(Error::ConflictingOutputOptions)
        ));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn gamma_round2_unsupported_collectors_return_errors_before_io() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        let arguments = Arguments::parse_from(["--perf".into()], None).unwrap();
        let error = run_perf(Path::new("unused"), &arguments, &artifacts).unwrap_err();
        assert!(matches!(
            error,
            Error::UnsupportedMode {
                mode: "perf",
                reason: "Linux perf is supported only on Linux"
            }
        ));
        let error = launch_perf_worker(Path::new("unused"), &[], &[], Path::new("unused"), Path::new("unused"), false, None).unwrap_err();
        assert!(matches!(
            error,
            Error::UnsupportedMode {
                mode: "perf",
                reason: "Linux perf is supported only on Linux"
            }
        ));
        let error = launch_worker(Mode::Gungraun, Path::new("unused"), &[], None, false, None).unwrap_err();
        assert!(matches!(
            error,
            Error::UnsupportedMode {
                mode: "gungraun",
                reason: "Gungraun is supported only on Linux"
            }
        ));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn gamma_round2_probe_modes_skip_non_probes_and_preserve_failure_policy() {
        let artifacts = ArtifactDirectory::gamma_round2_in_memory_directory();
        for policy in [FailureMode::FailFast, FailureMode::KeepGoing] {
            let mut arguments = Arguments::parse_from(["--perf".into()], None).unwrap();
            arguments.failure_mode = policy;
            let mut summary = EngineRunSummary {
                completed: Vec::new(),
                failures: Vec::new(),
            };
            let mut failures = Vec::new();
            let result = run_probe_modes(
                &[Mode::Criterion, Mode::Perf],
                Path::new("unused"),
                &arguments,
                &artifacts,
                &mut summary,
                &mut failures,
            );
            match policy {
                FailureMode::FailFast => {
                    assert!(matches!(result, Err(Error::UnsupportedMode { mode: "perf", .. })));
                    assert!(failures.is_empty());
                }
                FailureMode::KeepGoing => {
                    assert!(result.unwrap().is_empty());
                    assert_eq!(failures, ["perf is unavailable: Linux perf is supported only on Linux"]);
                }
            }
            assert!(summary.completed.is_empty());
            assert!(summary.failures.is_empty());
        }
    }

    #[test]
    fn gamma_round2_vtune_rejects_reserved_arguments_before_any_io() {
        let error = launch_vtune_worker(
            Path::new("unused"),
            &[],
            &["-result-dir".into()],
            Path::new("unused"),
            Path::new("unused"),
            Path::new("unused"),
            false,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, Error::UnsupportedVtuneArgument(value) if value == "-result-dir"));
    }

    #[test]
    fn gamma_round2_partial_path_without_extension_and_backend_presence() {
        assert_eq!(partial_report_path(Path::new("report")), PathBuf::from("report.partial"));
        let result = crate::report::NativeResult {
            identity: "case".into(),
            native_identity: "native".into(),
            source: "perf".into(),
            metrics: std::collections::BTreeMap::new(),
            raw_artifacts: vec!["raw.json".into()],
        };
        require_backend_results(&[result], Mode::Perf, Path::new("raw.json")).unwrap();
    }

    #[test]
    fn gamma_round2_selection_accepts_inline_option_values() {
        let target = serde_json::from_value::<ProbeTarget>(serde_json::json!({
            "identity": "case", "native_identity": "native", "group": 0, "benchmark": 0, "case": 0, "iteration": null
        }))
        .unwrap();
        let selected = select_native_probe_targets(
            vec![target],
            &["--color=always".into(), "--sample-size=12".into(), "--exact".into(), "case".into()],
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].identity, "case");
    }

    #[test]
    fn gamma_round2_empty_executable_path_has_an_actionable_target_error() {
        let error = effective_target_directory(Path::new(""), |_| None).unwrap_err();
        match error {
            Error::ArtifactFormat { path, message } => {
                assert_eq!(path, Path::new(""));
                assert_eq!(
                    message,
                    "cannot infer the Cargo target directory from this executable; set CARGO_TARGET_DIR for a relocated binary"
                );
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn gamma_survivor_engine_dispatch() {
        thread_local! { static CALLED: Cell<u8> = const { Cell::new(0) }; }
        let engines = EngineSet::both_with_unit(|| CALLED.set(1), || CALLED.set(2), Some("ns"), || Ok(()));
        for mode in Mode::ALL {
            CALLED.set(0);
            engines.run(mode);
            assert_eq!(CALLED.get(), if mode == Mode::Gungraun { 2 } else { 1 });
        }
    }

    #[test]
    fn gamma_survivor_parent_rejects_unselected_native_arguments() {
        let mut arguments = Arguments::parse_from(["--criterion".into(), "--vtune-arg".into(), "native".into()], None).unwrap();
        let engines = EngineSet::both_with_unit(|| {}, || {}, None, || Ok(()));
        assert!(matches!(
            run_parent(&mut arguments, engines, &[], "unused"),
            Err(Error::ArgumentsForUnselectedEngine("vtune"))
        ));
    }

    #[test]
    fn gamma_survivor_failure_aggregation_preserves_order() {
        finish_failures(vec![]).unwrap();
        assert!(matches!(finish_failures(vec!["first".into(), "second".into()]),
            Err(Error::MultipleModeFailures(messages)) if messages == ["first", "second"]));
        let primary = Error::MultipleModeFailures(vec!["worker".into()]);
        let secondary = Error::MultipleModeFailures(vec!["cleanup".into()]);
        assert!(matches!(combine_results::<()>(Err(primary), Err(secondary)),
            Err(Error::MultipleModeFailures(messages)) if messages == ["worker", "cleanup"]));
        let combined = multiple_failures([
            Error::MultipleModeFailures(vec!["a".into(), "b".into()]),
            Error::MultipleModeFailures(vec!["c".into()]),
        ]);
        assert!(matches!(combined, Error::MultipleModeFailures(messages) if messages == ["a", "b", "c"]));
        let summary = orchestrate_modes(&Mode::ALL, FailureMode::KeepGoing, |mode| {
            Err(Error::MultipleModeFailures(vec![mode.as_str().into()]))
        })
        .unwrap();
        assert_eq!(summary.failures, ["criterion", "gungraun", "perf", "vtune", "allocations"]);
    }

    #[test]
    fn gamma_survivor_probe_selection_mixed_options() {
        let targets = || {
            ["case", "case/child", "other"]
                .into_iter()
                .enumerate()
                .map(|(case, identity)| {
                    serde_json::from_value::<ProbeTarget>(serde_json::json!({
                        "identity": identity, "native_identity": identity, "group": 0, "benchmark": 0, "case": case, "iteration": null
                    }))
                    .unwrap()
                })
                .collect::<Vec<_>>()
        };
        for arguments in [
            vec!["--sample-size", "12", "--color", "always", "--exact", "case"],
            vec!["--sample-size=12", "--color=always", "--exact", "case"],
            vec!["--save-baseline", "discard", "--noplot", "--test", "--exact", "case"],
            vec!["--verbose", "--quiet", "--debug", "--discard-baseline", "-v", "--exact", "case"],
        ] {
            let arguments = arguments.into_iter().map(OsString::from).collect::<Vec<_>>();
            let selected = select_native_probe_targets(targets(), &arguments).unwrap();
            assert_eq!(selected.iter().map(|target| target.identity.as_str()).collect::<Vec<_>>(), ["case"]);
        }
        assert!(matches!(probe_selection_arguments(&["--sample-size".into()]),
            Err(Error::Probe(message)) if message == "missing a value for a forwarded Criterion option"));
        assert!(matches!(select_native_probe_targets(targets(), &["case".into(), "other".into()]),
            Err(Error::Probe(message)) if message == "only one case selection filter is supported"));
        assert_eq!(
            probe_selection_arguments(&["--save-baseline".into(), "discard".into(), "case".into()]).unwrap(),
            [OsString::from("case")]
        );
    }

    #[test]
    fn gamma_survivor_reserved_collector_arguments() {
        for option in [
            "--",
            "-o",
            "-r",
            "-I",
            "--output",
            "--json-output",
            "--control",
            "--delay",
            "--repeat",
            "--interval-print",
        ] {
            for value in [option.to_owned(), format!("{option}=x")] {
                assert!(validate_perf_arguments(&[value.into()]).is_err());
            }
        }
        for option in ["-oX", "-IX", "-rX"] {
            assert!(validate_perf_arguments(&[option.into()]).is_err());
        }
        validate_perf_arguments(&["-e".into(), "cycles".into()]).unwrap();
        validate_vtune_arguments(&["-knob".into(), "event-config=CPU_CLK_UNHALTED.THREAD".into()]).unwrap();
    }

    #[test]
    fn gamma_survivor_runner_writer_errors() {
        struct BrokenWriter;
        impl io::Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "scripted"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let target = serde_json::from_value::<ProbeTarget>(serde_json::json!({
            "identity": "case", "native_identity": "native", "group": 0, "benchmark": 0, "case": 0, "iteration": null
        }))
        .unwrap();
        assert!(
            matches!(write_probe_targets(&mut BrokenWriter, &[target]), Err(Error::ConsoleOutput(source))
            if source.kind() == io::ErrorKind::BrokenPipe)
        );
        assert!(
            matches!(write_progress(&mut BrokenWriter, Mode::Perf), Err(Error::ConsoleOutput(source))
            if source.kind() == io::ErrorKind::BrokenPipe)
        );
        ensure_success(Mode::Criterion, exit_status(0)).unwrap();
    }

    #[test]
    fn gamma_survivor_scripted_process_deadlines_and_errors() {
        let clock = FakeClock::new();
        let mut child = FakeProcess::new([Err(io::Error::new(io::ErrorKind::BrokenPipe, "poll"))]);
        assert!(
            matches!(wait_for_process(Mode::Perf, &mut child, Some(Duration::from_secs(1)), &clock),
            Err(Error::Spawn { source, .. }) if source.kind() == io::ErrorKind::BrokenPipe)
        );
        let mut child = FakeProcess::new([Err(io::Error::other("wait"))]);
        child.kill = Some(Err(io::Error::other("kill")));
        let error = terminate_timed_out_worker(Mode::Perf, &mut child, Duration::ZERO, &FakeClock::new()).unwrap_err();
        assert!(matches!(error, Error::MultipleModeFailures(messages) if messages == [
            "failed to terminate the perf worker: kill", "failed to terminate the perf worker: wait",
        ]));
        let mut child = FakeProcess::new([Err(io::Error::other("wait"))]);
        let error = terminate_timed_out_worker(Mode::Perf, &mut child, Duration::from_secs(2), &FakeClock::new()).unwrap_err();
        assert!(matches!(error, Error::MultipleModeFailures(messages) if messages == [
            "perf worker exceeded the 2s timeout", "failed to terminate the perf worker: wait",
        ]));
        let clock = FakeClock::new();
        let mut child = FakeProcess::new([]);
        assert!(matches!(kill_and_reap_worker(Mode::Perf, &mut child, &clock),
            Err(Error::WorkerTermination { source, .. })
                if source.to_string() == "worker did not exit before the termination reap deadline"));
        assert_eq!(clock.elapsed.get(), TERMINATION_REAP_GRACE);
    }

    #[test]
    fn gamma_survivor_captured_process_sleep_and_error_drain() {
        for had_output in [false, true] {
            let clock = FakeClock::new();
            let mut child = FakeProcess::new([Ok(None), Ok(Some(exit_status(0)))]);
            let mut drains = VecDeque::from([(false, had_output), (true, false)]);
            let status = wait_for_captured_process(
                Mode::Perf,
                &mut child,
                None,
                Path::new("log"),
                || Ok(drains.pop_front().unwrap()),
                &clock,
            )
            .unwrap();
            assert!(status.success());
            assert_eq!(
                clock.elapsed.get(),
                if had_output {
                    Duration::from_millis(1)
                } else {
                    WORKER_POLL_INTERVAL
                }
            );
        }
        let clock = FakeClock::new();
        let mut child = FakeProcess::new([Err(io::Error::other("poll")), Ok(None), Ok(Some(exit_status(0)))]);
        let mut drains = VecDeque::from([(false, false), (true, false)]);
        assert!(matches!(
            wait_for_captured_process(
                Mode::Perf,
                &mut child,
                None,
                Path::new("log"),
                || Ok(drains.pop_front().unwrap()),
                &clock
            ),
            Err(Error::Spawn { .. })
        ));
        assert_eq!(child.kill_calls, 1);
    }

    #[test]
    fn gamma_survivor_captured_drain_grace_starts_at_completion() {
        for failed_poll in [false, true] {
            let statuses = if failed_poll {
                vec![Err(io::Error::other("poll")), Ok(None), Ok(Some(exit_status(0)))]
            } else {
                vec![Ok(None), Ok(Some(exit_status(0))), Ok(Some(exit_status(0)))]
            };
            let mut child = FakeProcess::new(statuses);
            let clock = FakeClock::new();
            let result = wait_for_captured_process(Mode::Perf, &mut child, None, Path::new("log"), || Ok((false, false)), &clock);
            result.unwrap_err();
            assert_eq!(
                clock.elapsed.get(),
                if failed_poll {
                    TERMINATION_REAP_GRACE
                } else {
                    TERMINATION_REAP_GRACE + WORKER_POLL_INTERVAL
                }
            );
        }
    }

    #[test]
    fn gamma_survivor_termination_reap_has_a_bounded_grace() {
        struct BoundedClock(FakeClock);
        impl Clock for BoundedClock {
            type Mark = Duration;
            fn mark(&self) -> Duration {
                self.0.mark()
            }
            fn elapsed(&self, mark: &Duration) -> Duration {
                self.0.elapsed(mark)
            }
            fn sleep(&self, duration: Duration) {
                self.0.sleep(duration);
                assert!(
                    self.0.elapsed.get() <= Duration::from_millis(1_250),
                    "scripted reap exceeded its grace"
                );
            }
        }
        let clock = BoundedClock(FakeClock::new());
        let mut child = FakeProcess::new([]);
        assert!(
            matches!(terminate_timed_out_worker(Mode::Perf, &mut child, Duration::from_secs(2), &clock),
            Err(Error::WorkerTimedOut { timeout, .. }) if timeout == Duration::from_secs(2))
        );
        assert_eq!(clock.0.elapsed.get(), TERMINATION_REAP_GRACE);
    }

    #[test]
    fn artifacts_are_created_for_measurements_and_probe_listing_only() {
        let directory = tempfile::tempdir_in(".").unwrap();
        for (list, probes, expected) in [(false, false, true), (false, true, true), (true, false, false), (true, true, true)] {
            let root = directory.path().join(format!("{list}-{probes}"));
            let artifacts = create_run_artifacts(&root, list, probes).unwrap();
            assert_eq!(artifacts.is_some(), expected);
            assert_eq!(root.exists(), expected);
            if let Some(artifacts) = artifacts {
                assert!(artifacts.path().is_dir());
            }
        }
    }

    #[test]
    fn collector_environment_is_removed_without_removing_worker_or_native_settings() {
        let collectors = [
            allocation::ARTIFACT_ENV,
            allocation::IDENTITY_ENV,
            perf::CONTROL_ENV,
            perf::ACK_ENV,
            vtune::RESULT_DIR_ENV,
            crate::probe::DISCOVERY_ENV,
        ];
        let mut command = Command::new("unused");
        for name in collectors {
            command.env(name, "stale collector configuration");
        }
        command.env(WORKER_MODE_ENV, "criterion").env("CRITERION_HOME", "native");
        clear_collector_environment(&mut command);
        let settings = command.get_envs().collect::<Vec<_>>();
        for name in collectors {
            assert!(settings.contains(&(OsStr::new(name), None)));
        }
        assert!(settings.contains(&(OsStr::new(WORKER_MODE_ENV), Some(OsStr::new("criterion")))));
        assert!(settings.contains(&(OsStr::new("CRITERION_HOME"), Some(OsStr::new("native")))));
    }

    #[test]
    fn native_discovery_receiver_enforces_case_budget_and_listing_encoding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("discovery.json");
        let case = serde_json::json!({
            "identity": "group/escape\u{1b}[31m\r\nforged",
            "native_identity": "native",
            "group": 0, "benchmark": 0, "case": 0, "iteration": null
        });
        for (count, accepted) in [(10_000, true), (10_001, false)] {
            artifact::write_json(&path, &vec![case.clone(); count]).unwrap();
            let result = read_probe_targets(&path);
            assert_eq!(result.is_ok(), accepted);
            if let Ok(targets) = result {
                let mut output = Vec::new();
                write_probe_targets(&mut output, &targets[..1]).unwrap();
                assert_eq!(
                    String::from_utf8(output).unwrap(),
                    "group/escape\\u{1b}[31m\\r\\nforged: benchmark\n"
                );
                assert_eq!(targets[0].identity, "group/escape\u{1b}[31m\r\nforged");
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap()[0],
                    case
                );
            } else {
                assert!(result.unwrap_err().to_string().contains("10000-case limit"));
            }
        }
    }

    #[test]
    fn captured_logs_are_exclusive_and_excerpt_encoding_preserves_raw_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.log");
        let mut command = Command::new("unused");
        let mut log = capture_worker_output(&mut command, &path, false).unwrap().unwrap();
        let raw = b"failure\x1b[31m\r\nforged";
        log.file.write_all(raw).unwrap();
        assert!(matches!(
            capture_worker_output(&mut Command::new("unused"), &path, false),
            Err(Error::ArtifactIo { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists
        ));
        let mut output = Vec::new();
        write_worker_log(&mut output, Mode::Allocations, &path, log.file.native_file(), false).unwrap();
        assert!(String::from_utf8(output).unwrap().ends_with("failure\\u{1b}[31m\\r\\nforged\n"));
        assert_eq!(fs::read(&path).unwrap(), raw);
        #[cfg(unix)]
        {
            fs::rename(&path, directory.path().join("retained.log")).unwrap();
            fs::write(&path, b"substituted log").unwrap();
            let mut output = Vec::new();
            write_worker_log(&mut output, Mode::Allocations, &path, log.file.native_file(), false).unwrap();
            assert!(!String::from_utf8(output).unwrap().contains("substituted"));
        }
    }

    #[test]
    fn captured_logs_never_truncate_preexisting_link_targets() {
        let directory = tempfile::tempdir().unwrap();
        let sentinel = directory.path().join("sentinel");
        let path = directory.path().join("worker.log");
        fs::write(&sentinel, b"preserve me").unwrap();
        fs::hard_link(&sentinel, &path).unwrap();
        assert!(matches!(
            capture_worker_output(&mut Command::new("unused"), &path, false),
            Err(Error::ArtifactIo { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve me");
        #[cfg(unix)]
        {
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&sentinel, &path).unwrap();
            assert!(matches!(
                capture_worker_output(&mut Command::new("unused"), &path, false),
                Err(Error::ArtifactIo { source, .. }) if source.kind() == io::ErrorKind::AlreadyExists
            ));
            assert_eq!(fs::read(&sentinel).unwrap(), b"preserve me");
        }
    }

    #[test]
    fn captured_output_fixture() {
        let Some(scenario) = env::var_os("METABENCH_LOG_TEST_SCENARIO") else {
            return;
        };
        let mut stdout = io::stdout().lock();
        let mut stderr = io::stderr().lock();
        let noisy = scenario != "success" && scenario != "failure";
        let chunks = if noisy { 512 } else { 1 };
        let bytes = [b'x'; 8_192];
        let started = Instant::now();
        loop {
            for _ in 0..chunks {
                stdout.write_all(&bytes).unwrap();
                stderr.write_all(&bytes).unwrap();
            }
            stdout.write_all(b"\ncritical stdout diagnostic\n").unwrap();
            stdout.flush().unwrap();
            stderr.write_all(b"critical stderr diagnostic\xff\x1b[31m\n").unwrap();
            stderr.flush().unwrap();
            // A disabled parent deadline must fail its assertion without leaving this fixture running forever.
            if scenario != "timeout" || started.elapsed() >= Duration::from_secs(2) {
                std::process::exit(i32::from(scenario != "success"));
            }
        }
    }

    fn captured_fixture_command(scenario: &str) -> Command {
        let mut command = Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", "runner::tests::captured_output_fixture", "--nocapture"])
            .env("METABENCH_LOG_TEST_SCENARIO", scenario);
        command
    }

    #[test]
    fn captured_logs_keep_bounded_raw_tail_for_noisy_failures_and_normal_workers() {
        for scenario in ["success", "failure", "noisy-failure"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("worker.log");
            let mut command = captured_fixture_command(scenario);
            let mut log = capture_worker_output(&mut command, &path, false).unwrap().unwrap();
            let result = wait_for_captured_worker(Mode::Criterion, &mut command, Some(Duration::from_secs(10)), &path, &mut log);
            assert_eq!(result.unwrap().success(), scenario == "success");
            assert!(log.file.native_file().metadata().unwrap().len() <= crate::worker_log::LOG_LIMIT as u64);
            log.persist().unwrap();
            let bytes = fs::read(&path).unwrap();
            assert!(bytes.len() <= crate::worker_log::LOG_LIMIT);
            assert_eq!(log.truncated(), scenario == "noisy-failure");
            assert!(
                bytes
                    .windows(b"critical stdout diagnostic".len())
                    .any(|bytes| bytes == b"critical stdout diagnostic")
            );
            assert!(bytes.ends_with(b"critical stderr diagnostic\xff\x1b[31m\n"));
            if log.truncated() {
                assert_eq!(bytes.len(), crate::worker_log::LOG_LIMIT);
                assert!(bytes.starts_with(b"[metabench: output truncated;"));
            }
            let mut output = Vec::new();
            write_worker_excerpt(&mut output, Mode::Criterion, &path, &log.excerpt(), log.truncated()).unwrap();
            let excerpt = String::from_utf8(output).unwrap();
            assert!(excerpt.contains("critical stdout diagnostic"));
            assert!(excerpt.contains("critical stderr diagnostic"));
            assert!(excerpt.contains("\\u{1b}[31m\\n"));
            assert_eq!(excerpt.contains("output truncated"), scenario == "noisy-failure");
        }
    }

    #[test]
    fn captured_logs_bound_sustained_output_and_timeout_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.log");
        let mut command = captured_fixture_command("timeout");
        let log = capture_worker_output(&mut command, &path, false).unwrap();
        let started = Instant::now();
        let error = wait_for_logged_worker_with_output(
            Mode::Criterion,
            &mut command,
            Some(Duration::from_millis(150)),
            &path,
            log,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timeout"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(fs::metadata(&path).unwrap().len() <= crate::worker_log::LOG_LIMIT as u64);
    }

    #[cfg(unix)]
    #[test]
    fn captured_logs_bound_inherited_writer_lifetime_after_worker_exit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.log");
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & printf 'parent diagnostic\\n'; exit 0"]);
        let log = capture_worker_output(&mut command, &path, false).unwrap();
        let started = Instant::now();
        let error = wait_for_logged_worker_with_output(Mode::Criterion, &mut command, None, &path, log, &mut Vec::new()).unwrap_err();
        assert!(error.to_string().contains("pipe remained open"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(fs::read(&path).unwrap(), b"parent diagnostic\n");
    }

    #[test]
    fn captured_logs_publish_failures_through_the_original_open_handle() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.log");
        let mut command = captured_fixture_command("failure");
        let log = capture_worker_output(&mut command, &path, false).unwrap();
        #[cfg(unix)]
        {
            fs::rename(&path, directory.path().join("retained.log")).unwrap();
            fs::write(&path, b"substituted log").unwrap();
        }
        let mut output = Vec::new();
        let status = wait_for_logged_worker_with_output(
            Mode::Criterion,
            &mut command,
            Some(Duration::from_secs(10)),
            &path,
            log,
            &mut output,
        )
        .unwrap();
        assert!(!status.success());
        assert!(String::from_utf8(output).unwrap().contains("critical stderr diagnostic"));
        let retained = if cfg!(unix) {
            directory.path().join("retained.log")
        } else {
            path.clone()
        };
        assert!(fs::read(retained).unwrap().ends_with(b"critical stderr diagnostic\xff\x1b[31m\n"));
        #[cfg(unix)]
        assert_eq!(fs::read(&path).unwrap(), b"substituted log");
    }

    #[test]
    fn captured_logs_keep_diagnostics_when_file_publication_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("readonly.log");
        fs::write(&path, b"preserve me").unwrap();
        let mut command = captured_fixture_command("failure");
        let log = WorkerLog::new(fs::File::open(&path).unwrap(), &mut command).unwrap();
        let mut output = Vec::new();
        let error = wait_for_logged_worker_with_output(
            Mode::Criterion,
            &mut command,
            Some(Duration::from_secs(10)),
            &path,
            Some(log),
            &mut output,
        )
        .unwrap_err();
        assert!(error.to_string().contains("failed to access artifact"));
        assert!(error.to_string().contains("exited with status 1"));
        assert_eq!(fs::read(&path).unwrap(), b"preserve me");
        let diagnostics = String::from_utf8(output).unwrap();
        assert!(diagnostics.contains("critical stdout diagnostic"));
        assert!(diagnostics.contains("critical stderr diagnostic"));
    }

    #[test]
    fn captured_logs_show_output_does_not_create_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker.log");
        let mut command = Command::new("unused");
        assert!(capture_worker_output(&mut command, &path, true).unwrap().is_none());
        assert!(!path.exists());
    }
    #[test]
    fn empty_native_backend_cannot_promote_nonempty_probe_results() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().join("report");
        let json = base.with_extension("json");
        let markdown = base.with_extension("md");
        fs::write(&json, b"json sentinel").unwrap();
        fs::write(&markdown, b"markdown sentinel").unwrap();
        let arguments = Arguments::parse_from(
            [
                "--criterion".into(),
                "--allocations".into(),
                "--output".into(),
                base.into_os_string(),
            ],
            None,
        )
        .unwrap();
        for mode in [Mode::Criterion, Mode::Gungraun] {
            let artifacts = ArtifactDirectory::create(&directory.path().join("artifacts")).unwrap();
            fs::create_dir_all(artifacts.criterion_path()).unwrap();
            fs::create_dir_all(artifacts.gungraun_path()).unwrap();
            let probe = crate::report::NativeResult {
                identity: "native/work".to_owned(),
                native_identity: "native/work".to_owned(),
                source: "alloc_tracker".to_owned(),
                metrics: [(
                    "Allocations".to_owned(),
                    crate::report::Metric::integer("alloc_tracker", "Allocations", 1, crate::report::MetricDirection::LowerIsBetter),
                )]
                .into(),
                raw_artifacts: Vec::new(),
            };
            let error = build_report(
                &artifacts,
                &[mode, Mode::Allocations],
                &[],
                &arguments,
                "target",
                None,
                vec![probe],
                true,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(mode.as_str()));
            assert!(error.contains("no benchmark measurements"));
            assert_eq!(fs::read(&json).unwrap(), b"json sentinel");
            assert_eq!(fs::read(&markdown).unwrap(), b"markdown sentinel");
        }
    }

    #[test]
    fn native_probe_selection_validates_and_applies_filters() {
        let targets = || {
            serde_json::from_value(serde_json::json!([
                {"identity":"group/work/small","native_identity":"native/small","group":0,"benchmark":0,"case":0,"iteration":null},
                {"identity":"group/work/large","native_identity":"native/large","group":0,"benchmark":0,"case":1,"iteration":null}
            ]))
            .unwrap()
        };
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let selected = select_native_probe_targets(targets(), &args(&["small$", "--sample-size", "10"])).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].identity, "group/work/small");
        assert!(
            select_native_probe_targets(targets(), &args(&["small", "--exact"]))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            select_native_probe_targets(targets(), &args(&["group/work/large", "--exact"]))
                .unwrap()
                .len(),
            1
        );
        for invalid in [
            &["["][..],
            &["small", "large"],
            &["--unknown"],
            &["--sample-size"],
            &["--profile-time"],
        ] {
            assert!(select_native_probe_targets(targets(), &args(invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn probe_selection_preserves_option_values_but_discards_benchmark_mode() {
        let configured = [OsString::from("--sample-size"), OsString::from("20"), OsString::from("--bench")];

        assert_eq!(probe_selection_arguments(&configured).unwrap(), ["--sample-size", "20"]);
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: i32) -> ExitStatus {
        ExitStatus::from_raw(u32::try_from(code).unwrap())
    }

    struct FakeClock {
        elapsed: Cell<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                elapsed: Cell::new(Duration::ZERO),
            }
        }
    }

    impl Clock for FakeClock {
        type Mark = Duration;

        fn mark(&self) -> Self::Mark {
            self.elapsed.get()
        }

        fn elapsed(&self, mark: &Self::Mark) -> Duration {
            self.elapsed.get().saturating_sub(*mark)
        }

        fn sleep(&self, duration: Duration) {
            self.elapsed.set(self.elapsed.get() + duration);
        }
    }

    struct FakeProcess {
        try_wait: VecDeque<io::Result<Option<ExitStatus>>>,
        kill: Option<io::Result<()>>,
        wait: Option<io::Result<ExitStatus>>,
        kill_calls: usize,
        wait_calls: usize,
    }

    impl FakeProcess {
        fn new(try_wait: impl IntoIterator<Item = io::Result<Option<ExitStatus>>>) -> Self {
            Self {
                try_wait: try_wait.into_iter().collect(),
                kill: Some(Ok(())),
                wait: Some(Ok(exit_status(0))),
                kill_calls: 0,
                wait_calls: 0,
            }
        }
    }

    impl WorkerProcess for FakeProcess {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.try_wait.pop_front().unwrap_or(Ok(None))
        }

        fn kill(&mut self) -> io::Result<()> {
            self.kill_calls += 1;
            self.kill.take().unwrap_or_else(|| Err(io::Error::other("kill called twice")))
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.wait_calls += 1;
            self.wait.take().unwrap_or_else(|| Err(io::Error::other("wait called twice")))
        }
    }

    #[test]
    fn captured_process_waits_for_both_exit_and_output_eof() {
        for (statuses, drains) in [
            (vec![Ok(None), Ok(Some(exit_status(3)))], vec![(true, false), (true, false)]),
            (vec![Ok(Some(exit_status(3)))], vec![(false, true), (true, false)]),
        ] {
            let mut process = FakeProcess::new(statuses);
            let clock = FakeClock::new();
            let mut drains = VecDeque::from(drains);
            let status = wait_for_captured_process(
                Mode::Criterion,
                &mut process,
                None,
                Path::new("worker.log"),
                || Ok(drains.pop_front().unwrap()),
                &clock,
            )
            .unwrap();
            assert_eq!(status.code(), Some(3));
            assert!(drains.is_empty());
            assert_eq!(process.kill_calls, 0);
        }
    }

    #[test]
    fn captured_process_drains_until_the_exact_post_exit_deadline() {
        let mut process = FakeProcess::new([Ok(Some(exit_status(0))), Ok(Some(exit_status(0)))]);
        let clock = FakeClock::new();
        let mut polls = 0;
        let error = wait_for_captured_process(
            Mode::Criterion,
            &mut process,
            None,
            Path::new("worker.log"),
            || {
                polls += 1;
                clock.elapsed.set(match polls {
                    1 => Duration::ZERO,
                    2 => TERMINATION_REAP_GRACE.checked_sub(Duration::from_nanos(1)).unwrap(),
                    3 => TERMINATION_REAP_GRACE,
                    _ => panic!("draining must stop at the deadline"),
                });
                Ok((false, false))
            },
            &clock,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pipe remained open"));
        assert_eq!(polls, 3);
        assert_eq!(process.kill_calls, 1);
        assert_eq!(clock.elapsed.get(), TERMINATION_REAP_GRACE);
    }

    #[test]
    fn captured_process_read_errors_kill_and_reap_and_preserve_cleanup_errors() {
        let mut process = FakeProcess::new([Err(io::Error::other("reap failed"))]);
        let error = wait_for_captured_process(
            Mode::Criterion,
            &mut process,
            None,
            Path::new("worker.log"),
            || Err(io::Error::other("read failed")),
            &FakeClock::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("read failed"));
        assert!(error.contains("reap failed"));
        assert_eq!(process.kill_calls, 1);
        assert_eq!(process.wait_calls, 0);
    }

    #[test]
    fn captured_process_timeout_drain_deadline_includes_the_reap_grace() {
        let polls = TERMINATION_REAP_GRACE.as_nanos() / WORKER_POLL_INTERVAL.as_nanos();
        let mut process = FakeProcess::new((0..=polls + 1).map(|_| Ok(None)));
        let clock = FakeClock::new();
        let mut drains = 0;
        let error = wait_for_captured_process(
            Mode::Criterion,
            &mut process,
            Some(Duration::ZERO),
            Path::new("worker.log"),
            || {
                drains += 1;
                assert_eq!(drains, 1, "reaping has already exhausted the output drain deadline");
                Ok((false, false))
            },
            &clock,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("worker exceeded"));
        assert!(error.contains("pipe remained open"));
        assert_eq!(clock.elapsed.get(), TERMINATION_REAP_GRACE);
        assert_eq!(process.kill_calls, 2);
        assert_eq!(process.wait_calls, 0);
    }

    #[test]
    fn captured_process_deadline_preempts_a_finite_successful_exit() {
        let deadline = WORKER_POLL_INTERVAL * 2;
        for timeout in [Some(deadline), None] {
            let mut process = FakeProcess::new([Ok(None), Ok(None), Ok(None), Ok(Some(exit_status(0)))]);
            let clock = FakeClock::new();
            let mut polls = 0;
            let result = wait_for_captured_process(
                Mode::Criterion,
                &mut process,
                timeout,
                Path::new("worker.log"),
                || {
                    polls += 1;
                    assert!(polls <= 4, "the scripted worker exits successfully within four polls");
                    Ok((polls >= 3, false))
                },
                &clock,
            );
            if timeout.is_some() {
                assert!(matches!(
                    result,
                    Err(Error::WorkerTimedOut { mode: "criterion", timeout }) if timeout == deadline
                ));
                assert_eq!(polls, 3);
                assert_eq!(clock.elapsed.get(), deadline);
                assert_eq!(process.kill_calls, 1);
            } else {
                assert!(result.unwrap().success());
                assert_eq!(polls, 4);
                assert_eq!(clock.elapsed.get(), WORKER_POLL_INTERVAL * 3);
                assert_eq!(process.kill_calls, 0);
            }
            assert!(process.try_wait.is_empty());
            assert_eq!(process.wait_calls, 0);
        }
    }

    #[test]
    fn captured_process_timeout_is_enforced_while_the_pipe_is_pending() {
        let mut process = FakeProcess::new([Ok(None), Ok(None), Ok(Some(exit_status(9)))]);
        let clock = FakeClock::new();
        let mut drains = VecDeque::from([(false, false), (false, false), (true, false)]);
        let error = wait_for_captured_process(
            Mode::Criterion,
            &mut process,
            Some(WORKER_POLL_INTERVAL),
            Path::new("worker.log"),
            || Ok(drains.pop_front().unwrap()),
            &clock,
        )
        .unwrap_err();
        assert!(matches!(error, Error::WorkerTimedOut { .. }));
        assert!(drains.is_empty());
        assert_eq!(process.kill_calls, 1);
        assert_eq!(clock.elapsed.get(), WORKER_POLL_INTERVAL * 2);
    }

    #[test]
    fn kill_and_reap_polls_pending_exit_but_stops_at_the_exact_grace_deadline() {
        let mut process = FakeProcess::new([Ok(None), Ok(Some(exit_status(9)))]);
        let clock = FakeClock::new();
        kill_and_reap_worker(Mode::Criterion, &mut process, &clock).unwrap();
        assert_eq!(process.kill_calls, 1);
        assert_eq!(process.wait_calls, 0);
        assert_eq!(clock.elapsed.get(), WORKER_POLL_INTERVAL);
        assert!(process.try_wait.is_empty());

        let polls = TERMINATION_REAP_GRACE.as_nanos() / WORKER_POLL_INTERVAL.as_nanos();
        let mut statuses = (0..=polls).map(|_| Ok(None)).collect::<Vec<_>>();
        // A late exit also makes an accidentally unbounded grace finish deterministically.
        statuses.push(Ok(Some(exit_status(9))));
        let mut process = FakeProcess::new(statuses);
        let clock = FakeClock::new();
        let error = kill_and_reap_worker(Mode::Criterion, &mut process, &clock).unwrap_err();
        assert!(matches!(error, Error::WorkerTermination { source, .. } if source.kind() == io::ErrorKind::TimedOut));
        assert_eq!(clock.elapsed.get(), TERMINATION_REAP_GRACE);
        assert_eq!(process.kill_calls, 1);
        assert_eq!(process.wait_calls, 0);
        assert_eq!(process.try_wait.len(), 1);
    }

    #[test]
    fn kill_and_reap_propagates_failed_kill_without_polling() {
        let mut process = FakeProcess::new([Ok(Some(exit_status(0)))]);
        process.kill = Some(Err(io::Error::other("kill denied")));
        let clock = FakeClock::new();
        let error = kill_and_reap_worker(Mode::Criterion, &mut process, &clock).unwrap_err();
        assert!(error.to_string().contains("kill denied"));
        assert_eq!(process.kill_calls, 1);
        assert_eq!(process.try_wait.len(), 1);
        assert_eq!(clock.elapsed.get(), Duration::ZERO);
    }

    struct FailingOutput;

    impl io::Write for FailingOutput {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("output unavailable"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn incompatible_automatic_baselines_are_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        fs::write(
            &path,
            r#"{
                "schema_version": 7,
                "metadata": {
                    "timestamp_utc": "",
                    "target_triple": "",
                    "rustc_version": "",
                    "host_cpu": "",
                    "profile": "",
                    "compiler_flags": null,
                    "measured_backends": [],
                    "artifact_root": ""
                },
                "entries": []
            }"#,
        )
        .unwrap();

        assert!(read_baseline(&path, true).unwrap().is_none());
        assert!(matches!(
            read_baseline(&path, false),
            Err(Error::UnsupportedReportSchema { found: 7, expected: 9, .. })
        ));
    }

    #[test]
    fn baseline_warnings_encode_hostile_paths_and_schema_errors() {
        let mut output = Vec::new();
        let hostile = Path::new("baseline\x1b[31m\r\nforged\u{202e}.json");
        write_baseline_warning(
            &mut output,
            &format!(
                "skipping incompatible baseline {} (target, compiler, CPU, profile, or flags differ)",
                hostile.display()
            ),
        )
        .unwrap();
        let warning = String::from_utf8(output).unwrap();
        assert!(warning.contains("baseline\\u{1b}[31m\\r\\nforged\\u{202e}.json"));
        assert_eq!(warning.lines().count(), 1);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline\u{202e}.json");
        let report = serde_json::json!({
            "schema_version": 7,
            "metadata": {
                "timestamp_utc": "", "target_triple": "", "rustc_version": "",
                "host_cpu": "", "profile": "", "compiler_flags": null,
                "measured_backends": [], "artifact_root": ""
            },
            "entries": []
        });
        fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
        let mut output = Vec::new();
        assert!(read_baseline_with_output(&path, true, &mut output).unwrap().is_none());
        let warning = String::from_utf8(output).unwrap();
        assert!(warning.contains("baseline\\u{202e}.json"));
        assert!(warning.contains("unsupported report schema"));
        assert!(!warning.contains('\u{202e}'));
    }

    #[test]
    fn perf_arguments_cannot_override_measurement_protocol() {
        for argument in [
            "--",
            "-o",
            "-oresult.json",
            "--output=result.json",
            "--json-output",
            "--control=fifo:a,b",
            "--delay=1",
            "-r5",
            "--repeat=5",
            "-I100",
            "--interval-print=100",
        ] {
            assert!(matches!(
                validate_perf_arguments(&[argument.into()]),
                Err(Error::UnsupportedPerfArgument(_))
            ));
        }
        validate_perf_arguments(&["--event=cpu-clock".into(), "--no-scale".into()]).unwrap();
    }

    #[test]
    fn vtune_arguments_cannot_override_measurement_protocol() {
        for argument in [
            "--",
            "-r",
            "-r=result",
            "-collect",
            "-c",
            "-c=pause",
            "-collect-with",
            "-result-dir",
            "-result-dir=result",
            "-start-paused",
            "--start-paused",
            "-report",
            "-R",
            "-R=hw-events",
            "-report-output",
            "-format",
            "-command",
            "-C",
            "-C=pause",
        ] {
            assert!(matches!(
                validate_vtune_arguments(&[argument.into()]),
                Err(Error::UnsupportedVtuneArgument(_))
            ));
        }
        validate_vtune_arguments(&["-knob".into(), "event-config=INST_RETIRED.ANY".into()]).unwrap();
    }

    #[test]
    fn timeout_kills_reaps_and_reports_timeout() {
        let mut process = FakeProcess::new([Ok(None), Ok(None), Ok(Some(exit_status(9)))]);
        let clock = FakeClock::new();

        let error = wait_for_process(Mode::Criterion, &mut process, Some(Duration::from_millis(25)), &clock).unwrap_err();

        assert!(matches!(error, Error::WorkerTimedOut { .. }));
        assert_eq!(process.kill_calls, 1);
        assert_eq!(process.wait_calls, 0);
        assert_eq!(clock.elapsed.get(), Duration::from_millis(25));
    }

    #[test]
    fn worker_exit_before_deadline_is_returned_without_termination() {
        let mut process = FakeProcess::new([Ok(Some(exit_status(3)))]);
        let clock = FakeClock::new();

        let status = wait_for_process(Mode::Criterion, &mut process, Some(Duration::from_secs(1)), &clock).unwrap();

        assert_eq!(status.code(), Some(3));
        assert_eq!(process.kill_calls, 0);
        assert_eq!(process.wait_calls, 0);
    }

    #[test]
    fn timeout_distinguishes_exit_race_after_failed_kill() {
        let mut process = FakeProcess::new([Ok(None), Ok(None), Ok(Some(exit_status(0)))]);
        process.kill = Some(Err(io::Error::new(io::ErrorKind::InvalidInput, "already exited")));
        let clock = FakeClock::new();

        let status = wait_for_process(Mode::Gungraun, &mut process, Some(Duration::from_millis(25)), &clock).unwrap();

        assert!(status.success());
        assert_eq!(process.kill_calls, 1);
        assert_eq!(process.wait_calls, 0);
    }

    #[test]
    fn failed_termination_is_bounded_without_blocking_wait() {
        let mut process = FakeProcess::new([Ok(None), Ok(None), Ok(None)]);
        process.kill = Some(Err(io::Error::new(io::ErrorKind::PermissionDenied, "kill denied")));
        let clock = FakeClock::new();

        let error = wait_for_process(Mode::Criterion, &mut process, Some(Duration::from_millis(25)), &clock).unwrap_err();

        assert!(matches!(error, Error::WorkerTermination { .. }));
        assert_eq!(process.wait_calls, 0);
        assert_eq!(clock.elapsed.get(), Duration::from_millis(25));
    }

    #[test]
    fn failed_kill_and_reap_preserve_both_errors() {
        let mut process = FakeProcess::new([Ok(None), Ok(None), Err(io::Error::other("race check failed"))]);
        process.kill = Some(Err(io::Error::other("kill failed")));
        let clock = FakeClock::new();

        let error = wait_for_process(Mode::Criterion, &mut process, Some(Duration::from_millis(25)), &clock)
            .unwrap_err()
            .to_string();

        assert!(error.contains("kill failed"));
        assert!(error.contains("race check failed"));
        assert_eq!(process.wait_calls, 0);
    }

    #[test]
    fn successful_kill_has_bounded_reap_grace() {
        let mut process = FakeProcess::new([Ok(None), Ok(None)]);
        let clock = FakeClock::new();

        let error = wait_for_process(Mode::Criterion, &mut process, Some(Duration::from_millis(25)), &clock).unwrap_err();

        assert!(matches!(error, Error::WorkerTimedOut { .. }));
        assert_eq!(clock.elapsed.get(), Duration::from_millis(25) + TERMINATION_REAP_GRACE);
        assert_eq!(process.wait_calls, 0);
    }

    #[test]
    fn timeout_preserves_reap_error_alongside_timeout() {
        let mut process = FakeProcess::new([Ok(None), Ok(None), Err(io::Error::other("reap failed"))]);
        let clock = FakeClock::new();

        let error = wait_for_process(Mode::Criterion, &mut process, Some(Duration::from_millis(25)), &clock)
            .unwrap_err()
            .to_string();

        assert!(error.contains("criterion worker exceeded"));
        assert!(error.contains("reap failed"));
    }

    #[test]
    fn blocking_wait_is_used_only_without_timeout() {
        let mut process = FakeProcess::new([]);
        let clock = FakeClock::new();

        assert!(wait_for_process(Mode::Criterion, &mut process, None, &clock).unwrap().success());
        assert_eq!(process.wait_calls, 1);
        assert_eq!(process.kill_calls, 0);
    }

    #[test]
    fn orchestration_routes_modes_and_flattens_all_failures() {
        let mut routed = Vec::new();
        let summary = orchestrate_modes(&Mode::ALL, FailureMode::KeepGoing, |mode| {
            routed.push(mode);
            if mode == Mode::Criterion {
                combine_results::<()>(
                    Err(Error::WorkerFailed {
                        mode: "criterion",
                        code: Some(7),
                    }),
                    Err(Error::ArtifactIo {
                        path: "criterion-manifest.json".into(),
                        source: io::Error::other("manifest failed"),
                    }),
                )
            } else {
                Ok(())
            }
        })
        .unwrap();

        assert_eq!(routed, Mode::ALL);
        assert_eq!(summary.completed, [Mode::Gungraun, Mode::Perf, Mode::Vtune, Mode::Allocations]);
        assert_eq!(summary.failures.len(), 2);
        assert!(summary.failures[0].contains("status 7"));
        assert!(summary.failures[1].contains("manifest failed"));
    }

    #[test]
    fn fail_fast_preserves_launch_and_manifest_failures_and_stops() {
        let mut calls = 0;
        let error = orchestrate_modes(&Mode::ALL, FailureMode::FailFast, |_mode| {
            calls += 1;
            combine_results::<()>(
                Err(Error::Spawn {
                    mode: "criterion",
                    source: io::Error::other("launch failed"),
                }),
                Err(Error::ArtifactIo {
                    path: "manifest.json".into(),
                    source: io::Error::other("manifest failed"),
                }),
            )
        })
        .unwrap_err()
        .to_string();

        assert_eq!(calls, 1);
        assert!(error.contains("launch failed"));
        assert!(error.contains("manifest failed"));
    }

    #[test]
    fn cleanup_failure_is_retained_with_existing_aggregates() {
        let mut failures = vec!["earlier engine failure".to_owned()];
        append_failure(
            &mut failures,
            multiple_failures([
                Error::ConsumeWorkerToken(io::Error::other("token cleanup failed")),
                Error::ConsoleOutput(io::Error::other("console failed")),
            ]),
        );

        assert_eq!(failures.len(), 3);
        assert!(failures[1].contains("token cleanup failed"));
        assert!(failures[2].contains("console failed"));
    }

    #[test]
    fn worker_token_validation_is_deterministic_and_single_use() {
        let root = tempfile::tempdir().unwrap();
        let valid = root.path().join("metabench-worker-valid");
        fs::create_dir(&valid).unwrap();

        consume_worker_token_from(Some(valid.as_os_str()), root.path()).unwrap();
        assert!(matches!(
            consume_worker_token_from(Some(valid.as_os_str()), root.path()),
            Err(Error::ConsumeWorkerToken(_))
        ));
        assert!(matches!(
            consume_worker_token_from(None, root.path()),
            Err(Error::StaleWorkerMarker)
        ));

        let malformed = root.path().join("worker-invalid");
        fs::create_dir(&malformed).unwrap();
        assert!(matches!(
            consume_worker_token_from(Some(malformed.as_os_str()), root.path()),
            Err(Error::StaleWorkerMarker)
        ));
        let empty_suffix = root.path().join("metabench-worker-");
        fs::create_dir(&empty_suffix).unwrap();
        assert!(matches!(
            consume_worker_token_from(Some(empty_suffix.as_os_str()), root.path()),
            Err(Error::StaleWorkerMarker)
        ));

        let foreign_root = tempfile::tempdir().unwrap();
        let foreign = foreign_root.path().join("metabench-worker-foreign");
        fs::create_dir(&foreign).unwrap();
        assert!(matches!(
            consume_worker_token_from(Some(foreign.as_os_str()), root.path()),
            Err(Error::StaleWorkerMarker)
        ));
    }

    #[test]
    fn report_paths_reject_explicit_alias_of_derived_default() {
        let root = tempfile::tempdir().unwrap();
        let mut arguments = Arguments::parse_from(["--export-json".into(), root.path().join("report.md").into()], None).unwrap();

        assert!(matches!(
            report_paths_with_root(&arguments, root.path()),
            Err(Error::ConflictingOutputOptions)
        ));

        arguments.no_output = true;
        assert_eq!(report_paths_with_root(&arguments, root.path()).unwrap(), (None, None));
    }

    #[test]
    fn probe_filters_are_replaced_without_losing_option_values() {
        let arguments = ["--sample-size", "20", "--color", "never", "group", "--exact", "--bench"].map(OsString::from);
        assert_eq!(
            probe_selection_arguments(&arguments).unwrap(),
            ["--sample-size", "20", "--color", "never", "group", "--exact"]
        );
        assert_eq!(
            probe_selection_arguments(&[
                "--profile-time".into(),
                "60".into(),
                "--load-baseline=old".into(),
                "--baseline".into(),
                "old".into(),
                "--noplot".into(),
                "group".into(),
            ],)
            .unwrap(),
            ["group"]
        );
    }

    #[test]
    fn native_probe_selection_accepts_criterion_aliases_and_output_options() {
        for arguments in [
            vec![
                "-s", "old", "-b", "old", "-n", "-c", "never", "--format", "terse", "group", "--exact",
            ],
            vec!["-s=old", "-b=old", "-n", "-c=never", "--format=terse", "group", "--exact"],
        ] {
            let arguments = arguments.into_iter().map(OsString::from).collect::<Vec<_>>();
            let targets = serde_json::from_str(
                r#"[{"identity":"group","native_identity":"native/group","group":0,"benchmark":0,"case":0,"iteration":null}]"#,
            )
            .unwrap();
            let selected = select_native_probe_targets(targets, &arguments).unwrap();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].identity, "group");
        }
        for arguments in [vec!["-s"], vec!["-b"], vec!["-c"], vec!["--format"]] {
            assert!(matches!(
                probe_selection_arguments(&arguments.into_iter().map(OsString::from).collect::<Vec<_>>()),
                Err(Error::Probe(_))
            ));
        }
    }

    #[test]
    fn configured_target_root_disambiguates_a_triple_named_directory() {
        let configured = PathBuf::from("custom").join(env!("METABENCH_TARGET"));
        for executable in [
            configured.join("release/deps/benchmark"),
            configured.join(env!("METABENCH_TARGET")).join("release/deps/benchmark"),
        ] {
            assert_eq!(
                effective_target_directory(&executable, |name| (name == "CARGO_TARGET_DIR").then(|| configured.clone().into())).unwrap(),
                configured
            );
        }
        assert_eq!(
            effective_target_directory(&configured.join("release/deps/benchmark"), |_| Some(configured.join(".").into())).unwrap(),
            configured
        );
    }

    #[test]
    fn target_directory_follows_executable_layout_over_environment() {
        for (executable, expected) in [
            ("custom/release/deps/benchmark", "custom"),
            ("custom/debug/examples/benchmark", "custom"),
            ("custom/release/benchmark", "custom"),
        ] {
            assert_eq!(
                effective_target_directory(Path::new(executable), |_| Some("ignored".into())).unwrap(),
                PathBuf::from(expected)
            );
        }
        let executable = PathBuf::from("custom")
            .join(env!("METABENCH_TARGET"))
            .join("release/deps/benchmark");
        assert_eq!(effective_target_directory(&executable, |_| None).unwrap(), PathBuf::from("custom"));
        let custom = tempfile::tempdir().unwrap();
        fs::create_dir_all(custom.path().join("special/.fingerprint")).unwrap();
        assert_eq!(
            effective_target_directory(&custom.path().join("special/deps/benchmark"), |_| None).unwrap(),
            custom.path()
        );
    }

    #[test]
    fn environment_and_output_boundaries_map_exact_errors() {
        assert_eq!(
            effective_target_directory(Path::new("relocated/benchmark"), |name| (name == "CARGO_TARGET_DIR")
                .then(|| OsString::from("custom-target")),)
            .unwrap(),
            PathBuf::from("custom-target")
        );
        assert!(matches!(
            current_executable(|| Err(io::Error::other("no executable"))),
            Err(Error::CurrentExecutable(_))
        ));
        assert!(matches!(
            effective_target_directory(Path::new("relocated/benchmark"), |_| None),
            Err(Error::ArtifactFormat { .. })
        ));

        let mut progress = Vec::new();
        write_progress(&mut progress, Mode::Criterion).unwrap();
        assert_eq!(progress, b"Running criterion benchmarks...\n");

        assert!(matches!(
            write_console_output(FailingOutput, "report"),
            Err(Error::ConsoleOutput(_))
        ));
    }
}
