// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Debug;
use std::fs::{File, OpenOptions};
use std::hash::Hash;
use std::io::{self, BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::process::{self, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use gungraun_summary::either_or_both::EitherOrBoth;
use gungraun_summary::v6::{BenchmarkSummary, Metric as GungraunMetric, MetricsSummary, ToolMetricSummary, ValgrindTool};
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
        fs::create_dir_all(&run).map_err(|source| Error::ArtifactIo { path: run.clone(), source })?;
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
    median: CriterionEstimate,
}

#[derive(Debug, Deserialize)]
struct CriterionEstimate {
    confidence_interval: CriterionConfidenceInterval,
    point_estimate: f64,
}

#[derive(Debug, Deserialize)]
struct CriterionConfidenceInterval {
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
        let median = estimates.median;
        validate_criterion_estimate(&estimates_path, &median)?;
        let mut metric = Metric::float("criterion", "median", median.point_estimate, MetricDirection::LowerIsBetter);
        metric.unit = unit.map(str::to_owned);
        metric.lower_bound = Some(median.confidence_interval.lower_bound);
        metric.upper_bound = Some(median.confidence_interval.upper_bound);
        let mut metrics = BTreeMap::from([("median".to_owned(), metric)]);
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
        validate_json_size(&path, MAX_ARTIFACT_JSON_BYTES)?;
        let summary = gungraun_summary::v6::parse(&path).map_err(|error| Error::ArtifactFormat {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if summary.version != "6" {
            return Err(Error::ArtifactFormat {
                path,
                message: format!("unsupported Gungraun summary schema {}", summary.version),
            });
        }

        let native_identity = gungraun_native_identity(&summary);
        let default_identity = gungraun_canonical_identity(&summary);
        let identity = identities.resolve_gungraun(&native_identity, default_identity);
        for profile in &summary.profiles.0 {
            let tool = tool_name(profile.tool);
            if !is_benchmark_tool(profile.tool) {
                return Err(Error::ArtifactFormat {
                    path,
                    message: format!("Gungraun {tool} is not a benchmark measurement tool"),
                });
            }
            let source = format!("gungraun.{tool}");
            let metrics = tool_metrics(&source, &profile.summaries.total.summary).ok_or_else(|| Error::ArtifactFormat {
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
    validate_json_size(path, MAX_ARTIFACT_JSON_BYTES)?;
    let contents = fs::read_to_string(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
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

pub(crate) fn allocation_criterion_identity(native_identity: &str, identities: &[BenchmarkIdentity]) -> Option<String> {
    CanonicalIdentityIndex::new(identities).resolve_allocation_criterion(native_identity)
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
    let mut parts = summary.module_path.split("::").collect::<Vec<_>>();
    if parts.last().copied() == Some(summary.function_name.as_str()) {
        parts.pop();
    }
    let group = parts.last().copied().unwrap_or("gungraun");
    match &summary.id {
        Some(id) if id != "default" => format!("{group}/{}/{id}", summary.function_name),
        _ => format!("{group}/{}", summary.function_name),
    }
}

struct CanonicalIdentityIndex {
    criterion: BTreeMap<String, String>,
    allocation_criterion: BTreeMap<String, String>,
    gungraun: BTreeMap<String, String>,
}

impl CanonicalIdentityIndex {
    fn new(identities: &[BenchmarkIdentity]) -> Self {
        let criterion = identities
            .iter()
            .map(|identity| (identity.criterion_identity(), identity.canonical_identity()))
            .collect();
        let allocation_criterion = identities
            .iter()
            .filter(|identity| identity.allocation_tracking())
            .map(|identity| (identity.criterion_identity(), identity.canonical_identity()))
            .collect();
        let gungraun = identities
            .iter()
            .map(|identity| (identity.gungraun_identity(), identity.canonical_identity()))
            .collect();
        Self {
            criterion,
            allocation_criterion,
            gungraun,
        }
    }

    fn resolve_criterion(&self, native: &str) -> String {
        self.resolve_registered_criterion(native).unwrap_or_else(|| native.to_owned())
    }

    fn resolve_registered_criterion(&self, native: &str) -> Option<String> {
        Self::resolve(&self.criterion, native)
    }

    fn resolve_allocation_criterion(&self, native: &str) -> Option<String> {
        Self::resolve(&self.allocation_criterion, native)
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

fn tool_name(tool: ValgrindTool) -> &'static str {
    match tool {
        ValgrindTool::Callgrind => "callgrind",
        ValgrindTool::Cachegrind => "cachegrind",
        ValgrindTool::DHAT => "dhat",
        ValgrindTool::Memcheck => "memcheck",
        ValgrindTool::Helgrind => "helgrind",
        ValgrindTool::DRD => "drd",
        ValgrindTool::Massif => "massif",
        ValgrindTool::BBV => "bbv",
    }
}

const fn is_benchmark_tool(tool: ValgrindTool) -> bool {
    matches!(tool, ValgrindTool::Callgrind | ValgrindTool::Cachegrind | ValgrindTool::DHAT)
}

fn tool_metrics(source: &str, summary: &ToolMetricSummary) -> Option<BTreeMap<String, Metric>> {
    let metrics = match summary {
        ToolMetricSummary::Dhat(metrics) => Some(copy_metrics(source, metrics)),
        ToolMetricSummary::Callgrind(metrics) => Some(copy_metrics(source, metrics)),
        ToolMetricSummary::Cachegrind(metrics) => Some(copy_metrics(source, metrics)),
        ToolMetricSummary::None | ToolMetricSummary::ErrorTool(_) => None,
    };
    metrics.filter(|metrics| !metrics.is_empty())
}

fn copy_metrics<K>(source: &str, summary: &MetricsSummary<K>) -> BTreeMap<String, Metric>
where
    K: Debug + Eq + Hash + Serialize,
{
    summary
        .0
        .iter()
        .filter_map(|(kind, value)| {
            let current = match value.metrics {
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
    if !point.is_finite() || !lower.is_finite() || !upper.is_finite() {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "contains a non-finite Criterion estimate".to_owned(),
        })
    } else if point < 0.0 || lower < 0.0 || upper < 0.0 {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Criterion estimates must be non-negative; found {lower}..{point}..{upper}"),
        })
    } else if lower > point || point > upper {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Criterion confidence interval must contain its point estimate; found {lower}..{point}..{upper}"),
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

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    validate_json_size(path, MAX_ARTIFACT_JSON_BYTES)?;
    let file = fs::File::open(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })
}

fn validate_json_size(path: &Path, limit: u64) -> Result<(), Error> {
    let length = fs::metadata(path)
        .map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        })?
        .len();
    if length > limit {
        Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("JSON file is {length} bytes; limit is {limit} bytes"),
        })
    } else {
        Ok(())
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
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

    use super::*;

    #[test]
    fn accepts_only_performance_measurement_tools() {
        for tool in [ValgrindTool::Callgrind, ValgrindTool::Cachegrind, ValgrindTool::DHAT] {
            assert!(is_benchmark_tool(tool));
        }
        for tool in [
            ValgrindTool::Memcheck,
            ValgrindTool::Helgrind,
            ValgrindTool::DRD,
            ValgrindTool::Massif,
            ValgrindTool::BBV,
        ] {
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
            format!(r#"{{"median":{{"confidence_interval":{{"lower_bound":{lower},"upper_bound":{upper}}},"point_estimate":{point}}}}}"#),
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
        let median = &result.metrics["median"];
        assert_eq!(median.value, crate::report::MetricValue::Float(20.0));
        assert_eq!(median.direction, MetricDirection::LowerIsBetter);
        assert_eq!(median.unit.as_deref(), Some("ns"));
        assert_eq!(median.lower_bound, Some(10.0));
        assert_eq!(median.upper_bound, Some(30.0));
        assert_eq!(
            result.metrics["declared_throughput.bytes"].value,
            crate::report::MetricValue::Integer(10)
        );
        let index = CanonicalIdentityIndex::new(&identities);
        assert_eq!(index.resolve_criterion("group/bench/case_0"), "group/bench/case_0");
        assert_eq!(index.resolve_criterion("other/bench/case_0"), "other/bench/case_0");
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

        for (point, lower, upper) in [(-1.0, 0.0, 1.0), (1.0, -1.0, 2.0), (1.0, 2.0, 3.0), (3.0, 1.0, 2.0)] {
            let directory = tempfile::tempdir().unwrap();
            write_criterion(directory.path(), point, lower, upper);
            assert!(matches!(
                parse_criterion(directory.path(), directory.path(), &[], None),
                Err(Error::ArtifactFormat { .. })
            ));
        }
    }

    #[test]
    fn parses_gungraun_schema_and_strips_native_diffs() {
        let source = fs::read_to_string(fixture("gungraun/minimal/summary.json")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("summary.json"),
            source.replace(r#""diffs": null"#, r#""diffs":{"diff_pct":"42.0","factor":"1.42"}"#),
        )
        .unwrap();

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
    fn gungraun_rejects_malformed_input() {
        assert!(matches!(
            parse_gungraun(&fixture("gungraun/malformed"), Path::new(""), &[]),
            Err(Error::ArtifactFormat { .. })
        ));
        assert!(parse_gungraun(&fixture("gungraun/empty"), Path::new(""), &[]).unwrap().is_empty());

        let source = fs::read_to_string(fixture("gungraun/minimal/summary.json")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("summary.json"), source.replace(r#""Left""#, r#""Right""#)).unwrap();
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
        assert!(matches!(validate_json_size(&path, 10), Err(Error::ArtifactFormat { .. })));
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
