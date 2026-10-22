// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;
use std::{error, fmt, io};

use crate::runner::terminal_text;

#[derive(Debug)]
pub(crate) enum Error {
    InvalidMode(String),
    Probe(String),
    UnsupportedMode {
        mode: &'static str,
        reason: &'static str,
    },
    #[cfg(any(target_os = "linux", test))]
    UnsupportedPerfArgument(OsString),
    UnsupportedVtuneArgument(OsString),
    InvalidGungraunTestArgument(OsString),
    MissingOptionValue(&'static str),
    UnknownOption(OsString),
    AmbiguousArguments(Vec<OsString>),
    ArgumentsForUnselectedEngine(&'static str),
    ConflictingModes,
    ConflictingFailureModes,
    ConflictingOutputOptions,
    ConflictingBaselineOptions,
    CurrentExecutable(io::Error),
    HelpOutput(io::Error),
    ConsoleOutput(io::Error),
    Spawn {
        mode: &'static str,
        source: io::Error,
    },
    WorkerTermination {
        mode: &'static str,
        source: io::Error,
    },
    WorkerFailed {
        mode: &'static str,
        code: Option<i32>,
    },
    WorkerTimedOut {
        mode: &'static str,
        timeout: Duration,
    },
    StaleWorkerMarker,
    CreateWorkerToken(io::Error),
    ConsumeWorkerToken(io::Error),
    PerfControl(io::Error),
    PerfWorkloadNotMeasured,
    VtuneControl(io::Error),
    VtuneWorkloadNotMeasured,
    MultipleModeFailures(Vec<String>),
    ReportIo {
        path: PathBuf,
        source: io::Error,
    },
    ReportJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    ReportFormat {
        path: PathBuf,
        message: String,
    },
    UnsupportedReportSchema {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    DuplicateReportBenchmark(String),
    Metadata(String),
    InvalidThreshold(String),
    InvalidTimeout(String),
    ArtifactIo {
        path: PathBuf,
        source: io::Error,
    },
    ArtifactJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    ArtifactFormat {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for Error {
    #[expect(clippy::renamed_function_params, reason = "the descriptive name improves readability")]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Probe(message) => write!(formatter, "native benchmark probe failed: {}", terminal_text(message)),
            Self::InvalidMode(mode) => {
                write!(
                    formatter,
                    "unknown mode '{mode}'; expected criterion, gungraun, perf, vtune, or allocations"
                )
            }
            Self::UnsupportedMode { mode, reason } => write!(formatter, "{mode} is unavailable: {reason}"),
            #[cfg(any(target_os = "linux", test))]
            Self::UnsupportedPerfArgument(argument) => write!(
                formatter,
                "perf argument {} conflicts with metabench's measurement protocol",
                argument.to_string_lossy()
            ),
            Self::UnsupportedVtuneArgument(argument) => write!(
                formatter,
                "vtune argument {} conflicts with metabench's measurement protocol",
                argument.to_string_lossy()
            ),
            Self::InvalidGungraunTestArgument(argument) => write!(
                formatter,
                "invalid Gungraun test argument '{}'. Use --gungraun-arg=--test=true or --gungraun-arg=--test=false.",
                terminal_text(&argument.to_string_lossy())
            ),
            Self::MissingOptionValue(option) => write!(formatter, "{option} requires a value"),
            Self::UnknownOption(option) => write!(
                formatter,
                "unknown metabench option {}; use --criterion-arg, --gungraun-arg, --perf-arg, or --vtune-arg for native options",
                option.to_string_lossy()
            ),
            Self::AmbiguousArguments(arguments) => {
                formatter.write_str("arguments after -- are ambiguous with multiple engines selected:")?;
                for argument in arguments {
                    write!(formatter, " {}", argument.to_string_lossy())?;
                }
                Ok(())
            }
            Self::ArgumentsForUnselectedEngine(mode) => {
                write!(formatter, "{mode} arguments were supplied without selecting --{mode}")
            }
            Self::ConflictingModes => formatter.write_str("--all-engines cannot be combined with individual engine selectors"),
            Self::ConflictingFailureModes => formatter.write_str("--fail-fast cannot be combined with --keep-going"),
            Self::ConflictingOutputOptions => formatter.write_str("--no-output, --output, --export-json, and --export-md cannot conflict"),
            Self::ConflictingBaselineOptions => formatter.write_str("--no-baseline cannot be combined with --baseline"),
            Self::CurrentExecutable(error) => write!(formatter, "failed to locate the benchmark executable: {error}"),
            Self::HelpOutput(error) => write!(formatter, "failed to write help text: {error}"),
            Self::ConsoleOutput(error) => write!(formatter, "failed to write console output: {error}"),
            Self::Spawn { mode, source } => write!(formatter, "failed to launch the {mode} worker: {source}"),
            Self::WorkerTermination { mode, source } => write!(formatter, "failed to terminate the {mode} worker: {source}"),
            Self::WorkerFailed { mode, code } => match code {
                Some(code) => write!(formatter, "{mode} worker exited with status {code}"),
                None => write!(formatter, "{mode} worker terminated without an exit code"),
            },
            Self::WorkerTimedOut { mode, timeout } => write!(formatter, "{mode} worker exceeded the {timeout:?} timeout"),
            Self::StaleWorkerMarker => formatter.write_str("refusing an inherited or stale internal worker marker"),
            Self::CreateWorkerToken(error) => write!(formatter, "failed to create an internal worker token: {error}"),
            Self::ConsumeWorkerToken(error) => write!(formatter, "failed to consume an internal worker token: {error}"),
            Self::PerfControl(error) => write!(formatter, "failed to control Linux perf counters: {error}"),
            Self::PerfWorkloadNotMeasured => formatter.write_str("the perf worker did not execute its generated probe"),
            Self::VtuneControl(error) => write!(formatter, "failed to control the VTune collection: {error}"),
            Self::VtuneWorkloadNotMeasured => formatter.write_str("the vtune worker did not execute its generated probe"),
            Self::MultipleModeFailures(failures) => {
                formatter.write_str("multiple benchmark failures occurred")?;
                for failure in failures {
                    write!(formatter, "\n- {failure}")?;
                }
                Ok(())
            }
            Self::ReportIo { path, source } => write!(formatter, "failed to access report {}: {source}", path.display()),
            Self::ReportJson { path, source } => write!(formatter, "invalid report JSON {}: {source}", path.display()),
            Self::ReportFormat { path, message } => write!(formatter, "invalid report {}: {message}", path.display()),
            Self::UnsupportedReportSchema { path, found, expected } => write!(
                formatter,
                "unsupported report schema {found} in {}; expected schema {expected}",
                path.display()
            ),
            Self::DuplicateReportBenchmark(identity) => {
                write!(formatter, "report contains duplicate benchmark '{}'", terminal_text(identity))
            }
            Self::Metadata(message) => write!(formatter, "failed to collect environment metadata: {message}"),
            Self::InvalidThreshold(value) => write!(
                formatter,
                "invalid regression threshold '{value}'; expected a finite non-negative percentage"
            ),
            Self::InvalidTimeout(value) => write!(
                formatter,
                "invalid worker timeout '{value}'; expected a positive integer followed by ms, s, m, or h"
            ),
            Self::ArtifactIo { path, source } => write!(formatter, "failed to access artifact {}: {source}", path.display()),
            Self::ArtifactJson { path, source } => write!(formatter, "invalid artifact JSON {}: {source}", path.display()),
            Self::ArtifactFormat { path, message } => write!(formatter, "invalid artifact {}: {message}", path.display()),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::CurrentExecutable(source)
            | Self::HelpOutput(source)
            | Self::ConsoleOutput(source)
            | Self::CreateWorkerToken(source)
            | Self::ConsumeWorkerToken(source)
            | Self::PerfControl(source)
            | Self::VtuneControl(source)
            | Self::Spawn { source, .. }
            | Self::WorkerTermination { source, .. }
            | Self::ReportIo { source, .. }
            | Self::ArtifactIo { source, .. } => Some(source),
            Self::ReportJson { source, .. } | Self::ArtifactJson { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn probe_diagnostics_escape_untrusted_messages() {
        let message = "unsupported option --bad\n\u{1b}[31m\r\t\u{061c}\u{200e}\u{200f}\u{202e}\u{2066}";
        let error = Error::Probe(message.to_owned());
        assert_eq!(
            error.to_string(),
            r"native benchmark probe failed: unsupported option --bad\n\u{1b}[31m\r\t\u{61c}\u{200e}\u{200f}\u{202e}\u{2066}"
        );
        assert!(matches!(error, Error::Probe(raw) if raw == message));
    }

    #[test]
    fn duplicate_report_diagnostics_escape_disk_supplied_identities() {
        let error = Error::DuplicateReportBenchmark("case\n\u{1b}[31m\u{202e}".into());
        assert_eq!(error.to_string(), r"report contains duplicate benchmark 'case\n\u{1b}[31m\u{202e}'");
    }

    #[test]
    fn gungraun_test_diagnostics_escape_untrusted_values() {
        let argument = "--test=no\n\u{1b}[31m\u{202e}";
        let error = Error::InvalidGungraunTestArgument(argument.into());
        assert_eq!(
            error.to_string(),
            r"invalid Gungraun test argument '--test=no\n\u{1b}[31m\u{202e}'. Use --gungraun-arg=--test=true or --gungraun-arg=--test=false."
        );
        assert!(matches!(error, Error::InvalidGungraunTestArgument(raw) if raw == argument));
    }

    #[test]
    fn source_returns_the_wrapped_io_error_for_control_protocol_variants() {
        let error = Error::VtuneControl(io::Error::other("control pipe closed"));
        assert!(error::Error::source(&error).is_some());

        let error = Error::PerfControl(io::Error::other("control pipe closed"));
        assert!(error::Error::source(&error).is_some());
    }

    #[test]
    fn source_returns_none_for_variants_without_an_underlying_cause() {
        assert!(error::Error::source(&Error::VtuneWorkloadNotMeasured).is_none());
        assert!(error::Error::source(&Error::ConflictingModes).is_none());
    }
}
