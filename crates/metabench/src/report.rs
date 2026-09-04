// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::{self, Write as _};
use std::io::{BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::error::Error;
use crate::metric_display::TableMetric;

pub(crate) const DEFAULT_REGRESSION_THRESHOLD: f64 = 5.0;
const REPORT_SCHEMA_VERSION: u32 = 8;
const MAX_REPORT_JSON_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_CONSOLE_WIDTH: usize = 120;
const MARKDOWN_TABLE_WIDTH: usize = 180;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetricDirection {
    LowerIsBetter,
    HigherIsBetter,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum MetricValue {
    Integer(u64),
    Float(f64),
}

impl MetricValue {
    #[expect(clippy::cast_precision_loss, reason = "percentage comparisons are approximate")]
    fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

impl fmt::Display for MetricValue {
    #[expect(clippy::renamed_function_params, reason = "the descriptive name improves readability")]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(value) => value.fmt(formatter),
            Self::Float(value) => write!(formatter, "{value:.4}"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Metric {
    pub(crate) value: MetricValue,
    #[serde(default)]
    pub(crate) display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unit: Option<String>,
    pub(crate) direction: MetricDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lower_bound: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) upper_bound: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) change_percentage: Option<f64>,
}

impl Metric {
    pub(crate) fn integer(engine: &str, name: &str, value: u64, direction: MetricDirection) -> Self {
        Self {
            value: MetricValue::Integer(value),
            display_name: TableMetric::new(engine.to_owned(), name.to_owned()).label.into_owned(),
            unit: None,
            direction,
            lower_bound: None,
            upper_bound: None,
            change_percentage: None,
        }
    }

    pub(crate) fn float(engine: &str, name: &str, value: f64, direction: MetricDirection) -> Self {
        Self {
            value: MetricValue::Float(value),
            display_name: TableMetric::new(engine.to_owned(), name.to_owned()).label.into_owned(),
            unit: None,
            direction,
            lower_bound: None,
            upper_bound: None,
            change_percentage: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NativeResult {
    pub(crate) identity: String,
    pub(crate) native_identity: String,
    pub(crate) source: String,
    pub(crate) metrics: BTreeMap<String, Metric>,
    pub(crate) raw_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EngineResult {
    pub(crate) metrics: BTreeMap<String, Metric>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) raw_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BenchmarkEntry {
    pub(crate) identity: String,
    pub(crate) results: BTreeMap<String, EngineResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prior: Option<BTreeMap<String, EngineResult>>,
    pub(crate) status: BenchmarkStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BenchmarkStatus {
    Uncompared,
    Stable,
    Improved,
    Regressed,
}

impl fmt::Display for BenchmarkStatus {
    #[expect(clippy::renamed_function_params, reason = "the descriptive name improves readability")]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uncompared => formatter.write_str("UNCOMPARED"),
            Self::Stable => formatter.write_str("STABLE"),
            Self::Improved => formatter.write_str("IMPROVED"),
            Self::Regressed => formatter.write_str("REGRESSED"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BenchmarkReport {
    pub(crate) schema_version: u32,
    pub(crate) metadata: EnvironmentMetadata,
    pub(crate) entries: Vec<BenchmarkEntry>,
}

impl BenchmarkReport {
    pub(crate) fn publication_incomplete(path: &Path) -> bool {
        incomplete_marker(path).is_file()
    }

    pub(crate) fn from_results(
        results: impl IntoIterator<Item = NativeResult>,
        backends: Vec<String>,
        artifact_root: String,
    ) -> Result<Self, Error> {
        let mut entries = BTreeMap::<String, BenchmarkEntry>::new();
        for result in results {
            let entry = entries.entry(result.identity.clone()).or_insert_with(|| BenchmarkEntry {
                identity: result.identity,
                results: BTreeMap::new(),
                prior: None,
                status: BenchmarkStatus::Uncompared,
            });
            if entry
                .results
                .insert(
                    result.source.clone(),
                    EngineResult {
                        metrics: result.metrics,
                        raw_artifacts: result.raw_artifacts,
                    },
                )
                .is_some()
            {
                return Err(Error::DuplicateReportBenchmark(format!("{} ({})", entry.identity, result.source)));
            }
        }
        let report = Self {
            schema_version: REPORT_SCHEMA_VERSION,
            metadata: EnvironmentMetadata::collect(backends, artifact_root)?,
            entries: entries.into_values().collect(),
        };
        validate_report(&report)?;
        Ok(report)
    }

    pub(crate) fn read_json(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let incomplete = incomplete_marker(path);
        if Self::publication_incomplete(path) {
            return Err(Error::ReportFormat {
                path: path.to_owned(),
                message: format!("publication is incomplete; marker {} is present", incomplete.display()),
            });
        }
        let length = fs::metadata(path)
            .map_err(|source| Error::ReportIo {
                path: path.to_owned(),
                source,
            })?
            .len();
        if length > MAX_REPORT_JSON_BYTES {
            return Err(Error::ReportFormat {
                path: path.to_owned(),
                message: format!("JSON file is {length} bytes; limit is {MAX_REPORT_JSON_BYTES} bytes"),
            });
        }
        let file = fs::File::open(path).map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?;
        let mut report: Self = serde_json::from_reader(BufReader::new(file)).map_err(|source| Error::ReportJson {
            path: path.to_owned(),
            source,
        })?;
        if report.schema_version != REPORT_SCHEMA_VERSION {
            return Err(Error::UnsupportedReportSchema {
                path: path.to_owned(),
                found: report.schema_version,
                expected: REPORT_SCHEMA_VERSION,
            });
        }
        report.refresh_display_names();
        validate_report(&report)?;
        Ok(report)
    }

    pub(crate) fn apply_baseline(&mut self, baseline: &Self, threshold: f64) -> Result<(), Error> {
        if !threshold.is_finite() || threshold < 0.0 {
            return Err(Error::InvalidThreshold(threshold.to_string()));
        }
        if baseline.schema_version != REPORT_SCHEMA_VERSION {
            return Err(Error::UnsupportedReportSchema {
                path: "<in-memory baseline>".into(),
                found: baseline.schema_version,
                expected: REPORT_SCHEMA_VERSION,
            });
        }
        validate_report(baseline)?;
        let previous = baseline
            .entries
            .iter()
            .map(|entry| (entry.identity.as_str(), entry))
            .collect::<HashMap<_, _>>();
        for entry in &mut self.entries {
            for result in entry.results.values_mut() {
                for metric in result.metrics.values_mut() {
                    metric.change_percentage = None;
                }
            }
            let Some(previous) = previous.get(entry.identity.as_str()) else {
                continue;
            };
            entry.prior = Some(previous.results.clone());
            let mut signed_changes = Vec::new();
            for (source, current_result) in &mut entry.results {
                let Some(previous_result) = previous.results.get(source) else {
                    continue;
                };
                for (name, current) in &mut current_result.metrics {
                    let Some(old) = previous_result.metrics.get(name) else {
                        continue;
                    };
                    if current.unit != old.unit || current.direction != old.direction || current.direction == MetricDirection::Unknown {
                        continue;
                    }
                    let change = percentage_shift(old.value.as_f64(), current.value.as_f64());
                    current.change_percentage = change;
                    if let Some(change) = change {
                        signed_changes.push(match current.direction {
                            MetricDirection::LowerIsBetter => change,
                            MetricDirection::HigherIsBetter => -change,
                            MetricDirection::Unknown => unreachable!("unknown metrics are excluded above"),
                        });
                    }
                }
            }
            entry.status = classify(&signed_changes, threshold);
        }
        Ok(())
    }

    pub(crate) fn write_reports(&self, json_path: Option<&Path>, markdown_path: Option<&Path>) -> Result<(), Error> {
        let json = json_path.map(|path| PreparedReport::json(path, self)).transpose()?;
        let markdown = markdown_path.map(|path| PreparedReport::markdown(path, self)).transpose()?;
        let marker = json_path.zip(markdown_path).map(|(json, markdown)| {
            let marker = incomplete_marker(json);
            write_atomic_bytes(
                &marker,
                format!("json={}\nmarkdown={}\n", json.display(), markdown.display()).as_bytes(),
            )?;
            Ok::<_, Error>(marker)
        });
        let marker = marker.transpose()?;
        if let Some(markdown) = markdown {
            markdown.publish()?;
        }
        if let Some(json) = json {
            json.publish()?;
        }
        if let Some(marker) = marker {
            fs::remove_file(&marker).map_err(|source| Error::ReportIo { path: marker, source })?;
        }
        Ok(())
    }

    pub(crate) fn render_console_table(&self) -> String {
        let width = terminal_size::terminal_size().map_or(DEFAULT_CONSOLE_WIDTH, |(terminal_size::Width(width), _)| usize::from(width));
        self.render_console_table_with_width(width)
    }

    fn render_console_table_with_width(&self, width: usize) -> String {
        let metrics = self.table_metrics();
        let rows = self.table_rows(&metrics, render_metric_console);
        let selected = select_metric_count(&metrics, &rows, width, console_projected_width);
        render_console_grid(&table_headers(&metrics[..selected]), &project_rows(&rows, selected))
    }

    fn write_markdown(&self, output: &mut impl std::io::Write) -> Result<(), io::Error> {
        let metrics = self.table_metrics();
        let rows = self.table_rows(&metrics, render_metric_markdown);
        let selected = select_metric_count(&metrics, &rows, MARKDOWN_TABLE_WIDTH, markdown_projected_width);
        let metrics = &metrics[..selected];
        let headers = table_headers(metrics);
        let rows = project_rows(&rows, selected);
        let mut environment = vec![
            vec!["Timestamp".to_owned(), self.metadata.timestamp_utc.clone()],
            vec!["Target".to_owned(), self.metadata.target_triple.clone()],
            vec!["Rust compiler".to_owned(), self.metadata.rustc_version.clone()],
            vec!["Host CPU".to_owned(), self.metadata.host_cpu.clone()],
            vec!["Profile".to_owned(), self.metadata.profile.clone()],
            vec!["Measured backends".to_owned(), self.metadata.measured_backends.join(", ")],
            vec!["Raw artifacts".to_owned(), self.metadata.artifact_root.clone()],
        ];
        if let Some(flags) = &self.metadata.compiler_flags {
            environment.push(vec!["Compiler flags".to_owned(), flags.clone()]);
        }
        output.write_all(b"# Benchmark Report\n\n## Environment\n\n")?;
        write_markdown_grid(output, &["Field".to_owned(), "Value".to_owned()], &environment)?;

        output.write_all(b"\n## Results\n\n")?;
        write_markdown_grid(output, &headers, &rows)?;
        output.write_all(b"\n")?;
        Ok(())
    }

    fn table_metrics(&self) -> Vec<TableMetric> {
        let mut metrics = self
            .entries
            .iter()
            .flat_map(|entry| entry.results.iter().chain(entry.prior.iter().flat_map(|results| results.iter())))
            .flat_map(|(engine, result)| result.metrics.keys().map(|name| (engine.clone(), name.clone())))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(engine, name)| TableMetric::new(engine, name))
            .collect::<Vec<_>>();
        metrics.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        metrics
    }

    fn refresh_display_names(&mut self) {
        for entry in &mut self.entries {
            for (engine, result) in &mut entry.results {
                for (name, metric) in &mut result.metrics {
                    metric.display_name = TableMetric::new(engine.clone(), name.clone()).label.into_owned();
                }
            }
            if let Some(prior) = &mut entry.prior {
                for (engine, result) in prior {
                    for (name, metric) in &mut result.metrics {
                        metric.display_name = TableMetric::new(engine.clone(), name.clone()).label.into_owned();
                    }
                }
            }
        }
    }

    fn table_rows(&self, metrics: &[TableMetric], render: fn(&str, &Metric) -> String) -> Vec<Vec<String>> {
        self.entries
            .iter()
            .flat_map(|entry| {
                let current = report_row(entry.identity.clone(), &entry.results, metrics, render, entry.status.to_string());
                let prior = entry
                    .prior
                    .as_ref()
                    .map(|results| report_row(format!("{} (PRIOR)", entry.identity), results, metrics, render, "-".to_owned()));
                std::iter::once(current).chain(prior)
            })
            .collect()
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
    pub(crate) measured_backends: Vec<String>,
    pub(crate) artifact_root: String,
}

impl EnvironmentMetadata {
    fn collect(measured_backends: Vec<String>, artifact_root: String) -> Result<Self, Error> {
        let timestamp_utc = Timestamp::now().to_string();
        let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .arg("-Vv")
            .output()
            .map_err(|error| Error::Metadata(error.to_string()))?;
        if !output.status.success() {
            return Err(Error::Metadata(format!("rustc -Vv exited with {}", output.status)));
        }
        Ok(Self {
            timestamp_utc,
            target_triple: env!("METABENCH_TARGET").to_owned(),
            rustc_version: String::from_utf8(output.stdout)
                .map_err(|error| Error::Metadata(error.to_string()))?
                .trim()
                .to_owned(),
            host_cpu: host_cpu(),
            profile: env!("METABENCH_PROFILE").to_owned(),
            compiler_flags: option_env!("METABENCH_RUSTFLAGS")
                .map(str::to_owned)
                .filter(|flags| !flags.is_empty()),
            measured_backends,
            artifact_root,
        })
    }
}

fn report_row(
    benchmark: String,
    results: &BTreeMap<String, EngineResult>,
    metrics: &[TableMetric],
    render: fn(&str, &Metric) -> String,
    status: String,
) -> Vec<String> {
    let mut row = vec![benchmark];
    row.extend(metrics.iter().map(|metric| {
        results
            .get(&metric.engine)
            .and_then(|result| result.metrics.get(&metric.key))
            .map_or_else(|| "-".to_owned(), |value| render(&metric.key, value))
    }));
    row.push(status);
    row
}

fn write_markdown_grid(output: &mut impl io::Write, headers: &[String], rows: &[Vec<String>]) -> Result<(), io::Error> {
    let escaped_headers = headers.iter().map(|cell| escape_table(cell)).collect::<Vec<_>>();
    let escaped_rows = rows
        .iter()
        .map(|row| row.iter().map(|cell| escape_table(cell)).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let widths = markdown_column_widths(&escaped_headers, &escaped_rows);
    write_markdown_result_row(output, &escaped_headers, &widths)?;
    let separators = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            if is_markdown_metric_column(index, widths.len()) {
                format!("{}:", "-".repeat(width - 1))
            } else {
                "-".repeat(*width)
            }
        })
        .collect::<Vec<_>>();
    write_markdown_result_row(output, &separators, &widths)?;
    for row in &escaped_rows {
        write_markdown_result_row(output, row, &widths)?;
    }
    Ok(())
}

fn write_markdown_result_row(output: &mut impl io::Write, row: &[String], widths: &[usize]) -> Result<(), io::Error> {
    output.write_all(b"|")?;
    for (index, cell) in row.iter().enumerate() {
        let padding = widths[index] - UnicodeWidthStr::width(cell.as_str());
        if is_markdown_metric_column(index, widths.len()) {
            write!(output, " {}{} |", " ".repeat(padding), cell)?;
        } else {
            write!(output, " {}{} |", cell, " ".repeat(padding))?;
        }
    }
    output.write_all(b"\n")
}

fn is_markdown_metric_column(index: usize, column_count: usize) -> bool {
    index > 0 && index + 1 < column_count
}

fn render_metric_console(name: &str, metric: &Metric) -> String {
    let unit = metric.unit.as_ref().map_or_else(String::new, |unit| format!(" {unit}"));
    format!("{}{unit}", render_metric_value(name, metric))
}

fn render_metric_markdown(name: &str, metric: &Metric) -> String {
    let mut value = render_metric_value(name, metric);
    if let Some(unit) = &metric.unit {
        let _ = write!(value, " {unit}");
    }
    if let (Some(lower), Some(upper)) = (metric.lower_bound, metric.upper_bound) {
        if name == "median" {
            let _ = write!(value, " ({lower:.2}..{upper:.2})");
        } else {
            let _ = write!(value, " ({lower:.4}..{upper:.4})");
        }
    }
    value
}

fn render_metric_value(name: &str, metric: &Metric) -> String {
    match (name, metric.value) {
        ("median", MetricValue::Float(value)) => format!("{value:.2}"),
        _ => metric.value.to_string(),
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

fn classify(changes: &[f64], threshold: f64) -> BenchmarkStatus {
    if changes.iter().any(|change| {
        if threshold > 0.0 {
            *change >= threshold
        } else {
            *change > threshold
        }
    }) {
        BenchmarkStatus::Regressed
    } else if changes.iter().any(|change| {
        if threshold > 0.0 {
            *change <= -threshold
        } else {
            *change < -threshold
        }
    }) {
        BenchmarkStatus::Improved
    } else if changes.is_empty() {
        BenchmarkStatus::Uncompared
    } else {
        BenchmarkStatus::Stable
    }
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br>")
}

fn table_headers(metrics: &[TableMetric]) -> Vec<String> {
    let mut headers = vec!["Benchmark".to_owned()];
    headers.extend(metrics.iter().map(|metric| metric.label.to_string()));
    headers.push("Status".to_owned());
    headers
}

fn select_metric_count(
    metrics: &[TableMetric],
    rows: &[Vec<String>],
    width: usize,
    projected_width: fn(&[TableMetric], &[Vec<String>], usize) -> usize,
) -> usize {
    if metrics.is_empty() {
        return 0;
    }

    let mut selected = 1;
    for count in 1..=metrics.len() {
        if projected_width(metrics, rows, count) > width {
            break;
        }
        selected = count;
    }
    selected
}

fn project_rows(rows: &[Vec<String>], metric_count: usize) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            let mut projected = Vec::with_capacity(metric_count + 2);
            projected.extend_from_slice(&row[..=metric_count]);
            projected.push(row.last().cloned().unwrap_or_default());
            projected
        })
        .collect()
}

fn console_projected_width(metrics: &[TableMetric], rows: &[Vec<String>], metric_count: usize) -> usize {
    let headers = table_headers(&metrics[..metric_count]);
    let rows = project_rows(rows, metric_count);
    console_column_widths(&headers, &rows).into_iter().sum::<usize>() + 2 * (headers.len() - 1)
}

fn markdown_projected_width(metrics: &[TableMetric], rows: &[Vec<String>], metric_count: usize) -> usize {
    let headers = table_headers(&metrics[..metric_count]);
    let rows = project_rows(rows, metric_count);
    let escaped_headers = headers.iter().map(|cell| escape_table(cell)).collect::<Vec<_>>();
    let escaped_rows = rows
        .iter()
        .map(|row| row.iter().map(|cell| escape_table(cell)).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    markdown_column_widths(&escaped_headers, &escaped_rows).into_iter().sum::<usize>() + 3 * headers.len() + 1
}

fn markdown_column_widths(headers: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths = headers
        .iter()
        .map(|header| UnicodeWidthStr::width(header.as_str()).max(3))
        .collect::<Vec<_>>();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(value.as_str()));
        }
    }
    widths
}

fn render_console_grid(headers: &[String], rows: &[Vec<String>]) -> String {
    let widths = console_column_widths(headers, rows);
    let mut output = String::new();
    render_console_row(&mut output, headers, &widths);
    let separators = widths.iter().map(|width| "-".repeat(*width)).collect::<Vec<_>>();
    render_console_row(&mut output, &separators, &widths);
    for row in rows {
        render_console_row(&mut output, row, &widths);
    }
    output
}

fn console_column_widths(headers: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths = headers
        .iter()
        .map(|header| UnicodeWidthStr::width(header.as_str()))
        .collect::<Vec<_>>();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(value.as_str()));
        }
    }
    widths
}

fn render_console_row(output: &mut String, cells: &[String], widths: &[usize]) {
    for (index, cell) in cells.iter().enumerate() {
        if index != 0 {
            output.push_str("  ");
        }
        output.push_str(cell);
        output.push_str(&" ".repeat(widths[index] - UnicodeWidthStr::width(cell.as_str())));
    }
    output.push('\n');
}

fn validate_report(report: &BenchmarkReport) -> Result<(), Error> {
    let mut identities = BTreeSet::new();
    for entry in &report.entries {
        if !identities.insert(entry.identity.as_str()) {
            return Err(Error::DuplicateReportBenchmark(entry.identity.clone()));
        }
    }
    Ok(())
}

struct PreparedReport {
    path: PathBuf,
    temporary: tempfile::NamedTempFile,
}

impl PreparedReport {
    fn json(path: &Path, report: &BenchmarkReport) -> Result<Self, Error> {
        Self::prepare(path, |writer| {
            serde_json::to_writer_pretty(writer, report).map_err(|source| Error::ReportJson {
                path: path.to_owned(),
                source,
            })
        })
    }

    fn markdown(path: &Path, report: &BenchmarkReport) -> Result<Self, Error> {
        Self::prepare(path, |writer| {
            report.write_markdown(writer).map_err(|source| Error::ReportIo {
                path: path.to_owned(),
                source,
            })
        })
    }

    fn prepare(path: &Path, write: impl FnOnce(&mut BufWriter<&mut fs::File>) -> Result<(), Error>) -> Result<Self, Error> {
        let parent = report_parent(path);
        fs::create_dir_all(parent).map_err(|source| Error::ReportIo {
            path: parent.to_owned(),
            source,
        })?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".metabench-")
            .tempfile_in(parent)
            .map_err(|source| Error::ReportIo {
                path: parent.to_owned(),
                source,
            })?;
        {
            let mut writer = BufWriter::new(temporary.as_file_mut());
            write(&mut writer)?;
            writer.flush().map_err(|source| Error::ReportIo {
                path: path.to_owned(),
                source,
            })?;
        }
        temporary.as_file().sync_all().map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self {
            path: path.to_owned(),
            temporary,
        })
    }

    fn publish(self) -> Result<(), Error> {
        self.temporary.persist(&self.path).map(|_| ()).map_err(|error| Error::ReportIo {
            path: self.path,
            source: error.error,
        })
    }
}

fn write_atomic_bytes(path: &Path, contents: &[u8]) -> Result<(), Error> {
    let prepared = PreparedReport::prepare(path, |writer| {
        writer.write_all(contents).map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })
    })?;
    prepared.publish()
}

