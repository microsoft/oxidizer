// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use std::{env, mem};

use crate::error::Error;
use crate::mode::Mode;
use crate::report::DEFAULT_REGRESSION_THRESHOLD;

pub(crate) const WORKER_MODE_ENV: &str = "METABENCH_INTERNAL_WORKER_MODE";
pub(crate) const WORKER_TOKEN_ENV: &str = "METABENCH_INTERNAL_WORKER_TOKEN";

#[derive(Debug, Default)]
struct EngineArguments {
    criterion: Vec<OsString>,
    gungraun: Vec<OsString>,
    perf: Vec<OsString>,
}

impl EngineArguments {
    fn get(&self, mode: Mode) -> &[OsString] {
        match mode {
            Mode::Criterion | Mode::Allocations => &self.criterion,
            Mode::Gungraun => &self.gungraun,
            Mode::Perf => &self.perf,
        }
    }

    fn get_mut(&mut self, mode: Mode) -> &mut Vec<OsString> {
        match mode {
            Mode::Criterion | Mode::Allocations => &mut self.criterion,
            Mode::Gungraun => &mut self.gungraun,
            Mode::Perf => &mut self.perf,
        }
    }
}

#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent CLI switches are represented directly")]
pub(crate) struct Arguments {
    pub(crate) modes: Vec<Mode>,
    pub(crate) help: bool,
    pub(crate) list: bool,
    pub(crate) show_engine_output: bool,
    pub(crate) failure_mode: FailureMode,
    pub(crate) no_output: bool,
    pub(crate) export_markdown: Option<PathBuf>,
    pub(crate) export_json: Option<PathBuf>,
    pub(crate) baseline: Option<PathBuf>,
    pub(crate) automatic_baseline: bool,
    pub(crate) regression_threshold: f64,
    pub(crate) timeout: Option<Duration>,
    native_args: EngineArguments,
    trailing: Vec<OsString>,
    cargo_bench: bool,
    cargo_test: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailureMode {
    KeepGoing,
    FailFast,
}

impl Arguments {
    pub(crate) fn parse() -> Result<Self, Error> {
        Self::parse_from(env::args_os().skip(1), env::var_os("BENCH_ENGINE"))
    }

    pub(crate) fn args_for(&self, mode: Mode) -> &[OsString] {
        self.native_args.get(mode)
    }

    pub(crate) fn criterion_args(&self) -> &[OsString] {
        &self.native_args.criterion
    }

