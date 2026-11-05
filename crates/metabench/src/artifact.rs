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
    _active_locks: Vec<ActiveRunLock>,
}

#[derive(Debug)]
enum ActiveRunLock {
    Native {
        _file: File,
    },
    #[cfg(test)]
    InMemory,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestArtifactSources {
    name: fn() -> Result<String, Error>,
    create: fn(&Path) -> Result<(), Error>,
    mark: fn(&Path) -> Result<ActiveRunLock, Error>,
    prune: fn(&Path, &Path) -> Result<(), Error>,
    write: fn(&Path, serde_json::Value) -> Result<(), Error>,
    find: fn(&Path) -> Result<Vec<PathBuf>, Error>,
}

#[cfg(test)]
thread_local! {
    static TEST_ARTIFACT_SOURCES: std::cell::Cell<Option<TestArtifactSources>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn with_in_memory_artifact_run<R>(run: impl FnOnce() -> R) -> R {
    struct Restore(Option<TestArtifactSources>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_ARTIFACT_SOURCES.set(self.0);
        }
    }
    let _restore = Restore(TEST_ARTIFACT_SOURCES.replace(Some(TestArtifactSources {
        name: || Ok("in-memory-run".to_owned()),
        create: |_| Ok(()),
        mark: |_| Ok(ActiveRunLock::InMemory),
        prune: |_, _| Ok(()),
        write: |_, _| Ok(()),
        find: |_| Ok(Vec::new()),
    })));
    run()
}

#[cfg(test)]
pub(super) fn with_in_memory_artifact_creation_failure<R>(run: impl FnOnce() -> R) -> R {
    with_in_memory_artifact_run(|| {
        let mut sources = TEST_ARTIFACT_SOURCES
            .get()
            .expect("the enclosing artifact fixture installs its native observation source");
        sources.name = || Err(Error::Metadata("injected artifact creation failure".to_owned()));
        TEST_ARTIFACT_SOURCES.set(Some(sources));
        run()
    })
}

#[cfg(test)]
pub(super) fn with_in_memory_json_sink<R>(write: fn(&Path, serde_json::Value) -> Result<(), Error>, run: impl FnOnce() -> R) -> R {
    with_in_memory_artifact_run(|| {
        let mut sources = TEST_ARTIFACT_SOURCES
            .get()
            .expect("the enclosing artifact fixture installs its native observation source");
        sources.write = write;
        TEST_ARTIFACT_SOURCES.set(Some(sources));
        run()
    })
}

#[cfg(test)]
pub(super) fn with_in_memory_artifact_listing<R>(find: fn(&Path) -> Result<Vec<PathBuf>, Error>, run: impl FnOnce() -> R) -> R {
    with_in_memory_artifact_run(|| {
        let mut sources = TEST_ARTIFACT_SOURCES
            .get()
            .expect("the enclosing artifact fixture installs its native observation source");
        sources.find = find;
        TEST_ARTIFACT_SOURCES.set(Some(sources));
        run()
    })
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
    #[cfg(test)]
    pub(crate) fn gamma_round2_in_memory_directory() -> Self {
        Self {
            root: PathBuf::from("unused-artifact-root"),
            run: PathBuf::from("unused-artifact-root").join("run"),
            criterion: PathBuf::from("unused-artifact-root").join("criterion"),
            gungraun: PathBuf::from("unused-artifact-root").join("gungraun"),
            _active_locks: Vec::new(),
        }
    }

    pub(crate) fn create(root: &Path) -> Result<Self, Error> {
        Self::create_with_environment(root, |name| env::var_os(name), Self::create_with_homes)
    }

    fn create_with_environment(
        root: &Path,
        mut environment: impl FnMut(&str) -> Option<OsString>,
        create: impl FnOnce(&Path, Option<OsString>, Option<OsString>) -> Result<Self, Error>,
    ) -> Result<Self, Error> {
        create(root, environment("CRITERION_HOME"), environment("GUNGRAUN_HOME"))
    }

    fn create_with_homes(root: &Path, criterion_home: Option<OsString>, gungraun_home: Option<OsString>) -> Result<Self, Error> {
        let (run, criterion, gungraun, active_locks) = prepare_artifact_run(
            root,
            criterion_home,
            gungraun_home,
            artifact_run_name,
            Self::create_private_run,
            retain_active_run,
            Self::prune_owned_runs,
        )?;
        Ok(Self {
            root: root.to_owned(),
            run,
            criterion,
            gungraun,
            _active_locks: active_locks,
        })
    }

    fn create_private_run(path: &Path) -> Result<(), Error> {
        #[cfg(test)]
        if let Some(sources) = TEST_ARTIFACT_SOURCES.get() {
            return (sources.create)(path);
        }
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
        #[cfg(test)]
        if let Some(sources) = TEST_ARTIFACT_SOURCES.get() {
            return (sources.prune)(runs, current);
        }
        Self::prune_owned_runs_with(
            runs,
            current,
            |path| fs::read_dir(path),
            run_may_be_active,
            |path| fs::remove_dir_all(path),
        )
    }

    fn prune_owned_runs_with<E: ArtifactEntry, I: IntoIterator<Item = io::Result<E>>>(
        runs: &Path,
        current: &Path,
        entries: impl FnOnce(&Path) -> io::Result<I>,
        mut active: impl FnMut(&Path) -> bool,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<(), Error> {
        let mut directories = entries(runs)
            .map_err(|source| Error::ArtifactIo {
                path: runs.to_owned(),
                source,
            })?
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .kind()
                    .ok()
                    .filter(|file_type| *file_type == ArtifactKind::Directory)
                    .map(|_| entry.path())
            })
            .collect::<Vec<_>>();
        directories.sort();
        let mut remove_count = directories.len().saturating_sub(RETAINED_RUN_DIRECTORIES);
        for directory in directories {
            if remove_count == 0 {
                // #[gamma::skip(loop.break_to_continue, reason = "after the quota reaches zero every later iteration also immediately continues, making no more active checks or removals; remaining PathBuf values are dropped in order either way")]
                break;
            }
            if directory != current && !active(&directory) {
                match remove(&directory) {
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

fn artifact_run_name() -> Result<String, Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_ARTIFACT_SOURCES.get() {
        return (sources.name)();
    }
    run_name_with(SystemTime::now, &RUN_SEQUENCE, process::id)
}

fn retain_active_run(path: &Path) -> Result<ActiveRunLock, Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_ARTIFACT_SOURCES.get() {
        return (sources.mark)(path);
    }
    mark_run_active(path).map(|file| ActiveRunLock::Native { _file: file })
}

fn run_name_with(now: impl FnOnce() -> SystemTime, sequence: &AtomicU64, process_id: impl FnOnce() -> u32) -> Result<String, Error> {
    let timestamp = now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Metadata(error.to_string()))?
        .as_nanos();
    let sequence = sequence.fetch_add(1, Ordering::Relaxed);
    Ok(format!("{timestamp}-{}-{sequence}", process_id()))
}

type PreparedRun<L> = (PathBuf, PathBuf, PathBuf, Vec<L>);

fn prepare_artifact_run<L>(
    root: &Path,
    criterion_home: Option<OsString>,
    gungraun_home: Option<OsString>,
    name: impl FnOnce() -> Result<String, Error>,
    mut create: impl FnMut(&Path) -> Result<(), Error>,
    mut mark: impl FnMut(&Path) -> Result<L, Error>,
    mut prune: impl FnMut(&Path, &Path) -> Result<(), Error>,
) -> Result<PreparedRun<L>, Error> {
    let run_name = name()?;
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
    create(&run)?;
    let mut marked_runs = BTreeSet::from([run.clone()]);
    let mut active_locks = vec![mark(&run)?];
    prune(&runs, &run)?;
    for (owned_runs, current) in [
        criterion_runs.as_deref().map(|runs| (runs, criterion.as_path())),
        gungraun_runs.as_deref().map(|runs| (runs, gungraun.as_path())),
    ]
    .into_iter()
    .flatten()
    {
        if marked_runs.insert(current.to_owned()) {
            create(current)?;
            active_locks.push(mark(current)?);
        }
        prune(owned_runs, current)?;
    }
    Ok((run, criterion, gungraun, active_locks))
}

pub(crate) fn write_run_manifest(
    manifest_root: &Path,
    artifact_root: &Path,
    target: &str,
    engine: &str,
    arguments: &[OsString],
    status: Option<ExitStatus>,
) -> Result<(), Error> {
    write_run_manifest_with(manifest_root, artifact_root, target, engine, arguments, status, write_json)
}

fn write_run_manifest_with(
    manifest_root: &Path,
    artifact_root: &Path,
    target: &str,
    engine: &str,
    arguments: &[OsString],
    status: Option<ExitStatus>,
    write: impl FnOnce(&Path, &EngineRunManifest) -> Result<(), Error>,
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
    write(&manifest_root.join("manifests").join(format!("{engine}.json")), &manifest)
}

pub(crate) fn record_discovered_identities(manifest_root: &Path, engine: &str, results: &[NativeResult]) -> Result<(), Error> {
    record_discovered_identities_with(manifest_root, engine, results, read_json::<EngineRunManifest>, write_json)
}

fn record_discovered_identities_with(
    manifest_root: &Path,
    engine: &str,
    results: &[NativeResult],
    read: impl FnOnce(&Path) -> Result<EngineRunManifest, Error>,
    write: impl FnOnce(&Path, &EngineRunManifest) -> Result<(), Error>,
) -> Result<(), Error> {
    let path = manifest_root.join("manifests").join(format!("{engine}.json"));
    let mut manifest = read(&path)?;
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
    write(&path, &manifest)
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
    parse_criterion_with(root, artifact_root, identities, unit, find_artifact_files, read_regular_file)
}

fn parse_criterion_with(
    root: &Path,
    artifact_root: &Path,
    identities: &[BenchmarkIdentity],
    unit: Option<&str>,
    discover: impl FnOnce(&Path, fn(&Path) -> bool) -> Result<Vec<PathBuf>, Error>,
    read: impl FnMut(&Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<Vec<NativeResult>, Error> {
    let paths = discover(root, criterion_artifact_path)?;
    parse_criterion_paths(paths, artifact_root, identities, unit, read)
}

fn criterion_artifact_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "benchmark.json")
        && path.parent().and_then(Path::file_name).is_some_and(|name| name == "new")
}

fn parse_criterion_paths(
    paths: Vec<PathBuf>,
    artifact_root: &Path,
    identities: &[BenchmarkIdentity],
    unit: Option<&str>,
    mut read: impl FnMut(&Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<Vec<NativeResult>, Error> {
    let identities = CanonicalIdentityIndex::new(identities);
    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = read_json_with::<CriterionMetadata>(&path, &mut read)?;
        let native_identity = criterion_identity(&metadata);
        let identity = identities.resolve_criterion(&native_identity);
        let estimates_path = path.with_file_name("estimates.json");
        let estimates = read_json_with::<CriterionEstimates>(&estimates_path, &mut read)?;
        let (statistic, estimate) = match estimates.slope {
            Some(slope) => ("slope", slope),
            None => ("mean", estimates.mean),
        };
        validate_criterion_estimate(&estimates_path, &estimate)?;
        let mut metric = Metric::float(
            {
                // #[gamma::skip(literal.str_to_empty, literal.str_to_xyzzy, reason = "this engine argument affects only the display label; criterion/time, /time and xyzzy/time all render as Time, while TableMetric engine/rank fields are discarded")]
                "criterion"
            },
            "time",
            estimate.point_estimate,
            MetricDirection::LowerIsBetter,
        );
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
    parse_gungraun_with(root, artifact_root, identities, find_artifact_files, read_regular_file)
}

fn parse_gungraun_with(
    root: &Path,
    artifact_root: &Path,
    identities: &[BenchmarkIdentity],
    discover: impl FnOnce(&Path, fn(&Path) -> bool) -> Result<Vec<PathBuf>, Error>,
    read: impl FnMut(&Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<Vec<NativeResult>, Error> {
    let paths = discover(root, |path| path.file_name().is_some_and(|name| name == "summary.json"))?;
    parse_gungraun_paths(paths, artifact_root, identities, read)
}

fn parse_gungraun_paths(
    paths: Vec<PathBuf>,
    artifact_root: &Path,
    identities: &[BenchmarkIdentity],
    mut read: impl FnMut(&Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<Vec<NativeResult>, Error> {
    let identities = CanonicalIdentityIndex::new(identities);
    let mut results = Vec::new();
    for path in paths {
        let contents = read(&path, MAX_ARTIFACT_JSON_BYTES, "JSON")?;
        results.extend(parse_gungraun_summary(&contents, &path, artifact_root, &identities)?);
    }
    Ok(results)
}

fn parse_gungraun_summary(
    contents: &[u8],
    path: &Path,
    artifact_root: &Path,
    identities: &CanonicalIdentityIndex,
) -> Result<Vec<NativeResult>, Error> {
    let summary = serde_json::from_slice::<BenchmarkSummary>(contents).map_err(|source| Error::ArtifactFormat {
        path: path.to_owned(),
        message: source.to_string(),
    })?;
    validate_gungraun_summary(path, &summary)?;

    let native_identity = gungraun_native_identity(&summary);
    let default_identity = gungraun_canonical_identity(&summary);
    let identity = identities.resolve_gungraun(&native_identity, default_identity);
    let mut results = Vec::with_capacity(summary.profiles.0.len());
    for profile in &summary.profiles.0 {
        let tool = validate_gungraun_tool(path, profile.tool)?;
        let source = format!("gungraun.{tool}");
        let metrics = tool_metrics(&source, &profile.data.total.metrics).ok_or_else(|| Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Gungraun {tool} summary contains no benchmark metrics"),
        })?;
        results.push(NativeResult {
            identity: identity.clone(),
            native_identity: native_identity.clone(),
            source,
            metrics,
            raw_artifacts: vec![relative_artifact(artifact_root, path)],
        });
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
    parse_perf_text(&contents, path, artifact_root, identity, native_identity)
}

fn parse_perf_text(
    contents: &str,
    path: &Path,
    artifact_root: &Path,
    identity: String,
    native_identity: String,
) -> Result<NativeResult, Error> {
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
                    // #[gamma::skip(assign_value.default, reason = "bool Default is exactly the assigned false value")]
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
    parse_vtune_text(&contents, path, artifact_root, identity, native_identity)
}

fn parse_vtune_text(
    contents: &str,
    path: &Path,
    artifact_root: &Path,
    identity: String,
    native_identity: String,
) -> Result<NativeResult, Error> {
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
            // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
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
    #[cfg(test)]
    if let Some(sources) = TEST_ARTIFACT_SOURCES.get() {
        let mut paths: Vec<_> = (sources.find)(root)?.into_iter().filter(|path| predicate(path)).collect();
        paths.sort();
        return Ok(paths);
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArtifactKind {
    Directory,
    File,
    SymbolicLink,
    Other,
}

trait ArtifactEntry {
    fn path(&self) -> PathBuf;
    fn kind(&self) -> io::Result<ArtifactKind>;
}

impl ArtifactEntry for fs::DirEntry {
    fn path(&self) -> PathBuf {
        self.path()
    }

    fn kind(&self) -> io::Result<ArtifactKind> {
        classify_artifact(|| self.file_type())
    }
}

trait ArtifactFileType {
    fn is_symlink(&self) -> bool;
    fn is_dir(&self) -> bool;
    fn is_file(&self) -> bool;
}

impl ArtifactFileType for fs::FileType {
    // #[gamma::skip(fn_value.bool_false, reason = "This leaf queries native filesystem metadata; scripted ArtifactFileType tests cover symlink rejection without creating filesystem fixtures")]
    fn is_symlink(&self) -> bool {
        self.is_symlink()
    }

    fn is_dir(&self) -> bool {
        self.is_dir()
    }

    // #[gamma::skip(fn_value.bool_true, reason = "This leaf queries native filesystem metadata; scripted ArtifactFileType tests distinguish regular files from special entries without creating filesystem fixtures")]
    fn is_file(&self) -> bool {
        self.is_file()
    }
}

fn classify_artifact<K: ArtifactFileType>(file_type: impl FnOnce() -> io::Result<K>) -> io::Result<ArtifactKind> {
    let kind = file_type()?;
    Ok(if kind.is_symlink() {
        ArtifactKind::SymbolicLink
    } else if kind.is_dir() {
        ArtifactKind::Directory
    } else if kind.is_file() {
        ArtifactKind::File
    } else {
        ArtifactKind::Other
    })
}

fn find_artifact_files_with_limits(root: &Path, predicate: impl Fn(&Path) -> bool, limits: DiscoveryLimits) -> Result<Vec<PathBuf>, Error> {
    find_artifact_files_with_io(root, predicate, limits, Path::try_exists, |path| fs::read_dir(path))
}

fn find_artifact_files_with_io<E: ArtifactEntry, I: IntoIterator<Item = io::Result<E>>>(
    root: &Path,
    predicate: impl Fn(&Path) -> bool,
    limits: DiscoveryLimits,
    exists: impl FnOnce(&Path) -> io::Result<bool>,
    mut entries: impl FnMut(&Path) -> io::Result<I>,
) -> Result<Vec<PathBuf>, Error> {
    if !exists(root).map_err(|source| Error::ArtifactIo {
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
        for entry in entries(&directory).map_err(|source| Error::ArtifactIo {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| Error::ArtifactIo {
                path: directory.clone(),
                source,
            })?;
            let file_type = entry.kind().map_err(|source| Error::ArtifactIo {
                path: entry.path(),
                source,
            })?;
            // #[gamma::skip(cond.always_false, loop.delete_continue, reason = "SymbolicLink is a distinct ArtifactKind; falling through cannot enter either the Directory or File branch or invoke the predicate")]
            if file_type == ArtifactKind::SymbolicLink {
                continue;
            }
            if file_type == ArtifactKind::Directory {
                if directory_count + pending.len() + 1 > limits.directories {
                    return Err(Error::ArtifactFormat {
                        path: root.to_owned(),
                        message: format!("artifact tree exceeds the limit of {} directories", limits.directories),
                    });
                }
                pending.push((entry.path(), depth + 1));
            } else if file_type == ArtifactKind::File && predicate(&entry.path()) {
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
    run_may_be_active_with(path, open_marker, File::try_lock)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "this private adapter preserves four independent std::fs::OpenOptions flags"
)]
struct MarkerAccess {
    read: bool,
    write: bool,
    create: bool,
    truncate: bool,
}

fn open_marker(path: &Path, access: MarkerAccess) -> io::Result<File> {
    OpenOptions::new()
        .read(access.read)
        .write(access.write)
        .create(access.create)
        .truncate(access.truncate)
        .open(path)
}

fn run_may_be_active_with<L, E>(
    path: &Path,
    open: impl FnOnce(&Path, MarkerAccess) -> io::Result<L>,
    lock: impl FnOnce(&L) -> Result<(), E>,
) -> bool {
    let access = MarkerAccess {
        read: true,
        write: true,
        create: false,
        truncate: false,
    };
    let Ok(marker) = open(&path.join(ACTIVE_RUN_MARKER), access) else {
        return false;
    };
    match lock(&marker) {
        Ok(()) => false,
        Err(_) => true,
    }
}

fn mark_run_active(path: &Path) -> Result<File, Error> {
    mark_run_active_with(path, |path| fs::create_dir_all(path), open_marker, File::lock)
}

fn mark_run_active_with<L>(
    path: &Path,
    create: impl FnOnce(&Path) -> io::Result<()>,
    open: impl FnOnce(&Path, MarkerAccess) -> io::Result<L>,
    lock: impl FnOnce(&L) -> io::Result<()>,
) -> Result<L, Error> {
    create(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    let marker = path.join(ACTIVE_RUN_MARKER);
    let access = MarkerAccess {
        read: true,
        write: true,
        create: true,
        truncate: true,
    };
    let file = open(&marker, access).map_err(|source| Error::ArtifactIo {
        path: marker.clone(),
        source,
    })?;
    lock(&file).map_err(|source| Error::ArtifactIo { path: marker, source })?;
    Ok(file)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    read_json_with(path, read_regular_file)
}

fn read_json_with<T: serde::de::DeserializeOwned>(
    path: &Path,
    read: impl FnOnce(&Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<T, Error> {
    let bytes = read(path, MAX_ARTIFACT_JSON_BYTES, "JSON")?;
    serde_json::from_slice(&bytes).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })
}

fn read_artifact_text(path: &Path, format: &str) -> Result<String, Error> {
    read_artifact_text_with(path, format, read_regular_file)
}

fn read_artifact_text_with(
    path: &Path,
    format: &str,
    read: impl FnOnce(&Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<String, Error> {
    let bytes = read(path, MAX_ARTIFACT_JSON_BYTES, format)?;
    String::from_utf8(bytes).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })
}

pub(crate) fn read_regular_file(path: &Path, limit: u64, format: &str) -> Result<Vec<u8>, Error> {
    read_regular_file_with(path, limit, format, open_regular_file, read_bounded_file)
}

#[derive(Clone, Copy, Debug)]
struct RegularFileRequest {
    read: bool,
    #[cfg(unix)]
    flags: i32,
    #[cfg(windows)]
    flags: u32,
    #[cfg(windows)]
    share: u32,
}

#[derive(Clone, Copy, Debug)]
struct RegularMetadata {
    regular: bool,
    length: u64,
    #[cfg(windows)]
    attributes: u32,
}

enum RegularFile {
    Native(File),
    #[cfg(test)]
    Memory(io::Cursor<Vec<u8>>),
}

impl io::Read for RegularFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Native(file) => file.read(buf),
            #[cfg(test)]
            Self::Memory(bytes) => bytes.read(buf),
        }
    }
}

impl RegularFile {
    fn metadata(&self) -> io::Result<RegularMetadata> {
        match self {
            Self::Native(file) => file.metadata().map(|metadata| RegularMetadata {
                regular: metadata.is_file(),
                length: metadata.len(),
                #[cfg(windows)]
                attributes: metadata.file_attributes(),
            }),
            #[cfg(test)]
            Self::Memory(bytes) => Ok(RegularMetadata {
                regular: true,
                length: bytes.get_ref().len() as u64,
                #[cfg(windows)]
                attributes: 0,
            }),
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_REGULAR_BYTES: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn with_in_memory_artifact<R>(contents: &[u8], run: impl FnOnce() -> R) -> R {
    struct Restore(Option<Vec<u8>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_REGULAR_BYTES.set(self.0.take());
        }
    }
    let _restore = Restore(TEST_REGULAR_BYTES.replace(Some(contents.to_vec())));
    run()
}

#[cfg(test)]
type TestRegularSource = fn(&Path) -> Vec<u8>;

#[cfg(test)]
thread_local! {
    static TEST_REGULAR_SOURCE: std::cell::Cell<Option<TestRegularSource>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn with_in_memory_artifact_source<R>(source: fn(&Path) -> Vec<u8>, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<TestRegularSource>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_REGULAR_SOURCE.set(self.0);
        }
    }
    let _restore = Restore(TEST_REGULAR_SOURCE.replace(Some(source)));
    run()
}

fn read_regular_file_with<R>(
    path: &Path,
    limit: u64,
    format: &str,
    open: impl FnOnce(&Path) -> Result<R, Error>,
    read: impl FnOnce(R, &Path, u64, &str) -> Result<Vec<u8>, Error>,
) -> Result<Vec<u8>, Error> {
    let file = open(path)?;
    read(file, path, limit, format)
}

/// Admits the same regular-file handle later consumed by the bounded reader.
fn open_regular_file(path: &Path) -> Result<RegularFile, Error> {
    open_regular_file_with(
        path,
        |path, request| {
            #[cfg(test)]
            if let Some(source) = TEST_REGULAR_SOURCE.get() {
                return Ok(RegularFile::Memory(io::Cursor::new(source(path))));
            }
            #[cfg(test)]
            if let Some(bytes) = TEST_REGULAR_BYTES.with_borrow(Clone::clone) {
                return Ok(RegularFile::Memory(io::Cursor::new(bytes)));
            }
            let mut options = OpenOptions::new();
            options.read(request.read);
            // #[gamma::skip(stmt.delete_call, reason = "this adapter applies native no-follow and nonblocking open flags; regular-file admission policy and requested flags are tested with scripted open and metadata operations without filesystem fixtures")]
            #[cfg(any(unix, windows))]
            options.custom_flags(request.flags);
            #[cfg(windows)]
            options.share_mode(request.share);
            options.open(path).map(RegularFile::Native)
        },
        RegularFile::metadata,
    )
}

#[cfg(test)]
mod november6_file_type_tests {
    use super::*;

    struct Kind(bool, bool, bool);

    impl ArtifactFileType for Kind {
        fn is_symlink(&self) -> bool {
            self.0
        }
        fn is_dir(&self) -> bool {
            self.1
        }
        fn is_file(&self) -> bool {
            self.2
        }
    }

    #[test]
    fn november6_artifact_kind_preserves_symlink_priority_special_files_and_native_errors() {
        for (kind, expected) in [
            (Kind(true, false, false), ArtifactKind::SymbolicLink),
            (Kind(true, true, true), ArtifactKind::SymbolicLink),
            (Kind(false, true, false), ArtifactKind::Directory),
            (Kind(false, false, true), ArtifactKind::File),
            (Kind(false, false, false), ArtifactKind::Other),
        ] {
            assert_eq!(classify_artifact(|| Ok(kind)).unwrap(), expected);
        }
        assert_eq!(
            classify_artifact::<Kind>(|| Err(io::Error::from_raw_os_error(5)))
                .unwrap_err()
                .raw_os_error(),
            Some(5)
        );
    }
}

fn open_regular_file_with<R>(
    path: &Path,
    open: impl FnOnce(&Path, RegularFileRequest) -> io::Result<R>,
    metadata: impl FnOnce(&R) -> io::Result<RegularMetadata>,
) -> Result<R, Error> {
    let request = RegularFileRequest {
        read: true,
        #[cfg(unix)]
        flags: libc::O_NOFOLLOW | libc::O_NONBLOCK,
        #[cfg(windows)]
        flags: 0x0020_0000,
        #[cfg(windows)]
        share: 1,
    };
    let file = open(path, request).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    let metadata = metadata(&file).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    #[cfg(windows)]
    {
        reject_windows_reparse_point(path, metadata.attributes)?;
    }
    if !metadata.regular {
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

fn read_bounded_file(file: RegularFile, path: &Path, limit: u64, format: &str) -> Result<Vec<u8>, Error> {
    read_bounded_file_with(file, path, limit, format, |file| file.metadata().map(|metadata| metadata.length))
}

fn read_bounded_file_with<R: io::Read>(
    file: R,
    path: &Path,
    limit: u64,
    format: &str,
    length: impl FnOnce(&R) -> io::Result<u64>,
) -> Result<Vec<u8>, Error> {
    let length = length(&file).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
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
    if u64::try_from(bytes.len()).unwrap_or(
        // #[gamma::skip(expr.decrement, reason = "usize is at most 64 bits on supported Rust targets, so the byte-vector length always converts and this fallback is unreachable")]
        u64::MAX,
    ) > limit
    {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("{format} stream exceeds the {limit}-byte limit"),
        });
    }
    Ok(bytes)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    #[cfg(test)]
    if let Some(sources) = TEST_ARTIFACT_SOURCES.get() {
        let value = serde_json::to_value(value).map_err(|source| Error::ArtifactJson {
            path: path.to_owned(),
            source,
        })?;
        return (sources.write)(path, value);
    }
    write_json_with(
        path,
        value,
        |path| fs::create_dir_all(path),
        |parent, prefix| tempfile::Builder::new().prefix(prefix).tempfile_in(parent),
        |temporary| temporary.as_file().sync_all(),
        |temporary, path| temporary.persist(path).map(|_| ()).map_err(|error| error.error),
    )
}

fn write_json_with<W: io::Write>(
    path: &Path,
    value: &impl Serialize,
    create_parent: impl FnOnce(&Path) -> io::Result<()>,
    create_temporary: impl FnOnce(&Path, &str) -> io::Result<W>,
    sync: impl FnOnce(&W) -> io::Result<()>,
    persist: impl FnOnce(W, &Path) -> io::Result<()>,
) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_parent(parent).map_err(|source| Error::ArtifactIo {
        path: parent.to_owned(),
        source,
    })?;
    let mut temporary = create_temporary(parent, ".metabench-").map_err(|source| Error::ArtifactIo {
        path: parent.to_owned(),
        source,
    })?;
    {
        let mut writer = BufWriter::new(&mut temporary);
        serde_json::to_writer_pretty(&mut writer, value).map_err(|source| Error::ArtifactJson {
            path: path.to_owned(),
            source,
        })?;
        writer.flush().map_err(|source| Error::ArtifactIo {
            path: path.to_owned(),
            source,
        })?;
    }
    sync(&temporary).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    persist(temporary, path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #[test]
    fn november_actual_artifact_directory_retains_both_home_owners_and_returns_preparation_errors_without_io() {
        with_in_memory_artifact_run(|| {
            let directory =
                ArtifactDirectory::create_with_homes(Path::new("root"), Some("criterion-home".into()), Some("gungraun-home".into()))
                    .unwrap();
            assert_eq!(directory.root(), Path::new("root"));
            assert_eq!(directory.path(), Path::new("root").join("runs").join("in-memory-run"));
            assert_eq!(
                directory.criterion_path(),
                Path::new("criterion-home").join(".metabench-runs").join("in-memory-run")
            );
            assert_eq!(
                directory.gungraun_path(),
                Path::new("gungraun-home").join(".metabench-runs").join("in-memory-run")
            );
            let ArtifactDirectory { _active_locks: locks, .. } = directory;
            assert_eq!(locks.len(), 3);
            let mut sources = TEST_ARTIFACT_SOURCES.get().unwrap();
            sources.name = || Err(Error::Metadata("injected run name failure".to_owned()));
            TEST_ARTIFACT_SOURCES.set(Some(sources));
            assert!(
                matches!(ArtifactDirectory::create_with_homes(Path::new("root"), None, None), Err(Error::Metadata(message)) if message == "injected run name failure")
            );
        });
    }

    #[test]
    fn november_actual_manifest_wrapper_publishes_exact_success_exit_status_and_command_line_without_files() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;

        thread_local! {
            static VALUES: std::cell::RefCell<Vec<serde_json::Value>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        struct Restore(Vec<serde_json::Value>);
        impl Drop for Restore {
            fn drop(&mut self) {
                VALUES.set(std::mem::take(&mut self.0));
            }
        }
        let _restore = Restore(VALUES.take());
        with_in_memory_artifact_run(|| {
            let mut sources = TEST_ARTIFACT_SOURCES.get().unwrap();
            sources.write = |path, value| {
                assert_eq!(path, Path::new("run").join("manifests").join("criterion.json"));
                VALUES.with_borrow_mut(|values| values.push(value));
                Ok(())
            };
            TEST_ARTIFACT_SOURCES.set(Some(sources));
            for status in [None, Some(ExitStatus::from_raw(0))] {
                write_run_manifest(
                    Path::new("run"),
                    Path::new("artifacts"),
                    "target",
                    "criterion",
                    &["--bench".into()],
                    status,
                )
                .unwrap();
            }
            VALUES.with_borrow(|values| {
                assert_eq!(values.len(), 2);
                assert_eq!(values[0]["success"], false);
                assert_eq!(values[0]["exit_code"], serde_json::Value::Null);
                assert_eq!(values[1]["success"], true);
                assert_eq!(values[1]["exit_code"], 0);
                for value in values {
                    assert_eq!(value["target"], "target");
                    assert_eq!(value["engine"], "criterion");
                    assert_eq!(value["command_line"], serde_json::json!(["--bench"]));
                    assert_eq!(value["artifact_root"], "artifacts");
                }
            });
        });
    }

    #[test]
    fn november_regular_file_admission_preserves_exact_native_requests_metadata_errors_and_owned_resources() {
        for case in ["open", "metadata", "not-file", "success"] {
            let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let path = Path::new("memory-only.json");
            let result = open_regular_file_with(
                path,
                |actual, request| {
                    assert_eq!(actual, path);
                    assert!(request.read);
                    #[cfg(windows)]
                    assert_eq!((request.flags, request.share), (0x0020_0000, 1));
                    #[cfg(unix)]
                    assert_eq!(request.flags, libc::O_NOFOLLOW | libc::O_NONBLOCK);
                    calls.borrow_mut().push("open");
                    if case == "open" {
                        Err(io::Error::from_raw_os_error(5))
                    } else {
                        Ok(MemoryMarker {
                            calls: std::rc::Rc::clone(&calls),
                        })
                    }
                },
                |_| {
                    assert_eq!(*calls.borrow(), ["open"]);
                    calls.borrow_mut().push("metadata");
                    if case == "metadata" {
                        Err(io::Error::from_raw_os_error(6))
                    } else {
                        Ok(RegularMetadata {
                            regular: case != "not-file",
                            length: 3,
                            #[cfg(windows)]
                            attributes: 0,
                        })
                    }
                },
            );
            if case == "success" {
                let owner = result.unwrap();
                assert_eq!(*calls.borrow(), ["open", "metadata"]);
                drop(owner);
            } else {
                match result.unwrap_err() {
                    Error::ArtifactIo { path: actual, source } => {
                        assert_eq!(actual, path);
                        assert_eq!(source.raw_os_error(), Some(if case == "open" { 5 } else { 6 }));
                    }
                    Error::ArtifactFormat { path: actual, message } => {
                        assert_eq!(actual, path);
                        assert_eq!(case, "not-file");
                        assert_eq!(message, "artifact must be a regular file");
                    }
                    error => panic!("unexpected admission error: {error}"),
                }
            }
            assert_eq!(
                *calls.borrow(),
                if case == "open" {
                    vec!["open"]
                } else {
                    vec!["open", "metadata", "drop"]
                }
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn november_regular_file_admission_rejects_reparse_attributes_before_file_kind_and_releases_owner() {
        let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let error = open_regular_file_with(
            Path::new("memory-only"),
            |_, _| {
                Ok(MemoryMarker {
                    calls: std::rc::Rc::clone(&calls),
                })
            },
            |_| {
                Ok(RegularMetadata {
                    regular: true,
                    length: 3,
                    attributes: 0x400,
                })
            },
        )
        .unwrap_err();
        assert!(matches!(error, Error::ArtifactFormat { message, .. } if message == "artifact must not be a reparse point"));
        assert_eq!(*calls.borrow(), ["drop"]);
    }

    #[test]
    fn november_actual_regular_reader_preserves_the_exact_configured_limit_without_filesystem_io() {
        with_in_memory_artifact(b"ABC", || {
            assert_eq!(read_regular_file(Path::new("memory-only"), 3, "JSON").unwrap(), b"ABC");
            assert!(matches!(
                read_regular_file(Path::new("memory-only"), 2, "JSON"),
                Err(Error::ArtifactFormat { message, .. }) if message == "JSON file is 3 bytes; limit is 2 bytes"
            ));
        });
    }

    #[test]
    fn november_criterion_time_engine_literal_equivalence_preserves_every_serialized_metric_field() {
        let expected = serde_json::to_value(Metric::float("criterion", "time", 17.0, MetricDirection::LowerIsBetter)).unwrap();
        assert_eq!(expected["display_name"], "Time");
        for engine in ["", "xyzzy"] {
            assert_eq!(
                serde_json::to_value(Metric::float(engine, "time", 17.0, MetricDirection::LowerIsBetter)).unwrap(),
                expected
            );
        }
    }

    #[derive(Clone)]
    struct MemoryEntry {
        path: PathBuf,
        kind: Result<ArtifactKind, io::ErrorKind>,
    }

    impl ArtifactEntry for MemoryEntry {
        fn path(&self) -> PathBuf {
            self.path.clone()
        }

        fn kind(&self) -> io::Result<ArtifactKind> {
            self.kind.map_err(|kind| io::Error::new(kind, "entry type failure"))
        }
    }

    #[derive(Debug)]
    struct MemoryMarker {
        calls: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    }

    impl Drop for MemoryMarker {
        fn drop(&mut self) {
            self.calls.borrow_mut().push("drop");
        }
    }

    #[test]
    fn november_active_run_probe_passes_exact_access_and_distinguishes_lock_outcomes() {
        let run = Path::new("run");
        for opened in [false, true] {
            for busy in [false, true] {
                let checked = std::cell::Cell::new(false);
                let active = run_may_be_active_with(
                    run,
                    |path, access| {
                        assert_eq!(path, run.join(ACTIVE_RUN_MARKER));
                        assert_eq!(
                            access,
                            MarkerAccess {
                                read: true,
                                write: true,
                                create: false,
                                truncate: false
                            }
                        );
                        if opened { Ok(17) } else { Err(io::ErrorKind::NotFound.into()) }
                    },
                    |marker| {
                        assert_eq!(*marker, 17);
                        checked.set(true);
                        if busy { Err(()) } else { Ok(()) }
                    },
                );
                assert_eq!(active, opened && busy);
                assert_eq!(checked.get(), opened);
            }
        }
    }

    #[test]
    fn november_active_run_creation_retains_lock_ownership_and_preserves_every_error_path() {
        let run = Path::new("run");
        for failure in 0..=3 {
            let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let result = mark_run_active_with(
                run,
                |path| {
                    assert_eq!(path, run);
                    calls.borrow_mut().push("create");
                    if failure == 0 {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "create refused"))
                    } else {
                        Ok(())
                    }
                },
                |path, access| {
                    assert_eq!(path, run.join(ACTIVE_RUN_MARKER));
                    assert_eq!(
                        access,
                        MarkerAccess {
                            read: true,
                            write: true,
                            create: true,
                            truncate: true
                        }
                    );
                    calls.borrow_mut().push("open");
                    if failure == 1 {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "open refused"))
                    } else {
                        Ok(MemoryMarker {
                            calls: std::rc::Rc::clone(&calls),
                        })
                    }
                },
                |_| {
                    calls.borrow_mut().push("lock");
                    if failure == 2 {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "lock refused"))
                    } else {
                        Ok(())
                    }
                },
            );
            if failure == 3 {
                let marker = result.unwrap();
                assert_eq!(*calls.borrow(), ["create", "open", "lock"]);
                drop(marker);
                assert_eq!(*calls.borrow(), ["create", "open", "lock", "drop"]);
            } else {
                let Error::ArtifactIo { path, source } = result.unwrap_err() else {
                    panic!("original marker error must survive")
                };
                assert_eq!(
                    path,
                    if failure == 0 {
                        run.to_owned()
                    } else {
                        run.join(ACTIVE_RUN_MARKER)
                    }
                );
                assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(source.to_string(), ["create refused", "open refused", "lock refused"][failure]);
                assert_eq!(
                    *calls.borrow(),
                    [vec!["create"], vec!["create", "open"], vec!["create", "open", "lock", "drop"]][failure]
                );
            }
        }
    }

    #[test]
    fn november_retention_prunes_sorted_inactive_runs_and_counts_only_successful_removals() {
        let runs = Path::new("runs");
        let current = runs.join("00");
        let checked = std::cell::RefCell::new(Vec::new());
        let removed = std::cell::RefCell::new(Vec::new());
        let mut entries = (0..24)
            .rev()
            .map(|index| {
                Ok(MemoryEntry {
                    path: runs.join(format!("{index:02}")),
                    kind: Ok(ArtifactKind::Directory),
                })
            })
            .collect::<Vec<_>>();
        entries.extend([
            Err(io::Error::other("unreadable directory entry")),
            Ok(MemoryEntry {
                path: runs.join("file"),
                kind: Ok(ArtifactKind::File),
            }),
            Ok(MemoryEntry {
                path: runs.join("link"),
                kind: Ok(ArtifactKind::SymbolicLink),
            }),
            Ok(MemoryEntry {
                path: runs.join("failed-type"),
                kind: Err(io::ErrorKind::PermissionDenied),
            }),
        ]);
        ArtifactDirectory::prune_owned_runs_with(
            runs,
            &current,
            |path| {
                assert_eq!(path, runs);
                Ok(entries)
            },
            |path| {
                checked.borrow_mut().push(path.to_owned());
                path == runs.join("01")
            },
            |path| {
                removed.borrow_mut().push(path.to_owned());
                if path == runs.join("02") {
                    Err(io::ErrorKind::NotFound.into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(
            *checked.borrow(),
            (1..=6).map(|index| runs.join(format!("{index:02}"))).collect::<Vec<_>>()
        );
        assert_eq!(
            *removed.borrow(),
            (2..=6).map(|index| runs.join(format!("{index:02}"))).collect::<Vec<_>>()
        );
    }

    #[test]
    fn november_retention_preserves_exact_listing_and_removal_errors() {
        let runs = Path::new("runs");
        let current = runs.join("23");
        for listing_failure in [false, true] {
            let removed = std::cell::RefCell::new(Vec::new());
            let result = ArtifactDirectory::prune_owned_runs_with(
                runs,
                &current,
                |_| {
                    if listing_failure {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "listing refused"))
                    } else {
                        Ok((0..24)
                            .map(|index| {
                                Ok(MemoryEntry {
                                    path: runs.join(format!("{index:02}")),
                                    kind: Ok(ArtifactKind::Directory),
                                })
                            })
                            .collect::<Vec<_>>())
                    }
                },
                |_| false,
                |path| {
                    removed.borrow_mut().push(path.to_owned());
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "removal refused"))
                },
            );
            let Error::ArtifactIo { path, source } = result.unwrap_err() else {
                panic!("original I/O error must be preserved")
            };
            assert_eq!(path, if listing_failure { runs.to_owned() } else { runs.join("00") });
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(
                source.to_string(),
                if listing_failure { "listing refused" } else { "removal refused" }
            );
            assert_eq!(*removed.borrow(), if listing_failure { Vec::new() } else { vec![runs.join("00")] });
        }
    }

    #[test]
    fn november_artifact_discovery_filters_kinds_and_sorts_exact_paths_in_memory() {
        let root = Path::new("root");
        let checked = std::cell::RefCell::new(Vec::new());
        let mut entries = vec![
            ("link.json", ArtifactKind::SymbolicLink),
            ("z.json", ArtifactKind::File),
            ("device.json", ArtifactKind::Other),
            ("skip.txt", ArtifactKind::File),
            ("a.json", ArtifactKind::File),
        ]
        .into_iter()
        .map(|(name, kind)| {
            Ok(MemoryEntry {
                path: root.join(name),
                kind: Ok(kind),
            })
        })
        .collect::<Vec<_>>();
        let paths = find_artifact_files_with_io(
            root,
            |path| {
                checked.borrow_mut().push(path.to_owned());
                path.extension().is_some_and(|extension| extension == "json")
            },
            DiscoveryLimits {
                directories: 1,
                artifacts: 2,
            },
            |path| {
                assert_eq!(path, root);
                Ok(true)
            },
            |path| {
                assert_eq!(path, root);
                Ok(std::mem::take(&mut entries))
            },
        )
        .unwrap();
        assert_eq!(paths, [root.join("a.json"), root.join("z.json")]);
        assert_eq!(*checked.borrow(), [root.join("z.json"), root.join("skip.txt"), root.join("a.json")]);
    }

    #[test]
    fn november_artifact_discovery_enforces_directory_and_file_limits_before_extra_io() {
        let root = Path::new("root");
        for limit in 0..=3 {
            let visited = std::cell::RefCell::new(Vec::new());
            let result = find_artifact_files_with_io(
                root,
                |_| true,
                DiscoveryLimits {
                    directories: limit,
                    artifacts: 2,
                },
                |_| Ok(true),
                |path| {
                    visited.borrow_mut().push(path.to_owned());
                    Ok(if path == root {
                        vec![
                            Ok(MemoryEntry {
                                path: root.join("first"),
                                kind: Ok(ArtifactKind::Directory),
                            }),
                            Ok(MemoryEntry {
                                path: root.join("second"),
                                kind: Ok(ArtifactKind::Directory),
                            }),
                        ]
                    } else {
                        vec![Ok(MemoryEntry {
                            path: path.join("result.json"),
                            kind: Ok(ArtifactKind::File),
                        })]
                    })
                },
            );
            if limit == 3 {
                assert_eq!(
                    result.unwrap(),
                    [root.join("first").join("result.json"), root.join("second").join("result.json")]
                );
                assert_eq!(*visited.borrow(), [root.to_owned(), root.join("second"), root.join("first")]);
            } else {
                let Error::ArtifactFormat { path, message } = result.unwrap_err() else {
                    panic!("directory limit error")
                };
                assert_eq!(path, root);
                assert_eq!(message, format!("artifact tree exceeds the limit of {limit} directories"));
                assert_eq!(*visited.borrow(), if limit == 0 { Vec::new() } else { vec![root.to_owned()] });
            }
        }
        for limit in [0, 1, 2] {
            let result = find_artifact_files_with_io(
                root,
                |_| true,
                DiscoveryLimits {
                    directories: 1,
                    artifacts: limit,
                },
                |_| Ok(true),
                |_| {
                    Ok(["a", "z"].map(|name| {
                        Ok(MemoryEntry {
                            path: root.join(name),
                            kind: Ok(ArtifactKind::File),
                        })
                    }))
                },
            );
            if limit == 2 {
                assert_eq!(result.unwrap(), [root.join("a"), root.join("z")]);
            } else {
                let Error::ArtifactFormat { path, message } = result.unwrap_err() else {
                    panic!("file limit error")
                };
                assert_eq!(path, root);
                assert_eq!(message, format!("artifact tree exceeds the limit of {limit} matching files"));
            }
        }
    }

    #[test]
    fn november_artifact_discovery_propagates_each_error_with_its_own_path() {
        let root = Path::new("root");
        let missing = find_artifact_files_with_io(
            root,
            |_| panic!("missing tree has no files"),
            DiscoveryLimits {
                directories: 0,
                artifacts: 0,
            },
            |_| Ok(false),
            |_| -> io::Result<Vec<io::Result<MemoryEntry>>> { panic!("missing tree must not be opened") },
        )
        .unwrap();
        assert!(missing.is_empty());
        for stage in 0..4 {
            let result = find_artifact_files_with_io(
                root,
                |_| panic!("failed tree has no accepted files"),
                DiscoveryLimits {
                    directories: 1,
                    artifacts: 1,
                },
                |_| {
                    if stage == 0 {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "exists failure"))
                    } else {
                        Ok(true)
                    }
                },
                |_| {
                    if stage == 1 {
                        Err(io::Error::new(io::ErrorKind::PermissionDenied, "listing failure"))
                    } else if stage == 2 {
                        Ok(vec![Err(io::Error::new(io::ErrorKind::PermissionDenied, "entry failure"))])
                    } else {
                        Ok(vec![Ok(MemoryEntry {
                            path: root.join("entry"),
                            kind: Err(io::ErrorKind::PermissionDenied),
                        })])
                    }
                },
            );
            let Error::ArtifactIo { path, source } = result.unwrap_err() else {
                panic!("I/O error must be preserved")
            };
            assert_eq!(path, if stage == 3 { root.join("entry") } else { root.to_owned() });
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(
                source.to_string(),
                ["exists failure", "listing failure", "entry failure", "entry type failure"][stage]
            );
        }
    }

    #[test]
    fn november_artifact_discovery_depth_boundary_has_an_exact_error_without_directories() {
        let root = Path::new("root");
        for depth in [MAX_ARTIFACT_DIRECTORY_DEPTH, MAX_ARTIFACT_DIRECTORY_DEPTH + 1] {
            let visited = std::cell::Cell::new(0);
            let result = find_artifact_files_with_io(
                root,
                |_| true,
                DiscoveryLimits {
                    directories: 128,
                    artifacts: 1,
                },
                |_| Ok(true),
                |path| {
                    visited.set(visited.get() + 1);
                    Ok(vec![Ok(if path.components().count() - 1 < depth {
                        MemoryEntry {
                            path: path.join("d"),
                            kind: Ok(ArtifactKind::Directory),
                        }
                    } else {
                        MemoryEntry {
                            path: path.join("summary.json"),
                            kind: Ok(ArtifactKind::File),
                        }
                    })])
                },
            );
            let mut deepest = root.to_owned();
            for _ in 0..depth {
                deepest.push("d");
            }
            if depth == MAX_ARTIFACT_DIRECTORY_DEPTH {
                assert_eq!(result.unwrap(), [deepest.join("summary.json")]);
            } else {
                let Error::ArtifactFormat { path, message } = result.unwrap_err() else {
                    panic!("depth error")
                };
                assert_eq!(path, deepest);
                assert_eq!(message, "artifact directory nesting is too deep");
            }
            assert_eq!(visited.get(), MAX_ARTIFACT_DIRECTORY_DEPTH + 1);
        }
    }

    #[test]
    fn gamma_round2_regular_file_admission_forwards_exact_reader_policy_and_open_failures() {
        let path = Path::new("artifact.json");
        let events = std::cell::RefCell::new(Vec::new());
        let bytes = read_regular_file_with(
            path,
            3,
            "JSON",
            |input| {
                assert_eq!(input, path);
                events.borrow_mut().push("open");
                Ok(17)
            },
            |handle, input, limit, format| {
                assert_eq!(handle, 17);
                assert_eq!(input, path);
                assert_eq!(limit, 3);
                assert_eq!(format, "JSON");
                events.borrow_mut().push("read");
                Ok(b"abc".to_vec())
            },
        )
        .unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(*events.borrow(), ["open", "read"]);
        assert!(matches!(read_regular_file_with::<u8>(path, 3, "JSON",
            |_| Err(Error::Metadata("admission refused".into())), |_, _, _, _| panic!("admission failed")),
            Err(Error::Metadata(message)) if message == "admission refused"));
    }

    #[test]
    fn gamma_round2_bounded_file_checks_metadata_and_stream_growth_without_native_files() {
        let path = Path::new("artifact.json");
        assert_eq!(
            read_bounded_file_with(io::Cursor::new(b"abc"), path, 3, "JSON", |_| Ok(3)).unwrap(),
            b"abc"
        );
        let mut reader = io::Cursor::new(b"abcd");
        assert!(matches!(read_bounded_file_with(&mut reader, path, 3, "JSON", |_| Ok(4)),
            Err(Error::ArtifactFormat { path: failed, message }) if failed == path && message == "JSON file is 4 bytes; limit is 3 bytes"));
        assert_eq!(reader.position(), 0);
        assert!(
            matches!(read_bounded_file_with(io::Cursor::new(b"abcd"), path, 3, "JSON", |_| Ok(3)),
            Err(Error::ArtifactFormat { path: failed, message }) if failed == path && message == "JSON stream exceeds the 3-byte limit")
        );
        let mut reader = io::Cursor::new(b"abc");
        assert!(matches!(read_bounded_file_with(&mut reader, path, 3, "JSON",
            |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "metadata refused"))),
            Err(Error::ArtifactIo { path: failed, source }) if failed == path && source.kind() == io::ErrorKind::PermissionDenied && source.to_string() == "metadata refused"));
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn gamma_round2_artifact_environment_and_run_names_preserve_inputs_and_sequence() {
        let events = std::cell::RefCell::new(Vec::new());
        ArtifactDirectory::create_with_environment(
            Path::new("root"),
            |name| {
                events.borrow_mut().push(name.to_owned());
                Some(format!("home-{name}").into())
            },
            |root, criterion, gungraun| {
                assert_eq!(root, Path::new("root"));
                assert_eq!(criterion, Some("home-CRITERION_HOME".into()));
                assert_eq!(gungraun, Some("home-GUNGRAUN_HOME".into()));
                Ok(ArtifactDirectory::gamma_round2_in_memory_directory())
            },
        )
        .unwrap();
        assert_eq!(*events.borrow(), ["CRITERION_HOME", "GUNGRAUN_HOME"]);
        let sequence = AtomicU64::new(7);
        for expected in ["42000000000-17-7", "42000000000-17-8"] {
            assert_eq!(
                run_name_with(|| UNIX_EPOCH + std::time::Duration::from_secs(42), &sequence, || 17).unwrap(),
                expected
            );
        }
        assert_eq!(sequence.load(Ordering::Relaxed), 9);
        assert!(
            matches!(run_name_with(|| UNIX_EPOCH - std::time::Duration::from_secs(1), &sequence, || panic!("invalid time")),
            Err(Error::Metadata(message)) if !message.is_empty())
        );
        assert_eq!(sequence.load(Ordering::Relaxed), 9);
    }

    #[test]
    fn gamma_round2_artifact_creation_orders_homes_and_retains_each_distinct_lock() {
        for (criterion_home, gungraun_home) in [
            (None, None),
            (Some("criterion home"), None),
            (None, Some("gungraun home")),
            (Some("criterion home"), Some("gungraun home")),
            (Some("shared home"), Some("shared home")),
        ] {
            let events = std::cell::RefCell::new(Vec::new());
            let (run, criterion, gungraun, locks) = prepare_artifact_run(
                Path::new("root"),
                criterion_home.map(OsString::from),
                gungraun_home.map(OsString::from),
                || Ok("fixed-run".into()),
                |path| {
                    events.borrow_mut().push(("create", path.to_owned(), None));
                    Ok(())
                },
                |path| {
                    events.borrow_mut().push(("mark", path.to_owned(), None));
                    Ok(path.to_owned())
                },
                |runs, current| {
                    events.borrow_mut().push(("prune", runs.to_owned(), Some(current.to_owned())));
                    Ok(())
                },
            )
            .unwrap();
            let expected_run = Path::new("root").join("runs").join("fixed-run");
            let expected_criterion = criterion_home.map_or_else(
                || expected_run.join("criterion"),
                |home| Path::new(home).join(".metabench-runs").join("fixed-run"),
            );
            let expected_gungraun = gungraun_home.map_or_else(
                || expected_run.join("gungraun"),
                |home| Path::new(home).join(".metabench-runs").join("fixed-run"),
            );
            assert_eq!(run, expected_run);
            assert_eq!(criterion, expected_criterion);
            assert_eq!(gungraun, expected_gungraun);
            let mut expected = vec![
                ("create", run.clone(), None),
                ("mark", run.clone(), None),
                ("prune", Path::new("root").join("runs"), Some(run.clone())),
            ];
            let mut expected_locks = vec![run];
            for (home, current) in [(criterion_home, criterion), (gungraun_home, gungraun)] {
                if let Some(home) = home {
                    if !expected_locks.contains(&current) {
                        expected.push(("create", current.clone(), None));
                        expected.push(("mark", current.clone(), None));
                        expected_locks.push(current.clone());
                    }
                    expected.push(("prune", Path::new(home).join(".metabench-runs"), Some(current)));
                }
            }
            assert_eq!(*events.borrow(), expected);
            assert_eq!(locks, expected_locks);
        }
    }

    #[test]
    fn gamma_round2_artifact_creation_propagates_each_name_create_mark_and_prune_failure() {
        let expected = [
            "name", "create", "mark", "prune", "create", "mark", "prune", "create", "mark", "prune",
        ];
        for failing in 0..expected.len() {
            let events = std::cell::RefCell::new(Vec::new());
            let event = |name| {
                let index = events.borrow().len();
                events.borrow_mut().push(name);
                if index == failing {
                    Err(Error::Metadata(format!("phase {index}")))
                } else {
                    Ok(index)
                }
            };
            let result = prepare_artifact_run(
                Path::new("root"),
                Some("criterion".into()),
                Some("gungraun".into()),
                || event("name").map(|_| "fixed".into()),
                |_| event("create").map(|_| ()),
                |_| event("mark"),
                |_, _| event("prune").map(|_| ()),
            );
            assert!(matches!(result, Err(Error::Metadata(message)) if message == format!("phase {failing}")));
            assert_eq!(*events.borrow(), expected[..=failing]);
        }
    }

    #[test]
    fn gamma_round2_manifests_preserve_optional_status_and_discovered_identity_updates() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;
        let root = Path::new("run");
        let make_manifest = || EngineRunManifest {
            target: "bench".into(),
            engine: "criterion".into(),
            command_line: vec!["argument".into()],
            engine_version: None,
            artifact_root: "artifacts".into(),
            success: true,
            exit_code: Some(0),
            discovered_native_identities: vec!["obsolete".into()],
            native_to_canonical: BTreeMap::new(),
        };
        for status in [None, Some(ExitStatus::from_raw(0)), Some(ExitStatus::from_raw(1))] {
            write_run_manifest_with(
                root,
                Path::new("artifacts"),
                "bench",
                "criterion",
                &["case with spaces".into(), "".into()],
                status,
                |path, manifest| {
                    assert_eq!(path, root.join("manifests").join("criterion.json"));
                    assert_eq!(manifest.target, "bench");
                    assert_eq!(manifest.engine, "criterion");
                    assert_eq!(manifest.command_line, ["case with spaces", ""]);
                    assert!(manifest.engine_version.is_none());
                    assert_eq!(manifest.artifact_root, "artifacts");
                    assert_eq!(manifest.success, status.is_some_and(|status| status.success()));
                    assert_eq!(manifest.exit_code, status.and_then(|status| status.code()));
                    assert!(manifest.discovered_native_identities.is_empty());
                    assert!(manifest.native_to_canonical.is_empty());
                    Ok(())
                },
            )
            .unwrap();
        }
        let results = [("b", "canonical-b"), ("a", "first-a"), ("a", "last-a")].map(|(native, canonical)| NativeResult {
            identity: canonical.into(),
            native_identity: native.into(),
            source: "criterion".into(),
            metrics: BTreeMap::new(),
            raw_artifacts: vec![],
        });
        record_discovered_identities_with(
            root,
            "criterion",
            &results,
            |path| {
                assert_eq!(path, root.join("manifests").join("criterion.json"));
                Ok(make_manifest())
            },
            |path, manifest| {
                assert_eq!(path, root.join("manifests").join("criterion.json"));
                assert_eq!(manifest.discovered_native_identities, ["a", "b"]);
                assert_eq!(
                    manifest.native_to_canonical,
                    BTreeMap::from([("a".into(), "last-a".into()), ("b".into(), "canonical-b".into())])
                );
                assert_eq!(manifest.command_line, ["argument"]);
                assert!(manifest.success);
                Ok(())
            },
        )
        .unwrap();
        assert!(matches!(record_discovered_identities_with(root, "criterion", &results,
            |_| Err(Error::Metadata("manifest read refused".into())), |_, _| panic!("read failed")),
            Err(Error::Metadata(message)) if message == "manifest read refused"));
        assert!(matches!(write_run_manifest_with(root, root, "bench", "perf", &[], None,
            |_, _| Err(Error::Metadata("manifest write refused".into()))), Err(Error::Metadata(message)) if message == "manifest write refused"));
    }

    #[test]
    fn gamma_round2_native_artifact_discovery_errors_propagate_before_reading() {
        let root = Path::new("artifacts");
        assert!(matches!(parse_criterion_with(root, root, &[], None, |directory, predicate| {
            assert_eq!(directory, root); assert!(predicate(&root.join("new").join("benchmark.json")));
            assert!(!predicate(&root.join("base").join("benchmark.json")));
            Err(Error::Metadata("criterion discovery refused".into()))
        }, |_, _, _| panic!("discovery failed")), Err(Error::Metadata(message)) if message == "criterion discovery refused"));
        assert!(matches!(parse_gungraun_with(root, root, &[], |directory, predicate| {
            assert_eq!(directory, root); assert!(predicate(&root.join("summary.json"))); assert!(!predicate(&root.join("other.json")));
            Err(Error::Metadata("gungraun discovery refused".into()))
        }, |_, _, _| panic!("discovery failed")), Err(Error::Metadata(message)) if message == "gungraun discovery refused"));
        assert!(
            parse_criterion_with(root, root, &[], None, |_, _| Ok(vec![]), |_, _, _| panic!("no paths"))
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_gungraun_with(root, root, &[], |_, _| Ok(vec![]), |_, _, _| panic!("no paths"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn gamma_round2_artifact_text_reader_forwards_exact_byte_limit_and_utf8_errors() {
        let path = Path::new("counter.csv");
        assert_eq!(
            read_artifact_text_with(path, "CSV", |input, limit, format| {
                assert_eq!(input, path);
                assert_eq!(limit, 16 * 1024 * 1024);
                assert_eq!(format, "CSV");
                Ok("counter α".as_bytes().to_vec())
            })
            .unwrap(),
            "counter α"
        );
        assert!(matches!(read_artifact_text_with(path, "CSV", |_, _, _| Ok(vec![0xff])),
            Err(Error::ArtifactIo { path: failed, source }) if failed == path && source.kind() == io::ErrorKind::InvalidData));
        assert!(
            matches!(read_artifact_text_with(path, "CSV", |_, _, _| Err(Error::Metadata("read refused".into()))),
            Err(Error::Metadata(message)) if message == "read refused")
        );
    }

    #[test]
    fn gamma_round2_atomic_json_writer_preserves_parent_prefix_bytes_and_commit_error_boundaries() {
        struct MemoryTemporary {
            bytes: Vec<u8>,
            flush_error: bool,
        }
        impl io::Write for MemoryTemporary {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                if self.flush_error {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "flush refused"))
                } else {
                    Ok(())
                }
            }
        }
        for path in [
            PathBuf::from("report.json"),
            Path::new("nested").join("report.json"),
            PathBuf::new(),
        ] {
            let parent = if path.starts_with("nested") {
                Path::new("nested")
            } else {
                Path::new(".")
            };
            for failing in [None, Some("mkdir"), Some("temporary"), Some("flush"), Some("sync"), Some("persist")] {
                let events = std::cell::RefCell::new(Vec::new());
                let result = write_json_with(
                    &path,
                    &serde_json::json!({"count": 17}),
                    |directory| {
                        assert_eq!(directory, parent);
                        events.borrow_mut().push("mkdir");
                        if failing == Some("mkdir") {
                            Err(io::Error::new(io::ErrorKind::PermissionDenied, "mkdir refused"))
                        } else {
                            Ok(())
                        }
                    },
                    |directory, prefix| {
                        assert_eq!(directory, parent);
                        assert_eq!(prefix, ".metabench-");
                        events.borrow_mut().push("temporary");
                        if failing == Some("temporary") {
                            Err(io::Error::new(io::ErrorKind::PermissionDenied, "temporary refused"))
                        } else {
                            Ok(MemoryTemporary {
                                bytes: vec![],
                                flush_error: failing == Some("flush"),
                            })
                        }
                    },
                    |temporary| {
                        assert_eq!(temporary.bytes, b"{\n  \"count\": 17\n}");
                        events.borrow_mut().push("sync");
                        if failing == Some("sync") {
                            Err(io::Error::new(io::ErrorKind::PermissionDenied, "sync refused"))
                        } else {
                            Ok(())
                        }
                    },
                    |temporary, destination| {
                        assert_eq!(destination, path);
                        assert_eq!(temporary.bytes, b"{\n  \"count\": 17\n}");
                        events.borrow_mut().push("persist");
                        if failing == Some("persist") {
                            Err(io::Error::new(io::ErrorKind::PermissionDenied, "persist refused"))
                        } else {
                            Ok(())
                        }
                    },
                );
                if let Some(phase) = failing {
                    assert!(matches!(result, Err(Error::ArtifactIo { path: failed, source })
                        if failed == if matches!(phase, "mkdir" | "temporary") { parent } else { &path } && source.to_string() == format!("{phase} refused")));
                    let expected: &[&str] = match phase {
                        "mkdir" => &["mkdir"],
                        "temporary" | "flush" => &["mkdir", "temporary"],
                        "sync" => &["mkdir", "temporary", "sync"],
                        "persist" => &["mkdir", "temporary", "sync", "persist"],
                        _ => unreachable!(),
                    };
                    assert_eq!(*events.borrow(), expected);
                } else {
                    result.unwrap();
                    assert_eq!(*events.borrow(), ["mkdir", "temporary", "sync", "persist"]);
                }
            }
        }
    }

    #[test]
    fn gamma_round2_atomic_json_writer_returns_serialization_error_before_sync_or_publication() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("serialization refused"))
            }
        }
        assert!(
            matches!(write_json_with(Path::new("invalid.json"), &Invalid, |_| Ok(()), |_, _| Ok(Vec::new()),
            |_| panic!("serialization failed"), |_, _| panic!("serialization failed")),
            Err(Error::ArtifactJson { path, source }) if path == Path::new("invalid.json") && source.to_string() == "serialization refused")
        );
    }

    #[test]
    fn gamma_round2_criterion_inputs_use_new_artifacts_and_preserve_units_and_metrics() {
        let root = Path::new("artifacts");
        let path = root.join("new").join("benchmark.json");
        assert!(criterion_artifact_path(&path));
        assert!(!criterion_artifact_path(&root.join("base").join("benchmark.json")));
        assert!(!criterion_artifact_path(&root.join("new").join("estimates.json")));
        let metadata = br#"{"group_id":"native","function_id":"case","value_str":"7","throughput":{"Bytes":123}}"#;
        let estimate =
            br#"{"mean":{"point_estimate":10,"confidence_interval":{"confidence_level":0.95,"lower_bound":9,"upper_bound":11}}}"#;
        let mut read_paths = Vec::new();
        let results = parse_criterion_paths(vec![path.clone()], root, &[], Some("ns"), |read_path, limit, format| {
            read_paths.push(read_path.to_owned());
            assert_eq!(limit, MAX_ARTIFACT_JSON_BYTES);
            assert_eq!(format, "JSON");
            Ok(if read_path == path { metadata.to_vec() } else { estimate.to_vec() })
        })
        .unwrap();
        assert_eq!(read_paths, [path.clone(), path.with_file_name("estimates.json")]);
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.identity, "native/case/7");
        assert_eq!(result.native_identity, "native/case/7");
        assert_eq!(result.source, "criterion");
        assert_eq!(result.raw_artifacts, ["new/benchmark.json", "new/estimates.json"]);
        let time = &result.metrics["time"];
        assert_eq!(time.value.as_f64().to_bits(), 10.0_f64.to_bits());
        assert_eq!(time.unit.as_deref(), Some("ns"));
        assert_eq!(time.statistic.as_deref(), Some("mean"));
        assert_eq!(time.confidence_level, Some(0.95));
        assert_eq!((time.lower_bound, time.upper_bound), (Some(9.0), Some(11.0)));
        assert_eq!(time.display_name, "Time");
        assert_eq!(
            result.metrics["declared_throughput.bytes"].value.as_f64().to_bits(),
            123.0_f64.to_bits()
        );
        assert_eq!(result.metrics["declared_throughput.bytes"].display_name, "Input bytes");
        assert!(
            matches!(parse_criterion_paths(vec![path.clone()], root, &[], None, |read_path, _, _| {
            if read_path == path { Ok(metadata.to_vec()) } else { Err(Error::Metadata("estimates read failed".into())) }
        }), Err(Error::Metadata(message)) if message == "estimates read failed")
        );
    }

    #[test]
    fn gamma_round2_gungraun_inputs_validate_read_policy_and_propagate_read_failure() {
        let root = Path::new("artifacts");
        let path = root.join("summary.json");
        let results = parse_gungraun_paths(vec![path.clone()], root, &[], |read_path, limit, format| {
            assert_eq!(read_path, path);
            assert_eq!(limit, MAX_ARTIFACT_JSON_BYTES);
            assert_eq!(format, "JSON");
            Ok(GUNGRAUN_SUMMARY_FIXTURE.as_bytes().to_vec())
        })
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "gungraun.callgrind");
        assert_eq!(results[0].raw_artifacts, ["summary.json"]);
        assert!(!results[0].metrics.is_empty());
        assert!(
            matches!(parse_gungraun_paths(vec![path], root, &[], |_, _, _| Err(Error::Metadata("summary read failed".into()))),
            Err(Error::Metadata(message)) if message == "summary read failed")
        );
    }