fn report_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn incomplete_marker(path: &Path) -> PathBuf {
    let mut marker = path.as_os_str().to_owned();
    marker.push(".metabench-incomplete");
    PathBuf::from(marker)
}

fn host_cpu() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo")
            && let Some(model) = linux_cpu_identity(&cpuinfo)
        {
            return model;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let brand = command_value("sysctl", &["-n", "machdep.cpu.brand_string"]);
        let model = command_value("sysctl", &["-n", "hw.model"]);
        if let Some(identity) = macos_cpu_identity(brand.as_deref(), model.as_deref()) {
            return identity;
        }
    }
    #[cfg(target_os = "windows")]
    {
        let registry = command_value(
            "reg",
            &[
                "query",
                r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                "/v",
                "ProcessorNameString",
            ],
        );
        if let Some(identity) = windows_cpu_identity(registry.as_deref(), env::var("PROCESSOR_IDENTIFIER").ok().as_deref()) {
            return identity;
        }
    }
    cpu_identity_fallback(env::consts::ARCH)
}

#[cfg(any(target_os = "linux", test))]
fn linux_cpu_identity(cpuinfo: &str) -> Option<String> {
    ["model name", "Model", "Hardware", "Processor"].into_iter().find_map(|field| {
        cpuinfo.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim() == field)
                .then(|| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    })
}