    pub(crate) fn finalize_native_args(&mut self, modes: &[Mode]) -> Result<(), Error> {
        if !self.trailing.is_empty() {
            let [mode] = modes else {
                return Err(Error::AmbiguousArguments(mem::take(&mut self.trailing)));
            };
            self.native_args.get_mut(*mode).append(&mut self.trailing);
        }
        if !self.native_args.criterion.is_empty()
            && !modes.contains(&Mode::Criterion)
            && !modes.contains(&Mode::Allocations)
            && !modes.contains(&Mode::Perf)
        {
            return Err(Error::ArgumentsForUnselectedEngine(Mode::Criterion.as_str()));
        }
        if !self.native_args.gungraun.is_empty() && !modes.contains(&Mode::Gungraun) {
            return Err(Error::ArgumentsForUnselectedEngine(Mode::Gungraun.as_str()));
        }
        if !self.native_args.perf.is_empty() && !modes.contains(&Mode::Perf) {
            return Err(Error::ArgumentsForUnselectedEngine(Mode::Perf.as_str()));
        }
        if modes.contains(&Mode::Criterion) || modes.contains(&Mode::Allocations) || modes.contains(&Mode::Perf) {
            if self.cargo_bench || !self.cargo_test {
                push_argument_unique(&mut self.native_args.criterion, "--bench");
            }
            if self.cargo_test {
                push_argument_unique(&mut self.native_args.criterion, "--test");
            }
            if self.list {
                push_argument_unique(&mut self.native_args.criterion, "--list");
            }
        }
        if modes.contains(&Mode::Gungraun) {
            if self.cargo_bench {
                push_argument_unique(&mut self.native_args.gungraun, "--bench");
            }
            if self.cargo_test {
                push_argument_unique(&mut self.native_args.gungraun, "--test");
            }
            if self.list {
                push_argument_unique(&mut self.native_args.gungraun, "--list");
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "single-pass parsing preserves native argument order")]
    pub(crate) fn parse_from(arguments: impl IntoIterator<Item = OsString>, environment_engine: Option<OsString>) -> Result<Self, Error> {
        let mut selected = Vec::new();
        let mut all_engines = false;
        let mut help = false;
        let mut list = false;
        let mut show_engine_output = false;
        let mut failure_mode = None;
        let mut no_output = false;
        let mut output_base = None;
        let mut export_markdown = None;
        let mut export_json = None;
        let mut baseline = None;
        let mut automatic_baseline = true;
        let mut regression_threshold = DEFAULT_REGRESSION_THRESHOLD;
        let mut timeout = None;
        let mut engine_arguments = EngineArguments::default();
        let mut trailing = Vec::new();
        let mut cargo_bench = false;
        let mut cargo_test = false;
        let mut arguments = arguments.into_iter();

        while let Some(argument) = arguments.next() {
            if argument == OsStr::new("--") {
                trailing.extend(arguments);
                break;
            }
            match argument.to_str() {
                Some("--criterion") => push_unique(&mut selected, Mode::Criterion),
                Some("--gungraun") => push_unique(&mut selected, Mode::Gungraun),
                Some("--perf") => push_unique(&mut selected, Mode::Perf),
                Some("--allocations") => push_unique(&mut selected, Mode::Allocations),
                Some("--all-engines") => all_engines = true,
                Some("--help" | "-h") => help = true,
                Some("--list") => list = true,
                Some("--show-engine-output") => show_engine_output = true,
                Some("--fail-fast") => set_failure_mode(&mut failure_mode, FailureMode::FailFast)?,
                Some("--keep-going") => set_failure_mode(&mut failure_mode, FailureMode::KeepGoing)?,
                Some("--no-output") => {
                    if no_output {
                        return Err(Error::ConflictingOutputOptions);
                    }
                    no_output = true;
                }
                Some("--no-baseline") => automatic_baseline = false,
                Some("--bench") => cargo_bench = true,
                Some("--test") => cargo_test = true,
                Some("--criterion-arg") => engine_arguments
                    .criterion
                    .push(arguments.next().ok_or(Error::MissingOptionValue("--criterion-arg"))?),
                Some("--gungraun-arg") => engine_arguments
                    .gungraun
                    .push(arguments.next().ok_or(Error::MissingOptionValue("--gungraun-arg"))?),
                Some("--perf-arg") => engine_arguments
                    .perf
                    .push(arguments.next().ok_or(Error::MissingOptionValue("--perf-arg"))?),
                Some("--export-md") => export_markdown = Some(next_path(&mut arguments, "--export-md")?),
                Some("--export-json") => export_json = Some(next_path(&mut arguments, "--export-json")?),
                Some("--output") => output_base = Some(next_path(&mut arguments, "--output")?),
                Some("--baseline") => baseline = Some(next_path(&mut arguments, "--baseline")?),
                Some("--timeout") => {
                    timeout = Some(parse_duration(&arguments.next().ok_or(Error::MissingOptionValue("--timeout"))?)?);
                }
                Some("--regression-threshold") => {
                    regression_threshold = parse_threshold(&arguments.next().ok_or(Error::MissingOptionValue("--regression-threshold"))?)?;
                }
                Some(value) if value.starts_with("--criterion-arg=") => {
                    engine_arguments.criterion.push(value["--criterion-arg=".len()..].into());
                }
                Some(value) if value.starts_with("--gungraun-arg=") => {
                    engine_arguments.gungraun.push(value["--gungraun-arg=".len()..].into());
                }
                Some(value) if value.starts_with("--perf-arg=") => {
                    engine_arguments.perf.push(value["--perf-arg=".len()..].into());
                }
                Some(value) if value.starts_with("--export-md=") => {
                    export_markdown = Some(value["--export-md=".len()..].into());
                }
                Some(value) if value.starts_with("--export-json=") => {
                    export_json = Some(value["--export-json=".len()..].into());
                }
                Some(value) if value.starts_with("--output=") => {
                    output_base = Some(value["--output=".len()..].into());
                }
                Some(value) if value.starts_with("--baseline=") => {
                    baseline = Some(value["--baseline=".len()..].into());
                }
                Some(value) if value.starts_with("--timeout=") => {
                    timeout = Some(parse_duration(OsStr::new(&value["--timeout=".len()..]))?);
                }
                Some(value) if value.starts_with("--regression-threshold=") => {
                    regression_threshold = parse_threshold(OsStr::new(&value["--regression-threshold=".len()..]))?;
                }
                _ => return Err(Error::UnknownOption(argument)),
            }
        }

        if all_engines && !selected.is_empty() {
            return Err(Error::ConflictingModes);
        }
        if !automatic_baseline && baseline.is_some() {
            return Err(Error::ConflictingBaselineOptions);
        }
        if no_output && (output_base.is_some() || export_markdown.is_some() || export_json.is_some()) {
            return Err(Error::ConflictingOutputOptions);
        }
        if let Some(base) = output_base {
            if export_markdown.is_some() || export_json.is_some() {
                return Err(Error::ConflictingOutputOptions);
            }
            export_markdown = Some(base.with_extension("md"));
            export_json = Some(base.with_extension("json"));
        }
        if let (Some(markdown), Some(json)) = (&export_markdown, &export_json)
            && paths_alias(markdown, json)
        {
            return Err(Error::ConflictingOutputOptions);
        }

        if selected.is_empty()
            && !all_engines
            && let Some(value) = environment_engine
        {
            selected.push(Mode::from_os_str(&value)?);
        }
        let modes = if all_engines {
            Mode::ALL.to_vec()
        } else if selected.is_empty() {
            Mode::default_set()
        } else {
            selected
        };
        Ok(Self {
            modes,
            help,
            list,
            show_engine_output,
            failure_mode: failure_mode.unwrap_or(FailureMode::KeepGoing),
            no_output,
            export_markdown,
            export_json,
            baseline,
            automatic_baseline,
            regression_threshold,
            timeout,
            native_args: engine_arguments,
            trailing,
            cargo_bench,
            cargo_test,
        })
    }
}

pub(crate) fn paths_alias(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left_metadata), Ok(right_metadata)) = (left.metadata(), right.metadata())
        && metadata_identifies_same_file(&left_metadata, &right_metadata)
    {
        return true;
    }
    resolved_path(left)
        .zip(resolved_path(right))
        .is_some_and(|(left, right)| left == right)
}

