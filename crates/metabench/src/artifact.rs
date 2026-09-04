// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use alloc_tracker::Report;
use gungraun_summary::either_or_both::EitherOrBoth;
use gungraun_summary::v6::{BenchmarkSummary, EventKind, Metric, MetricsSummary, ToolMetricSummary, ValgrindTool};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_128;

use crate::Error;
use crate::report::{AllocationMetrics, CallgrindMetrics, PerfCounter, PerfMetrics, WallClockMetrics};

pub(crate) const ARTIFACT_DIR_ENV: &str = "METABENCH_INTERNAL_ARTIFACT_DIR";
const MAX_ARTIFACT_DIRECTORY_DEPTH: usize = 64;

#[derive(Debug)]
pub(crate) struct ArtifactDirectory {
    directory: tempfile::TempDir,
}

impl ArtifactDirectory {
    pub(crate) fn create() -> Result<Self, Error> {
        let directory = tempfile::Builder::new()
            .prefix("metabench-")
            .tempdir()
            .map_err(|source| Error::ArtifactIo {
                path: std::env::temp_dir(),
                source,
            })?;
        Ok(Self { directory })
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn perf_path(&self, benchmark: &str) -> PathBuf {
        perf_path(self.path(), benchmark)
    }

    pub(crate) fn allocation_path(&self) -> PathBuf {
        allocation_path(self.path())
    }

    pub(crate) fn criterion_path(&self) -> PathBuf {
        self.path().join("criterion")
    }

    pub(crate) fn gungraun_path(&self) -> PathBuf {
        self.path().join("gungraun")
    }
}

pub(crate) fn perf_path(root: &Path, benchmark: &str) -> PathBuf {
    root.join("perf").join(format!("{}.jsonl", artifact_key(benchmark)))
}

pub(crate) fn allocation_path(root: &Path) -> PathBuf {
    root.join("allocations.json")
}

#[derive(Debug, Deserialize)]
struct CriterionMetadata {
    group_id: String,
    function_id: Option<String>,
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

pub(crate) fn parse_criterion(root: &Path) -> Result<BTreeMap<String, WallClockMetrics>, Error> {
    let metadata_paths = find_artifact_files(root, |path| {
        path.file_name().is_some_and(|name| name == "benchmark.json")
            && path.parent().and_then(Path::file_name).is_some_and(|name| name == "new")
    })?;
    let mut metrics = BTreeMap::new();
    for metadata_path in metadata_paths {
        let metadata = read_json::<CriterionMetadata>(&metadata_path)?;
        let function = metadata.function_id.ok_or_else(|| Error::ArtifactFormat {
            path: metadata_path.clone(),
            message: "Criterion result has no function_id".to_owned(),
        })?;
        let benchmark = format!("{}/{function}", metadata.group_id);
        let estimates_path = metadata_path.with_file_name("estimates.json");
        let estimates = read_json::<CriterionEstimates>(&estimates_path)?;
        let median = estimates.median;
        let wall_clock = WallClockMetrics::new(
            criterion_duration(&estimates_path, median.point_estimate)?,
            criterion_duration(&estimates_path, median.confidence_interval.lower_bound)?,
            criterion_duration(&estimates_path, median.confidence_interval.upper_bound)?,
            None,
        );
        if metrics.insert(benchmark.clone(), wall_clock).is_some() {
            return Err(Error::ArtifactFormat {
                path: metadata_path,
                message: format!("Criterion benchmark {benchmark} occurs more than once"),
            });
        }
    }
    if metrics.is_empty() {
        return Err(Error::ArtifactFormat {
            path: root.to_owned(),
            message: "contains no Criterion benchmark results".to_owned(),
        });
    }
    Ok(metrics)
}

fn criterion_duration(path: &Path, nanoseconds: f64) -> Result<Duration, Error> {
    if !nanoseconds.is_finite() || nanoseconds < 0.0 {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("invalid Criterion duration {nanoseconds}"),
        });
    }
    Duration::try_from_secs_f64(nanoseconds / 1_000_000_000.0).map_err(|error| Error::ArtifactFormat {
        path: path.to_owned(),
        message: error.to_string(),
    })
}