#[cfg(any(target_os = "macos", test))]
fn macos_cpu_identity(brand: Option<&str>, model: Option<&str>) -> Option<String> {
    nonempty(brand).or_else(|| nonempty(model)).map(str::to_owned)
}

#[cfg(any(target_os = "windows", test))]
fn windows_cpu_identity(registry_output: Option<&str>, processor_identifier: Option<&str>) -> Option<String> {
    registry_output
        .and_then(|output| {
            output
                .lines()
                .find_map(|line| line.split_once("REG_SZ").and_then(|(_, value)| nonempty(Some(value))))
        })
        .or_else(|| nonempty(processor_identifier))
        .map(str::to_owned)
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn command_value(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|value| nonempty(Some(&value)).map(str::to_owned))
}

fn cpu_identity_fallback(architecture: &str) -> String {
    format!("{architecture} architecture (CPU model unavailable)")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> EnvironmentMetadata {
        EnvironmentMetadata {
            timestamp_utc: "2026-01-02T03:04:05Z".to_owned(),
            target_triple: "test-target".to_owned(),
            rustc_version: "rustc test".to_owned(),
            host_cpu: "test cpu".to_owned(),
            profile: "release".to_owned(),
            compiler_flags: Some("-C target-cpu=native".to_owned()),
            measured_backends: vec!["criterion".to_owned()],
            artifact_root: "artifacts".to_owned(),
        }
    }

    fn unescaped_pipe_positions(line: &str) -> Vec<usize> {
        line.bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'|' && line.as_bytes().get(index.wrapping_sub(1)) != Some(&b'\\')).then_some(index))
            .collect()
    }

    #[test]
    fn cpu_identity_helpers_cover_supported_platform_shapes() {
        assert_eq!(
            linux_cpu_identity("processor: 0\nmodel name : AMD Ryzen 9 7950X\n"),
            Some("AMD Ryzen 9 7950X".to_owned())
        );
        assert_eq!(
            linux_cpu_identity("Processor : AArch64 Processor rev 1\nHardware : Example SoC\n"),
            Some("Example SoC".to_owned())
        );
        assert_eq!(linux_cpu_identity("processor : 0\n"), None);

        assert_eq!(
            macos_cpu_identity(Some(" Apple M4 Max \n"), Some("Mac16,5")),
            Some("Apple M4 Max".to_owned())
        );
        assert_eq!(macos_cpu_identity(Some(""), Some(" Mac16,5 ")), Some("Mac16,5".to_owned()));
        assert_eq!(macos_cpu_identity(None, None), None);

        let registry = "\n    ProcessorNameString    REG_SZ    Intel(R) Core(TM) Ultra 9 285K\n";
        assert_eq!(
            windows_cpu_identity(Some(registry), Some("AMD64 Family 6 Model 198 Stepping 2")),
            Some("Intel(R) Core(TM) Ultra 9 285K".to_owned())
        );
        assert_eq!(
            windows_cpu_identity(None, Some(" AMD64 Family 6 Model 198 Stepping 2 ")),
            Some("AMD64 Family 6 Model 198 Stepping 2".to_owned())
        );
        assert_eq!(windows_cpu_identity(Some("unrecognized output"), Some("")), None);
        assert_eq!(cpu_identity_fallback("riscv64"), "riscv64 architecture (CPU model unavailable)");
    }

    fn engine(value: f64, direction: MetricDirection) -> EngineResult {
        EngineResult {
            metrics: BTreeMap::from([("median".to_owned(), Metric::float("criterion", "median", value, direction))]),
            raw_artifacts: vec!["raw.json".to_owned()],
        }
    }

    fn report(identity: &str, value: f64, direction: MetricDirection) -> BenchmarkReport {
        BenchmarkReport {
            schema_version: REPORT_SCHEMA_VERSION,
            metadata: metadata(),
            entries: vec![BenchmarkEntry {
                identity: identity.to_owned(),
                results: BTreeMap::from([("criterion".to_owned(), engine(value, direction))]),
                prior: None,
                status: BenchmarkStatus::Uncompared,
            }],
        }
    }

    #[test]
    fn merges_sources_by_canonical_identity() {
        let results = ["criterion", "gungraun.callgrind"]
            .into_iter()
            .zip(["median", "Ir"])
            .map(|(source, metric)| NativeResult {
                identity: "group/bench".to_owned(),
                native_identity: source.to_owned(),
                source: source.to_owned(),
                metrics: BTreeMap::from([(
                    metric.to_owned(),
                    Metric::integer(source, metric, 1, MetricDirection::LowerIsBetter),
                )]),
                raw_artifacts: Vec::new(),
            });
        let report = BenchmarkReport::from_results(results, Vec::new(), String::new()).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].results.len(), 2);
    }

    #[test]
    fn duplicate_metric_names_from_different_sources_are_disambiguated() {
        let results = ["criterion", "gungraun.callgrind"].map(|source| NativeResult {
            identity: "group/bench".to_owned(),
            native_identity: source.to_owned(),
            source: source.to_owned(),
            metrics: BTreeMap::from([(
                "shared".to_owned(),
                Metric::integer(source, "shared", 1, MetricDirection::LowerIsBetter),
            )]),
            raw_artifacts: Vec::new(),
        });

        let report = BenchmarkReport::from_results(results, Vec::new(), String::new()).unwrap();
        let metrics = report.table_metrics();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].engine, "criterion");
        assert_eq!(metrics[1].engine, "gungraun.callgrind");
    }

    #[test]
    fn duplicate_identities_are_rejected_on_read_and_apply() {
        for values in [[1.0, 2.0], [2.0, 1.0]] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("baseline.json");
            let mut baseline = report("duplicate", values[0], MetricDirection::LowerIsBetter);
            baseline
                .entries
                .push(report("duplicate", values[1], MetricDirection::LowerIsBetter).entries.remove(0));
            fs::write(&path, serde_json::to_vec(&baseline).unwrap()).unwrap();
            assert!(matches!(BenchmarkReport::read_json(&path), Err(Error::DuplicateReportBenchmark(_))));

            let mut current = report("duplicate", 3.0, MetricDirection::LowerIsBetter);
            assert!(matches!(
                current.apply_baseline(&baseline, DEFAULT_REGRESSION_THRESHOLD),
                Err(Error::DuplicateReportBenchmark(_))
            ));
        }
    }

    #[test]
    fn zero_threshold_distinguishes_stable_and_directional_changes() {
        for (direction, previous, current, expected) in [
            (MetricDirection::LowerIsBetter, 10.0, 10.0, BenchmarkStatus::Stable),
            (MetricDirection::LowerIsBetter, 10.0, 9.0, BenchmarkStatus::Improved),
            (MetricDirection::LowerIsBetter, 10.0, 11.0, BenchmarkStatus::Regressed),
            (MetricDirection::HigherIsBetter, 10.0, 11.0, BenchmarkStatus::Improved),
            (MetricDirection::HigherIsBetter, 10.0, 9.0, BenchmarkStatus::Regressed),
        ] {
            let baseline = report("bench", previous, direction);
            let mut current = report("bench", current, direction);
            current.apply_baseline(&baseline, 0.0).unwrap();
            assert_eq!(current.entries[0].status, expected);
        }
    }

    #[test]
    fn generated_comparisons_preserve_directional_classification() {
        bolero::check!()
            .with_type::<(i16, i16, u8)>()
            .for_each(|(previous, current, threshold)| {
                let previous = f64::from(*previous);
                let current = f64::from(*current);
                let threshold = f64::from(*threshold);
                let change = percentage_shift(previous, current).unwrap();
                let status = classify(&[change], threshold);
                if if threshold > 0.0 { change >= threshold } else { change > threshold } {
                    assert_eq!(status, BenchmarkStatus::Regressed);
                } else if if threshold > 0.0 {
                    change <= -threshold
                } else {
                    change < -threshold
                } {
                    assert_eq!(status, BenchmarkStatus::Improved);
                } else {
                    assert_eq!(status, BenchmarkStatus::Stable);
                }
            });
    }

    #[test]
    fn baseline_matrix_respects_compatibility_and_missing_values() {
        let baseline = report("bench", 100.0, MetricDirection::LowerIsBetter);
        for (identity, direction, unit, expected) in [
            ("other", MetricDirection::LowerIsBetter, None, BenchmarkStatus::Uncompared),
            ("bench", MetricDirection::Unknown, None, BenchmarkStatus::Uncompared),
            ("bench", MetricDirection::LowerIsBetter, Some("ns"), BenchmarkStatus::Uncompared),
            ("bench", MetricDirection::LowerIsBetter, None, BenchmarkStatus::Stable),
        ] {
            let mut current = report(identity, 101.0, direction);
            current.entries[0]
                .results
                .get_mut("criterion")
                .unwrap()
                .metrics
                .get_mut("median")
                .unwrap()
                .unit = unit.map(str::to_owned);
            current.apply_baseline(&baseline, 5.0).unwrap();
            assert_eq!(current.entries[0].status, expected);
        }
    }

    #[test]
    fn rendering_is_deterministic_and_escapes_markdown() {
        let mut current = report("b|ench\n測試", 10.0, MetricDirection::LowerIsBetter);
        let mut allocations = engine(20.0, MetricDirection::HigherIsBetter);
        allocations.metrics = BTreeMap::from([(
            "Allocations".to_owned(),
            Metric::integer("alloc_tracker", "Allocations", 20, MetricDirection::LowerIsBetter),
        )]);
        current.entries[0].results.insert("alloc_tracker".to_owned(), allocations);
        let mut baseline = report("b|ench\n測試", 8.0, MetricDirection::LowerIsBetter);
        let mut old_allocations = engine(18.0, MetricDirection::HigherIsBetter);
        old_allocations.metrics = BTreeMap::from([(
            "Allocations".to_owned(),
            Metric::integer("alloc_tracker", "Allocations", 18, MetricDirection::LowerIsBetter),
        )]);
        baseline.entries[0].results.insert("alloc_tracker".to_owned(), old_allocations);
        current.apply_baseline(&baseline, 5.0).unwrap();
        let console = current.render_console_table_with_width(usize::MAX);
        let header = console.lines().next().unwrap();
        assert!(header.contains("Benchmark"));
        assert!(header.contains("Time"));
        assert!(header.contains("Allocs"));
        assert!(!console.contains("criterion"));
        assert!(!console.contains("alloc_tracker"));
        assert!(console.contains("測試"));
        assert!(console.contains("b|ench\n測試 (PRIOR)"));

        let mut markdown = Vec::new();
        current.write_markdown(&mut markdown).unwrap();
        let markdown = String::from_utf8(markdown).unwrap();
        assert!(markdown.contains("b\\|ench<br>測試"));
        assert!(markdown.contains("Benchmark"));
        assert!(markdown.contains("Time"));
        assert!(markdown.contains("Allocs"));
        assert!(markdown.contains("10.00"));
        assert!(markdown.contains("b\\|ench<br>測試 (PRIOR)"));
        assert!(markdown.contains("8.00"));
        assert!(markdown.ends_with("\n\n"));
    }

    #[test]
    fn console_columns_are_priority_ordered_and_width_limited() {
        let mut report = report("bench", 10.0, MetricDirection::LowerIsBetter);
        report.entries[0].results.insert(
            "gungraun.callgrind".to_owned(),
            EngineResult {
                metrics: BTreeMap::from([(
                    "Ir".to_owned(),
                    Metric::integer("gungraun.callgrind", "Ir", 20, MetricDirection::LowerIsBetter),
                )]),
                raw_artifacts: Vec::new(),
            },
        );
        report.entries[0].results.insert(
            "perf".to_owned(),
            EngineResult {
                metrics: BTreeMap::from([(
                    "instructions".to_owned(),
                    Metric::integer("perf", "instructions", 30, MetricDirection::LowerIsBetter),
                )]),
                raw_artifacts: Vec::new(),
            },
        );

        let narrow = report.render_console_table_with_width(44);
        assert!(narrow.lines().next().unwrap().contains("Time"));
        assert!(narrow.lines().next().unwrap().contains("Instr"));
        assert!(!narrow.contains("HW Instr"));
        assert!(narrow.lines().all(|line| UnicodeWidthStr::width(line) <= 44));

        let wide = report.render_console_table_with_width(80);
        assert!(wide.lines().next().unwrap().contains("HW Instr"));
    }

    #[test]
    fn markdown_result_columns_are_aligned() {
        let mut current = report("short", 10.0, MetricDirection::LowerIsBetter);
        current.entries.push(
            report("a much longer benchmark", 123_456.0, MetricDirection::LowerIsBetter)
                .entries
                .remove(0),
        );
        let baseline = report("short", 8.0, MetricDirection::LowerIsBetter);
        current.apply_baseline(&baseline, 5.0).unwrap();

        let mut markdown = Vec::new();
        current.write_markdown(&mut markdown).unwrap();
        let markdown = String::from_utf8(markdown).unwrap();
        let table = markdown.split_once("## Results\n\n").unwrap().1;
        let lines = table.lines().take_while(|line| !line.is_empty()).collect::<Vec<_>>();
        let expected = unescaped_pipe_positions(lines[0]);
        assert!(lines.iter().all(|line| unescaped_pipe_positions(line) == expected));
        assert!(lines[1].split('|').nth(2).unwrap().trim_end().ends_with(':'));
        let time_cell = lines[2].split('|').nth(2).unwrap();
        assert!(time_cell.len() - time_cell.trim_start().len() > 1);
        assert!(time_cell.trim().starts_with("10.00"));
    }

    #[test]
    fn markdown_environment_columns_are_aligned() {
        let mut markdown = Vec::new();
        report("bench", 10.0, MetricDirection::LowerIsBetter)
            .write_markdown(&mut markdown)
            .unwrap();
        let markdown = String::from_utf8(markdown).unwrap();
        let table = markdown
            .split_once("## Environment\n\n")
            .unwrap()
            .1
            .split_once("\n\n## Results")
            .unwrap()
            .0;
        let lines = table.lines().collect::<Vec<_>>();
        let expected = unescaped_pipe_positions(lines[0]);

        assert!(lines.iter().all(|line| unescaped_pipe_positions(line) == expected));
        assert!(lines[2].contains("| Timestamp         |"));
        assert!(lines[7].contains("| Measured backends |"));
    }

    #[test]
    fn time_values_use_two_decimal_places_only_for_display() {
        let mut time = Metric::float("criterion", "median", 12.3456, MetricDirection::LowerIsBetter);
        time.unit = Some("ns".to_owned());
        time.lower_bound = Some(11.2345);
        time.upper_bound = Some(13.4567);

        assert_eq!(render_metric_console("median", &time), "12.35 ns");
        assert_eq!(render_metric_markdown("median", &time), "12.35 ns (11.23..13.46)");
        assert_eq!(time.value, MetricValue::Float(12.3456));

        let counter = Metric::float("perf", "cycles", 12.3456, MetricDirection::LowerIsBetter);
        assert_eq!(render_metric_console("cycles", &counter), "12.3456");
    }

    #[test]
    fn markdown_result_table_respects_its_width_budget() {
        let mut report = report("bench", 10.0, MetricDirection::LowerIsBetter);
        let metrics = &mut report.entries[0].results.get_mut("criterion").unwrap().metrics;
        for name in [
            "Allocated bytes",
            "Allocations",
            "Ir",
            "EstimatedCycles",
            "TotalBytes",
            "TotalBlocks",
            "MaximumBytes",
            "MaximumBlocks",
            "declared_throughput.bits",
        ] {
            metrics.insert(
                name.to_owned(),
                Metric::integer("criterion", name, 20, MetricDirection::LowerIsBetter),
            );
        }

        let mut markdown = Vec::new();
        report.write_markdown(&mut markdown).unwrap();
        let markdown = String::from_utf8(markdown).unwrap();
        let results = markdown.split_once("## Results\n\n").unwrap().1;
        assert!(
            results
                .lines()
                .take_while(|line| !line.is_empty())
                .all(|line| line.len() <= MARKDOWN_TABLE_WIDTH)
        );
        assert!(!results.lines().next().unwrap().contains("Declared bits"));
    }

    #[test]
    fn narrow_tables_keep_one_metric_and_json_keeps_every_metric() {
        let mut report = report("benchmark-with-a-long-name", 10.0, MetricDirection::LowerIsBetter);
        report.entries[0].results.get_mut("criterion").unwrap().metrics.insert(
            "custom_metric".to_owned(),
            Metric::integer("criterion", "custom_metric", 20, MetricDirection::Unknown),
        );

        let table = report.render_console_table_with_width(1);
        assert!(table.contains("Time"));
        assert!(!table.contains("Custom Metric"));

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("custom_metric"));
    }

    #[test]
    fn report_publication_is_atomic_and_marks_incomplete_pairs() {
        let directory = tempfile::tempdir().unwrap();
        let json = directory.path().join("report.json");
        let markdown = directory.path().join("report.md");
        report("bench", 1.0, MetricDirection::LowerIsBetter)
            .write_reports(Some(&json), Some(&markdown))
            .unwrap();
        BenchmarkReport::read_json(&json).unwrap();
        assert!(!incomplete_marker(&json).exists());
        assert!(fs::read_to_string(&markdown).unwrap().starts_with("# Benchmark Report"));

        fs::write(incomplete_marker(&json), "interrupted").unwrap();
        assert!(matches!(BenchmarkReport::read_json(&json), Err(Error::ReportFormat { .. })));

        fs::remove_file(incomplete_marker(&json)).unwrap();
        let blocked_json = directory.path().join("blocked.json");
        fs::create_dir(&blocked_json).unwrap();
        let paired_markdown = directory.path().join("paired.md");
        assert!(
            report("bench", 2.0, MetricDirection::LowerIsBetter)
                .write_reports(Some(&blocked_json), Some(&paired_markdown))
                .is_err()
        );
        assert!(incomplete_marker(&blocked_json).is_file());
    }

    #[test]
    fn report_size_limit_is_checked_before_deserialization() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.json");
        fs::File::create(&path).unwrap().set_len(MAX_REPORT_JSON_BYTES + 1).unwrap();
        assert!(matches!(BenchmarkReport::read_json(&path), Err(Error::ReportFormat { .. })));
    }

    #[test]
    fn truncated_report_json_is_never_accepted() {
        let bytes = serde_json::to_vec(&report("bench", 1.0, MetricDirection::LowerIsBetter)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        for end in (0..bytes.len()).step_by(7) {
            fs::write(&path, &bytes[..end]).unwrap();
            assert!(BenchmarkReport::read_json(&path).is_err(), "accepted prefix length {end}");
        }
        fs::write(&path, bytes).unwrap();
        BenchmarkReport::read_json(&path).unwrap();
    }

    #[test]
    fn report_schema_round_trip_and_parser_boundaries_are_deterministic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let expected = report("bench", 1.25, MetricDirection::LowerIsBetter);
        fs::write(&path, serde_json::to_vec_pretty(&expected).unwrap()).unwrap();
        let actual = BenchmarkReport::read_json(&path).unwrap();
        assert_eq!(actual.schema_version, 8);
        assert_eq!(actual.entries[0].identity, "bench");
        assert_eq!(
            actual.entries[0].results["criterion"].metrics["median"].value,
            MetricValue::Float(1.25)
        );
        assert_eq!(actual.entries[0].results["criterion"].metrics["median"].display_name, "Time");

        let mut stale_name = expected.clone();
        stale_name.entries[0]
            .results
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("median")
            .unwrap()
            .display_name = "Old Time Label".to_owned();
        fs::write(&path, serde_json::to_vec(&stale_name).unwrap()).unwrap();
        let refreshed = BenchmarkReport::read_json(&path).unwrap();
        assert_eq!(refreshed.entries[0].results["criterion"].metrics["median"].display_name, "Time");

        let valid = String::from_utf8(serde_json::to_vec(&expected).unwrap()).unwrap();
        for invalid in [
            valid.replacen(r#""schema_version":8"#, r#""schema_version":7"#, 1),
            valid.replacen(r#""schema_version":8"#, r#""schema_version":8,"schema_version":8"#, 1),
            valid.replacen("1.25", "1e9999", 1),
            format!("{valid} trailing"),
        ] {
            fs::write(&path, invalid).unwrap();
            BenchmarkReport::read_json(&path).unwrap_err();
        }
    }
}