    #[test]
    fn gamma_round2_perf_text_preserves_rows_identity_and_zero_counts() {
        let root = Path::new("artifacts");
        let path = root.join("perf.json");
        let result = parse_perf_text(
            "\n \t\n{\"counter-value\":0,\"event\":\"cycles:u\"}\n\n{\"counter-value\":\"12.5\",\"event\":\"instructions:u\"}\n",
            &path,
            root,
            "canonical/case".into(),
            "native::case".into(),
        )
        .unwrap();
        assert_eq!(result.identity, "canonical/case");
        assert_eq!(result.native_identity, "native::case");
        assert_eq!(result.source, "perf");
        assert_eq!(result.raw_artifacts, ["perf.json"]);
        assert_eq!(result.metrics.len(), 2);
        assert_eq!(result.metrics["cycles"].value.as_f64().to_bits(), 0.0_f64.to_bits());
        assert_eq!(result.metrics["instructions"].value.as_f64().to_bits(), 12.5_f64.to_bits());
        assert_eq!(result.metrics["cycles"].display_name, "HW Cycles");
        assert_eq!(result.metrics["cycles"].direction, MetricDirection::LowerIsBetter);
    }

    #[test]
    fn gamma_round2_perf_text_errors_keep_type_path_and_exact_diagnostic() {
        let path = Path::new("perf.json");
        for (contents, expected) in [
            ("", "contains no perf event records"),
            ("\n \t\n", "contains no perf event records"),
            (
                "{\"counter-value\":-1,\"event\":\"cycles:u\"}",
                "line 1 event cycles:u has invalid counter value -1",
            ),
            (
                "{\"counter-value\":\"NaN\",\"event\":\"cycles:u\"}",
                "line 1 event cycles:u has invalid counter value NaN",
            ),
            (
                "{\"counter-value\":\"inf\",\"event\":\"cycles:u\"}",
                "line 1 event cycles:u has invalid counter value inf",
            ),
            (
                "{\"counter-value\":null,\"event\":\"cycles:u\"}",
                "line 1 event cycles:u has non-numeric counter value null",
            ),
            (
                "{\"counter-value\":1,\"event\":\"cycles:u\"}\n{\"counter-value\":2,\"event\":\"cycles\"}",
                "perf event cycles occurs more than once",
            ),
        ] {
            assert!(
                matches!(
                    parse_perf_text(contents, path, Path::new(""), "case".into(), "native".into()),
                    Err(Error::ArtifactFormat { path: failed, message }) if failed == path && message == expected
                ),
                "{contents}"
            );
        }
        assert!(matches!(
            parse_perf_text("not JSON", path, Path::new(""), "case".into(), "native".into()),
            Err(Error::ArtifactJson { path: failed, .. }) if failed == path
        ));
    }

