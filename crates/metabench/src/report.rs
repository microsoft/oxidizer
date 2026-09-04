// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::Error;

pub(crate) const DEFAULT_REGRESSION_THRESHOLD: f64 = 5.0;
const REPORT_SCHEMA_VERSION: u32 = 5;

#[derive(Deserialize)]
struct BenchmarkReportWire {
    schema_version: u32,
    metadata: EnvironmentMetadata,
    entries: Vec<BenchmarkEntry>,
}

#[derive(Clone, Copy)]
struct ResultColumns(u8);

impl ResultColumns {
    const DESCRIPTION: u8 = 1 << 0;
    const CRITERION: u8 = 1 << 1;
    const GUNGRAUN: u8 = 1 << 2;
    const PERF: u8 = 1 << 3;
    const ALLOCATIONS: u8 = 1 << 4;

    fn for_entries(entries: &[BenchmarkEntry]) -> Self {
        let mut columns = 0;
        if entries.iter().any(|entry| entry.description.is_some()) {
            columns |= Self::DESCRIPTION;
        }
        if entries
            .iter()
            .any(|entry| entry.wall_clock.is_some() || entry.prior.as_ref().is_some_and(|prior| prior.wall_clock.is_some()))
        {
            columns |= Self::CRITERION;
        }
        if entries
            .iter()
            .any(|entry| entry.callgrind.is_some() || entry.prior.as_ref().is_some_and(|prior| prior.callgrind.is_some()))
        {
            columns |= Self::GUNGRAUN;
        }
        if entries
            .iter()
            .any(|entry| entry.perf.is_some() || entry.prior.as_ref().is_some_and(|prior| prior.perf.is_some()))
        {
            columns |= Self::PERF;
        }
        if entries
            .iter()
            .any(|entry| entry.allocations.is_some() || entry.prior.as_ref().is_some_and(|prior| prior.allocations.is_some()))
        {
            columns |= Self::ALLOCATIONS;
        }
        Self(columns)
    }

    const fn contains(self, column: u8) -> bool {
        self.0 & column != 0
    }
}

#[derive(Clone, Copy)]
struct Measurements<'a> {
    wall_clock: Option<&'a WallClockMetrics>,
    callgrind: Option<&'a CallgrindMetrics>,
    perf: Option<&'a PerfMetrics>,
    allocations: Option<&'a AllocationMetrics>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BenchmarkReport {
    pub(crate) schema_version: u32,
    pub(crate) metadata: EnvironmentMetadata,
    pub(crate) entries: Vec<BenchmarkEntry>,
}

impl BenchmarkReport {
    pub(crate) fn new(benchmarks: impl IntoIterator<Item = (String, String, Option<String>)>) -> Result<Self, Error> {
        let report = Self {
            schema_version: REPORT_SCHEMA_VERSION,
            metadata: EnvironmentMetadata::collect()?,
            entries: benchmarks
                .into_iter()
                .map(|(group, name, case)| BenchmarkEntry::new(group, name, case))
                .collect(),
        };
        report.validate_unique_entries()?;
        Ok(report)
    }

    pub(crate) fn read_json(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?;
        let wire: BenchmarkReportWire = serde_json::from_slice(&bytes).map_err(|source| Error::ReportJson {
            path: path.to_owned(),
            source,
        })?;
        Self::from_wire(wire, path)
    }

    fn from_wire(wire: BenchmarkReportWire, path: &Path) -> Result<Self, Error> {
        if wire.schema_version != REPORT_SCHEMA_VERSION {
            return Err(Error::UnsupportedReportSchema {
                path: path.to_owned(),
                found: wire.schema_version,
                expected: REPORT_SCHEMA_VERSION,
            });
        }
        let report = Self {
            schema_version: wire.schema_version,
            metadata: wire.metadata,
            entries: wire.entries,
        };
        report.validate_unique_entries()?;
        Ok(report)
    }

