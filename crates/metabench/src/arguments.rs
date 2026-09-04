// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

use crate::report::DEFAULT_REGRESSION_THRESHOLD;
use crate::{Error, Mode};

pub(crate) const WORKER_MODE_ENV: &str = "METABENCH_INTERNAL_WORKER_MODE";
pub(crate) const WORKER_BENCHMARK_ENV: &str = "METABENCH_INTERNAL_BENCHMARK";
pub(crate) const WORKER_TOKEN_ENV: &str = "METABENCH_INTERNAL_WORKER_TOKEN";
pub(crate) const WORKER_FILTERS_ENV: &str = "METABENCH_INTERNAL_FILTERS";
pub(crate) const PERF_CONTROL_ENV: &str = "METABENCH_INTERNAL_PERF_CONTROL";
pub(crate) const PERF_ACK_ENV: &str = "METABENCH_INTERNAL_PERF_ACK";

#[derive(Debug, Default)]
struct BackendArguments {
    criterion: Vec<OsString>,
    callgrind: Vec<OsString>,
    perf: Vec<OsString>,
    allocations: Vec<OsString>,
}

impl BackendArguments {
    fn get(&self, mode: Mode) -> &[OsString] {
        match mode {
            Mode::Criterion => &self.criterion,
            Mode::Callgrind => &self.callgrind,
            Mode::Perf => &self.perf,
            Mode::Allocations => &self.allocations,
        }
    }