    #[test]
    fn gamma_round2_vtune_text_preserves_columns_aggregation_and_metric_source() {
        let root = Path::new("artifacts");
        let path = root.join("vtune.csv");
        let result = parse_vtune_text(
            "Samples,Hardware Event Count:Self,Hardware Event Type\nignored,\"1,234\",lowerCase\n \t\nignored,6,lowerCase\nignored,0,anotherEvent\n",
            &path, root, "canonical/case".into(), "native::case".into(),
        ).unwrap();
        assert_eq!(result.identity, "canonical/case");
        assert_eq!(result.native_identity, "native::case");
        assert_eq!(result.source, "vtune");
        assert_eq!(result.raw_artifacts, ["vtune.csv"]);
        assert_eq!(result.metrics.len(), 2);
        assert_eq!(result.metrics["lowerCase"].value.as_f64().to_bits(), 1_240.0_f64.to_bits());
        assert_eq!(result.metrics["anotherEvent"].value.as_f64().to_bits(), 0.0_f64.to_bits());
        assert_eq!(result.metrics["lowerCase"].display_name, "lowerCase");
        assert_eq!(result.metrics["anotherEvent"].display_name, "anotherEvent");
        assert_eq!(result.metrics["lowerCase"].direction, MetricDirection::LowerIsBetter);
    }