pub(crate) fn parse_callgrind(root: &Path, benchmark_names: &[String]) -> Result<BTreeMap<String, CallgrindMetrics>, Error> {
    let manifest_path = root.join("metabench-identities.json");
    let manifest = if manifest_path.try_exists().map_err(|source| Error::ArtifactIo {
        path: manifest_path.clone(),
        source,
    })? {
        let identities = read_json::<Vec<String>>(&manifest_path)?;
        validate_gungraun_manifest(&manifest_path, &identities, benchmark_names)?;
        Some(identities)
    } else {
        None
    };
    let identities = manifest.as_deref().unwrap_or(benchmark_names);
    let summary_paths = find_artifact_files(root, |path| path.file_name().is_some_and(|name| name == "summary.json"))?;
    let mut metrics = BTreeMap::new();
    for path in summary_paths {
        let summary = gungraun_summary::v6::parse(&path).map_err(|error| Error::ArtifactFormat {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let index = callgrind_index(&path, &summary)?;
        let benchmark = identities.get(index).ok_or_else(|| Error::ArtifactFormat {
            path: path.clone(),
            message: format!("Gungraun case index {index} is out of range"),
        })?;
        let callgrind = callgrind_metrics(&path, &summary)?;
        if metrics.insert(benchmark.clone(), callgrind).is_some() {
            return Err(Error::ArtifactFormat {
                path,
                message: format!("Gungraun case index {index} occurs more than once"),
            });
        }
    }
    if metrics.len() != benchmark_names.len() {
        return Err(Error::ArtifactFormat {
            path: root.to_owned(),
            message: format!("expected {} Gungraun summaries, found {}", benchmark_names.len(), metrics.len()),
        });
    }
    Ok(metrics)
}

fn validate_gungraun_manifest(path: &Path, identities: &[String], benchmark_names: &[String]) -> Result<(), Error> {
    let mut unique = BTreeSet::new();
    for identity in identities {
        if !unique.insert(identity) {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("Gungraun identity {identity} occurs more than once"),
            });
        }
    }

    let selected = benchmark_names.iter().collect::<BTreeSet<_>>();
    if identities.len() != benchmark_names.len() || unique != selected {
        let missing = selected.difference(&unique).copied().collect::<Vec<_>>();
        let unexpected = unique.difference(&selected).copied().collect::<Vec<_>>();
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!(
                "Gungraun identities do not exactly match selected benchmarks \
                 (expected {}, found {}; missing: {missing:?}; unexpected: {unexpected:?})",
                benchmark_names.len(),
                identities.len()
            ),
        });
    }
    Ok(())
}

fn callgrind_index(path: &Path, summary: &BenchmarkSummary) -> Result<usize, Error> {
    if summary.function_name != "metabench_adapter" {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("unexpected Gungraun function {}", summary.function_name),
        });
    }
    let id = summary.id.as_deref().ok_or_else(|| Error::ArtifactFormat {
        path: path.to_owned(),
        message: "Gungraun summary has no case id".to_owned(),
    })?;
    id.strip_prefix("case_")
        .ok_or_else(|| Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("unexpected Gungraun case id {id}"),
        })?
        .parse()
        .map_err(|error: std::num::ParseIntError| Error::ArtifactFormat {
            path: path.to_owned(),
            message: error.to_string(),
        })
}

fn callgrind_metrics(path: &Path, summary: &BenchmarkSummary) -> Result<CallgrindMetrics, Error> {
    let profile = summary
        .profiles
        .0
        .iter()
        .find(|profile| profile.tool == ValgrindTool::Callgrind)
        .ok_or_else(|| Error::ArtifactFormat {
            path: path.to_owned(),
            message: "summary contains no Callgrind profile".to_owned(),
        })?;
    let ToolMetricSummary::Callgrind(metrics) = &profile.summaries.total.summary else {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "Callgrind profile has an unexpected metric type".to_owned(),
        });
    };
    let mut result = CallgrindMetrics::new(
        required_current_u64(path, metrics, EventKind::Ir)?,
        required_current_u64(path, metrics, EventKind::L1hits)?,
        required_current_u64(path, metrics, EventKind::RamHits)?,
    );
    result.instruction_shift_percentage = metrics
        .0
        .get(&EventKind::Ir)
        .and_then(|metric| metric.diffs.as_ref())
        .map(|diffs| diffs.diff_pct);
    Ok(result)
}