fn resolved_path(path: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir().ok()?.join(path)
    };
    let normalized = normalize_path(&absolute);
    let mut existing = normalized.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        suffix.push(existing.file_name()?.to_owned());
        existing = existing.parent()?;
    }
    let mut resolved = existing.canonicalize().ok()?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Some(normalize_path(&resolved))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normalized.file_name().is_some() => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(unix)]
fn metadata_identifies_same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn metadata_identifies_same_file(_left: &std::fs::Metadata, _right: &std::fs::Metadata) -> bool {
    false
}

fn next_path(arguments: &mut impl Iterator<Item = OsString>, option: &'static str) -> Result<PathBuf, Error> {
    arguments.next().map(PathBuf::from).ok_or(Error::MissingOptionValue(option))
}

fn set_failure_mode(current: &mut Option<FailureMode>, requested: FailureMode) -> Result<(), Error> {
    if current.is_some_and(|value| value != requested) {
        Err(Error::ConflictingFailureModes)
    } else {
        *current = Some(requested);
        Ok(())
    }
}

fn push_unique(values: &mut Vec<Mode>, value: Mode) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn push_argument_unique(arguments: &mut Vec<OsString>, argument: &str) {
    if !arguments.iter().any(|current| current == OsStr::new(argument)) {
        arguments.push(argument.into());
    }
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

fn parse_duration(value: &OsStr) -> Result<Duration, Error> {
    let text = value.to_string_lossy();
    let (number, multiplier) = if let Some(number) = text.strip_suffix("ms") {
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
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::InvalidTimeout(text.into_owned()));
    }
    let amount = number
        .parse::<u64>()
        .map_err(|_error| Error::InvalidTimeout(text.clone().into_owned()))?;
    let milliseconds = amount
        .checked_mul(multiplier)
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::InvalidTimeout(text.into_owned()))?;
    Ok(Duration::from_millis(milliseconds))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str], environment: Option<&str>) -> Result<Arguments, Error> {
        Arguments::parse_from(arguments.iter().map(OsString::from), environment.map(OsString::from))
    }

    fn error(arguments: &[&str]) -> String {
        parse(arguments, None).unwrap_err().to_string()
    }

    fn assert_float_eq(actual: f64, expected: f64) {
        assert!((actual - expected).abs() <= f64::EPSILON);
    }

    #[test]
    fn parses_native_engine_selection_and_arguments() {
        let mut arguments = Arguments::parse_from(
            [
                "--criterion".into(),
                "--criterion-arg".into(),
                "--sample-size".into(),
                "--criterion-arg=20".into(),
            ],
            None,
        )
        .unwrap();
        arguments.finalize_native_args(&[Mode::Criterion]).unwrap();

        assert_eq!(arguments.modes, [Mode::Criterion]);
        assert!(arguments.args_for(Mode::Criterion).contains(&"--bench".into()));
        assert!(arguments.args_for(Mode::Criterion).contains(&"--sample-size".into()));
    }

    #[test]
    fn parses_gungraun_selection() {
        let arguments = Arguments::parse_from(["--gungraun".into(), "--gungraun-arg".into(), "--baseline".into()], None).unwrap();
        assert_eq!(arguments.modes, [Mode::Gungraun]);
        assert_eq!(arguments.args_for(Mode::Gungraun), ["--baseline"]);
    }

    #[test]
    fn parses_perf_selection_and_arguments() {
        let mut arguments = Arguments::parse_from(["--perf".into(), "--perf-arg".into(), "--event=cpu-clock".into()], None).unwrap();
        arguments.finalize_native_args(&[Mode::Perf]).unwrap();

        assert_eq!(arguments.modes, [Mode::Perf]);
        assert_eq!(arguments.args_for(Mode::Perf), ["--event=cpu-clock"]);
        assert_eq!(arguments.criterion_args(), ["--bench"]);
    }

    #[test]
    fn rejects_removed_valgrind_selector() {
        assert!(matches!(
            Arguments::parse_from(["--valgrind".into()], None),
            Err(Error::UnknownOption(_))
        ));
    }

    #[test]
    fn defaults_exclude_opt_in_perf_and_keep_going() {
        let arguments = parse(&[], None).unwrap();

        assert_eq!(arguments.modes, Mode::default_set());
        assert_eq!(arguments.failure_mode, FailureMode::KeepGoing);
        assert!(!arguments.help);
        assert!(!arguments.list);
        assert!(!arguments.show_engine_output);
        assert!(!arguments.no_output);
        assert!(arguments.export_markdown.is_none());
        assert!(arguments.export_json.is_none());
        assert!(arguments.baseline.is_none());
        assert!(arguments.timeout.is_none());
        assert!(arguments.automatic_baseline);
        assert_float_eq(arguments.regression_threshold, DEFAULT_REGRESSION_THRESHOLD);
    }

    #[test]
    fn environment_selects_one_engine() {
        assert_eq!(parse(&[], Some("criterion")).unwrap().modes, [Mode::Criterion]);
        assert_eq!(parse(&[], Some("perf")).unwrap().modes, [Mode::Perf]);
        assert_eq!(parse(&[], Some("allocations")).unwrap().modes, [Mode::Allocations]);
        assert!(matches!(parse(&[], Some("unknown")), Err(Error::InvalidMode(_))));
    }

    #[test]
    fn explicit_selector_takes_precedence_over_environment() {
        assert_eq!(parse(&["--gungraun"], Some("criterion")).unwrap().modes, [Mode::Gungraun]);
    }

    #[test]
    fn rejects_conflicting_modes_and_failure_modes() {
        assert_eq!(
            error(&["--all-engines", "--criterion"]),
            "--all-engines cannot be combined with individual engine selectors"
        );
        assert_eq!(
            error(&["--fail-fast", "--keep-going"]),
            "--fail-fast cannot be combined with --keep-going"
        );
    }

    #[test]
    fn rejects_conflicting_baseline_and_output_options() {
        assert!(matches!(
            parse(&["--baseline", "old.json", "--no-baseline"], None),
            Err(Error::ConflictingBaselineOptions)
        ));
        for options in [
            &["--no-output", "--output", "report"][..],
            &["--output", "report", "--export-json", "other.json"],
            &["--export-json", "report", "--export-md", "report"],
        ] {
            assert!(matches!(parse(options, None), Err(Error::ConflictingOutputOptions)));
        }
    }

    #[test]
    fn derives_output_extensions() {
        for options in [&["--output", "reports/latest"][..], &["--output=reports/latest"][..]] {
            let arguments = parse(options, None).unwrap();
            assert_eq!(arguments.export_json, Some(PathBuf::from("reports/latest.json")));
            assert_eq!(arguments.export_markdown, Some(PathBuf::from("reports/latest.md")));
        }
    }

    #[test]
    fn parses_every_value_option_in_separate_and_inline_forms() {
        for options in [
            vec!["--export-md", "report.md"],
            vec!["--export-md=report.md"],
            vec!["--export-json", "report.json"],
            vec!["--export-json=report.json"],
            vec!["--baseline", "before.json"],
            vec!["--baseline=before.json"],
            vec!["--timeout", "2s"],
            vec!["--timeout=2s"],
            vec!["--regression-threshold", "2.5"],
            vec!["--regression-threshold=2.5"],
        ] {
            let arguments = parse(&options, None).unwrap();
            if options[0].starts_with("--export-md") {
                assert_eq!(arguments.export_markdown.as_deref(), Some(Path::new("report.md")));
            } else if options[0].starts_with("--export-json") {
                assert_eq!(arguments.export_json.as_deref(), Some(Path::new("report.json")));
            } else if options[0].starts_with("--baseline") {
                assert_eq!(arguments.baseline.as_deref(), Some(Path::new("before.json")));
            } else if options[0].starts_with("--timeout") {
                assert_eq!(arguments.timeout, Some(Duration::from_secs(2)));
            } else {
                assert_float_eq(arguments.regression_threshold, 2.5);
            }
        }
    }

    #[test]
    fn parses_switches_and_repeated_selectors() {
        let arguments = parse(
            &[
                "--criterion",
                "--criterion",
                "-h",
                "--list",
                "--show-engine-output",
                "--fail-fast",
                "--no-output",
                "--no-baseline",
            ],
            None,
        )
        .unwrap();

        assert_eq!(arguments.modes, [Mode::Criterion]);
        assert!(arguments.help);
        assert!(arguments.list);
        assert!(arguments.show_engine_output);
        assert_eq!(arguments.failure_mode, FailureMode::FailFast);
        assert!(arguments.no_output);
        assert!(!arguments.automatic_baseline);
    }

    #[test]
    fn reports_exact_missing_value_and_unknown_option_errors() {
        for option in [
            "--criterion-arg",
            "--gungraun-arg",
            "--export-md",
            "--export-json",
            "--output",
            "--baseline",
            "--timeout",
            "--regression-threshold",
        ] {
            assert_eq!(error(&[option]), format!("{option} requires a value"));
        }
        assert_eq!(
            error(&["--native-option"]),
            "unknown metabench option --native-option; use --criterion-arg, --gungraun-arg, or --perf-arg for native options"
        );
    }

    #[test]
    fn validates_timeout_and_threshold_boundaries() {
        for timeout in ["0ms", "1", "-1s", "18446744073709551615h"] {
            assert!(matches!(parse(&["--timeout", timeout], None), Err(Error::InvalidTimeout(_))));
        }
        assert_eq!(parse(&["--timeout", "1ms"], None).unwrap().timeout, Some(Duration::from_millis(1)));
        for threshold in ["-1", "NaN", "inf"] {
            assert!(matches!(
                parse(&["--regression-threshold", threshold], None),
                Err(Error::InvalidThreshold(_))
            ));
        }
        assert_float_eq(parse(&["--regression-threshold", "0"], None).unwrap().regression_threshold, 0.0);
    }

    #[test]
    fn forwards_trailing_arguments_only_to_one_engine() {
        let mut arguments = parse(&["--criterion", "--", "--sample-size", "10"], None).unwrap();
        arguments.finalize_native_args(&[Mode::Criterion]).unwrap();
        assert_eq!(arguments.args_for(Mode::Criterion), ["--sample-size", "10", "--bench"]);

        let mut arguments = parse(&["--", "--sample-size"], None).unwrap();
        assert!(matches!(
            arguments.finalize_native_args(&Mode::ALL),
            Err(Error::AmbiguousArguments(_))
        ));
    }

    #[test]
    fn rejects_arguments_for_unselected_engine() {
        let mut arguments = parse(&["--criterion", "--gungraun-arg", "--baseline"], None).unwrap();
        assert!(matches!(
            arguments.finalize_native_args(&[Mode::Criterion]),
            Err(Error::ArgumentsForUnselectedEngine("gungraun"))
        ));
    }

    #[test]
    fn cargo_flags_and_listing_are_forwarded_once() {
        let mut arguments = parse(&["--criterion", "--bench", "--test", "--list", "--criterion-arg=--bench"], None).unwrap();
        arguments.finalize_native_args(&[Mode::Criterion]).unwrap();

        assert_eq!(arguments.args_for(Mode::Criterion), ["--bench", "--test", "--list"]);
    }

    #[test]
    fn native_routing_matrix_matches_engine_contracts() {
        struct Case {
            options: &'static [&'static str],
            modes: &'static [Mode],
            criterion: &'static [&'static str],
            gungraun: &'static [&'static str],
        }
        for case in [
            Case {
                options: &["--criterion"],
                modes: &[Mode::Criterion],
                criterion: &["--bench"],
                gungraun: &[],
            },
            Case {
                options: &["--criterion", "--test"],
                modes: &[Mode::Criterion],
                criterion: &["--test"],
                gungraun: &[],
            },
            Case {
                options: &["--gungraun", "--bench", "--list"],
                modes: &[Mode::Gungraun],
                criterion: &[],
                gungraun: &["--bench", "--list"],
            },
            Case {
                options: &["--all-engines", "--test"],
                modes: &Mode::ALL,
                criterion: &["--test"],
                gungraun: &["--test"],
            },
        ] {
            let mut arguments = parse(case.options, None).unwrap();
            arguments.finalize_native_args(case.modes).unwrap();
            assert_eq!(arguments.args_for(Mode::Criterion), case.criterion);
            assert_eq!(arguments.args_for(Mode::Gungraun), case.gungraun);
        }
    }

    #[test]
    fn generated_duration_inputs_have_contract_oracles() {
        for (suffix, multiplier) in [("ms", 1_u64), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)] {
            for amount in [1_u64, 2, 17, 999] {
                let value = format!("{amount}{suffix}");
                assert_eq!(
                    parse(&["--timeout", &value], None).unwrap().timeout,
                    Some(Duration::from_millis(amount * multiplier))
                );
            }
        }
        for value in ["", "0", "0s", "1.0s", " 1s", "1S", "+1s", "1seconds"] {
            assert_eq!(
                error(&["--timeout", value]),
                format!("invalid worker timeout '{value}'; expected a positive integer followed by ms, s, m, or h")
            );
        }
    }

    #[test]
    fn path_aliases_include_lexical_canonical_and_hard_link_aliases() {
        let root = tempfile::tempdir().unwrap();
        let report = root.path().join("report.json");
        std::fs::write(&report, "{}").unwrap();

        assert!(paths_alias(&report, &root.path().join(".").join("report.json")));
        #[cfg(unix)]
        {
            let link = root.path().join("hard-link.json");
            std::fs::hard_link(&report, &link).unwrap();
            assert!(paths_alias(&report, &link));
        }
        assert!(!paths_alias(&report, &root.path().join("other.json")));
    }

    #[test]
    fn generated_argument_streams_are_deterministic_and_preserve_invariants() {
        const OPTIONS: [&str; 13] = [
            "--criterion",
            "--gungraun",
            "--allocations",
            "--all-engines",
            "--help",
            "--list",
            "--show-engine-output",
            "--fail-fast",
            "--keep-going",
            "--no-output",
            "--no-baseline",
            "--bench",
            "--test",
        ];

        bolero::check!().with_type::<Vec<u8>>().for_each(|input| {
            let values = input
                .iter()
                .map(|byte| OsString::from(OPTIONS[usize::from(*byte) % OPTIONS.len()]))
                .collect::<Vec<_>>();
            let first = Arguments::parse_from(values.clone(), None);
            let second = Arguments::parse_from(values, None);
            assert_eq!(format!("{first:?}"), format!("{second:?}"));
            if let Ok(arguments) = first {
                assert!(!arguments.modes.is_empty());
                assert!(arguments.regression_threshold.is_finite());
                assert!(arguments.regression_threshold >= 0.0);
                assert!(arguments.timeout.is_none_or(|timeout| !timeout.is_zero()));
            }
        });
    }
}