    #[test]
    fn gamma_round2_vtune_text_rejects_empty_malformed_and_nonfinite_reports() {
        let header = "Hardware Event Type,Hardware Event Count:Self";
        let path = Path::new("vtune.csv");
        for contents in [String::new(), header.to_owned(), format!("{header}\n \t\n")] {
            assert!(matches!(
                parse_vtune_text(&contents, path, Path::new(""), "case".into(), "native".into()),
                Err(Error::ArtifactFormat { path: failed, message })
                    if failed == path && message == "contains no vtune event records"
            ));
        }
        for (row, expected) in [
            ("cycles", "line 2 has 1 field(s); expected 2 matching the header"),
            ("cycles,nope", "line 2 event cycles has non-numeric count nope"),
            ("cycles,-1", "line 2 event cycles has invalid count -1"),
            ("cycles,NaN", "line 2 event cycles has invalid count NaN"),
            ("cycles,inf", "line 2 event cycles has invalid count inf"),
            (
                "cycles,1e308\ncycles,1e308",
                "line 3 event cycles aggregate count inf is not finite",
            ),
        ] {
            assert!(
                matches!(
                    parse_vtune_text(&format!("{header}\n{row}"), path, Path::new(""), "case".into(), "native".into()),
                    Err(Error::ArtifactFormat { path: failed, message }) if failed == path && message == expected
                ),
                "{row}"
            );
        }
        assert!(matches!(
            parse_vtune_text("name,count", path, Path::new(""), "case".into(), "native".into()),
            Err(Error::ArtifactFormat { path: failed, message })
                if failed == path && message == "header is missing the \"Hardware Event Type\" or \"Hardware Event Count:Self\" column"
        ));
    }

