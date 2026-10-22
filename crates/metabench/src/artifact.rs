// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Debug;
use std::fs::{File, OpenOptions};
use std::hash::Hash;
use std::io::{self, BufWriter, Read as _, Write as _};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{self, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use either_or_both::EitherOrBoth;
use gungraun_runner::api::Tool;
use gungraun_runner::metrics::model::{Metric as GungraunMetric, MetricResults};
use gungraun_runner::summary::model::{BenchmarkSummary, SCHEMA_VERSION, ToolMetricResults};
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::identity::BenchmarkIdentity;
use crate::report::{Metric, MetricDirection, NativeResult};

const MAX_ARTIFACT_DIRECTORY_DEPTH: usize = 64;
const MAX_ARTIFACT_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARTIFACT_DIRECTORIES: usize = 4_096;
const MAX_ARTIFACT_FILES: usize = 10_000;
const RETAINED_RUN_DIRECTORIES: usize = 20;
const ACTIVE_RUN_MARKER: &str = ".metabench-active";
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct ArtifactDirectory {
    root: PathBuf,
    run: PathBuf,
    criterion: PathBuf,
    gungraun: PathBuf,
    _active_locks: Vec<File>,
}

#[derive(Deserialize, Serialize)]
struct EngineRunManifest {
    target: String,
    engine: String,
    command_line: Vec<String>,
    engine_version: Option<String>,
    artifact_root: String,
    success: bool,
    exit_code: Option<i32>,
    discovered_native_identities: Vec<String>,
    #[serde(default)]
    native_to_canonical: BTreeMap<String, String>,
}

impl ArtifactDirectory {
    pub(crate) fn create(root: &Path) -> Result<Self, Error> {
        Self::create_with_homes(root, env::var_os("CRITERION_HOME"), env::var_os("GUNGRAUN_HOME"))
    }

    fn create_with_homes(root: &Path, criterion_home: Option<OsString>, gungraun_home: Option<OsString>) -> Result<Self, Error> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| Error::Metadata(error.to_string()))?
            .as_nanos();
        let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let run_name = format!("{timestamp}-{}-{sequence}", process::id());
        let runs = root.join("runs");
        let run = runs.join(&run_name);
        let criterion_runs = criterion_home.map(|home| PathBuf::from(home).join(".metabench-runs"));
        let gungraun_runs = gungraun_home.map(|home| PathBuf::from(home).join(".metabench-runs"));
        let criterion = criterion_runs
            .as_ref()
            .map_or_else(|| run.join("criterion"), |runs| runs.join(&run_name));
        let gungraun = gungraun_runs
            .as_ref()
            .map_or_else(|| run.join("gungraun"), |runs| runs.join(&run_name));
        Self::create_private_run(&run)?;
        let mut marked_runs = BTreeSet::from([run.clone()]);
        let mut active_locks = vec![mark_run_active(&run)?];
        Self::prune_owned_runs(&runs, &run)?;
        for (owned_runs, current) in [
            criterion_runs.as_deref().map(|runs| (runs, criterion.as_path())),
            gungraun_runs.as_deref().map(|runs| (runs, gungraun.as_path())),
        ]
        .into_iter()
        .flatten()
        {
            if marked_runs.insert(current.to_owned()) {
                Self::create_private_run(current)?;
                active_locks.push(mark_run_active(current)?);
            }
            Self::prune_owned_runs(owned_runs, current)?;
        }
        Ok(Self {
            root: root.to_owned(),
            run,
            criterion,
            gungraun,
            _active_locks: active_locks,
        })
    }

    fn create_private_run(path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
                path: parent.to_owned(),
                source,
            })?;
        }
        let directory = fs::DirBuilder::new();
        #[cfg(unix)]
        let directory = {
            let mut directory = directory;
            directory.mode(0o700);
            directory
        };
        directory.create(path).map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        })
    }

    fn prune_owned_runs(runs: &Path, current: &Path) -> Result<(), Error> {
        let mut directories = fs::read_dir(runs)
            .map_err(|source| Error::ArtifactIo {
                path: runs.to_owned(),
                source,
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|file_type| file_type.is_dir() && !file_type.is_symlink())
                    .map(|_| entry.path())
            })
            .collect::<Vec<_>>();
        directories.sort();
        let mut remove_count = directories.len().saturating_sub(RETAINED_RUN_DIRECTORIES);
        for directory in directories {
            if remove_count == 0 {
                break;
            }
            if directory != current && !run_may_be_active(&directory) {
                match fs::remove_dir_all(&directory) {
                    Ok(()) => remove_count -= 1,
                    Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => return Err(Error::ArtifactIo { path: directory, source }),
                }
            }
        }
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.run
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn criterion_path(&self) -> PathBuf {
        self.criterion.clone()
    }

    pub(crate) fn gungraun_path(&self) -> PathBuf {
        self.gungraun.clone()
    }

    pub(crate) fn perf_path(&self, index: usize) -> PathBuf {
        self.run.join("perf").join(format!("{index}.jsonl"))
    }

    pub(crate) fn vtune_result_path(&self, index: usize) -> PathBuf {
        self.run.join("vtune-results").join(index.to_string())
    }

    pub(crate) fn vtune_path(&self, index: usize) -> PathBuf {
        self.run.join("vtune").join(format!("{index}.csv"))
    }
}

pub(crate) fn write_run_manifest(
    manifest_root: &Path,
    artifact_root: &Path,
    target: &str,
    engine: &str,
    arguments: &[OsString],
    status: Option<ExitStatus>,
) -> Result<(), Error> {
    let manifest = EngineRunManifest {
        target: target.to_owned(),
        engine: engine.to_owned(),
        command_line: arguments.iter().map(|argument| argument.to_string_lossy().into_owned()).collect(),
        engine_version: None,
        artifact_root: artifact_root.to_string_lossy().into_owned(),
        success: status.is_some_and(|status| status.success()),
        exit_code: status.and_then(|status| status.code()),
        discovered_native_identities: Vec::new(),
        native_to_canonical: BTreeMap::new(),
    };
    write_json(&manifest_root.join("manifests").join(format!("{engine}.json")), &manifest)
}