fn required_current_u64(path: &Path, metrics: &MetricsSummary<EventKind>, event: EventKind) -> Result<u64, Error> {
    let metric = metrics.0.get(&event).ok_or_else(|| Error::ArtifactFormat {
        path: path.to_owned(),
        message: format!("Callgrind metric {event} is missing"),
    })?;
    let current = match &metric.metrics {
        EitherOrBoth::Left(current) | EitherOrBoth::Both(current, _) => current,
        EitherOrBoth::Right(_) => {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("Callgrind metric {event} has no current value"),
            });
        }
    };
    match current {
        Metric::Int(value) => Ok(*value),
        Metric::Float(value) => Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!("Callgrind metric {event} unexpectedly contains float {value}"),
        }),
    }
}

fn find_artifact_files(root: &Path, is_match: impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>, Error> {
    let root_metadata = fs::symlink_metadata(root).map_err(|source| Error::ArtifactIo {
        path: root.to_owned(),
        source,
    })?;
    if root_metadata.file_type().is_symlink() {
        return Err(Error::ArtifactFormat {
            path: root.to_owned(),
            message: "artifact root must not be a symbolic link".to_owned(),
        });
    }

    let mut pending = vec![(root.to_owned(), 0_usize)];
    let mut output = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| Error::ArtifactIo {
            path: directory.clone(),
            source,
        })?;
        let mut entries = entries
            .map(|entry| {
                entry.map_err(|source| Error::ArtifactIo {
                    path: directory.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_unstable_by_key(fs::DirEntry::file_name);

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| Error::ArtifactIo {
                path: path.clone(),
                source,
            })?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if depth == MAX_ARTIFACT_DIRECTORY_DEPTH {
                    return Err(Error::ArtifactFormat {
                        path,
                        message: format!("artifact directory depth exceeds {MAX_ARTIFACT_DIRECTORY_DEPTH}"),
                    });
                }
                pending.push((path, depth + 1));
            } else if file_type.is_file() && is_match(&path) {
                output.push(path);
            }
        }
    }
    output.sort_unstable();
    Ok(output)
}

#[derive(Debug, Deserialize)]
struct PerfStatRecord {
    #[serde(rename = "counter-value")]
    counter_value: String,
    event: String,
    #[serde(rename = "event-runtime")]
    event_runtime: Option<u64>,
    #[serde(rename = "pcnt-running")]
    percent_running: Option<f64>,
}

pub(crate) fn parse_perf(path: &Path) -> Result<PerfMetrics, Error> {
    let contents = fs::read_to_string(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    let mut events = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str::<PerfStatRecord>(line).map_err(|source| Error::ArtifactJson {
            path: path.to_owned(),
            source,
        })?;
        let value = record.counter_value.parse::<f64>().map_err(|_error| Error::ArtifactFormat {
            path: path.to_owned(),
            message: format!(
                "line {} event {} has non-numeric counter value {:?}",
                index + 1,
                record.event,
                record.counter_value
            ),
        })?;
        if !value.is_finite() || value < 0.0 {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!(
                    "line {} event {} has invalid counter value {:?}",
                    index + 1,
                    record.event,
                    record.counter_value
                ),
            });
        }
        let event = record.event.strip_suffix(":u").unwrap_or(&record.event);
        let time_running = record.event_runtime.map(Duration::from_nanos);
        let time_enabled = match (record.event_runtime, record.percent_running) {
            (Some(running), Some(percent)) if percent.is_finite() && percent > 0.0 && percent <= 100.0 => {
                let seconds = Duration::from_nanos(running).as_secs_f64() * 100.0 / percent;
                Some(Duration::try_from_secs_f64(seconds).map_err(|error| Error::ArtifactFormat {
                    path: path.to_owned(),
                    message: format!(
                        "line {} event {} has invalid running percentage {percent}: {error}",
                        index + 1,
                        record.event
                    ),
                })?)
            }
            (Some(_), Some(percent)) => {
                return Err(Error::ArtifactFormat {
                    path: path.to_owned(),
                    message: format!("line {} event {} has invalid running percentage {percent}", index + 1, record.event),
                });
            }
            _ => None,
        };
        let counter = PerfCounter::new(value, time_enabled, time_running);
        if events.insert(event.to_owned(), counter).is_some() {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("event {event} occurs more than once"),
            });
        }
    }
    if events.is_empty() {
        return Err(Error::ArtifactFormat {
            path: path.to_owned(),
            message: "contains no perf event records".to_owned(),
        });
    }
    Ok(PerfMetrics { events })
}