    fn deserialize_wire<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BenchmarkReportWire::deserialize(deserializer)?;
        Self::from_wire(wire, Path::new("<deserialized report>")).map_err(serde::de::Error::custom)
    }

    pub(crate) fn write_reports(&self, json_path: Option<&Path>, markdown_path: Option<&Path>) -> Result<(), Error> {
        let json = json_path
            .map(|path| {
                serde_json::to_vec_pretty(self).map_err(|source| Error::ReportJson {
                    path: path.to_owned(),
                    source,
                })
            })
            .transpose()?;
        let markdown = markdown_path.map(|_| self.render_markdown());
        publish_reports(
            json_path.zip(json.as_deref()),
            markdown_path.zip(markdown.as_deref().map(str::as_bytes)),
        )
    }

    pub(crate) fn apply_baseline(&mut self, baseline: &Self, threshold_percentage: f64) -> Result<(), Error> {
        if !threshold_percentage.is_finite() || threshold_percentage < 0.0 {
            return Err(Error::InvalidThreshold(threshold_percentage.to_string()));
        }
        if baseline.schema_version != REPORT_SCHEMA_VERSION {
            return Err(Error::UnsupportedReportSchema {
                path: "<in-memory baseline>".into(),
                found: baseline.schema_version,
                expected: REPORT_SCHEMA_VERSION,
            });
        }
        baseline.validate_unique_entries()?;
        let mut baseline_entries = HashMap::with_capacity(baseline.entries.len());
        for entry in &baseline.entries {
            let identity = (entry.group.as_str(), entry.name.as_str(), entry.case.as_deref());
            baseline_entries.insert(identity, entry);
        }

        for entry in &mut self.entries {
            entry.clear_shifts();
            let Some(previous) = baseline_entries.get(&(entry.group.as_str(), entry.name.as_str(), entry.case.as_deref())) else {
                entry.status = BenchmarkStatus::Uncompared;
                entry.prior = None;
                continue;
            };

            entry.prior = Some(PriorMeasurements::from(*previous));
            let mut shifts = Vec::new();
            if let (Some(current), Some(previous)) = (&mut entry.wall_clock, &previous.wall_clock) {
                current.shift_percentage = percentage_shift(duration_ns(previous.p50), duration_ns(current.p50));
                shifts.extend(current.shift_percentage);
            }
            if let (Some(current), Some(previous)) = (&mut entry.callgrind, &previous.callgrind) {
                current.instruction_shift_percentage = percentage_shift_u64(previous.instructions, current.instructions);
                shifts.extend(current.instruction_shift_percentage);
            }
            if let (Some(current), Some(previous)) = (&mut entry.allocations, &previous.allocations) {
                current.allocated_bytes_shift_percentage = percentage_shift_u64(previous.allocated_bytes, current.allocated_bytes);
                shifts.extend(current.allocated_bytes_shift_percentage);
                current.allocation_shift_percentage = percentage_shift_u64(previous.allocation_count, current.allocation_count);
                shifts.extend(current.allocation_shift_percentage);
            }
            if let (Some(current), Some(previous)) = (&mut entry.perf, &previous.perf) {
                for (name, counter) in &mut current.events {
                    if let Some(previous_counter) = previous.events.get(name) {
                        counter.shift_percentage = percentage_shift(previous_counter.value, counter.value);
                        shifts.extend(counter.shift_percentage);
                    }
                }
            }

            entry.status = classify(&shifts, threshold_percentage);
        }
        Ok(())
    }

    #[cfg(test)]
    fn apply_default_baseline(&mut self, baseline: &Self) -> Result<(), Error> {
        self.apply_baseline(baseline, DEFAULT_REGRESSION_THRESHOLD)
    }

    fn validate_unique_entries(&self) -> Result<(), Error> {
        let mut identities = std::collections::HashSet::with_capacity(self.entries.len());
        for entry in &self.entries {
            if !identities.insert((entry.group.as_str(), entry.name.as_str(), entry.case.as_deref())) {
                return Err(Error::DuplicateReportBenchmark(entry.id()));
            }
        }
        Ok(())
    }

    fn render_markdown(&self) -> String {
        let mut output = String::from("# Benchmark Report\n\n");
        render_metadata(&mut output, &self.metadata);

        render_results_table(&mut output, &self.entries);

        output.push_str("\n## Causal Regression Analysis\n\n");
        let mut notes = 0_u64;
        for entry in &self.entries {
            if entry.status != BenchmarkStatus::Regressed {
                continue;
            }
            let latency = entry.wall_clock.as_ref().and_then(|metrics| metrics.shift_percentage);
            let instructions = entry.callgrind.as_ref().and_then(|metrics| metrics.instruction_shift_percentage);
            let allocations = entry.allocations.as_ref().and_then(|metrics| metrics.allocation_shift_percentage);
            if latency.is_some_and(|shift| shift > 0.0) && instructions.is_some_and(|shift| shift > 0.0) {
                let _ = writeln!(
                    output,
                    "- **{}:** latency and retired instruction count both increased.",
                    escape_inline(&entry.id())
                );
                notes += 1;
            }
            if latency.is_some_and(|shift| shift > 0.0) && allocations.is_some_and(|shift| shift > 0.0) {
                let _ = writeln!(
                    output,
                    "- **{}:** latency and allocation count both increased.",
                    escape_inline(&entry.id())
                );
                notes += 1;
            }
        }
        if notes == 0 {
            output.push_str("No correlated regression signals were detected.\n");
        }

        output
    }

    pub(crate) fn render_console_table(&self) -> String {
        let columns = ResultColumns::for_entries(&self.entries);
        let mut headers = vec!["Benchmark"];
        if columns.contains(ResultColumns::CRITERION) {
            headers.push("Criterion");
        }
        if columns.contains(ResultColumns::GUNGRAUN) {
            headers.push("Gungraun");
        }
        if columns.contains(ResultColumns::PERF) {
            headers.push("perf");
        }
        if columns.contains(ResultColumns::ALLOCATIONS) {
            headers.push("Allocations");
        }
        let rows = self
            .entries
            .iter()
            .flat_map(|entry| {
                let current = Self::console_result_row(
                    entry.id(),
                    entry.wall_clock.as_ref(),
                    entry.callgrind.as_ref(),
                    entry.perf.as_ref(),
                    entry.allocations.as_ref(),
                    columns,
                );
                let mut rows = vec![current];
                if let Some(prior) = &entry.prior {
                    rows.push(Self::console_result_row(
                        format!("{} (PRIOR)", entry.id()),
                        prior.wall_clock.as_ref(),
                        prior.callgrind.as_ref(),
                        prior.perf.as_ref(),
                        prior.allocations.as_ref(),
                        columns,
                    ));
                }
                rows
            })
            .collect::<Vec<_>>();

        render_console_grid(&headers, &rows)
    }

    fn console_result_row(
        benchmark: String,
        wall_clock: Option<&WallClockMetrics>,
        callgrind: Option<&CallgrindMetrics>,
        perf_metrics: Option<&PerfMetrics>,
        allocation_metrics: Option<&AllocationMetrics>,
        columns: ResultColumns,
    ) -> Vec<String> {
        let mut row = vec![benchmark];
        if columns.contains(ResultColumns::CRITERION) {
            row.push(console_wall_clock(wall_clock));
        }
        if columns.contains(ResultColumns::GUNGRAUN) {
            row.push(console_callgrind(callgrind));
        }
        if columns.contains(ResultColumns::PERF) {
            row.push(console_perf(perf_metrics));
        }
        if columns.contains(ResultColumns::ALLOCATIONS) {
            row.push(console_allocations(allocation_metrics));
        }
        row
    }
}

impl<'de> Deserialize<'de> for BenchmarkReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::deserialize_wire(deserializer)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EnvironmentMetadata {
    pub(crate) timestamp_utc: String,
    pub(crate) target_triple: String,
    pub(crate) rustc_version: String,
    pub(crate) host_cpu: String,
    pub(crate) profile: String,
    pub(crate) compiler_flags: Option<String>,
    #[serde(default)]
    pub(crate) measured_backends: Vec<String>,
}

impl EnvironmentMetadata {
    fn collect() -> Result<Self, Error> {
        let timestamp_utc = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|error| Error::Metadata(error.to_string()))?;
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .arg("-Vv")
            .output()
            .map_err(|error| Error::Metadata(error.to_string()))?;
        if !output.status.success() {
            return Err(Error::Metadata(format!("rustc -Vv exited with {}", output.status)));
        }
        let rustc_version = String::from_utf8(output.stdout)
            .map_err(|error| Error::Metadata(error.to_string()))?
            .trim()
            .to_owned();

        let compiler_flags = option_env!("METABENCH_RUSTFLAGS")
            .map(str::to_owned)
            .filter(|flags| !flags.is_empty());