pub(crate) fn record_discovered_identities(manifest_root: &Path, engine: &str, results: &[NativeResult]) -> Result<(), Error> {
    let path = manifest_root.join("manifests").join(format!("{engine}.json"));
    let mut manifest = read_json::<EngineRunManifest>(&path)?;
    manifest.discovered_native_identities = results
        .iter()
        .map(|result| result.native_identity.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    manifest.native_to_canonical = results
        .iter()
        .map(|result| (result.native_identity.clone(), result.identity.clone()))
        .collect();
    write_json(&path, &manifest)
}

#[derive(Debug, Deserialize)]
struct CriterionMetadata {
    group_id: String,
    function_id: Option<String>,
    value_str: Option<String>,
    throughput: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CriterionEstimates {
    slope: Option<CriterionEstimate>,
    mean: CriterionEstimate,
}

#[derive(Debug, Deserialize)]
struct CriterionEstimate {
    confidence_interval: CriterionConfidenceInterval,
    point_estimate: f64,
}

#[derive(Debug, Deserialize)]
struct CriterionConfidenceInterval {
    confidence_level: f64,
    lower_bound: f64,
    upper_bound: f64,
}

pub(crate) fn parse_criterion(
    root: &Path,
    artifact_root: &Path,
    identities: &[BenchmarkIdentity],
    unit: Option<&str>,
) -> Result<Vec<NativeResult>, Error> {
    let paths = find_artifact_files(root, |path| {
        path.file_name().is_some_and(|name| name == "benchmark.json")
            && path.parent().and_then(Path::file_name).is_some_and(|name| name == "new")
    })?;
    let identities = CanonicalIdentityIndex::new(identities);
    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = read_json::<CriterionMetadata>(&path)?;
        let native_identity = criterion_identity(&metadata);
        let identity = identities.resolve_criterion(&native_identity);
        let estimates_path = path.with_file_name("estimates.json");
        let estimates = read_json::<CriterionEstimates>(&estimates_path)?;
        let (statistic, estimate) = match estimates.slope {
            Some(slope) => ("slope", slope),
            None => ("mean", estimates.mean),
        };
        validate_criterion_estimate(&estimates_path, &estimate)?;
        let mut metric = Metric::float("criterion", "time", estimate.point_estimate, MetricDirection::LowerIsBetter);
        metric.unit = unit.map(str::to_owned);
        metric.statistic = Some(statistic.to_owned());
        metric.confidence_level = Some(estimate.confidence_interval.confidence_level);
        metric.lower_bound = Some(estimate.confidence_interval.lower_bound);
        metric.upper_bound = Some(estimate.confidence_interval.upper_bound);
        let mut metrics = BTreeMap::from([("time".to_owned(), metric)]);
        if let Some((name, value)) = metadata.throughput.as_ref().and_then(parse_criterion_throughput) {
            metrics.insert(name.clone(), Metric::integer("criterion", &name, value, MetricDirection::Unknown));
        }
        results.push(NativeResult {
            identity,
            native_identity,
            source: "criterion".to_owned(),
            metrics,
            raw_artifacts: vec![
                relative_artifact(artifact_root, &path),
                relative_artifact(artifact_root, &estimates_path),
            ],
        });
    }
    Ok(results)
}

pub(crate) fn parse_gungraun(root: &Path, artifact_root: &Path, identities: &[BenchmarkIdentity]) -> Result<Vec<NativeResult>, Error> {
    let paths = find_artifact_files(root, |path| path.file_name().is_some_and(|name| name == "summary.json"))?;
    let identities = CanonicalIdentityIndex::new(identities);
    let mut results = Vec::new();
    for path in paths {
        let summary = read_json::<BenchmarkSummary>(&path).map_err(|error| match error {
            Error::ArtifactJson { path, source } => Error::ArtifactFormat {
                path,
                message: source.to_string(),
            },
            error => error,
        })?;
        validate_gungraun_summary(&path, &summary)?;

        let native_identity = gungraun_native_identity(&summary);
        let default_identity = gungraun_canonical_identity(&summary);
        let identity = identities.resolve_gungraun(&native_identity, default_identity);
        for profile in &summary.profiles.0 {
            let tool = validate_gungraun_tool(&path, profile.tool)?;
            let source = format!("gungraun.{tool}");
            let metrics = tool_metrics(&source, &profile.data.total.metrics).ok_or_else(|| Error::ArtifactFormat {
                path: path.clone(),
                message: format!("Gungraun {tool} summary contains no benchmark metrics"),
            })?;
            results.push(NativeResult {
                identity: identity.clone(),
                native_identity: native_identity.clone(),
                source,
                metrics,
                raw_artifacts: vec![relative_artifact(artifact_root, &path)],
            });
        }
    }
    Ok(results)
}

#[derive(Deserialize)]
struct PerfStatRecord {
    #[serde(rename = "counter-value")]
    counter_value: serde_json::Value,
    event: String,
}

pub(crate) fn parse_perf(path: &Path, artifact_root: &Path, identity: String, native_identity: String) -> Result<NativeResult, Error> {
    let contents = read_artifact_text(path, "JSON")?;
    let mut metrics = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str::<PerfStatRecord>(line).map_err(|source| Error::ArtifactJson {
            path: path.to_owned(),
            source,
        })?;
        let name = record.event.strip_suffix(":u").unwrap_or(&record.event).to_owned();
        let value = perf_counter_value(&record.counter_value).ok_or_else(|| Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!(
                "line {} event {} has non-numeric counter value {}",
                index + 1,
                record.event,
                record.counter_value
            ),
        })?;
        if !value.is_finite() || value < 0.0 {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("line {} event {} has invalid counter value {value}", index + 1, record.event),
            });
        }
        if metrics
            .insert(name.clone(), Metric::float("perf", &name, value, MetricDirection::LowerIsBetter))
            .is_some()
        {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("perf event {name} occurs more than once"),
            });
        }
    }
    if metrics.is_empty() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "contains no perf event records".to_owned(),
        });
    }
    Ok(NativeResult {
        identity,
        native_identity,
        source: "perf".to_owned(),
        metrics,
        raw_artifacts: vec![relative_artifact(artifact_root, path)],
    })
}

fn perf_counter_value(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(value) => value.as_f64(),
        serde_json::Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

/// Splits one CSV line into its comma-separated fields, honoring `"..."`
/// quoting (including a doubled `""` as an escaped literal quote) so a
/// quoted field such as `"1,234"` is treated as a single field rather than
/// being split on its embedded comma. Unquoted commas always separate
/// fields, matching ordinary CSV.
fn split_csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if in_quotes {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(character);
            }
        } else if character == '"' {
            in_quotes = true;
        } else if character == ',' {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    fields.push(current);
    fields
}

/// The `hw-events` report column holding each event's name, and the column
/// holding the count this crate measures. Real `VTune` reports may include
/// further columns (sample count, events-per-sample, precision, and so on);
/// those are read from the header and ignored rather than assumed absent.
const VTUNE_EVENT_TYPE_COLUMN: &str = "Hardware Event Type";
const VTUNE_EVENT_COUNT_COLUMN: &str = "Hardware Event Count:Self";

/// Parses a `VTune` `-report hw-events -format csv` report into a
/// [`NativeResult`].
///
/// The report is a header row naming its columns, followed by one row per
/// hardware event (see [`crate::vtune`] and `runner::launch_vtune_worker` for
/// how it is produced). This is our best documented understanding of that
/// report's shape and has not been verified against a live `VTune` install in
/// this repository's environment; adjust it here if a real report's columns
/// differ. Only [`VTUNE_EVENT_TYPE_COLUMN`] and [`VTUNE_EVENT_COUNT_COLUMN`]
/// are read; any other columns a real report includes (sample count,
/// events-per-sample, precision, and so on) are ignored rather than assumed
/// absent. Fields are split with [`split_csv_fields`] so a count that groups
/// digits with `,` thousands separators (this too remains unconfirmed
/// against a live install) parses correctly as long as it is quoted per CSV
/// convention, e.g. `INST_RETIRED.ANY,"1,234,567"`.
pub(crate) fn parse_vtune(path: &Path, artifact_root: &Path, identity: String, native_identity: String) -> Result<NativeResult, Error> {
    let contents = read_artifact_text(path, "CSV")?;
    let mut lines = contents.lines().enumerate();
    let (name_column, count_column, expected_field_count) = match lines.next() {
        Some((_, header)) => {
            let header_fields = split_csv_fields(header.trim());
            let name_column = header_fields.iter().position(|field| field.trim() == VTUNE_EVENT_TYPE_COLUMN);
            let count_column = header_fields.iter().position(|field| field.trim() == VTUNE_EVENT_COUNT_COLUMN);
            match (name_column, count_column) {
                (Some(name_column), Some(count_column)) => (name_column, count_column, header_fields.len()),
                _ => {
                    return Err(Error::ArtifactFormat {
                        path: path.to_owned(),
                        message: format!("header is missing the {VTUNE_EVENT_TYPE_COLUMN:?} or {VTUNE_EVENT_COUNT_COLUMN:?} column"),
                    });
                }
            }
        }
        None => {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: "contains no vtune event records".to_owned(),
            });
        }
    };
    let mut metrics = BTreeMap::new();
    for (index, line) in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields = split_csv_fields(line);
        if fields.len() != expected_field_count {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!(
                    "line {} has {} field(s); expected {expected_field_count} matching the header",
                    index + 1,
                    fields.len()
                ),
            });
        }
        let name = fields[name_column].trim().to_owned();
        let value_text = fields[count_column].trim();
        // A quoted count such as `"1,234,567"` protects its thousands
        // separators from being split as CSV field boundaries; strip them
        // here before parsing so the grouped count still parses as a number.
        let stripped_value_text = value_text.replace(',', "");
        let value = stripped_value_text.parse::<f64>().map_err(|_error| Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("line {} event {name} has non-numeric count {value_text}", index + 1),
        })?;
        if !value.is_finite() || value < 0.0 {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("line {} event {name} has invalid count {value}", index + 1),
            });
        }
        // Real `vtune -report hw-events` output can repeat the same event
        // name across multiple rows (for example, one row per core type or
        // per hardware thread group); the report command does not constrain
        // the grouping to a single row per event. Sum repeated rows into one
        // metric instead of treating a second occurrence as malformed input.
        let mut aggregate_error = None;
        metrics
            .entry(name.clone())
            .and_modify(|metric: &mut Metric| {
                let existing = metric.value.as_f64();
                let summed = existing + value;
                if summed.is_finite() {
                    *metric = Metric::float("vtune", &name, summed, MetricDirection::LowerIsBetter);
                } else {
                    // Two individually finite counts can still sum to
                    // infinity; surface that as malformed input rather than
                    // silently storing a non-finite aggregate metric.
                    aggregate_error = Some(Error::ArtifactFormat {
                        path: path.to_owned(),
                        message: format!("line {} event {name} aggregate count {summed} is not finite", index + 1),
                    });
                }
            })
            .or_insert_with(|| Metric::float("vtune", &name, value, MetricDirection::LowerIsBetter));
        if let Some(error) = aggregate_error {
            return Err(error);
        }
    }
    if metrics.is_empty() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "contains no vtune event records".to_owned(),
        });
    }
    Ok(NativeResult {
        identity,
        native_identity,
        source: "vtune".to_owned(),
        metrics,
        raw_artifacts: vec![relative_artifact(artifact_root, path)],
    })
}