    fn get_mut(&mut self, mode: Mode) -> &mut Vec<OsString> {
        match mode {
            Mode::Criterion => &mut self.criterion,
            Mode::Callgrind => &mut self.callgrind,
            Mode::Perf => &mut self.perf,
            Mode::Allocations => &mut self.allocations,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Arguments {
    pub(crate) modes: Vec<Mode>,
    pub(crate) explicit_engine_filter: bool,
    pub(crate) help: bool,
    pub(crate) behavior: RunBehavior,
    engine_args: BackendArguments,
    pub(crate) export_markdown: Option<PathBuf>,
    pub(crate) export_json: Option<PathBuf>,
    pub(crate) baseline: Option<PathBuf>,
    pub(crate) automatic_baseline: bool,
    pub(crate) regression_threshold: f64,
    pub(crate) filters: Vec<String>,
    pub(crate) timeout: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailureMode {
    KeepGoing,
    FailFast,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RunBehavior {
    pub(crate) show_engine_output: bool,
    pub(crate) list: bool,
    pub(crate) no_output: bool,
    pub(crate) failure_mode: FailureMode,
}

#[derive(Debug, Default)]
struct OrchestrationOptions {
    help: bool,
    show_engine_output: bool,
    list: bool,
    baseline: AutomaticBaseline,
    timeout: Option<Duration>,
    fail_fast: Option<bool>,
    output: OutputSelection,
}

#[derive(Debug, Default)]
enum OutputSelection {
    #[default]
    Default,
    Disabled,
    Base(PathBuf),
}

#[derive(Debug, Default, Eq, PartialEq)]
enum AutomaticBaseline {
    #[default]
    Enabled,
    Disabled,
}

impl Arguments {
    pub(crate) fn parse() -> Result<Self, Error> {
        Self::parse_from(env::args_os().skip(1), env::var_os("BENCH_ENGINE"))
    }

    pub(crate) fn args_for(&self, mode: Mode) -> &[OsString] {
        self.engine_args.get(mode)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "single-pass parsing keeps option ordering and passthrough handling explicit"
    )]
    fn parse_from(arguments: impl IntoIterator<Item = OsString>, environment_engine: Option<OsString>) -> Result<Self, Error> {
        let mut selected = Vec::new();
        let mut all_engines = false;
        let mut orchestration = OrchestrationOptions::default();
        let mut backend_arguments = BackendArguments::default();
        let mut export_markdown = None;
        let mut export_json = None;
        let mut baseline = None;
        let mut regression_threshold = DEFAULT_REGRESSION_THRESHOLD;
        let mut trailing = Vec::new();
        let mut cargo_bench = false;
        let mut filters = Vec::new();
        let mut arguments = arguments.into_iter();

        while let Some(argument) = arguments.next() {
            if argument == OsStr::new("--") {
                trailing.extend(arguments);
                break;
            }
            if argument == OsStr::new("--all-engines") {
                all_engines = true;
            } else if parse_orchestration_option(&argument, &mut arguments, &mut orchestration)? {
            } else if let Some(mode) = selector(&argument) {
                push_unique(&mut selected, mode);
            } else if let Some(mode) = scoped_argument_mode(&argument) {
                let value = arguments.next().ok_or_else(|| Error::MissingOptionValue(argument_name(mode)))?;
                backend_arguments.get_mut(mode).push(value);
            } else if let Some((mode, value)) = scoped_argument_value(&argument) {
                backend_arguments.get_mut(mode).push(value.into());
            } else if argument == OsStr::new("--export-md") {
                export_markdown = Some(Self::next_path(&mut arguments, "--export-md")?);
            } else if let Some(value) = option_value(&argument, "--export-md=") {
                export_markdown = Some(PathBuf::from(value));
            } else if argument == OsStr::new("--export-json") {
                export_json = Some(Self::next_path(&mut arguments, "--export-json")?);
            } else if let Some(value) = option_value(&argument, "--export-json=") {
                export_json = Some(PathBuf::from(value));
            } else if argument == OsStr::new("--baseline") {
                baseline = Some(Self::next_path(&mut arguments, "--baseline")?);
            } else if let Some(value) = option_value(&argument, "--baseline=") {
                baseline = Some(PathBuf::from(value));
            } else if parse_filter_option(&argument, &mut arguments, &mut filters)? {
            } else if argument == OsStr::new("--regression-threshold") {
                let value = arguments.next().ok_or(Error::MissingOptionValue("--regression-threshold"))?;
                regression_threshold = Self::parse_threshold(&value)?;
            } else if let Some(value) = option_value(&argument, "--regression-threshold=") {
                regression_threshold = Self::parse_threshold(OsStr::new(value))?;
            } else if argument == OsStr::new("--bench") {
                cargo_bench = true;
            } else {
                return Err(Error::UnknownOption(argument));
            }
        }

        validate_selection(all_engines, &selected, &filters)?;
        if orchestration.baseline == AutomaticBaseline::Disabled && baseline.is_some() {
            return Err(Error::ConflictingBaselineOptions);
        }
        let no_output = apply_output_selection(orchestration.output, &mut export_markdown, &mut export_json)?;
        let explicit_engine_filter = if selected.is_empty()
            && !all_engines
            && let Some(value) = environment_engine
        {
            selected.push(Mode::from_os_str(&value)?);
            true
        } else {
            !selected.is_empty()
        };
        let modes = if all_engines || selected.is_empty() {
            Mode::ALL.to_vec()
        } else {
            selected
        };
        if !trailing.is_empty() {
            let [mode] = modes.as_slice() else {
                return Err(Error::AmbiguousArguments(trailing));
            };
            backend_arguments.get_mut(*mode).extend(trailing);
        }
        for mode in Mode::ALL {
            if !backend_arguments.get(mode).is_empty() && !modes.contains(&mode) {
                return Err(Error::ArgumentsForUnselectedEngine(mode.as_str()));
            }
        }
        if modes.contains(&Mode::Criterion) {
            push_argument_unique(backend_arguments.get_mut(Mode::Criterion), "--bench");
        }
        if cargo_bench && modes.contains(&Mode::Callgrind) {
            push_argument_unique(backend_arguments.get_mut(Mode::Callgrind), "--bench");
        }

        Ok(Self {
            modes,
            explicit_engine_filter,
            help: orchestration.help,
            behavior: RunBehavior {
                show_engine_output: orchestration.show_engine_output,
                list: orchestration.list,
                no_output,
                failure_mode: if orchestration.fail_fast.unwrap_or(false) {
                    FailureMode::FailFast
                } else {
                    FailureMode::KeepGoing
                },
            },
            engine_args: backend_arguments,
            export_markdown,
            export_json,
            baseline,
            automatic_baseline: orchestration.baseline == AutomaticBaseline::Enabled,
            regression_threshold,
            filters,
            timeout: orchestration.timeout,
        })
    }

    fn next_path(arguments: &mut impl Iterator<Item = OsString>, option: &'static str) -> Result<PathBuf, Error> {
        arguments.next().map(PathBuf::from).ok_or(Error::MissingOptionValue(option))
    }

    fn next_string(arguments: &mut impl Iterator<Item = OsString>, option: &'static str) -> Result<String, Error> {
        let value = arguments.next().ok_or(Error::MissingOptionValue(option))?;
        value
            .into_string()
            .map_err(|value| Error::InvalidFilter(value.to_string_lossy().into_owned()))
    }

    fn parse_threshold(value: &OsStr) -> Result<f64, Error> {
        let text = value.to_string_lossy();
        let threshold = text
            .parse::<f64>()
            .map_err(|_error| Error::InvalidThreshold(text.clone().into_owned()))?;
        if threshold.is_finite() && threshold >= 0.0 {
            Ok(threshold)
        } else {
            Err(Error::InvalidThreshold(text.into_owned()))
        }
    }
}

fn apply_output_selection(
    output: OutputSelection,
    export_markdown: &mut Option<PathBuf>,
    export_json: &mut Option<PathBuf>,
) -> Result<bool, Error> {
    if let OutputSelection::Base(output) = output {
        if export_markdown.is_some() || export_json.is_some() {
            return Err(Error::ConflictingOutputOptions);
        }
        *export_markdown = Some(output.with_extension("md"));
        *export_json = Some(output.with_extension("json"));
        Ok(false)
    } else if matches!(output, OutputSelection::Disabled) {
        if export_markdown.is_some() || export_json.is_some() {
            return Err(Error::ConflictingOutputOptions);
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

fn parse_orchestration_option(
    argument: &OsStr,
    arguments: &mut impl Iterator<Item = OsString>,
    options: &mut OrchestrationOptions,
) -> Result<bool, Error> {
    match argument.to_str() {
        Some("--help" | "-h") => options.help = true,
        Some("--show-engine-output") => options.show_engine_output = true,
        Some("--list") => options.list = true,
        Some("--no-output") => set_output_selection(options, OutputSelection::Disabled)?,
        Some("--no-baseline") => options.baseline = AutomaticBaseline::Disabled,
        Some("--fail-fast") => set_failure_mode(options, true)?,
        Some("--keep-going") => set_failure_mode(options, false)?,
        Some("--timeout") => {
            let value = arguments.next().ok_or(Error::MissingOptionValue("--timeout"))?;
            options.timeout = Some(parse_duration(&value)?);
        }
        Some(value) if value.starts_with("--timeout=") => {
            options.timeout = Some(parse_duration(OsStr::new(&value["--timeout=".len()..]))?);
        }
        Some("--output") => {
            let path = Arguments::next_path(arguments, "--output")?;
            set_output_selection(options, OutputSelection::Base(path))?;
        }
        Some(value) if value.starts_with("--output=") => {
            let path = PathBuf::from(&value["--output=".len()..]);
            set_output_selection(options, OutputSelection::Base(path))?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn set_output_selection(options: &mut OrchestrationOptions, selection: OutputSelection) -> Result<(), Error> {
    if matches!(options.output, OutputSelection::Default) {
        options.output = selection;
        Ok(())
    } else {
        Err(Error::ConflictingOutputOptions)
    }
}

fn set_failure_mode(options: &mut OrchestrationOptions, fail_fast: bool) -> Result<(), Error> {
    if options.fail_fast.is_some_and(|current| current != fail_fast) {
        Err(Error::ConflictingFailureModes)
    } else {
        options.fail_fast = Some(fail_fast);
        Ok(())
    }
}

fn parse_duration(value: &OsStr) -> Result<Duration, Error> {
    let text = value.to_string_lossy();
    let (number, milliseconds) = if let Some(number) = text.strip_suffix("ms") {
        (number, 1)
    } else if let Some(number) = text.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 60_000)
    } else if let Some(number) = text.strip_suffix('h') {
        (number, 3_600_000)
    } else {
        return Err(Error::InvalidTimeout(text.into_owned()));
    };
    let milliseconds = number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(milliseconds))
        .filter(|duration| *duration != 0)
        .ok_or_else(|| Error::InvalidTimeout(text.into_owned()))?;
    Ok(Duration::from_millis(milliseconds))
}

fn validate_selection(all_engines: bool, selected: &[Mode], filters: &[String]) -> Result<(), Error> {
    if all_engines && !selected.is_empty() {
        Err(Error::ConflictingModes)
    } else if filters.iter().any(String::is_empty) {
        Err(Error::InvalidFilter(String::new()))
    } else {
        Ok(())
    }
}

fn parse_filter_option(argument: &OsStr, arguments: &mut impl Iterator<Item = OsString>, filters: &mut Vec<String>) -> Result<bool, Error> {
    if argument == OsStr::new("--filter") {
        filters.push(Arguments::next_string(arguments, "--filter")?);
        Ok(true)
    } else if let Some(value) = option_value(argument, "--filter=") {
        filters.push(value.to_owned());
        Ok(true)
    } else {
        Ok(false)
    }
}

fn selector(argument: &OsStr) -> Option<Mode> {
    match argument.to_str()? {
        "--criterion" => Some(Mode::Criterion),
        "--callgrind" => Some(Mode::Callgrind),
        "--perf" => Some(Mode::Perf),
        "--allocations" => Some(Mode::Allocations),
        _ => None,
    }
}

fn scoped_argument_mode(argument: &OsStr) -> Option<Mode> {
    match argument.to_str()? {
        "--criterion-arg" => Some(Mode::Criterion),
        "--callgrind-arg" => Some(Mode::Callgrind),
        "--perf-arg" => Some(Mode::Perf),
        _ => None,
    }
}

fn scoped_argument_value(argument: &OsStr) -> Option<(Mode, &str)> {
    for (prefix, mode) in [
        ("--criterion-arg=", Mode::Criterion),
        ("--callgrind-arg=", Mode::Callgrind),
        ("--perf-arg=", Mode::Perf),
    ] {
        if let Some(value) = option_value(argument, prefix) {
            return Some((mode, value));
        }
    }
    None
}

fn argument_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Criterion => "--criterion-arg",
        Mode::Callgrind => "--callgrind-arg",
        Mode::Perf => "--perf-arg",
        Mode::Allocations => unreachable!("allocation mode has no passthrough option"),
    }
}

fn push_unique(modes: &mut Vec<Mode>, mode: Mode) {
    if !modes.contains(&mode) {
        modes.push(mode);
    }
}

fn push_argument_unique(arguments: &mut Vec<OsString>, argument: &str) {
    if !arguments.iter().any(|value| value == OsStr::new(argument)) {
        arguments.insert(0, argument.into());
    }
}

fn option_value<'a>(argument: &'a OsStr, prefix: &str) -> Option<&'a str> {
    argument.to_str()?.strip_prefix(prefix)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Arguments, Error> {
        Arguments::parse_from(arguments.iter().map(OsString::from), None)
    }

    #[test]
    fn selects_multiple_engines_and_routes_arguments() {
        let arguments = parse(&[
            "--criterion",
            "--callgrind",
            "--criterion-arg=--sample-size",
            "--criterion-arg",
            "50",
            "--callgrind-arg=--save-summary=json",
        ])
        .expect("arguments should parse");

        assert_eq!(arguments.modes, [Mode::Criterion, Mode::Callgrind]);
        assert_eq!(
            arguments.args_for(Mode::Criterion),
            [OsStr::new("--bench"), OsStr::new("--sample-size"), OsStr::new("50")]
        );
        assert_eq!(arguments.args_for(Mode::Callgrind), [OsStr::new("--save-summary=json")]);
    }

    #[test]
    fn leaves_engine_selection_to_benchmark_code_by_default() {
        let arguments = parse(&[]).expect("arguments should parse");

        assert_eq!(arguments.modes, Mode::ALL);
        assert!(!arguments.explicit_engine_filter);
        assert!(!arguments.behavior.show_engine_output);
        assert_eq!(arguments.args_for(Mode::Criterion), [OsStr::new("--bench")]);
    }

    #[test]
    fn raw_engine_output_is_opt_in() {
        let arguments = parse(&["--show-engine-output"]).expect("output option should parse");

        assert!(arguments.behavior.show_engine_output);
    }

    #[test]
    fn accepts_multiple_benchmark_filters() {
        let arguments = parse(&["--filter", "hash_map/insert/*", "--filter=parsing/?hort"]).expect("filters should parse");

        assert_eq!(arguments.filters, ["hash_map/insert/*", "parsing/?hort"]);
    }

    #[test]
    fn rejects_empty_benchmark_filters() {
        assert!(matches!(
            parse(&["--filter="]),
            Err(Error::InvalidFilter(filter)) if filter.is_empty()
        ));
    }

    #[test]
    fn parses_listing_timeout_and_failure_mode() {
        let arguments = parse(&["--list", "--timeout=5m", "--fail-fast"]).expect("orchestration options should parse");

        assert!(arguments.behavior.list);
        assert_eq!(arguments.timeout, Some(Duration::from_mins(5)));
        assert_eq!(arguments.behavior.failure_mode, FailureMode::FailFast);
    }

    #[test]
    fn output_shortcut_derives_both_report_paths() {
        let arguments = parse(&["--output", "target/results"]).expect("output shortcut should parse");

        assert_eq!(arguments.export_json, Some(PathBuf::from("target/results.json")));
        assert_eq!(arguments.export_markdown, Some(PathBuf::from("target/results.md")));
    }

    #[test]
    fn rejects_conflicting_orchestration_options() {
        assert!(matches!(
            parse(&["--fail-fast", "--keep-going"]),
            Err(Error::ConflictingFailureModes)
        ));
        assert!(matches!(
            parse(&["--output=results", "--export-json=other.json"]),
            Err(Error::ConflictingOutputOptions)
        ));
        assert!(matches!(
            parse(&["--no-output", "--export-md=results.md"]),
            Err(Error::ConflictingOutputOptions)
        ));
    }

    #[test]
    fn accepts_console_only_output_mode() {
        let arguments = parse(&["--no-output"]).expect("no-output option should parse");

        assert!(arguments.behavior.no_output);
        assert!(arguments.export_json.is_none());
        assert!(arguments.export_markdown.is_none());
    }

    #[test]
    fn accepts_no_baseline_mode() {
        let arguments = parse(&["--no-baseline"]).expect("no-baseline option should parse");

        assert!(!arguments.automatic_baseline);
        assert!(arguments.baseline.is_none());
    }

    #[test]
    fn rejects_explicit_and_disabled_baselines() {
        assert!(matches!(
            parse(&["--baseline=previous.json", "--no-baseline"]),
            Err(Error::ConflictingBaselineOptions)
        ));
    }

    #[test]
    fn rejects_arguments_for_an_unselected_engine() {
        assert!(matches!(
            parse(&["--criterion", "--perf-arg=--event=cycles"]),
            Err(Error::ArgumentsForUnselectedEngine("perf"))
        ));
    }

    #[test]
    fn rejects_unscoped_arguments_for_multiple_engines() {
        assert!(matches!(
            parse(&["--criterion", "--callgrind", "--", "--sample-size", "50"]),
            Err(Error::AmbiguousArguments(arguments)) if arguments.len() == 2
        ));
    }

    #[test]
    fn rejects_all_engines_with_individual_selectors() {
        assert!(matches!(parse(&["--all-engines", "--criterion"]), Err(Error::ConflictingModes)));
    }

    #[test]
    fn validates_every_numeric_boundary() {
        for invalid in ["-1", "NaN", "inf", "-inf", "not-a-number"] {
            assert!(
                matches!(
                    parse(&["--regression-threshold", invalid]),
                    Err(Error::InvalidThreshold(value)) if value == invalid
                ),
                "threshold {invalid:?} should fail"
            );
        }
        for invalid in ["0ms", "0s", "1", "1.5s", "18446744073709551615h"] {
            assert!(
                matches!(
                    parse(&["--timeout", invalid]),
                    Err(Error::InvalidTimeout(value)) if value == invalid
                ),
                "timeout {invalid:?} should fail"
            );
        }

        assert!(
            parse(&["--regression-threshold=0"])
                .expect("zero threshold")
                .regression_threshold
                .abs()
                < f64::EPSILON
        );
        for (value, expected) in [
            ("1ms", Duration::from_millis(1)),
            ("2s", Duration::from_secs(2)),
            ("3m", Duration::from_mins(3)),
            ("4h", Duration::from_hours(4)),
        ] {
            assert_eq!(parse(&["--timeout", value]).expect("valid timeout").timeout, Some(expected));
        }
    }

    #[test]
    fn rejects_missing_values_for_every_value_option() {
        for (option, expected) in [
            ("--criterion-arg", "--criterion-arg"),
            ("--callgrind-arg", "--callgrind-arg"),
            ("--perf-arg", "--perf-arg"),
            ("--export-md", "--export-md"),
            ("--export-json", "--export-json"),
            ("--baseline", "--baseline"),
            ("--filter", "--filter"),
            ("--regression-threshold", "--regression-threshold"),
            ("--timeout", "--timeout"),
            ("--output", "--output"),
        ] {
            assert!(
                matches!(
                    parse(&[option]),
                    Err(Error::MissingOptionValue(name)) if name == expected
                ),
                "option {option:?} should require a value"
            );
        }
    }

    #[test]
    fn rejects_unsupported_allocation_passthrough_option() {
        assert!(matches!(
            parse(&["--allocations-arg", "value"]),
            Err(Error::UnknownOption(option)) if option == "--allocations-arg"
        ));
        assert!(matches!(
            parse(&["--allocations-arg=value"]),
            Err(Error::UnknownOption(option)) if option == "--allocations-arg=value"
        ));
    }

    #[test]
    fn environment_engine_is_used_only_without_cli_selection() {
        let from_environment =
            Arguments::parse_from(std::iter::empty::<OsString>(), Some(OsString::from("allocations"))).expect("environment selector");
        assert_eq!(from_environment.modes, [Mode::Allocations]);
        assert!(from_environment.explicit_engine_filter);

        let from_cli = Arguments::parse_from([OsString::from("--criterion")], Some(OsString::from("allocations"))).expect("CLI selector");
        assert_eq!(from_cli.modes, [Mode::Criterion]);
    }

    #[test]
    fn routes_trailing_arguments_for_one_selected_engine() {
        let arguments = parse(&["--perf", "--", "--event", "branches"]).expect("unscoped arguments should be routed");

        assert_eq!(arguments.args_for(Mode::Perf), [OsStr::new("--event"), OsStr::new("branches")]);
    }

    #[test]
    fn duplicate_engine_selectors_preserve_first_selection_order() {
        let arguments = parse(&["--perf", "--criterion", "--perf"]).expect("duplicate selectors should be ignored");

        assert_eq!(arguments.modes, [Mode::Perf, Mode::Criterion]);
    }

    #[test]
    fn bolero_cli_values_are_total() {
        bolero::check!().with_type::<String>().cloned().for_each(|value| {
            let _result = Arguments::parse_from([OsString::from(format!("--timeout={value}"))], None);
        });
    }
}