        Ok(Self {
            timestamp_utc,
            target_triple: env!("METABENCH_TARGET").to_owned(),
            rustc_version,
            host_cpu: host_cpu(),
            profile: env!("METABENCH_PROFILE").to_owned(),
            compiler_flags,
            measured_backends: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BenchmarkEntry {
    pub(crate) group: String,
    pub(crate) name: String,
    pub(crate) case: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) wall_clock: Option<WallClockMetrics>,
    pub(crate) callgrind: Option<CallgrindMetrics>,
    pub(crate) perf: Option<PerfMetrics>,
    pub(crate) allocations: Option<AllocationMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prior: Option<PriorMeasurements>,
    pub(crate) status: BenchmarkStatus,
}

impl BenchmarkEntry {
    pub(crate) fn new(group: impl Into<String>, name: impl Into<String>, case: Option<String>) -> Self {
        Self {
            group: group.into(),
            name: name.into(),
            case,
            description: None,
            wall_clock: None,
            callgrind: None,
            perf: None,
            allocations: None,
            prior: None,
            status: BenchmarkStatus::Uncompared,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PriorMeasurements {
    pub(crate) wall_clock: Option<WallClockMetrics>,
    pub(crate) callgrind: Option<CallgrindMetrics>,
    pub(crate) perf: Option<PerfMetrics>,
    pub(crate) allocations: Option<AllocationMetrics>,
}

impl From<&BenchmarkEntry> for PriorMeasurements {
    fn from(entry: &BenchmarkEntry) -> Self {
        let mut wall_clock = entry.wall_clock.clone();
        if let Some(metrics) = &mut wall_clock {
            metrics.shift_percentage = None;
        }
        let mut callgrind = entry.callgrind.clone();
        if let Some(metrics) = &mut callgrind {
            metrics.instruction_shift_percentage = None;
        }
        let mut perf = entry.perf.clone();
        if let Some(metrics) = &mut perf {
            for counter in metrics.events.values_mut() {
                counter.shift_percentage = None;
            }
        }
        let mut allocations = entry.allocations.clone();
        if let Some(metrics) = &mut allocations {
            metrics.allocated_bytes_shift_percentage = None;
            metrics.allocation_shift_percentage = None;
        }
        Self {
            wall_clock,
            callgrind,
            perf,
            allocations,
        }
    }
}

impl BenchmarkEntry {
    pub(crate) fn id(&self) -> String {
        match &self.case {
            Some(case) => format!("{}/{}/{case}", self.group, self.name),
            None => format!("{}/{}", self.group, self.name),
        }
    }

    fn clear_shifts(&mut self) {
        if let Some(metrics) = &mut self.wall_clock {
            metrics.shift_percentage = None;
        }
        if let Some(metrics) = &mut self.callgrind {
            metrics.instruction_shift_percentage = None;
        }
        if let Some(metrics) = &mut self.perf {
            for counter in metrics.events.values_mut() {
                counter.shift_percentage = None;
            }
        }
        if let Some(metrics) = &mut self.allocations {
            metrics.allocated_bytes_shift_percentage = None;
            metrics.allocation_shift_percentage = None;
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BenchmarkStatus {
    Uncompared,
    Stable,
    Improved,
    Regressed,
}

impl std::fmt::Display for BenchmarkStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uncompared => f.write_str("UNCOMPARED"),
            Self::Stable => f.write_str("STABLE"),
            Self::Improved => f.write_str("IMPROVED"),
            Self::Regressed => f.write_str("REGRESSED"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct WallClockMetrics {
    #[serde(with = "duration_nanoseconds")]
    pub(crate) p50: Duration,
    #[serde(with = "duration_nanoseconds")]
    pub(crate) lower_bound: Duration,
    #[serde(with = "duration_nanoseconds")]
    pub(crate) upper_bound: Duration,
    pub(crate) throughput_per_sec: Option<f64>,
    pub(crate) shift_percentage: Option<f64>,
}

impl WallClockMetrics {
    pub(crate) const fn new(p50: Duration, lower_bound: Duration, upper_bound: Duration, throughput_per_sec: Option<f64>) -> Self {
        Self {
            p50,
            lower_bound,
            upper_bound,
            throughput_per_sec,
            shift_percentage: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CallgrindMetrics {
    pub(crate) instructions: u64,
    pub(crate) l1_hits: u64,
    pub(crate) ll_misses: u64,
    pub(crate) instruction_shift_percentage: Option<f64>,
}

impl CallgrindMetrics {
    pub(crate) const fn new(instructions: u64, l1_hits: u64, ll_misses: u64) -> Self {
        Self {
            instructions,
            l1_hits,
            ll_misses,
            instruction_shift_percentage: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct PerfMetrics {
    pub(crate) events: BTreeMap<String, PerfCounter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PerfCounter {
    pub(crate) value: f64,
    #[serde(default, with = "optional_duration_nanoseconds")]
    pub(crate) time_enabled: Option<Duration>,
    #[serde(default, with = "optional_duration_nanoseconds")]
    pub(crate) time_running: Option<Duration>,
    pub(crate) shift_percentage: Option<f64>,
}

impl PerfCounter {
    pub(crate) const fn new(value: f64, time_enabled: Option<Duration>, time_running: Option<Duration>) -> Self {
        Self {
            value,
            time_enabled,
            time_running,
            shift_percentage: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AllocationMetrics {
    pub(crate) allocated_bytes: u64,
    pub(crate) allocation_count: u64,
    pub(crate) allocated_bytes_shift_percentage: Option<f64>,
    pub(crate) allocation_shift_percentage: Option<f64>,
}

impl AllocationMetrics {
    pub(crate) const fn new(allocated_bytes: u64, allocation_count: u64) -> Self {
        Self {
            allocated_bytes,
            allocation_count,
            allocated_bytes_shift_percentage: None,
            allocation_shift_percentage: None,
        }
    }
}

fn classify(shifts: &[f64], threshold: f64) -> BenchmarkStatus {
    if shifts.iter().any(|shift| *shift > 0.0 && *shift >= threshold) {
        BenchmarkStatus::Regressed
    } else if shifts.iter().any(|shift| *shift < 0.0 && *shift <= -threshold) {
        BenchmarkStatus::Improved
    } else if shifts.is_empty() {
        BenchmarkStatus::Uncompared
    } else {
        BenchmarkStatus::Stable
    }
}

fn percentage_shift(previous: f64, current: f64) -> Option<f64> {
    if !previous.is_finite() || !current.is_finite() {
        None
    } else if previous == 0.0 {
        Some(if current == 0.0 { 0.0 } else { 100.0 })
    } else {
        Some((current - previous) / previous * 100.0)
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "percentage reporting does not require integer precision above f64's mantissa"
)]
fn percentage_shift_u64(previous: u64, current: u64) -> Option<f64> {
    percentage_shift(previous as f64, current as f64)
}

fn duration_ns(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000_000.0
}

fn metadata_row(output: &mut String, field: &str, value: &str) {
    let _ = writeln!(output, "| {} | {} |", escape_table(field), escape_table(value));
}

fn render_metadata(output: &mut String, metadata: &EnvironmentMetadata) {
    output.push_str("## Environment\n\n");
    output.push_str("| Field | Value |\n|---|---|\n");
    metadata_row(output, "Timestamp", &metadata.timestamp_utc);
    metadata_row(output, "Target", &metadata.target_triple);
    metadata_row(output, "Rust compiler", &metadata.rustc_version);
    metadata_row(output, "Host CPU", &metadata.host_cpu);
    metadata_row(output, "Profile", &metadata.profile);
    metadata_row(output, "Measured backends", &metadata.measured_backends.join(", "));
    if let Some(flags) = &metadata.compiler_flags {
        metadata_row(output, "Compiler flags", flags);
    }
}

fn render_results_table(output: &mut String, entries: &[BenchmarkEntry]) {
    let columns = ResultColumns::for_entries(entries);
    let mut headers = vec!["Benchmark"];
    if columns.contains(ResultColumns::DESCRIPTION) {
        headers.push("Description");
    }
    if columns.contains(ResultColumns::CRITERION) {
        headers.push("Criterion");
    }
    if columns.contains(ResultColumns::GUNGRAUN) {
        headers.push("Gungraun");
    }
    if columns.contains(ResultColumns::PERF) {
        headers.push("perf");
    }
    if columns.contains(ResultColumns::ALLOCATIONS) {
        headers.push("Allocations");
    }
    output.push_str("\n## Results\n\n| ");
    output.push_str(&headers.join(" | "));
    output.push_str(" |\n|");
    for _ in &headers {
        output.push_str("---|");
    }
    output.push('\n');
    for entry in entries {
        render_markdown_result_row(
            output,
            entry.id(),
            entry.description.as_deref(),
            Measurements {
                wall_clock: entry.wall_clock.as_ref(),
                callgrind: entry.callgrind.as_ref(),
                perf: entry.perf.as_ref(),
                allocations: entry.allocations.as_ref(),
            },
            columns,
        );
        if let Some(prior) = &entry.prior {
            render_markdown_result_row(
                output,
                format!("{} (PRIOR)", entry.id()),
                None,
                Measurements {
                    wall_clock: prior.wall_clock.as_ref(),
                    callgrind: prior.callgrind.as_ref(),
                    perf: prior.perf.as_ref(),
                    allocations: prior.allocations.as_ref(),
                },
                columns,
            );
        }
    }
}

fn render_markdown_result_row(
    output: &mut String,
    benchmark: String,
    description_value: Option<&str>,
    measurements: Measurements<'_>,
    columns: ResultColumns,
) {
    let mut cells = vec![benchmark];
    if columns.contains(ResultColumns::DESCRIPTION) {
        cells.push(description_value.unwrap_or("-").to_owned());
    }
    if columns.contains(ResultColumns::CRITERION) {
        cells.push(markdown_wall_clock(measurements.wall_clock));
    }
    if columns.contains(ResultColumns::GUNGRAUN) {
        cells.push(markdown_callgrind(measurements.callgrind));
    }
    if columns.contains(ResultColumns::PERF) {
        cells.push(markdown_perf(measurements.perf));
    }
    if columns.contains(ResultColumns::ALLOCATIONS) {
        cells.push(markdown_allocations(measurements.allocations));
    }
    let cells = cells.iter().map(|cell| escape_table(cell)).collect::<Vec<_>>();
    let _ = writeln!(output, "| {} |", cells.join(" | "));
}

fn markdown_wall_clock(metrics: Option<&WallClockMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    let mut output = format!(
        "Median: {}<br>Range: {}..{}",
        format_duration(metrics.p50),
        format_duration(metrics.lower_bound),
        format_duration(metrics.upper_bound)
    );
    if let Some(throughput) = metrics.throughput_per_sec {
        let _ = write!(output, "<br>Throughput: {throughput:.3}/s");
    }
    output
}

fn markdown_callgrind(metrics: Option<&CallgrindMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    format!(
        "Instructions: {}<br>L1 hits: {}<br>Last-level misses: {}",
        metrics.instructions, metrics.l1_hits, metrics.ll_misses
    )
}

fn markdown_perf(metrics: Option<&PerfMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    let output = metrics
        .events
        .iter()
        .map(|(name, counter)| {
            let mut output = format!("{name}: {}", format_count(counter.value));
            if let Some(enabled) = counter.time_enabled {
                let _ = write!(output, ", enabled {}", format_duration(enabled));
            }
            if let Some(running) = counter.time_running {
                let _ = write!(output, ", running {}", format_duration(running));
            }
            if let (Some(enabled), Some(running)) = (counter.time_enabled, counter.time_running)
                && !enabled.is_zero()
            {
                let _ = write!(output, " ({:.1}%)", running.as_secs_f64() / enabled.as_secs_f64() * 100.0);
            }
            output
        })
        .collect::<Vec<_>>()
        .join("<br>");
    if output.is_empty() { "-".to_owned() } else { output }
}

fn markdown_allocations(metrics: Option<&AllocationMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    format!(
        "Allocated bytes: {}{}<br>Allocation operations: {}{}",
        metrics.allocated_bytes,
        format_shift(metrics.allocated_bytes_shift_percentage),
        metrics.allocation_count,
        format_shift(metrics.allocation_shift_percentage)
    )
}

fn console_wall_clock(metrics: Option<&WallClockMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    format!("{}..{}", format_duration(metrics.lower_bound), format_duration(metrics.upper_bound))
}

fn console_callgrind(metrics: Option<&CallgrindMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    format!("Ir {}; L1 {}; LL {}", metrics.instructions, metrics.l1_hits, metrics.ll_misses)
}

fn console_perf(metrics: Option<&PerfMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    metrics
        .events
        .iter()
        .map(|(name, counter)| {
            let mut output = format!("{name} {}", format_count(counter.value));
            if let (Some(enabled), Some(running)) = (counter.time_enabled, counter.time_running)
                && !enabled.is_zero()
            {
                let _ = write!(output, " ({:.1}% running)", running.as_secs_f64() / enabled.as_secs_f64() * 100.0);
            }
            output
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn console_allocations(metrics: Option<&AllocationMetrics>) -> String {
    let Some(metrics) = metrics else {
        return "-".to_owned();
    };
    format!(
        "{} B{}; {} allocs{}",
        metrics.allocated_bytes,
        format_shift(metrics.allocated_bytes_shift_percentage),
        metrics.allocation_count,
        format_shift(metrics.allocation_shift_percentage)
    )
}

fn format_shift(shift: Option<f64>) -> String {
    shift.map_or_else(String::new, |shift| format!(" ({shift:+.1}%)"))
}

fn render_console_grid(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths = headers.iter().map(|header| header.chars().count()).collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }

    let mut output = String::new();
    render_console_separator(&mut output, &widths);
    render_console_row(&mut output, &headers.iter().map(ToString::to_string).collect::<Vec<_>>(), &widths);
    render_console_separator(&mut output, &widths);
    for row in rows {
        render_console_row(&mut output, row, &widths);
    }
    render_console_separator(&mut output, &widths);
    output
}

fn render_console_separator(output: &mut String, widths: &[usize]) {
    output.push('+');
    for width in widths {
        output.push_str(&"-".repeat(width + 2));
        output.push('+');
    }
    output.push('\n');
}

fn render_console_row(output: &mut String, cells: &[String], widths: &[usize]) {
    output.push('|');
    for (cell, width) in cells.iter().zip(widths) {
        let _ = write!(output, " {cell:width$} |");
    }
    output.push('\n');
}

fn format_duration(duration: Duration) -> String {
    let nanoseconds = duration_ns(duration);
    if nanoseconds < 1_000.0 {
        format!("{nanoseconds:.2} ns")
    } else if nanoseconds < 1_000_000.0 {
        format!("{:.2} us", nanoseconds / 1_000.0)
    } else if nanoseconds < 1_000_000_000.0 {
        format!("{:.2} ms", nanoseconds / 1_000_000.0)
    } else {
        format!("{:.3} s", duration.as_secs_f64())
    }
}

fn format_count(value: f64) -> String {
    if value.abs() >= 1_000_000_000.0 {
        format!("{:.3}B", value / 1_000_000_000.0)
    } else if value.abs() >= 1_000_000.0 {
        format!("{:.3}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.3}K", value / 1_000.0)
    } else {
        format!("{value:.3}")
    }
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br>")
}

fn escape_inline(value: &str) -> String {
    value.replace('`', "\\`").replace('*', "\\*")
}

fn host_cpu() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(cpu_info) = fs::read_to_string("/proc/cpuinfo")
        && let Some(model) = cpu_info.lines().find_map(|line| line.strip_prefix("model name\t: "))
    {
        return model.to_owned();
    }

    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_error| std::env::consts::ARCH.to_owned())
}

fn write_report(path: &Path, contents: &[u8]) -> Result<(), Error> {
    let staged = stage_report(path, contents)?;
    staged.persist(path).map_err(|error| Error::ReportIo {
        path: path.to_owned(),
        source: error.error,
    })?;
    Ok(())
}

fn publish_reports(json: Option<(&Path, &[u8])>, markdown: Option<(&Path, &[u8])>) -> Result<(), Error> {
    match (json, markdown) {
        (Some((json_path, json)), Some((markdown_path, markdown))) => {
            if json_path == markdown_path {
                return Err(Error::ReportIo {
                    path: json_path.to_owned(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "JSON and Markdown report paths must be different"),
                });
            }
            let staged_json = stage_report(json_path, json)?;
            let staged_markdown = stage_report(markdown_path, markdown)?;
            publish_pair((json_path, staged_json), (markdown_path, staged_markdown))
        }
        (Some((path, contents)), None) | (None, Some((path, contents))) => write_report(path, contents),
        (None, None) => Ok(()),
    }
}

fn stage_report(path: &Path, contents: &[u8]) -> Result<tempfile::NamedTempFile, Error> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| Error::ReportIo {
            path: parent.to_owned(),
            source,
        })?;
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::Builder::new()
        .prefix(".metabench-report-")
        .tempfile_in(parent)
        .map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?;
    set_report_permissions(path, staged.as_file())?;
    staged
        .write_all(contents)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?;
    Ok(staged)
}

fn set_report_permissions(path: &Path, staged: &fs::File) -> Result<(), Error> {
    let permissions = if path.is_file() {
        fs::metadata(path)
            .map_err(|source| Error::ReportIo {
                path: path.to_owned(),
                source,
            })?
            .permissions()
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            fs::Permissions::from_mode(0o644)
        }
        #[cfg(not(unix))]
        {
            staged
                .metadata()
                .map_err(|source| Error::ReportIo {
                    path: path.to_owned(),
                    source,
                })?
                .permissions()
        }
    };
    staged.set_permissions(permissions).map_err(|source| Error::ReportIo {
        path: path.to_owned(),
        source,
    })
}

fn publish_pair(first: (&Path, tempfile::NamedTempFile), second: (&Path, tempfile::NamedTempFile)) -> Result<(), Error> {
    let first_backup = backup_report(first.0)?;
    let second_backup = match backup_report(second.0) {
        Ok(backup) => backup,
        Err(error) => {
            restore_report(first.0, first_backup.as_ref());
            return Err(error);
        }
    };

    if let Err(error) = first.1.persist(first.0) {
        restore_report(first.0, first_backup.as_ref());
        restore_report(second.0, second_backup.as_ref());
        return Err(Error::ReportIo {
            path: first.0.to_owned(),
            source: error.error,
        });
    }
    if let Err(error) = second.1.persist(second.0) {
        let _ = fs::remove_file(first.0);
        restore_report(first.0, first_backup.as_ref());
        restore_report(second.0, second_backup.as_ref());
        return Err(Error::ReportIo {
            path: second.0.to_owned(),
            source: error.error,
        });
    }
    Ok(())
}

fn backup_report(path: &Path) -> Result<Option<tempfile::TempPath>, Error> {
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Err(Error::ReportIo {
            path: path.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "report destination exists and is not a regular file",
            ),
        });
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let backup = tempfile::Builder::new()
        .prefix(".metabench-backup-")
        .tempfile_in(parent)
        .map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?
        .into_temp_path();
    fs::remove_file(&backup).map_err(|source| Error::ReportIo {
        path: path.to_owned(),
        source,
    })?;
    fs::rename(path, &backup).map_err(|source| Error::ReportIo {
        path: path.to_owned(),
        source,
    })?;
    Ok(Some(backup))
}

fn restore_report(path: &Path, backup: Option<&tempfile::TempPath>) {
    if let Some(backup) = backup {
        let _ = fs::rename(backup, path);
    }
}

mod duration_nanoseconds {
    use std::time::Duration;

    use serde::ser::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let nanoseconds = u64::try_from(duration.as_nanos()).map_err(|error| S::Error::custom(error.to_string()))?;
        serializer.serialize_u64(nanoseconds)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let nanoseconds = u64::deserialize(deserializer)?;
        Ok(Duration::from_nanos(nanoseconds))
    }
}

mod optional_duration_nanoseconds {
    use std::time::Duration;

    use serde::ser::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[expect(clippy::ref_option, reason = "serde's with-module contract passes the field by reference")]
    pub(super) fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let nanoseconds = duration
            .as_ref()
            .map(Duration::as_nanos)
            .map(u64::try_from)
            .transpose()
            .map_err(|error| S::Error::custom(error.to_string()))?;
        nanoseconds.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<u64>::deserialize(deserializer).map(|value| value.map(Duration::from_nanos))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn report(latency_ns: u64, instructions: u64) -> BenchmarkReport {
        BenchmarkReport {
            schema_version: REPORT_SCHEMA_VERSION,
            metadata: EnvironmentMetadata {
                timestamp_utc: "2026-01-01T00:00:00Z".to_owned(),
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                rustc_version: "rustc test".to_owned(),
                host_cpu: "test cpu".to_owned(),
                profile: "release".to_owned(),
                compiler_flags: None,
                measured_backends: vec!["criterion".to_owned(), "gungraun".to_owned()],
            },
            entries: vec![BenchmarkEntry {
                group: "group".to_owned(),
                name: "work".to_owned(),
                case: None,
                description: None,
                wall_clock: Some(WallClockMetrics {
                    p50: Duration::from_nanos(latency_ns),
                    lower_bound: Duration::from_nanos(latency_ns),
                    upper_bound: Duration::from_nanos(latency_ns),
                    throughput_per_sec: None,
                    shift_percentage: None,
                }),
                callgrind: Some(CallgrindMetrics {
                    instructions,
                    l1_hits: 0,
                    ll_misses: 0,
                    instruction_shift_percentage: None,
                }),
                perf: None,
                allocations: None,
                prior: None,
                status: BenchmarkStatus::Uncompared,
            }],
        }
    }

    #[test]
    fn baseline_marks_a_significant_increase_as_regressed() {
        let baseline = report(100, 1_000);
        let mut current = report(110, 1_100);
        current.apply_default_baseline(&baseline).expect("valid baseline");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Regressed);
        assert_eq!(
            current.entries[0]
                .callgrind
                .as_ref()
                .and_then(|metrics| metrics.instruction_shift_percentage),
            Some(10.0)
        );
    }

    #[test]
    fn json_round_trip_preserves_nanosecond_durations() {
        let original = report(123, 456);
        let encoded = serde_json::to_vec(&original).expect("serialize report");
        let decoded: BenchmarkReport = serde_json::from_slice(&encoded).expect("deserialize report");

        assert_eq!(
            decoded.entries[0].wall_clock.as_ref().map(|metrics| metrics.p50),
            Some(Duration::from_nanos(123))
        );
    }

    #[test]
    fn markdown_handles_partially_populated_reports() {
        let markdown = report(123, 456).render_markdown();

        assert!(markdown.contains("# Benchmark Report"));
        assert!(markdown.contains("| group/work |"));
        assert!(markdown.contains("## Results"));
        assert!(!markdown.contains("Workload Details"));
        assert!(markdown.contains("Median: 123.00 ns"));
        assert!(markdown.contains("Instructions: 456"));
    }

    #[test]
    fn console_table_contains_one_aligned_benchmark_row() {
        let table = report(123, 456).render_console_table();
        let lines = table.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 5);
        assert!(lines[1].contains("Benchmark"));
        assert!(lines[1].contains("Criterion"));
        assert!(lines[1].contains("Gungraun"));
        assert!(!lines[1].contains("perf"));
        assert!(!lines[1].contains("Allocations"));
        assert!(!lines[1].contains("Status"));
        assert!(lines[3].contains("group/work"));
        assert!(lines[3].contains("123.00 ns..123.00 ns"));
        assert!(!lines[3].contains('['));
        assert!(lines[3].contains("Ir 456"));
    }

    #[test]
    fn result_tables_show_prior_measurements_without_status() {
        let baseline = report(100, 1_000);
        let mut current = report(110, 1_100);
        current.apply_default_baseline(&baseline).expect("valid baseline");

        let table = current.render_console_table();
        let markdown = current.render_markdown();

        assert!(!table.contains("Status"));
        assert!(table.contains("group/work (PRIOR)"));
        assert!(table.contains("Ir 1000"));
        assert!(!markdown.contains("| Status |"));
        assert!(markdown.contains("| group/work (PRIOR) |"));
        assert!(markdown.contains("Instructions: 1000"));
    }

    #[test]
    fn markdown_results_include_complete_case_identity() {
        let mut report = report(123, 456);
        report.entries[0].case = Some("size=10".to_owned());

        let markdown = report.render_markdown();

        assert!(markdown.contains("| group/work/size=10 |"));
        assert!(!markdown.contains("Workload Details"));
    }

    #[test]
    fn markdown_results_include_every_collected_metric() {
        let mut report = report(123, 456);
        report.entries[0].perf = Some(PerfMetrics {
            events: BTreeMap::from([(
                "cycles".to_owned(),
                PerfCounter {
                    value: 789.0,
                    time_enabled: Some(Duration::from_nanos(10)),
                    time_running: Some(Duration::from_nanos(9)),
                    shift_percentage: Some(1.5),
                },
            )]),
        });
        report.entries[0].allocations = Some(AllocationMetrics {
            allocated_bytes: 64,
            allocation_count: 2,
            allocated_bytes_shift_percentage: Some(3.0),
            allocation_shift_percentage: Some(-2.0),
        });

        let markdown = report.render_markdown();

        assert!(markdown.contains("Range: 123.00 ns..123.00 ns"));
        assert!(markdown.contains("L1 hits: 0"));
        assert!(markdown.contains("Last-level misses: 0"));
        assert!(markdown.contains("cycles: 789.000"));
        assert!(markdown.contains("enabled 10.00 ns"));
        assert!(markdown.contains("running 9.00 ns (90.0%)"));
        assert!(markdown.contains("Allocated bytes: 64 (+3.0%)"));
        assert!(markdown.contains("Allocation operations: 2 (-2.0%)"));
    }

    #[test]
    fn baseline_matches_complete_case_identity() {
        let mut baseline = report(100, 1_000);
        baseline.entries[0].case = Some("size=10".to_owned());
        let mut current = report(110, 1_100);
        current.entries[0].case = Some("size=20".to_owned());

        current.apply_default_baseline(&baseline).expect("valid baseline");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);
    }

    #[test]
    fn zero_threshold_keeps_unchanged_metrics_stable() {
        let baseline = report(100, 1_000);
        let mut current = report(100, 1_000);

        current.apply_baseline(&baseline, 0.0).expect("zero is a valid threshold");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Stable);

        let mut improved = report(99, 999);
        improved.apply_baseline(&baseline, 0.0).expect("zero is a valid threshold");
        assert_eq!(improved.entries[0].status, BenchmarkStatus::Improved);

        let mut regressed = report(101, 1_001);
        regressed.apply_baseline(&baseline, 0.0).expect("zero is a valid threshold");
        assert_eq!(regressed.entries[0].status, BenchmarkStatus::Regressed);
    }

    #[test]
    fn zero_to_positive_is_a_finite_regression() {
        let baseline = report(0, 0);
        let mut current = report(1, 1);
        current.entries[0].perf = Some(PerfMetrics {
            events: BTreeMap::from([("cycles".to_owned(), PerfCounter::new(1.0, None, None))]),
        });
        current.entries[0].allocations = Some(AllocationMetrics::new(1, 1));
        let mut baseline = baseline;
        baseline.entries[0].perf = Some(PerfMetrics {
            events: BTreeMap::from([("cycles".to_owned(), PerfCounter::new(0.0, None, None))]),
        });
        baseline.entries[0].allocations = Some(AllocationMetrics::new(0, 0));

        current.apply_default_baseline(&baseline).expect("valid baseline");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Regressed);
        assert_eq!(
            current.entries[0]
                .callgrind
                .as_ref()
                .and_then(|metrics| metrics.instruction_shift_percentage),
            Some(100.0)
        );
        assert_eq!(
            current.entries[0]
                .perf
                .as_ref()
                .and_then(|metrics| metrics.events.get("cycles"))
                .and_then(|counter| counter.shift_percentage),
            Some(100.0)
        );
        assert_eq!(
            current.entries[0]
                .allocations
                .as_ref()
                .and_then(|metrics| metrics.allocated_bytes_shift_percentage),
            Some(100.0)
        );
        assert_eq!(
            current.entries[0]
                .allocations
                .as_ref()
                .and_then(|metrics| metrics.allocation_shift_percentage),
            Some(100.0)
        );
        serde_json::to_vec(&current).expect("finite shift serializes");
    }

    #[test]
    fn regression_takes_precedence_over_improvement() {
        let baseline = report(100, 1_000);
        let mut current = report(90, 1_100);

        current.apply_default_baseline(&baseline).expect("valid baseline");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Regressed);
    }

    #[test]
    fn repeated_comparisons_clear_stale_shifts() {
        let mut baseline = report(100, 1_000);
        baseline.entries[0].perf = Some(PerfMetrics {
            events: BTreeMap::from([("cycles".to_owned(), PerfCounter::new(100.0, None, None))]),
        });
        baseline.entries[0].allocations = Some(AllocationMetrics::new(100, 10));
        let mut current = report(110, 1_100);
        current.entries[0].perf = Some(PerfMetrics {
            events: BTreeMap::from([("cycles".to_owned(), PerfCounter::new(110.0, None, None))]),
        });
        current.entries[0].allocations = Some(AllocationMetrics::new(110, 11));
        current.apply_default_baseline(&baseline).expect("valid baseline");

        let mut missing_metrics = report(100, 1_000);
        missing_metrics.entries[0].wall_clock = None;
        missing_metrics.entries[0].callgrind = None;
        current.apply_default_baseline(&missing_metrics).expect("valid baseline");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);
        assert!(
            current.entries[0]
                .wall_clock
                .as_ref()
                .is_some_and(|metrics| metrics.shift_percentage.is_none())
        );
        assert!(
            current.entries[0]
                .callgrind
                .as_ref()
                .is_some_and(|metrics| metrics.instruction_shift_percentage.is_none())
        );
        assert!(
            current.entries[0]
                .perf
                .as_ref()
                .is_some_and(|metrics| metrics.events.values().all(|counter| counter.shift_percentage.is_none()))
        );
        assert!(current.entries[0].allocations.as_ref().is_some_and(|metrics| {
            metrics.allocated_bytes_shift_percentage.is_none() && metrics.allocation_shift_percentage.is_none()
        }));

        current.apply_default_baseline(&baseline).expect("valid baseline");
        baseline.entries[0].case = Some("different".to_owned());
        current.apply_default_baseline(&baseline).expect("valid baseline");

        assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);
        assert!(current.entries[0].prior.is_none());
        assert!(
            current.entries[0]
                .wall_clock
                .as_ref()
                .is_some_and(|metrics| metrics.shift_percentage.is_none())
        );
    }

    #[test]
    fn allocated_byte_regression_is_exposed_and_rendered() {
        let mut baseline = report(100, 1_000);
        baseline.entries[0].allocations = Some(AllocationMetrics::new(100, 2));
        let mut current = report(100, 1_000);
        current.entries[0].allocations = Some(AllocationMetrics::new(110, 2));

        current.apply_default_baseline(&baseline).expect("valid baseline");

        let allocations = current.entries[0].allocations.as_ref().expect("allocation metrics were provided");
        assert_eq!(current.entries[0].status, BenchmarkStatus::Regressed);
        assert_eq!(allocations.allocated_bytes_shift_percentage, Some(10.0));
        assert_eq!(allocations.allocation_shift_percentage, Some(0.0));
        assert!(current.render_markdown().contains("Allocated bytes: 110 (+10.0%)"));
        assert!(current.render_console_table().contains("110 B (+10.0%); 2 allocs (+0.0%)"));
    }

    #[test]
    fn exact_threshold_is_significant_but_smaller_changes_are_stable() {
        let baseline = report(100, 1_000);
        let mut exact = report(105, 1_000);
        exact.apply_baseline(&baseline, 5.0).expect("valid threshold");
        assert_eq!(exact.entries[0].status, BenchmarkStatus::Regressed);

        let mut below = report(104, 1_000);
        below.apply_baseline(&baseline, 5.0).expect("valid threshold");
        assert_eq!(below.entries[0].status, BenchmarkStatus::Stable);
    }

    #[test]
    fn prior_snapshot_strips_nested_shift_metadata() {
        let mut baseline = report(100, 1_000);
        baseline.entries[0].wall_clock.as_mut().expect("wall clock").shift_percentage = Some(5.0);
        baseline.entries[0]
            .callgrind
            .as_mut()
            .expect("Callgrind")
            .instruction_shift_percentage = Some(5.0);
        baseline.entries[0].allocations = Some(AllocationMetrics {
            allocated_bytes: 100,
            allocation_count: 2,
            allocated_bytes_shift_percentage: Some(5.0),
            allocation_shift_percentage: Some(5.0),
        });
        let mut current = report(100, 1_000);
        current.entries[0].allocations = Some(AllocationMetrics::new(100, 2));

        current.apply_default_baseline(&baseline).expect("valid baseline");

        let prior = current.entries[0].prior.as_ref().expect("prior snapshot");
        assert_eq!(prior.wall_clock.as_ref().and_then(|metric| metric.shift_percentage), None);
        assert_eq!(
            prior.callgrind.as_ref().and_then(|metric| metric.instruction_shift_percentage),
            None
        );
        let allocations = prior.allocations.as_ref().expect("prior allocation metrics");
        assert_eq!(allocations.allocated_bytes_shift_percentage, None);
        assert_eq!(allocations.allocation_shift_percentage, None);
    }

    #[test]
    fn rendering_uses_exact_unit_boundaries_and_escapes_cells() {
        assert_eq!(format_duration(Duration::from_nanos(999)), "999.00 ns");
        assert_eq!(format_duration(Duration::from_micros(1)), "1.00 us");
        assert_eq!(format_duration(Duration::from_millis(1)), "1.00 ms");
        assert_eq!(format_duration(Duration::from_secs(1)), "1.000 s");

        let mut report = report(1, 1);
        report.entries[0].description = Some("pipe | newline\nnext".to_owned());
        let markdown = report.render_markdown();
        assert!(markdown.contains("pipe \\| newline<br>next"));
    }

    #[test]
    fn invalid_threshold_does_not_mutate_the_report() {
        let baseline = report(100, 1_000);
        let mut current = report(110, 1_100);
        let original = serde_json::to_vec(&current).expect("serialize report");

        let error = current.apply_baseline(&baseline, f64::NAN).expect_err("NaN must be rejected");

        assert!(matches!(error, Error::InvalidThreshold(_)));
        assert_eq!(serde_json::to_vec(&current).expect("serialize report"), original);
    }

    #[test]
    fn incompatible_in_memory_baseline_is_rejected() {
        let mut baseline = report(100, 1_000);
        baseline.schema_version += 1;
        let mut current = report(110, 1_100);

        let error = current.apply_default_baseline(&baseline).expect_err("schema must be rejected");

        assert!(matches!(error, Error::UnsupportedReportSchema { .. }));
        assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);
    }

    #[test]
    fn duplicate_baseline_identity_is_rejected_before_mutation() {
        let mut baseline = report(100, 1_000);
        baseline.entries.push(baseline.entries[0].clone());
        let mut current = report(110, 1_100);

        let error = current
            .apply_default_baseline(&baseline)
            .expect_err("duplicate identity must be rejected");

        assert!(matches!(error, Error::DuplicateReportBenchmark(_)));
        assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);

        baseline.entries[1].callgrind.as_mut().expect("Callgrind metrics").instructions = 2_000;
        let error = current
            .apply_default_baseline(&baseline)
            .expect_err("conflicting duplicate identity must be rejected");
        assert!(matches!(error, Error::DuplicateReportBenchmark(_)));
    }

    #[test]
    fn read_json_rejects_incompatible_schemas() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("report.json");
        for found in [REPORT_SCHEMA_VERSION - 1, REPORT_SCHEMA_VERSION + 1] {
            let incompatible = serde_json::json!({
                "schema_version": found,
                "metadata": report(1, 1).metadata,
                "entries": []
            });
            fs::write(&path, serde_json::to_vec(&incompatible).expect("serialize fixture")).expect("write fixture");

            let error = BenchmarkReport::read_json(&path).expect_err("schema must be rejected");

            assert!(matches!(
                error,
                Error::UnsupportedReportSchema {
                    found: actual,
                    expected: REPORT_SCHEMA_VERSION,
                    ..
                } if actual == found
            ));
        }
    }

    #[test]
    fn report_writes_replace_both_formats() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let json = directory.path().join("report.json");
        let markdown = directory.path().join("report.md");
        fs::write(&json, b"old json").expect("seed json");
        fs::write(&markdown, b"old markdown").expect("seed markdown");

        report(123, 456)
            .write_reports(Some(&json), Some(&markdown))
            .expect("publish report pair");

        assert_eq!(
            BenchmarkReport::read_json(&json).expect("read report").entries[0]
                .callgrind
                .as_ref()
                .map(|metrics| metrics.instructions),
            Some(456)
        );
        assert!(fs::read_to_string(markdown).expect("read Markdown").contains("| group/work |"));
    }

    #[test]
    fn failed_pair_publication_restores_the_previous_report() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let json = directory.path().join("report.json");
        let markdown = directory.path().join("report.md");
        fs::write(&json, b"old json").expect("seed JSON");
        fs::create_dir(&markdown).expect("create invalid Markdown destination");

        let error = report(123, 456)
            .write_reports(Some(&json), Some(&markdown))
            .expect_err("pair publication must fail");

        assert!(matches!(error, Error::ReportIo { .. }));
        assert_eq!(fs::read(&json).expect("restored JSON"), b"old json");
        assert!(markdown.is_dir());
    }

    #[test]
    fn failure_between_pair_replacements_restores_both_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let json = directory.path().join("report.json");
        let markdown = directory.path().join("report.md");
        fs::write(&json, b"old json").expect("seed JSON");
        fs::write(&markdown, b"old markdown").expect("seed Markdown");
        let staged_json = stage_report(&json, b"new json").expect("stage JSON");
        let staged_markdown = stage_report(&markdown, b"new markdown").expect("stage Markdown");
        fs::remove_file(staged_markdown.path()).expect("invalidate staged Markdown");

        let error = publish_pair((&json, staged_json), (&markdown, staged_markdown)).expect_err("second replacement must fail");

        assert!(matches!(error, Error::ReportIo { .. }));
        assert_eq!(fs::read(&json).expect("restored JSON"), b"old json");
        assert_eq!(fs::read(&markdown).expect("restored Markdown"), b"old markdown");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_report_replacement_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("report.json");
        report(1, 1).write_reports(Some(&path), None).expect("write new report");
        assert_eq!(fs::metadata(&path).expect("report metadata").permissions().mode() & 0o777, 0o644);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("set report permissions");
        report(2, 2).write_reports(Some(&path), None).expect("replace existing report");
        assert_eq!(fs::metadata(path).expect("report metadata").permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn bolero_report_json_parser_is_total() {
        bolero::check!().with_type::<[u8; 256]>().for_each(|bytes| {
            let _result = serde_json::from_slice::<BenchmarkReport>(bytes);
        });
    }
}