fn criterion_identity(metadata: &CriterionMetadata) -> String {
    [
        Some(metadata.group_id.as_str()),
        metadata.function_id.as_deref(),
        metadata.value_str.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("/")
}

fn parse_criterion_throughput(value: &serde_json::Value) -> Option<(String, u64)> {
    let object = value.as_object()?;
    let (kind, value) = object.iter().next()?;
    let value = match value {
        serde_json::Value::Number(value) => value.as_u64()?,
        serde_json::Value::Object(values) if kind == "ElementsAndBytes" => values.get("elements")?.as_u64()?,
        _ => return None,
    };
    Some((format!("declared_throughput.{}", snake_case(kind)), value))
}

fn gungraun_native_identity(summary: &BenchmarkSummary) -> String {
    gungraun_canonical_identity(summary)
}

fn gungraun_canonical_identity(summary: &BenchmarkSummary) -> String {
    match &summary.id {
        Some(id) => format!("{}/{}/{id}", summary.group, summary.function_name),
        _ => format!("{}/{}", summary.group, summary.function_name),
    }
}

struct CanonicalIdentityIndex {
    criterion: BTreeMap<String, String>,
    gungraun: BTreeMap<String, String>,
}

impl CanonicalIdentityIndex {
    fn new(identities: &[BenchmarkIdentity]) -> Self {
        let criterion = identities
            .iter()
            .map(|identity| (identity.criterion_identity(), identity.canonical_identity()))
            .collect();
        let gungraun = identities
            .iter()
            .map(|identity| (identity.gungraun_identity(), identity.canonical_identity()))
            .collect();
        Self { criterion, gungraun }
    }

    fn resolve_criterion(&self, native: &str) -> String {
        self.resolve_registered_criterion(native).unwrap_or_else(|| native.to_owned())
    }

    fn resolve_registered_criterion(&self, native: &str) -> Option<String> {
        Self::resolve(&self.criterion, native)
    }

    fn resolve_gungraun(&self, native: &str, default_identity: String) -> String {
        Self::resolve(&self.gungraun, native).unwrap_or(default_identity)
    }

    fn resolve(index: &BTreeMap<String, String>, native: &str) -> Option<String> {
        let mut candidate = native;
        loop {
            if let Some(canonical) = index.get(candidate) {
                return Some(
                    native
                        .strip_prefix(candidate)
                        .map_or_else(|| canonical.clone(), |suffix| format!("{canonical}{suffix}")),
                );
            }
            let (prefix, _) = candidate.rsplit_once('/')?;
            candidate = prefix;
        }
    }
}

fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Callgrind => "callgrind",
        Tool::Cachegrind => "cachegrind",
        Tool::DHAT => "dhat",
        Tool::Memcheck => "memcheck",
        Tool::Helgrind => "helgrind",
        Tool::DRD => "drd",
        Tool::Massif => "massif",
        Tool::BBV => "bbv",
        Tool::Perf => "perf",
    }
}

const fn is_benchmark_tool(tool: Tool) -> bool {
    matches!(tool, Tool::Callgrind | Tool::Cachegrind | Tool::DHAT)
}

fn validate_gungraun_summary(path: &Path, summary: &BenchmarkSummary) -> Result<(), Error> {
    if summary.version != SCHEMA_VERSION {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("unsupported Gungraun summary schema {}", summary.version),
        });
    }
    if summary.profiles.0.is_empty() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "Gungraun summary contains no supported measurement profiles".to_owned(),
        });
    }
    Ok(())
}

fn validate_gungraun_tool(path: &Path, tool: Tool) -> Result<&'static str, Error> {
    let name = tool_name(tool);
    if is_benchmark_tool(tool) {
        Ok(name)
    } else {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Gungraun {name} is not a supported benchmark measurement tool"),
        })
    }
}

fn tool_metrics(source: &str, summary: &ToolMetricResults) -> Option<BTreeMap<String, Metric>> {
    let metrics = match summary {
        ToolMetricResults::Dhat(metrics) => Some(copy_metrics(source, metrics)),
        ToolMetricResults::Callgrind(metrics) => Some(copy_metrics(source, metrics)),
        ToolMetricResults::Cachegrind(metrics) => Some(copy_metrics(source, metrics)),
        ToolMetricResults::None
        | ToolMetricResults::Memcheck(_)
        | ToolMetricResults::Helgrind(_)
        | ToolMetricResults::DRD(_)
        | ToolMetricResults::Perf(_) => None,
    };
    metrics.filter(|metrics| !metrics.is_empty())
}

fn copy_metrics<K>(source: &str, summary: &MetricResults<K>) -> BTreeMap<String, Metric>
where
    K: Debug + Eq + Hash + Serialize,
{
    summary
        .0
        .iter()
        .filter_map(|(kind, value)| {
            let current = match value.values {
                EitherOrBoth::Left(current) | EitherOrBoth::Both(current, _) => current,
                EitherOrBoth::Right(_) => return None,
            };
            let name = serde_json::to_value(kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("{kind:?}"));
            let direction = if name.ends_with("HitRate") {
                MetricDirection::HigherIsBetter
            } else {
                MetricDirection::LowerIsBetter
            };
            let metric = match current {
                GungraunMetric::Int(value) => Metric::integer(source, &name, value, direction),
                GungraunMetric::Float(value) if value.is_finite() => Metric::float(source, &name, value, direction),
                GungraunMetric::Float(_) => return None,
            };
            Some((name, metric))
        })
        .collect()
}

fn validate_criterion_estimate(path: &Path, estimate: &CriterionEstimate) -> Result<(), Error> {
    let point = estimate.point_estimate;
    let lower = estimate.confidence_interval.lower_bound;
    let upper = estimate.confidence_interval.upper_bound;
    let confidence = estimate.confidence_interval.confidence_level;
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Criterion confidence level must be between zero and one; found {confidence}"),
        })
    } else if !point.is_finite() || !lower.is_finite() || !upper.is_finite() {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "contains a non-finite Criterion estimate".to_owned(),
        })
    } else if point < 0.0 || lower < 0.0 || upper < 0.0 {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Criterion estimates must be non-negative; found {lower}..{point}..{upper}"),
        })
    } else if lower > upper {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Criterion confidence interval bounds must be ordered; found {lower}..{upper}"),
        })
    } else {
        Ok(())
    }
}