    #[test]
    fn gamma_survivor_throughput_and_counter_shapes() {
        use serde_json::json;

        use super::*;
        for (value, expected) in [
            (json!({"Bytes": 7}), Some(("declared_throughput.bytes".into(), 7))),
            (
                json!({"ElementsAndBytes": {"elements": 3, "bytes": 8}}),
                Some(("declared_throughput.elements_and_bytes".into(), 3)),
            ),
            (json!({"Other": {"elements": 3}}), None),
            (json!({"ElementsAndBytes": {"bytes": 8}}), None),
            (json!({"ElementsAndBytes": {"elements": "3"}}), None),
            (json!({"Bytes": -1}), None),
            (json!({"Bytes": 1.5}), None),
            (json!({"Bytes": false}), None),
            (json!({}), None),
            (json!(null), None),
        ] {
            assert_eq!(parse_criterion_throughput(&value), expected, "{value}");
        }
        for value in [json!(null), json!({}), json!([]), json!(false)] {
            assert_eq!(perf_counter_value(&value), None);
        }
        assert_eq!(perf_counter_value(&json!("17.5")), Some(17.5));
        assert_eq!(split_csv_fields("x,\"a\"\"b\",z"), ["x", "a\"b", "z"]);
        assert_eq!(snake_case("ElementsAndBytes"), "elements_and_bytes");
    }