#[derive(Debug, Deserialize, Serialize)]
struct AllocationArtifact {
    benchmark: String,
    metrics: AllocationMetrics,
}

pub(crate) fn write_allocations(path: &Path, report: &Report, operations: &[(String, String)]) -> Result<(), Error> {
    let mut artifacts = Vec::with_capacity(operations.len());
    for (operation_name, benchmark_name) in operations {
        let operation = report
            .operations()
            .find_map(|(name, operation)| (name == operation_name).then_some(operation))
            .ok_or_else(|| Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("missing allocation operation {operation_name}"),
            })?;
        if operation.total_iterations() != 1 {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!(
                    "allocation operation {operation_name} recorded {} iterations instead of one",
                    operation.total_iterations()
                ),
            });
        }
        artifacts.push(AllocationArtifact {
            benchmark: benchmark_name.clone(),
            metrics: AllocationMetrics::new(operation.total_bytes_allocated(), operation.total_allocations_count()),
        });
    }
    write_json(path, &artifacts)
}

pub(crate) fn parse_allocations(path: &Path) -> Result<BTreeMap<String, AllocationMetrics>, Error> {
    let contents = fs::read(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    let artifacts = serde_json::from_slice::<Vec<AllocationArtifact>>(&contents).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })?;
    let mut allocations = BTreeMap::new();
    for artifact in artifacts {
        if allocations.insert(artifact.benchmark.clone(), artifact.metrics).is_some() {
            return Err(Error::ArtifactFormat {
                path: path.to_owned(),
                message: format!("benchmark {} occurs more than once", artifact.benchmark),
            });
        }
    }
    Ok(allocations)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::ArtifactIo {
            path: parent.to_owned(),
            source,
        })?;
    }

    let contents = serde_json::to_vec_pretty(value).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })?;
    fs::write(path, contents).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })
}

fn read_json<T>(path: &Path) -> Result<T, Error>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read(path).map_err(|source| Error::ArtifactIo {
        path: path.to_owned(),
        source,
    })?;
    serde_json::from_slice(&contents).map_err(|source| Error::ArtifactJson {
        path: path.to_owned(),
        source,
    })
}