fn relative_artifact(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn snake_case(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for (index, character) in value.chars().enumerate() {
        if character.is_uppercase() && index != 0 {
            output.push('_');
        }
        output.extend(character.to_lowercase());
    }
    output
}

fn find_artifact_files(root: &Path, predicate: impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>, Error> {
    find_artifact_files_with_limits(
        root,
        predicate,
        DiscoveryLimits {
            directories: MAX_ARTIFACT_DIRECTORIES,
            artifacts: MAX_ARTIFACT_FILES,
        },
    )
}

#[derive(Clone, Copy)]
struct DiscoveryLimits {
    directories: usize,
    artifacts: usize,
}

fn find_artifact_files_with_limits(root: &Path, predicate: impl Fn(&Path) -> bool, limits: DiscoveryLimits) -> Result<Vec<PathBuf>, Error> {
    if !root.try_exists().map_err(|source| Error::ArtifactIo {
        path: root.to_owned(),
        source,
    })? {
        return Ok(Vec::new());
    }
    let mut pending = vec![(root.to_owned(), 0_usize)];
    let mut paths = Vec::new();
    let mut directory_count = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        directory_count += 1;
        if directory_count > limits.directories {
            return Err(Error::ArtifactFormat {
                path: root.to_owned(),
                message: format!("artifact tree exceeds the limit of {} directories", limits.directories),
            });
        }
        if depth > MAX_ARTIFACT_DIRECTORY_DEPTH {
            return Err(Error::ArtifactFormat {
                path: directory,
                message: "artifact directory nesting is too deep".to_owned(),
            });
        }
        for entry in fs::read_dir(&directory).map_err(|source| Error::ArtifactIo {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| Error::ArtifactIo {
                path: directory.clone(),
                source,
            })?;
            let file_type = entry.file_type().map_err(|source| Error::ArtifactIo {
                path: entry.path(),
                source,
            })?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if directory_count + pending.len() + 1 > limits.directories {
                    return Err(Error::ArtifactFormat {
                        path: root.to_owned(),
                        message: format!("artifact tree exceeds the limit of {} directories", limits.directories),
                    });
                }
                pending.push((entry.path(), depth + 1));
            } else if file_type.is_file() && predicate(&entry.path()) {
                paths.push(entry.path());
                if paths.len() > limits.artifacts {
                    return Err(Error::ArtifactFormat {
                        path: root.to_owned(),
                        message: format!("artifact tree exceeds the limit of {} matching files", limits.artifacts),
                    });
                }
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn run_may_be_active(path: &Path) -> bool {
    let Ok(marker) = OpenOptions::new().read(true).write(true).open(path.join(ACTIVE_RUN_MARKER)) else {
        return false;
    };
    match marker.try_lock() {
        Ok(()) => false,
        Err(_) => true,
    }
}

fn mark_run_active(path: &Path) -> Result<File, Error> {
    fs::create_dir_all(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    let marker = path.join(ACTIVE_RUN_MARKER);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&marker)
        .map_err(|source| Error::ArtifactIo {
            path: marker.clone(),
            source,
        })?;
    file.lock().map_err(|source| Error::ArtifactIo { path: marker, source })?;
    Ok(file)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let bytes = read_regular_file(path, MAX_ARTIFACT_JSON_BYTES, "JSON")?;
    serde_json::from_slice(&bytes).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })
}

fn read_artifact_text(path: &Path, format: &str) -> Result<String, Error> {
    let bytes = read_regular_file(path, MAX_ARTIFACT_JSON_BYTES, format)?;
    String::from_utf8(bytes).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })
}

pub(crate) fn read_regular_file(path: &Path, limit: u64, format: &str) -> Result<Vec<u8>, Error> {
    let file = open_regular_file(path)?;
    read_bounded_file(file, path, limit, format)
}

/// Admits the same regular-file handle later consumed by the bounded reader.
fn open_regular_file(path: &Path) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        // Reject final links and avoid waiting for a FIFO writer before checking the file type.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        // Inspect rather than follow reparse points, denying concurrent writes and path replacement.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 1;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).share_mode(FILE_SHARE_READ);
    }
    let file = options.open(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    #[cfg(windows)]
    {
        reject_windows_reparse_point(path, metadata.file_attributes())?;
    }
    if !metadata.is_file() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "artifact must be a regular file".to_owned(),
        });
    }
    Ok(file)
}

#[cfg(windows)]
fn reject_windows_reparse_point(path: &Path, attributes: u32) -> Result<(), Error> {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "artifact must not be a reparse point".to_owned(),
        });
    }
    Ok(())
}

fn read_bounded_file(file: File, path: &Path, limit: u64, format: &str) -> Result<Vec<u8>, Error> {
    let length = file
        .metadata()
        .map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        })?
        .len();
    if length > limit {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("{format} file is {length} bytes; limit is {limit} bytes"),
        });
    }
    read_bounded_stream(file, path, limit, format)
}

