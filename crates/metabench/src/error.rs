// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsString;
use std::path::PathBuf;
use std::{fmt, io};

/// An error produced while configuring or running metabench.
#[derive(Debug)]
pub enum Error {
    /// A mode name was not recognized.
    InvalidMode(String),
    /// A metabench option was missing its value.
    MissingOptionValue(&'static str),
    /// An unrecognized unscoped option was supplied.
    UnknownOption(OsString),
    /// Arguments after `--` could not be assigned to one selected engine.
    AmbiguousArguments(Vec<OsString>),
    /// Scoped arguments targeted an engine that was not selected.
    ArgumentsForUnselectedEngine(&'static str),
    /// Conflicting engine selectors were supplied.
    ConflictingModes,
    /// Both failure-continuation modes were requested.
    ConflictingFailureModes,
    /// The combined output shortcut was mixed with explicit export paths.
    ConflictingOutputOptions,
    /// Automatic baseline suppression was mixed with an explicit baseline.
    ConflictingBaselineOptions,
    /// The current executable could not be located.
    CurrentExecutable(io::Error),
    /// The benchmark process working directory could not be located.
    CurrentDirectory(io::Error),
    /// Metabench help text could not be written.
    HelpOutput(io::Error),
    /// Benchmark progress or the console report could not be written.
    ConsoleOutput(io::Error),
    /// A worker process could not be launched.
    Spawn {
        /// The mode being launched.
        mode: &'static str,
        /// The process-launch error.
        source: io::Error,
    },
    /// A worker exited unsuccessfully.
    WorkerFailed {
        /// The failed mode.
        mode: &'static str,
        /// The worker exit code, if one was available.
        code: Option<i32>,
    },
    /// A worker exceeded the configured time limit.
    WorkerTimedOut {
        /// The engine whose worker timed out.
        mode: &'static str,
        /// Configured time limit.
        timeout: std::time::Duration,
    },
    /// An internal worker marker had an invalid value.
    InvalidWorkerMode(String),
    /// A mode is not supported on the current platform.
    UnsupportedMode {
        /// The unavailable mode.
        mode: &'static str,
        /// Why it is unavailable.
        reason: &'static str,
    },
    /// A worker was asked to run a benchmark that was not registered.
    UnknownBenchmark(String),
    /// A worker was asked to run a Gungraun case index that was not registered.
    InvalidGungraunCaseIndex(usize),
    /// More than one benchmark used the same name.
    DuplicateBenchmark(String),
    /// A benchmark group name was empty or contained the identity separator.
    InvalidGroupName(String),
    /// A benchmark name was empty or contained the identity separator.
    InvalidBenchmarkName {
        /// Group containing the benchmark.
        group: String,
        /// Invalid benchmark name.
        benchmark: String,
    },
    /// A data case name was empty or contained the identity separator.
    InvalidCaseName {
        /// Group and benchmark identity.
        benchmark: String,
        /// Invalid case name.
        case: String,
    },
    /// A benchmark did not select any engines.
    BenchmarkWithoutEngines(String),
    /// A CLI filter selected an engine unused by all benchmarks.
    EngineNotConfigured(&'static str),
    /// A benchmark callback did not define exactly one workload.
    InvalidRunCount {
        /// The benchmark name.
        benchmark: String,
        /// The number of workloads defined by its callback.
        count: u8,
    },
    /// A worker marker was inherited or otherwise no longer valid.
    StaleWorkerMarker,
    /// A one-shot worker token could not be created.
    CreateWorkerToken(io::Error),
    /// A one-shot worker token could not be consumed.
    ConsumeWorkerToken(io::Error),
    /// Arguments were supplied to a mode that cannot consume them.
    UnsupportedArguments {
        /// The selected mode.
        mode: &'static str,
        /// The unsupported arguments.
        arguments: Vec<OsString>,
    },
    /// Multiple independent modes failed during an all-mode run.
    MultipleModeFailures(Vec<String>),
    /// The perf control channel could not be created or used.
    PerfControl(io::Error),
    /// Perf returned an unexpected control acknowledgement.
    UnexpectedPerfAcknowledgement(String),
    /// A report file could not be read or written.
    ReportIo {
        /// Report path.
        path: PathBuf,
        /// Filesystem error.
        source: io::Error,
    },
    /// A report could not be serialized or parsed.
    ReportJson {
        /// Report path.
        path: PathBuf,
        /// JSON error.
        source: serde_json::Error,
    },
    /// A report used an unsupported schema version.
    UnsupportedReportSchema {
        /// Report path.
        path: PathBuf,
        /// Version encoded in the report.
        found: u32,
        /// Version supported by this metabench release.
        expected: u32,
    },
    /// A report contained the same benchmark identity more than once.
    DuplicateReportBenchmark(String),
    /// Environment metadata could not be collected.
    Metadata(String),
    /// A regression threshold was invalid.
    InvalidThreshold(String),
    /// A benchmark filter was empty, non-UTF-8, or internally malformed.
    InvalidFilter(String),
    /// A worker timeout could not be parsed.
    InvalidTimeout(String),
    /// No registered benchmark matched the requested filters.
    NoBenchmarksMatched(Vec<String>),
    /// A backend artifact could not be accessed.
    ArtifactIo {
        /// Artifact path.
        path: PathBuf,
        /// Filesystem error.
        source: io::Error,
    },
    /// A backend artifact contained invalid JSON.
    ArtifactJson {
        /// Artifact path.
        path: PathBuf,
        /// JSON error.
        source: serde_json::Error,
    },
    /// A backend artifact had invalid or unsupported contents.
    ArtifactFormat {
        /// Artifact path.
        path: PathBuf,
        /// Description of the invalid data.
        message: String,
    },
}

impl fmt::Display for Error {
    #[expect(
        clippy::too_many_lines,
        reason = "the exhaustive match keeps every public error message in one place"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMode(mode) => format_invalid_mode(f, mode),
            Self::MissingOptionValue(option) => write!(f, "{option} requires a value"),
            Self::UnknownOption(option) => write!(
                f,
                "unknown metabench option {}; use a scoped --*-arg option for engine arguments",
                option.to_string_lossy()
            ),
            Self::AmbiguousArguments(arguments) => {
                f.write_str("arguments after -- are ambiguous with multiple engines selected:")?;
                for argument in arguments {
                    write!(f, " {}", argument.to_string_lossy())?;
                }
                Ok(())
            }
            Self::ArgumentsForUnselectedEngine(mode) => {
                write!(f, "{mode} arguments were supplied without selecting --{mode}")
            }
            error @ (Self::ConflictingModes
            | Self::ConflictingFailureModes
            | Self::ConflictingOutputOptions
            | Self::ConflictingBaselineOptions) => format_conflict(f, error),
            Self::CurrentExecutable(error) => {
                write!(f, "failed to locate the benchmark executable: {error}")
            }
            Self::CurrentDirectory(error) => {
                write!(f, "failed to locate the benchmark working directory: {error}")
            }
            Self::HelpOutput(error) => write!(f, "failed to write help text: {error}"),
            Self::ConsoleOutput(error) => write!(f, "failed to write console output: {error}"),
            Self::Spawn { mode, source } => {
                write!(f, "failed to launch the {mode} worker: {source}")
            }
            Self::WorkerFailed { mode, code } => match code {
                Some(code) => write!(f, "{mode} worker exited with status {code}"),
                None => write!(f, "{mode} worker terminated without an exit code"),
            },
            Self::WorkerTimedOut { mode, timeout } => {
                write!(f, "{mode} worker exceeded the {timeout:?} timeout")
            }
            Self::InvalidWorkerMode(mode) => {
                write!(f, "invalid internal worker mode '{mode}'")
            }
            Self::UnsupportedMode { mode, reason } => {
                write!(f, "{mode} mode is unavailable: {reason}")
            }
            Self::UnknownBenchmark(name) => {
                write!(f, "worker benchmark '{name}' was not registered")
            }
            Self::InvalidGungraunCaseIndex(index) => {
                write!(f, "Gungraun case index {index} is out of range")
            }
            error @ (Self::DuplicateBenchmark(_)
            | Self::InvalidGroupName(_)
            | Self::InvalidBenchmarkName { .. }
            | Self::InvalidCaseName { .. }
            | Self::BenchmarkWithoutEngines(_)
            | Self::EngineNotConfigured(_)) => format_registration_error(f, error),
            Self::InvalidRunCount { benchmark, count } => write!(
                f,
                "benchmark '{benchmark}' defined {count} workloads; exactly one b.run call is required"
            ),
            Self::StaleWorkerMarker => f.write_str("refusing an inherited or stale internal worker marker"),
            Self::CreateWorkerToken(error) => {
                write!(f, "failed to create an internal worker token: {error}")
            }
            Self::ConsumeWorkerToken(error) => {
                write!(f, "failed to consume an internal worker token: {error}")
            }
            Self::UnsupportedArguments { mode, arguments } => {
                write!(f, "{mode} mode does not accept worker arguments:")?;
                for argument in arguments {
                    write!(f, " {}", argument.to_string_lossy())?;
                }
                Ok(())
            }
            Self::MultipleModeFailures(failures) => format_failures(f, failures),
            Self::PerfControl(error) => write!(f, "perf control channel failed: {error}"),
            Self::UnexpectedPerfAcknowledgement(acknowledgement) => {
                write!(f, "perf returned an unexpected control acknowledgement: {acknowledgement:?}")
            }
            Self::ReportIo { path, source } => {
                write!(f, "failed to access report {}: {source}", path.display())
            }
            Self::ReportJson { path, source } => {
                write!(f, "invalid report JSON {}: {source}", path.display())
            }
            Self::UnsupportedReportSchema { path, found, expected } => write!(
                f,
                "unsupported report schema {found} in {}; expected schema {expected}",
                path.display()
            ),
            Self::DuplicateReportBenchmark(identity) => {
                write!(f, "report contains duplicate benchmark '{identity}'")
            }
            Self::Metadata(message) => {
                write!(f, "failed to collect environment metadata: {message}")
            }
            error @ (Self::InvalidThreshold(_) | Self::InvalidFilter(_) | Self::InvalidTimeout(_) | Self::NoBenchmarksMatched(_)) => {
                format_cli_value_error(f, error)
            }
            Self::ArtifactIo { path, source } => {
                write!(f, "failed to access artifact {}: {source}", path.display())
            }
            Self::ArtifactJson { path, source } => {
                write!(f, "invalid artifact JSON {}: {source}", path.display())
            }
            Self::ArtifactFormat { path, message } => {
                write!(f, "invalid artifact {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentExecutable(source)
            | Self::CurrentDirectory(source)
            | Self::HelpOutput(source)
            | Self::ConsoleOutput(source)
            | Self::CreateWorkerToken(source)
            | Self::ConsumeWorkerToken(source)
            | Self::PerfControl(source)
            | Self::Spawn { source, .. }
            | Self::ReportIo { source, .. }
            | Self::ArtifactIo { source, .. } => Some(source),
            Self::ReportJson { source, .. } | Self::ArtifactJson { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn format_failures(f: &mut fmt::Formatter<'_>, failures: &[String]) -> fmt::Result {
    f.write_str("multiple benchmark engines failed")?;
    for failure in failures {
        write!(f, "\n- {failure}")?;
    }

    Ok(())
}

fn format_conflict(f: &mut fmt::Formatter<'_>, error: &Error) -> fmt::Result {
    match error {
        Error::ConflictingModes => f.write_str("--all-engines cannot be combined with individual engine selectors"),
        Error::ConflictingFailureModes => f.write_str("--fail-fast cannot be combined with --keep-going"),
        Error::ConflictingOutputOptions => f.write_str("--no-output, --output, --export-json, and --export-md cannot conflict"),
        Error::ConflictingBaselineOptions => f.write_str("--no-baseline cannot be combined with --baseline"),
        _ => unreachable!("format_conflict called with an unrelated error"),
    }
}

fn format_cli_value_error(f: &mut fmt::Formatter<'_>, error: &Error) -> fmt::Result {
    match error {
        Error::InvalidThreshold(value) => write!(
            f,
            "invalid regression threshold '{value}'; expected a finite non-negative percentage"
        ),
        Error::InvalidFilter(value) => write!(f, "invalid benchmark filter '{value}'; expected a non-empty UTF-8 glob"),
        Error::InvalidTimeout(value) => write!(
            f,
            "invalid worker timeout '{value}'; expected a positive integer followed by ms, s, m, or h"
        ),
        Error::NoBenchmarksMatched(filters) => {
            write!(f, "no benchmarks matched filter")?;
            if filters.len() != 1 {
                f.write_str("s")?;
            }
            for filter in filters {
                write!(f, " '{filter}'")?;
            }
            Ok(())
        }
        _ => unreachable!("format_cli_value_error called with an unrelated error"),
    }
}

fn format_invalid_mode(f: &mut fmt::Formatter<'_>, mode: &str) -> fmt::Result {
    write!(f, "unknown mode '{mode}'; expected criterion, callgrind, perf, or allocations")
}

fn format_registration_error(f: &mut fmt::Formatter<'_>, error: &Error) -> fmt::Result {
    match error {
        Error::DuplicateBenchmark(name) => {
            write!(f, "benchmark name '{name}' was registered more than once")
        }
        Error::InvalidGroupName(name) => {
            write!(f, "benchmark group name '{name}' must be non-empty and contain no '/'")
        }
        Error::InvalidBenchmarkName { group, benchmark } => write!(
            f,
            "benchmark name '{benchmark}' in group '{group}' must be non-empty and contain no '/'"
        ),
        Error::InvalidCaseName { benchmark, case } => write!(
            f,
            "case name '{case}' for benchmark '{benchmark}' must be non-empty and contain no '/'"
        ),
        Error::BenchmarkWithoutEngines(name) => {
            write!(f, "benchmark '{name}' did not select any engines")
        }
        Error::EngineNotConfigured(mode) => {
            write!(f, "--{mode} selected an engine used by no registered benchmark")
        }
        _ => unreachable!("called only for registration errors"),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn source_exposes_structured_causes_only_for_wrapping_variants() {
        let io_error = || io::Error::other("boom");
        let process = Error::Spawn {
            mode: "criterion",
            source: io_error(),
        };
        let report_io = Error::ReportIo {
            path: PathBuf::from("report.json"),
            source: io_error(),
        };
        let json_source = serde_json::from_str::<serde_json::Value>("{").expect_err("invalid JSON");
        let artifact_json = Error::ArtifactJson {
            path: PathBuf::from("artifact.json"),
            source: json_source,
        };

        assert!(process.source().is_some());
        assert!(report_io.source().is_some());
        assert!(artifact_json.source().is_some());
        assert!(Error::InvalidMode("unknown".to_owned()).source().is_none());
    }
}