    #[test]
    fn gamma_survivor_identity_prefix_resolution() {
        use super::*;
        let index = CanonicalIdentityIndex {
            criterion: BTreeMap::from([("native".into(), "canonical".into()), ("native/exact".into(), "specific".into())]),
            gungraun: BTreeMap::new(),
        };
        assert_eq!(
            index.resolve_registered_criterion("native/child/32"),
            Some("canonical/child/32".into())
        );
        assert_eq!(index.resolve_registered_criterion("native/exact/32"), Some("specific/32".into()));
        assert_eq!(index.resolve_registered_criterion("unregistered/child"), None);
    }

    #[test]
    fn gamma_survivor_criterion_estimate_error_messages() {
        use super::*;
        let path = Path::new("estimate.json");
        for (point, lower, upper, message) in [
            (f64::NAN, 0.0, 1.0, "contains a non-finite Criterion estimate".to_owned()),
            (1.0, 0.0, f64::INFINITY, "contains a non-finite Criterion estimate".to_owned()),
            (
                1.0,
                -2.0,
                -1.0,
                "Criterion estimates must be non-negative; found -2..1..-1".to_owned(),
            ),
            (
                1.0,
                0.0,
                -1.0,
                "Criterion estimates must be non-negative; found 0..1..-1".to_owned(),
            ),
        ] {
            let estimate = CriterionEstimate {
                point_estimate: point,
                confidence_interval: CriterionConfidenceInterval {
                    confidence_level: 0.95,
                    lower_bound: lower,
                    upper_bound: upper,
                },
            };
            assert!(matches!(validate_criterion_estimate(path, &estimate),
                Err(Error::ArtifactFormat { path: failed, message: actual }) if failed == path && actual == message));
        }
    }