fn read_bounded_stream(reader: impl io::Read, path: &Path, limit: u64, format: &str) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("{format} stream exceeds the {limit}-byte limit"),
        });
    }
    Ok(bytes)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
        path: parent.to_owned(),
        source,
    })?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".metabench-")
        .tempfile_in(parent)
        .map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value).map_err(|source| Error::ArtifactJson {
            path: path.to_owned(),
            source,
        })?;
        writer.flush().map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        })?;
    }
    temporary.as_file().sync_all().map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    temporary.persist(path).map(|_| ()).map_err(|error| Error::ArtifactIo {
        path: path.to_owned(),
        source: error.error,
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use super::*;

    const GUNGRAUN_SUMMARY_FIXTURE: &str = include_str!("../tests/fixtures/artifacts/gungraun/minimal/summary.json");

    #[test]
    fn private_run_creation_creates_parents_and_rejects_existing_paths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runs").join("new-run");
        ArtifactDirectory::create_private_run(&path).unwrap();
        assert!(path.is_dir());
        assert!(matches!(
            ArtifactDirectory::create_private_run(&path),
            Err(Error::ArtifactIo { path: failed, source })
                if failed == path && source.kind() == io::ErrorKind::AlreadyExists
        ));
        let file = directory.path().join("file");
        fs::write(&file, b"preserve me").unwrap();
        assert!(matches!(
            ArtifactDirectory::create_private_run(&file.join("run")),
            Err(Error::ArtifactIo { path: failed, .. }) if failed == file
        ));
        assert_eq!(fs::read(file).unwrap(), b"preserve me");
    }

    #[cfg(windows)]
    #[test]
    fn json_admission_rejects_reparse_attributes_without_symlink_privileges() {
        let path = Path::new("artifact.json");
        for attributes in [0, 0x80, 0x1000] {
            reject_windows_reparse_point(path, attributes).unwrap();
        }
        for attributes in [0x400, 0x480, 0x1400] {
            assert!(matches!(
                reject_windows_reparse_point(path, attributes),
                Err(Error::ArtifactFormat { path: failed, message })
                    if failed == path && message == "artifact must not be a reparse point"
            ));
        }
    }

    #[test]
    fn json_admission_limits_the_opened_stream_and_ignores_path_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.json");
        fs::write(&path, b"null").unwrap();
        let file = open_regular_file(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 4);
        #[cfg(unix)]
        {
            fs::write(&path, b"null      ").unwrap();
            assert!(
                read_bounded_stream(file, &path, 4, "JSON")
                    .unwrap_err()
                    .to_string()
                    .contains("stream exceeds")
            );
        }
        #[cfg(windows)]
        {
            // Admission denies write sharing while the artifact handle is open.
            fs::write(&path, b"null      ").unwrap_err();
            assert_eq!(read_bounded_file(file, &path, 4, "JSON").unwrap(), b"null");
            fs::write(&path, b"null      ").unwrap();
        }
        assert!(
            read_bounded_stream(io::Cursor::new(b"null      "), &path, 4, "JSON")
                .unwrap_err()
                .to_string()
                .contains("stream exceeds")
        );
        assert!(matches!(
            read_bounded_file(open_regular_file(&path).unwrap(), &path, 4, "JSON"),
            Err(Error::ArtifactFormat { .. })
        ));
        fs::write(&path, b"null").unwrap();
        assert_eq!(
            read_bounded_file(open_regular_file(&path).unwrap(), &path, 4, "JSON").unwrap(),
            b"null"
        );
        #[cfg(unix)]
        {
            let file = open_regular_file(&path).unwrap();
            fs::rename(&path, directory.path().join("original.json")).unwrap();
            fs::write(&path, b"not the opened artifact").unwrap();
            assert_eq!(read_bounded_file(file, &path, 4, "JSON").unwrap(), b"null");
        }
    }

    #[test]
    fn json_admission_rejects_oversized_allocation_identities() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("allocation.json");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"{\"identity\":\"").unwrap();
        for _ in 0..(MAX_ARTIFACT_JSON_BYTES / 8_192) {
            file.write_all(&[b'x'; 8_192]).unwrap();
        }
        file.write_all(b"\",\"allocated_bytes\":1,\"allocation_count\":1}").unwrap();
        drop(file);
        assert!(matches!(
            crate::allocation::read_artifact(&path, "native".to_owned(), "allocation.json".to_owned()),
            Err(Error::ArtifactFormat { .. })
        ));
        read_json::<serde_json::Value>(directory.path()).unwrap_err();
    }

    #[test]
    fn hardware_artifact_admission_rejects_invalid_files_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        for csv in [false, true] {
            let parse = if csv { parse_vtune } else { parse_perf };
            let parse = |path: &Path| parse(path, directory.path(), "group/case".into(), "native/case".into());
            parse(directory.path()).unwrap_err();
            fs::write(&path, [0xff]).unwrap();
            assert!(matches!(
                parse(&path),
                Err(Error::ArtifactIo { source, .. }) if source.kind() == io::ErrorKind::InvalidData
            ));
            File::create(&path).unwrap().set_len(MAX_ARTIFACT_JSON_BYTES + 1).unwrap();
            assert!(matches!(
                parse(&path),
                Err(Error::ArtifactFormat { message, .. })
                    if message.starts_with(if csv { "CSV file" } else { "JSON file" })
            ));
            #[cfg(unix)]
            {
                let link = directory.path().join(if csv { "csv-link" } else { "json-link" });
                symlink(&path, &link).unwrap();
                assert!(matches!(parse(&link), Err(Error::ArtifactIo { .. })));
            }
            #[cfg(target_os = "linux")]
            {
                let fifo = directory.path().join(if csv { "csv-fifo" } else { "json-fifo" });
                nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR).unwrap();
                assert!(matches!(
                    parse(&fifo),
                    Err(Error::ArtifactFormat { message, .. }) if message.contains("regular file")
                ));
            }
        }
    }

    #[test]
    fn bounded_hardware_streams_check_growth_and_retain_the_output_format() {
        let path = Path::new("hardware-output");
        for format in ["JSON", "CSV"] {
            assert_eq!(read_bounded_stream(io::Cursor::new(b"data"), path, 4, format).unwrap(), b"data");
            assert!(matches!(
                read_bounded_stream(io::Cursor::new(b"data grows"), path, 4, format),
                Err(Error::ArtifactFormat { message, .. })
                    if message == format!("{format} stream exceeds the 4-byte limit")
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn json_admission_rejects_links_and_fifos_without_waiting_for_a_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.json");
        let link = directory.path().join("link.json");
        fs::write(&path, b"null").unwrap();
        symlink(&path, &link).unwrap();
        read_json::<serde_json::Value>(&link).unwrap_err();
        #[cfg(target_os = "linux")]
        {
            let fifo = directory.path().join("fifo.json");
            nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR).unwrap();
            assert!(
                read_json::<serde_json::Value>(&fifo)
                    .unwrap_err()
                    .to_string()
                    .contains("regular file")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn artifact_run_directories_are_private() {
        let directory = tempfile::tempdir().unwrap();
        let artifacts = ArtifactDirectory::create_with_homes(
            &directory.path().join("target"),
            Some(directory.path().join("criterion").into_os_string()),
            Some(directory.path().join("gungraun").into_os_string()),
        )
        .unwrap();
        for path in [artifacts.path().to_owned(), artifacts.criterion_path(), artifacts.gungraun_path()] {
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn explicit_default_case_is_distinct_and_joins_matching_engine_results() {
        let source: serde_json::Value = read_json(&fixture("gungraun/minimal/summary.json")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut all = Vec::new();
        for id in [None, Some("default")] {
            let path = directory.path().join(if id.is_some() { "named" } else { "unnamed" });
            let mut summary = source.clone();
            summary["id"] = id.into();
            write_json(&path.join("summary.json"), &summary).unwrap();
            let results = parse_gungraun(&path, directory.path(), &[]).unwrap();
            let expected = if id.is_some() {
                "adapter/metabench_adapter/default"
            } else {
                "adapter/metabench_adapter"
            };
            assert_eq!(results[0].native_identity, expected);
            assert_eq!(results[0].identity, expected);
            all.extend(results);
        }
        let mut time = Metric::float("criterion", "ns", 1.0, MetricDirection::LowerIsBetter);
        time.statistic = Some("mean".to_owned());
        time.lower_bound = Some(1.0);
        time.upper_bound = Some(1.0);
        time.confidence_level = Some(0.95);
        all.push(NativeResult {
            identity: "adapter/metabench_adapter/default".to_owned(),
            native_identity: "adapter/metabench_adapter/default".to_owned(),
            source: "criterion".to_owned(),
            metrics: BTreeMap::from([("time".to_owned(), time)]),
            raw_artifacts: Vec::new(),
        });
        let report = crate::report::BenchmarkReport::from_results(all, vec![], String::new()).unwrap();
        assert_eq!(report.entries.len(), 2);
        let named = report.entries.iter().find(|entry| entry.identity.ends_with("/default")).unwrap();
        assert_eq!(named.results.len(), 2);
    }

    #[test]
    fn accepts_only_performance_measurement_tools() {
        for tool in [Tool::Callgrind, Tool::Cachegrind, Tool::DHAT] {
            assert!(is_benchmark_tool(tool));
        }
        for tool in [Tool::Memcheck, Tool::Helgrind, Tool::DRD, Tool::Massif, Tool::BBV, Tool::Perf] {
            assert!(!is_benchmark_tool(tool));
        }
    }

    fn fixture(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/artifacts").join(path)
    }

    fn write_criterion(directory: &Path, point: f64, lower: f64, upper: f64) {
        let new = directory.join("new");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("benchmark.json"), r#"{"group_id":"group","function_id":"bench"}"#).unwrap();
        fs::write(
            new.join("estimates.json"),
            format!(r#"{{"mean":{{"confidence_interval":{{"confidence_level":0.95,"lower_bound":{lower},"upper_bound":{upper}}},"point_estimate":{point}}}}}"#),
        )
        .unwrap();
    }

    #[test]
    fn parses_criterion_schema_mapping_and_provenance() {
        let root = fixture("criterion/minimal");
        let identities = [BenchmarkIdentity::__new("group", "bench", "unused", "unused", true)];
        let results = parse_criterion(&root, &root, &identities, Some("ns")).unwrap();

        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.identity, "group/bench");
        assert_eq!(result.native_identity, "group/bench");
        assert_eq!(result.source, "criterion");
        assert_eq!(result.raw_artifacts, ["new/benchmark.json", "new/estimates.json"]);
        let median = &result.metrics["time"];
        assert_eq!(median.value, crate::report::MetricValue::Float(20.0));
        assert_eq!(median.direction, MetricDirection::LowerIsBetter);
        assert_eq!(median.unit.as_deref(), Some("ns"));
        assert_eq!(median.lower_bound, Some(10.0));
        assert_eq!(median.upper_bound, Some(30.0));
        assert_eq!(median.statistic.as_deref(), Some("mean"));
        assert_eq!(median.confidence_level, Some(0.95));
        assert_eq!(
            result.metrics["declared_throughput.bytes"].value,
            crate::report::MetricValue::Integer(10)
        );
        let index = CanonicalIdentityIndex::new(&identities);
        assert_eq!(index.resolve_criterion("group/bench/case_0"), "group/bench/case_0");
        assert_eq!(index.resolve_criterion("other/bench/case_0"), "other/bench/case_0");
    }

    #[test]
    fn criterion_preserves_primary_estimate_and_its_confidence_interval() {
        let directory = tempfile::tempdir().unwrap();
        write_criterion(directory.path(), 20.0, 10.0, 30.0);
        let path = directory.path().join("new/estimates.json");
        let mut estimates: serde_json::Value = read_json(&path).unwrap();
        estimates["slope"] = serde_json::json!({
            "point_estimate": 100.0,
            "confidence_interval": {"lower_bound": 90.0, "upper_bound": 110.0, "confidence_level": 0.99}
        });
        estimates["median"] = serde_json::json!({
            "point_estimate": 2.0,
            "confidence_interval": {"lower_bound": 1.0, "upper_bound": 3.0, "confidence_level": 0.90}
        });
        write_json(&path, &estimates).unwrap();
        let results = parse_criterion(directory.path(), directory.path(), &[], Some("cycles")).unwrap();
        let time = &results[0].metrics["time"];
        assert_eq!(time.value, crate::report::MetricValue::Float(100.0));
        assert_eq!(time.lower_bound, Some(90.0));
        assert_eq!(time.upper_bound, Some(110.0));
        assert_eq!(time.confidence_level, Some(0.99));
        assert_eq!(time.statistic.as_deref(), Some("slope"));
        assert_eq!(time.unit.as_deref(), Some("cycles"));
        estimates["slope"] = serde_json::Value::Null;
        write_json(&path, &estimates).unwrap();
        let results = parse_criterion(directory.path(), directory.path(), &[], None).unwrap();
        assert_eq!(results[0].metrics["time"].value, crate::report::MetricValue::Float(20.0));
        assert_eq!(results[0].metrics["time"].statistic.as_deref(), Some("mean"));
        assert_eq!(results[0].metrics["time"].lower_bound, Some(10.0));
        assert_eq!(results[0].metrics["time"].upper_bound, Some(30.0));
        assert_eq!(results[0].metrics["time"].confidence_level, Some(0.95));
        assert_eq!(results[0].metrics["time"].unit, None);
        for confidence in [0.0, 1.0, -0.1] {
            estimates["mean"]["confidence_interval"]["confidence_level"] = serde_json::json!(confidence);
            write_json(&path, &estimates).unwrap();
            assert!(matches!(
                parse_criterion(directory.path(), directory.path(), &[], None),
                Err(Error::ArtifactFormat { .. })
            ));
        }
    }

    #[test]
    fn criterion_accepts_zero_and_equal_interval_bounds() {
        for (point, lower, upper) in [(0.0, 0.0, 0.0), (1.0, 0.0, 2.0), (1.0, 1.0, 1.0)] {
            let estimate = CriterionEstimate {
                point_estimate: point,
                confidence_interval: CriterionConfidenceInterval {
                    lower_bound: lower,
                    upper_bound: upper,
                    confidence_level: 0.95,
                },
            };
            validate_criterion_estimate(Path::new("estimates.json"), &estimate).unwrap();
        }
    }

    #[test]
    fn generated_native_identities_resolve_only_on_segment_boundaries() {
        let identities = [BenchmarkIdentity::__new("group", "bench", "native_group", "native_bench", true)];
        let index = CanonicalIdentityIndex::new(&identities);

        bolero::check!().for_each(|case: &[u8]| {
            let case = case.iter().fold(String::new(), |mut output, byte| {
                write!(output, "{byte:02x}").unwrap();
                output
            });
            let criterion = format!("group/bench/{case}");
            let gungraun = format!("native_group/native_bench/{case}");
            assert_eq!(index.resolve_criterion(&criterion), criterion);
            assert_eq!(
                index.resolve_gungraun(&gungraun, "fallback".to_owned()),
                format!("group/bench/{case}")
            );
            assert_eq!(
                index.resolve_gungraun(&format!("native_group/native_benchx/{case}"), "fallback".to_owned()),
                "fallback"
            );
        });
    }

    #[test]
    fn criterion_preserves_unicode_and_optional_components() {
        let unicode = parse_criterion(&fixture("criterion/unicode"), Path::new(""), &[], None).unwrap();
        let missing = parse_criterion(&fixture("criterion/missing_function"), Path::new(""), &[], None).unwrap();
        assert_eq!(unicode[0].native_identity, "grüppe/測試");
        assert_eq!(missing[0].native_identity, "group");
        assert!(
            parse_criterion(&fixture("criterion/empty"), Path::new(""), &[], None)
                .unwrap()
                .is_empty()
        );

        let duplicate = parse_criterion(&fixture("criterion/duplicate"), Path::new(""), &[], None).unwrap();
        assert_eq!(duplicate.len(), 2);
        assert_eq!(duplicate[0].native_identity, duplicate[1].native_identity);
        assert!(duplicate[0].raw_artifacts[0] < duplicate[1].raw_artifacts[0]);
    }

    #[test]
    fn criterion_rejects_malformed_and_impossible_estimates() {
        assert!(matches!(
            parse_criterion(&fixture("criterion/malformed"), Path::new(""), &[], None),
            Err(Error::ArtifactJson { .. })
        ));
        assert!(matches!(
            parse_criterion(&fixture("criterion/invalid_numeric"), Path::new(""), &[], None),
            Err(Error::ArtifactFormat { .. })
        ));

        for (point, lower, upper) in [(-1.0, 0.0, 1.0), (1.0, -1.0, 2.0), (1.0, 3.0, 2.0), (1.0, 0.0, -1.0)] {
            let directory = tempfile::tempdir().unwrap();
            write_criterion(directory.path(), point, lower, upper);
            assert!(matches!(
                parse_criterion(directory.path(), directory.path(), &[], None),
                Err(Error::ArtifactFormat { .. })
            ));
        }
    }

    #[test]
    fn criterion_accepts_bootstrap_intervals_excluding_the_point() {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        for point in [1.0, 4.0] {
            write_criterion(directory.path(), point, 2.0, 3.0);
            let path = directory.path().join("new/estimates.json");
            let mut estimates: serde_json::Value = read_json(&path).unwrap();
            for statistic in ["mean", "slope"] {
                if statistic == "slope" {
                    estimates["slope"] = estimates["mean"].clone();
                    write_json(&path, &estimates).unwrap();
                }
                let results = parse_criterion(directory.path(), directory.path(), &[], None).unwrap();
                let time = &results[0].metrics["time"];
                assert_eq!(time.value, crate::report::MetricValue::Float(point));
                assert_eq!(time.lower_bound, Some(2.0));
                assert_eq!(time.upper_bound, Some(3.0));
                assert_eq!(time.statistic.as_deref(), Some(statistic));
                assert_eq!(time.confidence_level, Some(0.95));
            }
        }
    }

    #[test]
    fn criterion_rejects_non_finite_estimate_components() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for component in 0..4 {
                let mut estimate = CriterionEstimate {
                    point_estimate: 2.0,
                    confidence_interval: CriterionConfidenceInterval {
                        lower_bound: 1.0,
                        upper_bound: 3.0,
                        confidence_level: 0.95,
                    },
                };
                match component {
                    0 => estimate.point_estimate = invalid,
                    1 => estimate.confidence_interval.lower_bound = invalid,
                    2 => estimate.confidence_interval.upper_bound = invalid,
                    _ => estimate.confidence_interval.confidence_level = invalid,
                }
                assert!(matches!(
                    validate_criterion_estimate(Path::new("estimates.json"), &estimate),
                    Err(Error::ArtifactFormat { .. })
                ));
            }
        }
    }

    #[test]
    fn parses_gungraun_schema_and_strips_native_diffs() {
        let mut summary: serde_json::Value = read_json(&fixture("gungraun/minimal/summary.json")).unwrap();
        summary["module_path"] = "unrelated::path".into();
        summary["profiles"][0]["data"]["total"]["metrics"]["Ir"] = serde_json::json!({
            "change": {"diff_pct": "42.0", "factor": "1.42"},
            "values": {"new": 100, "old": 70},
        });
        let directory = tempfile::tempdir().unwrap();
        write_json(&directory.path().join("summary.json"), &summary).unwrap();

        let results = parse_gungraun(directory.path(), directory.path(), &[]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].native_identity, "adapter/metabench_adapter/case_0");
        assert_eq!(results[0].source, "gungraun.callgrind");
        assert_eq!(results[0].raw_artifacts, ["summary.json"]);
        assert_eq!(results[0].metrics["Ir"].value, crate::report::MetricValue::Integer(100));
        assert_eq!(results[0].metrics["Ir"].direction, MetricDirection::LowerIsBetter);
        assert_eq!(results[0].metrics["Ir"].change_percentage, None);
    }

    #[test]
    fn gungraun_preserves_current_metric_types_and_ignores_old_values() {
        let mut summary: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        summary["profiles"][0]["data"]["total"]["metrics"] = serde_json::json!({
            "Ir": {"values": {"new": u64::MAX, "old": 1}},
            "L1HitRate": {"values": {"new": 99.5}},
            "RamHits": {"values": {"old": 5}},
        });
        let summary: BenchmarkSummary = serde_json::from_value(summary).unwrap();

        let metrics = tool_metrics("gungraun.callgrind", &summary.profiles.0[0].data.total.metrics).unwrap();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics["Ir"].value, crate::report::MetricValue::Integer(u64::MAX));
        assert_eq!(metrics["L1HitRate"].value, crate::report::MetricValue::Float(99.5));
        assert_eq!(metrics["L1HitRate"].direction, MetricDirection::HigherIsBetter);
        assert!(!metrics.contains_key("RamHits"));
    }

    #[test]
    fn gungraun_rejects_other_schema_versions_and_missing_profiles() {
        let source: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        let path = Path::new("summary.json");
        for version in ["6", "8"] {
            let mut summary = source.clone();
            summary["version"] = version.into();
            let summary: BenchmarkSummary = serde_json::from_value(summary).unwrap();
            assert!(matches!(
                validate_gungraun_summary(path, &summary).unwrap_err(),
                Error::ArtifactFormat { path: actual_path, message }
                    if actual_path == path && message == format!("unsupported Gungraun summary schema {version}")
            ));
        }
        for profiles in [serde_json::json!([]), serde_json::json!([{"tool": "FutureTool"}])] {
            let mut summary = source.clone();
            summary["profiles"] = profiles;
            let summary: BenchmarkSummary = serde_json::from_value(summary).unwrap();
            assert!(matches!(
                validate_gungraun_summary(path, &summary).unwrap_err(),
                Error::ArtifactFormat { path: actual_path, message }
                    if actual_path == path && message == "Gungraun summary contains no supported measurement profiles"
            ));
        }
    }

    #[test]
    fn gungraun_rejects_unsupported_measurement_tools() {
        let source: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        let path = Path::new("summary.json");
        for tool in ["Memcheck", "Helgrind", "DRD", "Massif", "BBV", "Perf"] {
            let mut summary = source.clone();
            summary["profiles"][0]["tool"] = tool.into();
            summary["profiles"][0]["data"]["total"]["metrics"] = serde_json::json!({});
            let summary: BenchmarkSummary = serde_json::from_value(summary).unwrap();
            assert!(matches!(
                validate_gungraun_tool(path, summary.profiles.0[0].tool).unwrap_err(),
                Error::ArtifactFormat { path: actual_path, message }
                    if actual_path == path
                        && message == format!("Gungraun {} is not a supported benchmark measurement tool", tool.to_ascii_lowercase())
            ));
        }
    }

    #[test]
    fn gungraun_rejects_malformed_input() {
        assert!(matches!(
            parse_gungraun(&fixture("gungraun/malformed"), Path::new(""), &[]),
            Err(Error::ArtifactFormat { .. })
        ));
        assert!(parse_gungraun(&fixture("gungraun/empty"), Path::new(""), &[]).unwrap().is_empty());

        let source = fs::read_to_string(fixture("gungraun/minimal/summary.json")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("summary.json"), source.replace(r#""new""#, r#""old""#)).unwrap();
        assert!(matches!(
            parse_gungraun(directory.path(), directory.path(), &[]),
            Err(Error::ArtifactFormat { message, .. }) if message.contains("contains no benchmark metrics")
        ));

        let duplicate = parse_gungraun(&fixture("gungraun/duplicate"), Path::new(""), &[]).unwrap();
        assert_eq!(duplicate.len(), 2);
        assert!(duplicate[0].raw_artifacts[0] < duplicate[1].raw_artifacts[0]);

        let numeric = parse_gungraun(&fixture("gungraun/invalid_numeric"), Path::new(""), &[]).unwrap();
        assert_eq!(numeric[0].metrics["Ir"].value, crate::report::MetricValue::Float(1.5));

        let wrong_mapping = parse_gungraun(&fixture("gungraun/wrong_mapping"), Path::new(""), &[]).unwrap();
        assert_eq!(wrong_mapping[0].native_identity, "bench/metabench_adapter/case_9");
    }

    #[test]
    fn parses_perf_json_lines_and_preserves_all_events() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("perf.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"counter-value":"1234","event":"instructions:u","event-runtime":10,"pcnt-running":100.0}"#,
                "\n",
                r#"{"counter-value":567.5,"event":"cycles:u","event-runtime":10,"pcnt-running":100.0}"#,
                "\n"
            ),
        )
        .unwrap();

        let result = parse_perf(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()).unwrap();
        assert_eq!(result.source, "perf");
        assert_eq!(result.identity, "group/bench");
        assert_eq!(result.raw_artifacts, ["perf.jsonl"]);
        assert_eq!(result.metrics["instructions"].value, crate::report::MetricValue::Float(1234.0));
        assert_eq!(result.metrics["instructions"].display_name, "HW Instr");
        assert_eq!(result.metrics["cycles"].value, crate::report::MetricValue::Float(567.5));
    }

    #[test]
    fn perf_rejects_unavailable_and_duplicate_counters() {
        for contents in [
            r#"{"counter-value":"<not counted>","event":"instructions:u"}"#.to_owned(),
            concat!(
                r#"{"counter-value":"1","event":"instructions:u"}"#,
                "\n",
                r#"{"counter-value":"2","event":"instructions:u"}"#
            )
            .to_owned(),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("perf.jsonl");
            fs::write(&path, contents).unwrap();
            assert!(matches!(
                parse_perf(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()),
                Err(Error::ArtifactFormat { .. })
            ));
        }
    }

    #[test]
    fn parses_vtune_csv_report_and_preserves_all_events() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            concat!(
                "Hardware Event Type,Hardware Event Count:Self\n",
                "INST_RETIRED.ANY,1234\n",
                "CPU_CLK_UNHALTED.THREAD,567.5\n",
            ),
        )
        .unwrap();

        let result = parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()).unwrap();
        assert_eq!(result.source, "vtune");
        assert_eq!(result.identity, "group/bench");
        assert_eq!(result.raw_artifacts, ["vtune.csv"]);
        assert_eq!(result.metrics["INST_RETIRED.ANY"].value, crate::report::MetricValue::Float(1234.0));
        assert_eq!(
            result.metrics["CPU_CLK_UNHALTED.THREAD"].value,
            crate::report::MetricValue::Float(567.5)
        );
    }

    #[test]
    fn parses_vtune_csv_report_with_additional_columns_by_header_name() {
        // Real `vtune -report hw-events -format csv` output includes further
        // columns (sample count, events-per-sample, precision) beyond the
        // two this crate measures; those must be located by header name and
        // ignored, not assumed absent.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            concat!(
                "Hardware Event Sample Count:Self,Hardware Event Type,Events Per Sample,Hardware Event Count:Self,Precise:Self\n",
                "10,INST_RETIRED.ANY,123.4,1234,Yes\n",
            ),
        )
        .unwrap();

        let result = parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()).unwrap();
        assert_eq!(result.metrics["INST_RETIRED.ANY"].value, crate::report::MetricValue::Float(1234.0));
    }

    #[test]
    fn vtune_rejects_a_header_missing_the_required_columns() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(&path, "Hardware Event Type,Some Other Column\nINST_RETIRED.ANY,1234\n").unwrap();

        assert!(matches!(
            parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()),
            Err(Error::ArtifactFormat { .. })
        ));
    }

    #[test]
    fn vtune_rejects_a_row_whose_field_count_does_not_match_the_header() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            "Hardware Event Type,Hardware Event Count:Self,Events Per Sample\nINST_RETIRED.ANY,1234\n",
        )
        .unwrap();

        assert!(matches!(
            parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()),
            Err(Error::ArtifactFormat { .. })
        ));
    }

    #[test]
    fn parses_vtune_csv_report_with_quoted_thousands_separated_counts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            concat!(
                "Hardware Event Type,Hardware Event Count:Self\n",
                "INST_RETIRED.ANY,\"1,234,567\"\n",
                "CPU_CLK_UNHALTED.THREAD,890\n",
            ),
        )
        .unwrap();

        let result = parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()).unwrap();
        assert_eq!(
            result.metrics["INST_RETIRED.ANY"].value,
            crate::report::MetricValue::Float(1_234_567.0)
        );
        assert_eq!(
            result.metrics["CPU_CLK_UNHALTED.THREAD"].value,
            crate::report::MetricValue::Float(890.0)
        );
    }

    #[test]
    fn vtune_rejects_an_unquoted_grouped_count_as_too_many_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            concat!("Hardware Event Type,Hardware Event Count:Self\n", "INST_RETIRED.ANY,1,234,567\n",),
        )
        .unwrap();

        assert!(matches!(
            parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()),
            Err(Error::ArtifactFormat { .. })
        ));
    }

    #[test]
    fn split_csv_fields_honors_quoting_and_escaped_quotes() {
        assert_eq!(split_csv_fields("a,b,c"), ["a", "b", "c"]);
        assert_eq!(split_csv_fields("a,\"1,234\",c"), ["a", "1,234", "c"]);
        assert_eq!(split_csv_fields("a,\"say \"\"hi\"\"\",c"), ["a", "say \"hi\"", "c"]);
    }

    #[test]
    fn vtune_rejects_malformed_and_missing_counters() {
        for contents in [
            "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,not-a-number\n".to_owned(),
            "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY\n".to_owned(),
            "Hardware Event Type,Hardware Event Count:Self\n".to_owned(),
            "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,-1\n".to_owned(),
            "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,inf\n".to_owned(),
            "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,NaN\n".to_owned(),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("vtune.csv");
            fs::write(&path, contents).unwrap();
            assert!(matches!(
                parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()),
                Err(Error::ArtifactFormat { .. })
            ));
        }
    }

    #[test]
    fn vtune_sums_repeated_event_rows_instead_of_rejecting_them() {
        // Real `vtune -report hw-events` output can repeat the same event
        // name across several rows (for example, one row per core type), so
        // the parser aggregates repeats into a single metric rather than
        // treating a second occurrence as malformed input.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,1\nINST_RETIRED.ANY,2\n",
        )
        .unwrap();

        let result = parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()).unwrap();
        assert_eq!(result.metrics["INST_RETIRED.ANY"].value, crate::report::MetricValue::Float(3.0));
    }

    #[test]
    fn vtune_rejects_repeated_rows_whose_sum_overflows_to_infinity() {
        // Two individually finite counts can still sum to infinity; that
        // must surface as malformed input rather than a silently stored
        // non-finite aggregate metric.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(
            &path,
            format!(
                "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,{max}\nINST_RETIRED.ANY,{max}\n",
                max = f64::MAX
            ),
        )
        .unwrap();

        assert!(matches!(
            parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()),
            Err(Error::ArtifactFormat { .. })
        ));
    }

    #[test]
    fn vtune_accepts_a_zero_count_as_valid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vtune.csv");
        fs::write(&path, "Hardware Event Type,Hardware Event Count:Self\nINST_RETIRED.ANY,0\n").unwrap();

        let result = parse_vtune(&path, directory.path(), "group/bench".to_owned(), "group/bench".to_owned()).unwrap();
        assert_eq!(result.metrics["INST_RETIRED.ANY"].value, crate::report::MetricValue::Float(0.0));
    }

    #[test]
    fn discovery_is_sorted_skips_symlinks_and_enforces_limits() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("b")).unwrap();
        fs::create_dir_all(directory.path().join("a")).unwrap();
        fs::write(directory.path().join("b/summary.json"), "{}").unwrap();
        fs::write(directory.path().join("a/summary.json"), "{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.path().join("a"), directory.path().join("linked")).unwrap();

        let paths = find_artifact_files(directory.path(), |path| path.ends_with("summary.json")).unwrap();
        assert_eq!(
            paths,
            [directory.path().join("a/summary.json"), directory.path().join("b/summary.json")]
        );
        assert!(matches!(
            find_artifact_files_with_limits(
                directory.path(),
                |path| path.ends_with("summary.json"),
                DiscoveryLimits {
                    directories: 2,
                    artifacts: 10
                }
            ),
            Err(Error::ArtifactFormat { .. })
        ));
        assert!(matches!(
            find_artifact_files_with_limits(
                directory.path(),
                |path| path.ends_with("summary.json"),
                DiscoveryLimits {
                    directories: 10,
                    artifacts: 1
                }
            ),
            Err(Error::ArtifactFormat { .. })
        ));
    }

    #[test]
    fn discovery_depth_boundary_is_exact() {
        for (depth, accepted) in [(MAX_ARTIFACT_DIRECTORY_DEPTH, true), (MAX_ARTIFACT_DIRECTORY_DEPTH + 1, false)] {
            let directory = tempfile::tempdir().unwrap();
            let mut leaf = directory.path().to_owned();
            for _ in 0..depth {
                leaf.push("d");
            }
            fs::create_dir_all(&leaf).unwrap();
            fs::write(leaf.join("summary.json"), "{}").unwrap();
            assert_eq!(find_artifact_files(directory.path(), |_| true).is_ok(), accepted);
        }
    }

    #[test]
    fn json_size_limit_is_checked_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.json");
        fs::File::create(&path).unwrap().set_len(11).unwrap();
        assert!(matches!(read_regular_file(&path, 10, "JSON"), Err(Error::ArtifactFormat { .. })));
    }

    #[test]
    fn run_manifest_retains_native_to_canonical_identity_mapping() {
        let directory = tempfile::tempdir().unwrap();
        write_run_manifest(directory.path(), directory.path(), "target", "criterion", &[], None).unwrap();
        let results = parse_criterion(&fixture("criterion/minimal"), &fixture("criterion"), &[], None).unwrap();
        record_discovered_identities(directory.path(), "criterion", &results).unwrap();
        let manifest: EngineRunManifest = read_json(&directory.path().join("manifests/criterion.json")).unwrap();
        assert_eq!(manifest.discovered_native_identities, [results[0].native_identity.clone()]);
        assert_eq!(manifest.native_to_canonical[&results[0].native_identity], results[0].identity);
    }

    #[test]
    fn atomic_json_write_preserves_previous_file_on_encoding_failure() {
        struct Invalid;

        impl Serialize for Invalid {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("injected encoding failure"))
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("manifest.json");
        fs::write(&path, "previous").unwrap();
        assert!(matches!(write_json(&path, &Invalid), Err(Error::ArtifactJson { .. })));
        assert_eq!(fs::read_to_string(path).unwrap(), "previous");
    }

    #[test]
    fn vtune_paths_are_scoped_under_the_run_directory_and_indexed() {
        let directory = tempfile::tempdir().unwrap();
        let run = ArtifactDirectory::create_with_homes(directory.path(), None, None).unwrap();
        assert_eq!(run.vtune_result_path(0), run.path().join("vtune-results").join("0"));
        assert_eq!(run.vtune_result_path(3), run.path().join("vtune-results").join("3"));
        assert_ne!(run.vtune_result_path(0), run.vtune_result_path(3));
        assert_eq!(run.vtune_path(0), run.path().join("vtune").join("0.csv"));
        assert_eq!(run.vtune_path(3), run.path().join("vtune").join("3.csv"));
        assert_ne!(run.vtune_path(0), run.vtune_path(3));
    }

    #[test]
    fn run_directories_isolate_engine_homes_and_prune_only_owned_runs() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let external_home = external.path().as_os_str().to_owned();
        let first =
            ArtifactDirectory::create_with_homes(directory.path(), Some(external_home.clone()), Some(external_home.clone())).unwrap();
        let second = ArtifactDirectory::create_with_homes(directory.path(), Some(external_home.clone()), Some(external_home)).unwrap();
        assert_ne!(first.criterion_path(), second.criterion_path());
        assert_ne!(first.gungraun_path(), second.gungraun_path());
        assert!(first.criterion_path().starts_with(external.path().join(".metabench-runs")));
        fs::write(external.path().join("preserve"), "external").unwrap();

        for index in 0..=RETAINED_RUN_DIRECTORIES {
            fs::create_dir_all(directory.path().join("runs").join(format!("000-{index:03}"))).unwrap();
        }
        let active = directory.path().join("runs/000-000");
        let _active_lock = mark_run_active(&active).unwrap();
        ArtifactDirectory::prune_owned_runs(&directory.path().join("runs"), second.path()).unwrap();
        assert!(external.path().join("preserve").is_file());
        assert!(active.is_dir());
        assert_eq!(
            fs::read_dir(directory.path().join("runs")).unwrap().count(),
            RETAINED_RUN_DIRECTORIES
        );
    }

    #[test]
    fn truncated_criterion_json_is_never_accepted() {
        let valid = br#"{"group_id":"group","function_id":"bench"}"#;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("benchmark.json");
        for end in 0..valid.len() {
            fs::write(&path, &valid[..end]).unwrap();
            assert!(read_json::<CriterionMetadata>(&path).is_err(), "accepted prefix length {end}");
        }
        fs::write(&path, valid).unwrap();
        read_json::<CriterionMetadata>(&path).unwrap();
    }
}