fn artifact_key(name: &str) -> String {
    format!("{:032x}", xxh3_128(name.as_bytes()))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn fixture(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/artifacts").join(path)
    }

    fn assert_error_contains(result: Result<impl std::fmt::Debug, Error>, expected: &str) {
        let error = result.expect_err("fixture should be rejected");
        assert!(error.to_string().contains(expected), "{error:?} does not contain {expected:?}");
    }

    #[test]
    fn parses_minimal_criterion_result() {
        let metrics = parse_criterion(&fixture("criterion/minimal")).expect("valid Criterion fixture");
        let metric = metrics.get("group/bench").expect("benchmark identity");

        assert_eq!(metric.p50, Duration::from_nanos(20));
        assert_eq!(metric.lower_bound, Duration::from_nanos(10));
        assert_eq!(metric.upper_bound, Duration::from_nanos(30));
    }

    #[test]
    fn preserves_unicode_criterion_identity() {
        let metrics = parse_criterion(&fixture("criterion/unicode")).expect("valid Unicode fixture");

        assert!(metrics.contains_key("grüppe/測試"));
    }

    #[test]
    fn rejects_empty_duplicate_and_incomplete_criterion_results() {
        assert_error_contains(
            parse_criterion(&fixture("criterion/empty")),
            "contains no Criterion benchmark results",
        );
        assert_error_contains(parse_criterion(&fixture("criterion/duplicate")), "occurs more than once");
        assert_error_contains(parse_criterion(&fixture("criterion/missing_function")), "has no function_id");
        assert_error_contains(parse_criterion(&fixture("criterion/malformed")), "invalid artifact JSON");
    }

    #[test]
    fn rejects_invalid_criterion_numeric_value() {
        assert_error_contains(
            parse_criterion(&fixture("criterion/invalid_numeric")),
            "invalid Criterion duration -1",
        );
    }

    #[test]
    fn parses_minimal_gungraun_schema_result() {
        let metrics = parse_callgrind(&fixture("gungraun/minimal"), &["group/bench".to_owned()]).expect("valid Gungraun v6 fixture");
        let metric = metrics.get("group/bench").expect("mapped benchmark");

        assert_eq!(metric.instructions, 100);
        assert_eq!(metric.l1_hits, 80);
        assert_eq!(metric.ll_misses, 5);
    }

    #[test]
    fn preserves_unicode_gungraun_mapping() {
        let benchmark = "grüppe/測試".to_owned();
        let metrics = parse_callgrind(&fixture("gungraun/minimal"), std::slice::from_ref(&benchmark)).expect("valid Unicode mapping");

        assert!(metrics.contains_key(&benchmark));
    }

    #[test]
    fn maps_gungraun_cases_through_identity_manifest() {
        let selected = ["first".to_owned(), "second".to_owned()];
        let metrics = parse_callgrind(&fixture("gungraun/manifest_order"), &selected).expect("valid identity manifest");

        assert_eq!(metrics["first"].instructions, 200);
        assert_eq!(metrics["second"].instructions, 100);
    }

    #[test]
    fn rejects_invalid_gungraun_identity_manifests() {
        let selected = ["first".to_owned(), "second".to_owned()];
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/manifest_duplicate"), &selected),
            "identity first occurs more than once",
        );
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/manifest_count"), &selected),
            "expected 2, found 1",
        );
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/manifest_wrong_set"), &selected),
            "missing: [\"second\"]; unexpected: [\"other\"]",
        );
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/manifest_malformed"), &selected),
            "invalid artifact JSON",
        );
    }

    #[test]
    fn rejects_empty_duplicate_malformed_and_wrong_gungraun_results() {
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/empty"), &["bench".to_owned()]),
            "expected 1 Gungraun summaries, found 0",
        );
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/duplicate"), &["bench".to_owned(), "other".to_owned()]),
            "occurs more than once",
        );
        assert_error_contains(parse_callgrind(&fixture("gungraun/malformed"), &["bench".to_owned()]), "EOF");
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/wrong_mapping"), &["bench".to_owned()]),
            "case index 9 is out of range",
        );
    }

    #[test]
    fn rejects_non_integer_required_gungraun_metric() {
        assert_error_contains(
            parse_callgrind(&fixture("gungraun/invalid_numeric"), &["bench".to_owned()]),
            "unexpectedly contains float",
        );
    }

    #[test]
    fn parses_perf_records_and_normalizes_user_event_suffix() {
        let metrics = parse_perf(&fixture("perf/minimal.jsonl")).expect("valid perf fixture");
        let cycles = metrics.events.get("cycles").expect("normalized event");

        assert!((cycles.value - 42.5).abs() < f64::EPSILON);
        assert_eq!(cycles.time_running, Some(Duration::from_nanos(50)));
        assert_eq!(cycles.time_enabled, Some(Duration::from_nanos(100)));
    }

    #[test]
    fn preserves_unicode_perf_event() {
        let metrics = parse_perf(&fixture("perf/unicode.jsonl")).expect("valid Unicode event");

        assert!(metrics.events.contains_key("événement"));
    }

    #[test]
    fn rejects_empty_duplicate_malformed_and_non_numeric_perf() {
        assert_error_contains(parse_perf(&fixture("perf/empty.jsonl")), "contains no perf event records");
        assert_error_contains(parse_perf(&fixture("perf/duplicate.jsonl")), "occurs more than once");
        assert_error_contains(parse_perf(&fixture("perf/malformed.jsonl")), "invalid artifact JSON");
        assert_error_contains(parse_perf(&fixture("perf/invalid_numeric.jsonl")), "non-numeric counter value");
    }

    #[test]
    fn rejects_invalid_perf_values() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary perf file");
        file.write_all(br#"{"counter-value":"NaN","event":"cycles:u","event-runtime":1,"pcnt-running":100}"#)
            .expect("write perf fixture");
        assert_error_contains(parse_perf(file.path()), "invalid counter value");

        let mut file = tempfile::NamedTempFile::new().expect("temporary perf file");
        file.write_all(br#"{"counter-value":"1","event":"cycles:u","event-runtime":1,"pcnt-running":-1}"#)
            .expect("write perf fixture");
        assert_error_contains(parse_perf(file.path()), "invalid running percentage");

        let mut file = tempfile::NamedTempFile::new().expect("temporary perf file");
        file.write_all(br#"{"counter-value":"1","event":"cycles:u","event-runtime":1,"pcnt-running":0}"#)
            .expect("write perf fixture");
        assert_error_contains(parse_perf(file.path()), "invalid running percentage");

        let mut file = tempfile::NamedTempFile::new().expect("temporary perf file");
        file.write_all(br#"{"counter-value":"1","event":"cycles:u","event-runtime":1,"pcnt-running":101}"#)
            .expect("write perf fixture");
        assert_error_contains(parse_perf(file.path()), "invalid running percentage");
    }

    #[test]
    fn artifact_keys_are_fixed_size_and_identity_specific() {
        let short = artifact_key("group/benchmark");
        let long = artifact_key(&"x".repeat(1_000));

        assert_eq!(short.len(), 32);
        assert_eq!(long.len(), 32);
        assert_ne!(short, artifact_key("group/other"));
    }

    #[test]
    fn parses_empty_minimal_and_unicode_allocations() {
        let empty = parse_allocations(&fixture("allocations/empty.json")).expect("an empty allocation set is valid");
        assert!(empty.is_empty());

        let metrics = parse_allocations(&fixture("allocations/minimal.json")).expect("valid allocation fixture");
        let allocation = metrics.get("grüppe/測試").expect("Unicode identity");
        assert_eq!(allocation.allocated_bytes, 128);
        assert_eq!(allocation.allocation_count, 2);
    }

    #[test]
    fn rejects_duplicate_malformed_and_invalid_allocation_numbers() {
        assert_error_contains(parse_allocations(&fixture("allocations/duplicate.json")), "occurs more than once");
        assert_error_contains(parse_allocations(&fixture("allocations/malformed.json")), "invalid artifact JSON");
        assert_error_contains(
            parse_allocations(&fixture("allocations/invalid_numeric.json")),
            "invalid artifact JSON",
        );
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_directory_symlinks() {
        let metrics = parse_criterion(&fixture("traversal/cycle")).expect("symlink cycle is ignored");

        assert_eq!(metrics.len(), 1);
    }

    #[test]
    fn rejects_artifact_trees_beyond_depth_limit() {
        let directory = tempfile::tempdir().expect("temporary artifact directory");
        let mut path = directory.path().to_owned();
        for _ in 0..=MAX_ARTIFACT_DIRECTORY_DEPTH {
            path.push("d");
            fs::create_dir(&path).expect("create nested artifact directory");
        }

        assert_error_contains(parse_criterion(directory.path()), "artifact directory depth exceeds 64");
    }

    #[test]
    fn bolero_line_oriented_artifact_parsers_are_total() {
        bolero::check!().with_type::<[u8; 256]>().for_each(|bytes| {
            let mut perf_file = tempfile::NamedTempFile::new().expect("temporary perf file");
            perf_file.write_all(bytes).expect("write perf input");
            let _perf = parse_perf(perf_file.path());

            let mut allocation_file = tempfile::NamedTempFile::new().expect("temporary allocation file");
            allocation_file.write_all(bytes).expect("write allocation input");
            let _allocations = parse_allocations(allocation_file.path());
        });
    }
}