    #[test]
    fn gamma_survivor_nonfinite_native_metrics_are_omitted() {
        use super::*;
        let mut metrics: MetricResults<String> = serde_json::from_value(serde_json::json!({
            "finite": {"values": {"new": 1.5}},
            "nonfinite": {"values": {"new": 2.5}}
        }))
        .unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            metrics.0.get_mut("nonfinite").unwrap().values = EitherOrBoth::Left(GungraunMetric::Float(value));
            let copied = copy_metrics("custom", &metrics);
            assert_eq!(copied.len(), 1);
            assert_eq!(copied["finite"].value, crate::report::MetricValue::Float(1.5));
        }
    }

    #[test]
    fn gamma_survivor_bounded_stream_error_and_read_ahead() {
        use super::*;
        struct ScriptedReader {
            remaining: usize,
        }
        impl io::Read for ScriptedReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "scripted"));
                }
                let count = self.remaining.min(buf.len());
                buf[..count].fill(b'x');
                self.remaining -= count;
                Ok(count)
            }
        }
        let path = Path::new("in-memory.json");
        assert!(matches!(read_bounded_stream(ScriptedReader { remaining: 4 }, path, 3, "JSON"),
            Err(Error::ArtifactFormat { path: failed, message })
                if failed == path && message == "JSON stream exceeds the 3-byte limit"));
        assert!(matches!(read_bounded_stream(ScriptedReader { remaining: 0 }, path, 3, "JSON"),
            Err(Error::ArtifactIo { path: failed, source })
                if failed == path && source.kind() == io::ErrorKind::BrokenPipe));
        assert_eq!(read_bounded_stream(&b"abc"[..], path, 3, "JSON").unwrap(), b"abc");
    }

    #[test]
    fn gamma_survivor_artifact_paths_without_io() {
        use super::*;
        let directory = ArtifactDirectory {
            root: "root".into(),
            run: "run".into(),
            criterion: "criterion".into(),
            gungraun: "gungraun".into(),
            _active_locks: vec![],
        };
        assert_eq!(directory.perf_path(17), Path::new("run").join("perf").join("17.jsonl"));
    }

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
        assert_eq!(file.metadata().unwrap().length, 4);
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

    fn parse_gungraun_fixture(summary: &serde_json::Value) -> Result<Vec<NativeResult>, Error> {
        parse_gungraun_summary(
            &serde_json::to_vec(summary).unwrap(),
            Path::new("summary.json"),
            Path::new(""),
            &CanonicalIdentityIndex::new(&[]),
        )
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
        let mut summary: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        summary["module_path"] = "unrelated::path".into();
        summary["profiles"][0]["data"]["total"]["metrics"]["Ir"] = serde_json::json!({
            "change": {"diff_pct": "42.0", "factor": "1.42"},
            "values": {"new": 100, "old": 70},
        });
        let results = parse_gungraun_fixture(&summary).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].native_identity, "adapter/metabench_adapter/case_0");
        assert_eq!(results[0].source, "gungraun.callgrind");
        assert_eq!(results[0].raw_artifacts, ["summary.json"]);
        assert_eq!(results[0].metrics["Ir"].value, crate::report::MetricValue::Integer(100));
        assert_eq!(results[0].metrics["Ir"].direction, MetricDirection::LowerIsBetter);
        assert_eq!(results[0].metrics["Ir"].change_percentage, None);
    }

    #[test]
    fn gungraun_parses_all_supported_profiles_with_mapping_and_provenance() {
        let mut summary: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        let mut profiles = Vec::new();
        for (tool, metrics) in [
            ("Callgrind", serde_json::json!({"Ir": {"values": {"new": 100}}})),
            (
                "Cachegrind",
                serde_json::json!({
                    "Ir": {"values": {"new": 200, "old": 150}},
                    "L1HitRate": {"values": {"new": 98.5}},
                }),
            ),
            (
                "DHAT",
                serde_json::json!({
                    "TotalBytes": {"values": {"new": 4096, "old": 2048}},
                    "TotalBlocks": {"values": {"new": 64}},
                }),
            ),
        ] {
            let mut profile = summary["profiles"][0].clone();
            profile["tool"] = tool.into();
            profile["data"]["total"]["metrics"] = metrics;
            profiles.push(profile);
        }
        summary["profiles"] = profiles.into();
        let root = Path::new("runs");
        let path = root.join("gungraun").join("summary.json");
        let identities = [BenchmarkIdentity::__new("group", "bench", "adapter", "metabench_adapter", true)];
        let results = parse_gungraun_summary(
            &serde_json::to_vec(&summary).unwrap(),
            &path,
            root,
            &CanonicalIdentityIndex::new(&identities),
        )
        .unwrap();

        assert_eq!(results.len(), 3);
        for (result, source) in results.iter().zip(["gungraun.callgrind", "gungraun.cachegrind", "gungraun.dhat"]) {
            assert_eq!(result.source, source);
            assert_eq!(result.native_identity, "adapter/metabench_adapter/case_0");
            assert_eq!(result.identity, "group/bench/case_0");
            assert_eq!(result.raw_artifacts, ["gungraun/summary.json"]);
        }
        assert_eq!(results[0].metrics["Ir"].value, crate::report::MetricValue::Integer(100));
        let cachegrind = &results[1].metrics;
        assert_eq!(cachegrind.len(), 2);
        assert_eq!(cachegrind["Ir"].value, crate::report::MetricValue::Integer(200));
        assert_eq!(cachegrind["Ir"].direction, MetricDirection::LowerIsBetter);
        assert_eq!(cachegrind["L1HitRate"].value, crate::report::MetricValue::Float(98.5));
        assert_eq!(cachegrind["L1HitRate"].direction, MetricDirection::HigherIsBetter);
        let dhat = &results[2].metrics;
        assert_eq!(dhat.len(), 2);
        assert_eq!(dhat["TotalBytes"].value, crate::report::MetricValue::Integer(4096));
        assert_eq!(dhat["TotalBlocks"].value, crate::report::MetricValue::Integer(64));
        assert_eq!(dhat["TotalBytes"].direction, MetricDirection::LowerIsBetter);
        assert_eq!(dhat["TotalBytes"].change_percentage, None);
    }

    #[test]
    fn gungraun_preserves_current_metric_types_and_ignores_old_values() {
        let mut summary: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        summary["profiles"][0]["data"]["total"]["metrics"] = serde_json::json!({
            "Ir": {"values": {"new": u64::MAX, "old": 1}},
            "L1HitRate": {"values": {"new": 99.5}},
            "RamHits": {"values": {"old": 5}},
        });
        let results = parse_gungraun_fixture(&summary).unwrap();
        assert_eq!(results.len(), 1);
        let metrics = &results[0].metrics;
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
            assert!(matches!(
                parse_gungraun_fixture(&summary).unwrap_err(),
                Error::ArtifactFormat { path: actual_path, message }
                    if actual_path == path && message == format!("unsupported Gungraun summary schema {version}")
            ));
        }
        for profiles in [serde_json::json!([]), serde_json::json!([{"tool": "FutureTool"}])] {
            let mut summary = source.clone();
            summary["profiles"] = profiles;
            assert!(matches!(
                parse_gungraun_fixture(&summary).unwrap_err(),
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
            assert!(matches!(
                parse_gungraun_fixture(&summary).unwrap_err(),
                Error::ArtifactFormat { path: actual_path, message }
                    if actual_path == path
                        && message == format!("Gungraun {} is not a supported benchmark measurement tool", tool.to_ascii_lowercase())
            ));
        }
    }

    #[test]
    fn gungraun_rejects_supported_profiles_without_current_metrics() {
        let source: serde_json::Value = serde_json::from_str(GUNGRAUN_SUMMARY_FIXTURE).unwrap();
        for (tool, name, metric) in [
            ("Callgrind", "callgrind", "Ir"),
            ("Cachegrind", "cachegrind", "Ir"),
            ("DHAT", "dhat", "TotalBytes"),
        ] {
            for metrics in [serde_json::json!({}), serde_json::json!({metric: {"values": {"old": 5}}})] {
                let mut summary = source.clone();
                summary["profiles"][0]["tool"] = tool.into();
                summary["profiles"][0]["data"]["total"]["metrics"] = metrics;
                assert!(matches!(
                    parse_gungraun_fixture(&summary).unwrap_err(),
                    Error::ArtifactFormat { path, message }
                        if path == Path::new("summary.json")
                            && message == format!("Gungraun {name} summary contains no benchmark metrics")
                ));
            }
        }
    }

    #[test]
    fn gungraun_rejects_malformed_input() {
        assert!(matches!(
            parse_gungraun(&fixture("gungraun/malformed"), Path::new(""), &[]),
            Err(Error::ArtifactFormat { .. })
        ));
        assert!(parse_gungraun(&fixture("gungraun/empty"), Path::new(""), &[]).unwrap().is_empty());

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
