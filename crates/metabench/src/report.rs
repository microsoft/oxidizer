// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::{self, Write as _};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::process::Command;
use std::{env, fs, io};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::artifact::read_regular_file;
use crate::error::Error;
use crate::metric_display::TableMetric;
use crate::runner::terminal_text;

pub(crate) const DEFAULT_REGRESSION_THRESHOLD: f64 = 5.0;
const REPORT_SCHEMA_VERSION: u32 = 9;
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
    pub(crate) fn as_f64(self) -> f64 {
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
    pub(crate) confidence_level: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) statistic: Option<String>,
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
            confidence_level: None,
            statistic: None,
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
            confidence_level: None,
            statistic: None,
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
        report_file_exists(&incomplete_marker(path))
    }

    pub(crate) fn from_results(
        results: impl IntoIterator<Item = NativeResult>,
        backends: Vec<String>,
        artifact_root: String,
    ) -> Result<Self, Error> {
        Self::from_results_with_metadata(results, backends, artifact_root, EnvironmentMetadata::collect)
    }

    fn from_results_with_metadata(
        results: impl IntoIterator<Item = NativeResult>,
        backends: Vec<String>,
        artifact_root: String,
        collect: impl FnOnce(Vec<String>, String) -> EnvironmentMetadata,
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
            metadata: collect(backends, artifact_root),
            entries: entries.into_values().collect(),
        };
        validate_report(&report)?;
        Ok(report)
    }

    pub(crate) fn read_json(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        Self::read_json_with(path, Self::publication_incomplete, read_regular_file)
    }

    fn read_json_with(
        path: &Path,
        publication_incomplete: impl FnOnce(&Path) -> bool,
        read: impl FnOnce(&Path, u64, &str) -> Result<Vec<u8>, Error>,
    ) -> Result<Self, Error> {
        let incomplete = incomplete_marker(path);
        if publication_incomplete(path) {
            return Err(Error::ReportFormat {
                path: path.to_owned(),
                message: format!("publication is incomplete; marker {} is present", incomplete.display()),
            });
        }
        let bytes = read(path, MAX_REPORT_JSON_BYTES, "JSON").map_err(Self::report_read_error)?;
        let mut report: Self = serde_json::from_slice(&bytes).map_err(|source| Error::ReportJson {
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

    fn report_read_error(error: Error) -> Error {
        match error {
            Error::ArtifactIo { path, source } => Error::ReportIo { path, source },
            Error::ArtifactFormat { path, message } => Error::ReportFormat { path, message },
            error => error,
        }
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
                    if current.unit != old.unit
                        || current.statistic != old.statistic
                        || current.direction != old.direction
                        || current.direction == MetricDirection::Unknown
                    {
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
        #[cfg(test)]
        if MEMORY_REPORT_FILES.with_borrow(Option::is_some) {
            return self.write_reports_with(json_path, markdown_path, &mut MemoryReports);
        }
        self.write_reports_with(json_path, markdown_path, &mut FilesystemReports)
    }

    fn write_reports_with<P: ReportPublication>(
        &self,
        json_path: Option<&Path>,
        markdown_path: Option<&Path>,
        publication: &mut P,
    ) -> Result<(), Error> {
        let json = json_path
            .map(|path| publication.prepare(path, self, ReportEncoding::Json))
            .transpose()?;
        let markdown = markdown_path
            .map(|path| publication.prepare(path, self, ReportEncoding::Markdown))
            .transpose()?;
        let marker = json_path.zip(markdown_path).map(|(json, markdown)| {
            let marker = incomplete_marker(json);
            publication.write_marker(
                &marker,
                format!("json={}\nmarkdown={}\n", json.display(), markdown.display()).as_bytes(),
            )?;
            Ok::<_, Error>(marker)
        });
        let marker = marker.transpose()?;
        if let Some(markdown) = markdown {
            publication.publish(markdown)?;
        }
        if let Some(json) = json {
            publication.publish(json)?;
        }
        if let Some(marker) = marker {
            publication.remove_marker(marker)?;
        }
        Ok(())
    }

    pub(crate) fn render_console_table(&self) -> String {
        self.render_console_table_with_size(terminal_size::terminal_size())
    }

    fn render_console_table_with_size(&self, size: Option<(terminal_size::Width, terminal_size::Height)>) -> String {
        let width = size.map_or(DEFAULT_CONSOLE_WIDTH, |(terminal_size::Width(width), _)| usize::from(width));
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
    pub(crate) fn comparable_to(&self, other: &Self) -> bool {
        self.target_triple == other.target_triple
            && self.rustc_version == other.rustc_version
            && self.host_cpu == other.host_cpu
            && self.profile == other.profile
            && self.compiler_flags == other.compiler_flags
    }

    fn collect(measured_backends: Vec<String>, artifact_root: String) -> Self {
        Self::collect_with(
            measured_backends,
            artifact_root,
            metadata_timestamp,
            host_cpu,
            option_env!("METABENCH_RUSTFLAGS"),
        )
    }

    fn collect_with(
        measured_backends: Vec<String>,
        artifact_root: String,
        timestamp: impl FnOnce() -> String,
        host_cpu: impl FnOnce() -> String,
        compiler_flags: Option<&str>,
    ) -> Self {
        let timestamp_utc = timestamp();
        Self {
            timestamp_utc,
            target_triple: env!("METABENCH_TARGET").to_owned(),
            rustc_version: env!("METABENCH_RUSTC_VERSION").to_owned(),
            host_cpu: host_cpu(),
            profile: env!("METABENCH_PROFILE").to_owned(),
            compiler_flags: compiler_flags.map(str::to_owned).filter(|flags| !flags.is_empty()),
            measured_backends,
            artifact_root,
        }
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
        if matches!(name, "time" | "median") {
            let _ = write!(value, " ({lower:.2}..{upper:.2})");
        } else {
            let _ = write!(value, " ({lower:.4}..{upper:.4})");
        }
        if let Some(level) = metric.confidence_level {
            let _ = write!(value, " {:.1}% CI", level * 100.0);
        }
        if let Some(statistic) = &metric.statistic {
            let _ = write!(value, " [{statistic}]");
        }
    }
    value
}

fn render_metric_value(name: &str, metric: &Metric) -> String {
    match (name, metric.value) {
        ("time" | "median", MetricValue::Float(value)) => format!("{value:.2}"),
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
            // #[gamma::skip(unary.remove_neg, reason = "apply_baseline is the sole production caller and rejects negative or nonfinite thresholds; this branch therefore compares against signed zero, whose negation preserves ordering")]
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
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            let _ = write!(escaped, "&#92;u&#123;{:x}&#125;", u32::from(character));
            continue;
        }
        if character.is_control()
            || matches!(
                character,
                '&' | '<'
                    | '>'
                    | '"'
                    | '\''
                    | '\\'
                    | '`'
                    | '*'
                    | '_'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '#'
                    | '+'
                    | '-'
                    | '.'
                    | '!'
                    | '|'
            )
            || matches!(character, '\u{2028}' | '\u{2029}')
        {
            let _ = write!(escaped, "&#{};", u32::from(character));
        } else {
            escaped.push(character);
        }
    }
    escaped
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
            // #[gamma::skip(arith.add_to_mul, literal.int_decrement, literal.int_increment, literal.int_to_zero, reason = "capacity does not determine the returned cells; extend and push grow the vector as needed")]
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
        .map(|header| UnicodeWidthStr::width(terminal_text(header).as_str()))
        .collect::<Vec<_>>();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(terminal_text(value).as_str()));
        }
    }
    widths
}

fn render_console_row(output: &mut String, cells: &[String], widths: &[usize]) {
    for (index, cell) in cells.iter().enumerate() {
        let cell = terminal_text(cell);
        if index != 0 {
            output.push_str("  ");
        }
        output.push_str(&cell);
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
        for (engine, result) in entry.results.iter().chain(entry.prior.iter().flat_map(|prior| prior.iter())) {
            for (name, metric) in &result.metrics {
                let invalid = match (metric.lower_bound, metric.upper_bound) {
                    (Some(lower), Some(upper)) => {
                        metric.confidence_level.is_none()
                            || !lower.is_finite()
                            || !upper.is_finite()
                            || lower > upper
                            || (engine == "criterion" && matches!(name.as_str(), "time" | "median") && {
                                // #[gamma::skip(logical.or_remove_right, reason = "earlier disjuncts reject nonfinite bounds and lower > upper; if lower is nonnegative then upper cannot be negative")]
                                lower < 0.0 || upper < 0.0
                            })
                    }
                    (None, None) => metric.confidence_level.is_some() || (engine == "criterion" && name == "time"),
                    _ => true,
                };
                let invalid_confidence = metric
                    .confidence_level
                    .is_some_and(|level| !level.is_finite() || level <= 0.0 || level >= 1.0);
                let invalid_timing = engine == "criterion"
                    && matches!(name.as_str(), "time" | "median")
                    && (!metric.value.as_f64().is_finite() || metric.value.as_f64() < 0.0);
                let invalid_statistic = engine == "criterion"
                    && name == "time"
                    && metric.statistic.as_deref().is_none_or(|statistic| statistic.trim().is_empty());
                if invalid || invalid_confidence || invalid_timing || invalid_statistic {
                    return Err(Error::ReportFormat {
                        path: "<report>".into(),
                        message: format!(
                            "invalid estimate or confidence metadata for {} ({}/{})",
                            terminal_text(&entry.identity),
                            terminal_text(engine),
                            terminal_text(name)
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReportEncoding {
    Json,
    Markdown,
}

trait ReportPublication {
    type Prepared;

    fn prepare(&mut self, path: &Path, report: &BenchmarkReport, encoding: ReportEncoding) -> Result<Self::Prepared, Error>;
    fn write_marker(&mut self, path: &Path, contents: &[u8]) -> Result<(), Error>;
    fn publish(&mut self, prepared: Self::Prepared) -> Result<(), Error>;
    fn remove_marker(&mut self, path: PathBuf) -> Result<(), Error>;
}

struct FilesystemReports;

pub(super) fn report_file_exists(path: &Path) -> bool {
    #[cfg(test)]
    if let Some(exists) = MEMORY_REPORT_FILES.with_borrow(|files| files.as_ref().map(|files| files.contains_key(path))) {
        return exists;
    }
    path.is_file()
}

#[cfg(test)]
type ReportFiles = BTreeMap<PathBuf, Vec<u8>>;

#[cfg(test)]
thread_local! {
    static MEMORY_REPORT_FILES: std::cell::RefCell<Option<ReportFiles>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn in_memory_report_file(path: &Path) -> Option<Vec<u8>> {
    MEMORY_REPORT_FILES.with_borrow(|files| files.as_ref().and_then(|files| files.get(path).cloned()))
}

#[cfg(test)]
pub(super) fn with_in_memory_report_files<R>(files: ReportFiles, run: impl FnOnce() -> R) -> (R, ReportFiles) {
    struct Reset(Option<ReportFiles>);
    impl Drop for Reset {
        fn drop(&mut self) {
            MEMORY_REPORT_FILES.set(self.0.take());
        }
    }
    let reset = Reset(MEMORY_REPORT_FILES.replace(Some(files)));
    let result = run();
    let files = MEMORY_REPORT_FILES
        .take()
        .expect("the enclosing report fixture installs its publication observations");
    drop(reset);
    (result, files)
}

#[cfg(test)]
struct MemoryReports;

#[cfg(test)]
impl ReportPublication for MemoryReports {
    type Prepared = (PathBuf, Vec<u8>);

    fn prepare(&mut self, path: &Path, report: &BenchmarkReport, encoding: ReportEncoding) -> Result<Self::Prepared, Error> {
        let mut bytes = Vec::new();
        match encoding {
            ReportEncoding::Json => serde_json::to_writer_pretty(&mut bytes, report).map_err(|source| Error::ReportJson {
                path: path.to_owned(),
                source,
            })?,
            ReportEncoding::Markdown => report.write_markdown(&mut bytes).map_err(|source| Error::ReportIo {
                path: path.to_owned(),
                source,
            })?,
        }
        Ok((path.to_owned(), bytes))
    }

    fn write_marker(&mut self, path: &Path, contents: &[u8]) -> Result<(), Error> {
        self.publish((path.to_owned(), contents.to_vec()))
    }

    fn publish(&mut self, (path, bytes): Self::Prepared) -> Result<(), Error> {
        MEMORY_REPORT_FILES.with_borrow_mut(|files| {
            files
                .as_mut()
                .expect("the owning report fixture retains its publication map")
                .insert(path, bytes);
        });
        Ok(())
    }

    fn remove_marker(&mut self, path: PathBuf) -> Result<(), Error> {
        MEMORY_REPORT_FILES.with_borrow_mut(|files| {
            assert!(
                files
                    .as_mut()
                    .expect("the owning report fixture retains its publication map")
                    .remove(&path)
                    .is_some()
            );
        });
        Ok(())
    }
}

impl ReportPublication for FilesystemReports {
    type Prepared = PreparedReport;

    fn prepare(&mut self, path: &Path, report: &BenchmarkReport, encoding: ReportEncoding) -> Result<Self::Prepared, Error> {
        match encoding {
            ReportEncoding::Json => PreparedReport::json(path, report),
            ReportEncoding::Markdown => PreparedReport::markdown(path, report),
        }
    }

    fn write_marker(&mut self, path: &Path, contents: &[u8]) -> Result<(), Error> {
        write_atomic_bytes(path, contents)
    }

    fn publish(&mut self, prepared: Self::Prepared) -> Result<(), Error> {
        prepared.publish()
    }

    fn remove_marker(&mut self, path: PathBuf) -> Result<(), Error> {
        fs::remove_file(&path).map_err(|source| Error::ReportIo { path, source })
    }
}

trait ReportTemporary {
    type Writer: Write;

    fn writer(&mut self) -> &mut Self::Writer;
    fn sync(&self) -> io::Result<()>;
}

impl ReportTemporary for tempfile::NamedTempFile {
    type Writer = fs::File;

    fn writer(&mut self) -> &mut Self::Writer {
        self.as_file_mut()
    }

    // #[gamma::skip(fn_value.ok, reason = "this adapter calls native file synchronization; publication flush and sync failure policy is tested through ReportTemporary without filesystem fixtures")]
    fn sync(&self) -> io::Result<()> {
        self.as_file().sync_all()
    }
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
        // #[gamma::skip(try.propagate_to_unwrap, reason = "this adapter constructs a native temporary file; prepare_report_with error propagation is tested with in-memory ReportTemporary implementations")]
        let temporary = prepare_report_with(
            path,
            |parent| fs::create_dir_all(parent),
            |prefix, parent| tempfile::Builder::new().prefix(prefix).tempfile_in(parent),
            write,
        )?;
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
    write_atomic_bytes_with(
        path,
        contents,
        |path, contents| {
            PreparedReport::prepare(path, |writer| {
                writer.write_all(contents).map_err(|source| Error::ReportIo {
                    path: path.to_owned(),
                    source,
                })
            })
        },
        PreparedReport::publish,
    )
}

fn prepare_report_with<T: ReportTemporary>(
    path: &Path,
    create_parent: impl FnOnce(&Path) -> io::Result<()>,
    create_temporary: impl FnOnce(&str, &Path) -> io::Result<T>,
    write: impl FnOnce(&mut BufWriter<&mut T::Writer>) -> Result<(), Error>,
) -> Result<T, Error> {
    let parent = report_parent(path);
    create_parent(parent).map_err(|source| Error::ReportIo {
        path: parent.to_owned(),
        source,
    })?;
    let mut temporary = create_temporary(".metabench-", parent).map_err(|source| Error::ReportIo {
        path: parent.to_owned(),
        source,
    })?;
    {
        let mut writer = BufWriter::new(temporary.writer());
        write(&mut writer)?;
        writer.flush().map_err(|source| Error::ReportIo {
            path: path.to_owned(),
            source,
        })?;
    }
    temporary.sync().map_err(|source| Error::ReportIo {
        path: path.to_owned(),
        source,
    })?;
    Ok(temporary)
}

fn write_atomic_bytes_with<T>(
    path: &Path,
    contents: &[u8],
    prepare: impl FnOnce(&Path, &[u8]) -> Result<T, Error>,
    publish: impl FnOnce(T) -> Result<(), Error>,
) -> Result<(), Error> {
    let prepared = prepare(path, contents)?;
    publish(prepared)
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

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct TestHostSources {
    timestamp: fn() -> String,
    output: fn(&mut Command) -> io::Result<std::process::Output>,
    environment: fn(&str) -> Result<String, env::VarError>,
    #[cfg(target_os = "linux")]
    cpuinfo: &'static str,
}

#[cfg(test)]
thread_local! {
    static TEST_HOST_SOURCES: std::cell::Cell<Option<TestHostSources>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn with_in_memory_metadata<R>(run: impl FnOnce() -> R) -> R {
    struct Restore(Option<TestHostSources>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_HOST_SOURCES.set(self.0);
        }
    }
    let _restore = Restore(TEST_HOST_SOURCES.replace(Some(TestHostSources {
        timestamp: || "2000-01-02T03:04:05Z".into(),
        output: |_| Err(io::Error::from(io::ErrorKind::NotFound)),
        environment: |_| Ok("in-memory CPU".into()),
        #[cfg(target_os = "linux")]
        cpuinfo: "model name : in-memory CPU\n",
    })));
    run()
}

fn metadata_timestamp() -> String {
    #[cfg(test)]
    if let Some(sources) = TEST_HOST_SOURCES.get() {
        return (sources.timestamp)();
    }
    Timestamp::now().to_string()
}

#[cfg(target_os = "linux")]
fn host_cpuinfo() -> io::Result<String> {
    #[cfg(test)]
    if let Some(sources) = TEST_HOST_SOURCES.get() {
        return Ok(sources.cpuinfo.to_owned());
    }
    fs::read_to_string("/proc/cpuinfo")
}

#[cfg(any(target_os = "windows", test))]
fn host_environment(name: &str) -> Result<String, env::VarError> {
    #[cfg(test)]
    if let Some(sources) = TEST_HOST_SOURCES.get() {
        return (sources.environment)(name);
    }
    env::var(name)
}

fn host_cpu() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = host_cpuinfo()
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
        windows_host_cpu(command_value, host_environment, env::consts::ARCH)
    }
    #[cfg(not(target_os = "windows"))]
    cpu_identity_fallback(env::consts::ARCH)
}

#[cfg(any(target_os = "windows", test))]
fn windows_host_cpu(
    command: impl FnOnce(&str, &[&str]) -> Option<String>,
    environment: impl FnOnce(&str) -> Result<String, env::VarError>,
    architecture: &str,
) -> String {
    let registry = command(
        "reg",
        &[
            "query",
            r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "/v",
            "ProcessorNameString",
        ],
    );
    if let Some(identity) = windows_cpu_identity(registry.as_deref(), environment("PROCESSOR_IDENTIFIER").ok().as_deref()) {
        return identity;
    }
    cpu_identity_fallback(architecture)
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

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn command_value(program: &str, arguments: &[&str]) -> Option<String> {
    command_value_with(program, arguments, host_command_output)
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn host_command_output(command: &mut Command) -> io::Result<std::process::Output> {
    #[cfg(test)]
    if let Some(sources) = TEST_HOST_SOURCES.get() {
        return (sources.output)(command);
    }
    command.output()
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn command_value_with(
    program: &str,
    arguments: &[&str],
    output: impl FnOnce(&mut Command) -> io::Result<std::process::Output>,
) -> Option<String> {
    let mut command = Command::new(program);
    command.args(arguments);
    let output = output(&mut command).ok()?;
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
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Debug)]
    struct MemoryPrepared {
        kind: &'static str,
        path: PathBuf,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl Drop for MemoryPrepared {
        fn drop(&mut self) {
            self.events.borrow_mut().push(format!("drop-{}", self.kind));
        }
    }

    struct MemoryPublication {
        failure: Option<&'static str>,
        events: Rc<RefCell<Vec<String>>>,
        marker: bool,
    }

    #[derive(Debug)]
    struct MemoryReportFile {
        contents: Vec<u8>,
        failure: &'static str,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl Write for MemoryReportFile {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.events.borrow_mut().push("write".into());
            if self.failure == "write" {
                Err(io::Error::other("write refused"))
            } else {
                self.contents.extend_from_slice(buf);
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            self.events.borrow_mut().push("flush".into());
            if self.failure == "flush" {
                Err(io::Error::other("flush refused"))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Debug)]
    struct MemoryTemporary {
        file: MemoryReportFile,
    }

    impl Drop for MemoryTemporary {
        fn drop(&mut self) {
            self.file.events.borrow_mut().push("drop".into());
        }
    }

    impl ReportTemporary for MemoryTemporary {
        type Writer = MemoryReportFile;

        fn writer(&mut self) -> &mut Self::Writer {
            &mut self.file
        }

        fn sync(&self) -> io::Result<()> {
            self.file.events.borrow_mut().push("sync".into());
            if self.file.failure == "sync" {
                Err(io::Error::other("sync refused"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn november_report_preparation_owns_temporary_until_write_flush_and_sync_finish() {
        let path = Path::new("reports").join("bench.json");
        for (case, expected) in [
            ("mkdir", vec!["mkdir"]),
            ("temporary", vec!["mkdir", "temporary"]),
            ("callback", vec!["mkdir", "temporary", "callback", "drop"]),
            ("write", vec!["mkdir", "temporary", "callback", "write", "write", "drop"]),
            ("flush", vec!["mkdir", "temporary", "callback", "write", "flush", "drop"]),
            ("sync", vec!["mkdir", "temporary", "callback", "write", "flush", "sync", "drop"]),
            ("success", vec!["mkdir", "temporary", "callback", "write", "flush", "sync", "drop"]),
        ] {
            let events = Rc::new(RefCell::new(Vec::new()));
            let result = prepare_report_with(
                &path,
                |parent| {
                    assert_eq!(parent, Path::new("reports"));
                    events.borrow_mut().push("mkdir".into());
                    if case == "mkdir" {
                        Err(io::Error::other("mkdir refused"))
                    } else {
                        Ok(())
                    }
                },
                |prefix, parent| {
                    assert_eq!(parent, Path::new("reports"));
                    assert_eq!(prefix, ".metabench-");
                    events.borrow_mut().push("temporary".into());
                    if case == "temporary" {
                        Err(io::Error::other("temporary refused"))
                    } else {
                        Ok(MemoryTemporary {
                            file: MemoryReportFile {
                                contents: Vec::new(),
                                failure: case,
                                events: Rc::clone(&events),
                            },
                        })
                    }
                },
                |writer| {
                    events.borrow_mut().push("callback".into());
                    if case == "callback" {
                        Err(Error::ReportIo {
                            path: "callback".into(),
                            source: io::Error::other("callback refused"),
                        })
                    } else {
                        writer.write_all(b"report").map_err(|source| Error::ReportIo {
                            path: "callback".into(),
                            source,
                        })
                    }
                },
            );
            if case == "success" {
                let temporary = result.unwrap();
                assert_eq!(temporary.file.contents, b"report");
                assert_eq!(events.borrow().last().unwrap(), "sync");
                drop(temporary);
            } else {
                let expected_path = match case {
                    "mkdir" | "temporary" => Path::new("reports"),
                    "callback" => Path::new("callback"),
                    _ => &path,
                };
                assert!(
                    matches!(result, Err(Error::ReportIo { path, source }) if path == expected_path && source.kind() == io::ErrorKind::Other && source.to_string() == format!("{case} refused"))
                );
            }
            assert_eq!(*events.borrow(), expected, "{case}");
        }
    }

    impl MemoryPublication {
        fn event(&self, operation: &'static str, path: &Path) -> Result<(), Error> {
            self.events.borrow_mut().push(operation.into());
            if self.failure == Some(operation) {
                Err(Error::ReportIo {
                    path: path.to_owned(),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, operation),
                })
            } else {
                Ok(())
            }
        }
    }

    impl ReportPublication for MemoryPublication {
        type Prepared = MemoryPrepared;

        fn prepare(&mut self, path: &Path, report: &BenchmarkReport, encoding: ReportEncoding) -> Result<Self::Prepared, Error> {
            assert_eq!(report.metadata.artifact_root, "artifacts");
            let (operation, kind, expected_path) = match encoding {
                ReportEncoding::Json => ("prepare-json", "json", "report.json"),
                ReportEncoding::Markdown => ("prepare-markdown", "markdown", "report.md"),
            };
            assert_eq!(path, Path::new(expected_path));
            self.event(operation, path)?;
            Ok(MemoryPrepared {
                kind,
                path: path.to_owned(),
                events: Rc::clone(&self.events),
            })
        }

        fn write_marker(&mut self, path: &Path, contents: &[u8]) -> Result<(), Error> {
            assert_eq!(path, Path::new("report.json.metabench-incomplete"));
            assert_eq!(contents, b"json=report.json\nmarkdown=report.md\n");
            self.event("marker-write", path)?;
            self.marker = true;
            Ok(())
        }

        fn publish(&mut self, prepared: Self::Prepared) -> Result<(), Error> {
            self.event(
                if prepared.kind == "json" {
                    "publish-json"
                } else {
                    "publish-markdown"
                },
                &prepared.path,
            )
        }

        fn remove_marker(&mut self, path: PathBuf) -> Result<(), Error> {
            assert_eq!(path, Path::new("report.json.metabench-incomplete"));
            assert!(self.marker);
            self.event("marker-remove", &path)?;
            self.marker = false;
            Ok(())
        }
    }

    #[test]
    fn november_report_publication_preserves_exact_order_markers_errors_and_partial_ownership() {
        let report = BenchmarkReport {
            schema_version: REPORT_SCHEMA_VERSION,
            metadata: metadata(),
            entries: Vec::new(),
        };
        for (paths, failure, expected) in [
            (0, None, vec![]),
            (1, None, vec!["prepare-json", "publish-json", "drop-json"]),
            (2, None, vec!["prepare-markdown", "publish-markdown", "drop-markdown"]),
            (
                3,
                None,
                vec![
                    "prepare-json",
                    "prepare-markdown",
                    "marker-write",
                    "publish-markdown",
                    "drop-markdown",
                    "publish-json",
                    "drop-json",
                    "marker-remove",
                ],
            ),
            (3, Some("prepare-json"), vec!["prepare-json"]),
            (3, Some("prepare-markdown"), vec!["prepare-json", "prepare-markdown", "drop-json"]),
            (
                3,
                Some("marker-write"),
                vec!["prepare-json", "prepare-markdown", "marker-write", "drop-markdown", "drop-json"],
            ),
            (
                3,
                Some("publish-markdown"),
                vec![
                    "prepare-json",
                    "prepare-markdown",
                    "marker-write",
                    "publish-markdown",
                    "drop-markdown",
                    "drop-json",
                ],
            ),
            (
                3,
                Some("publish-json"),
                vec![
                    "prepare-json",
                    "prepare-markdown",
                    "marker-write",
                    "publish-markdown",
                    "drop-markdown",
                    "publish-json",
                    "drop-json",
                ],
            ),
            (
                3,
                Some("marker-remove"),
                vec![
                    "prepare-json",
                    "prepare-markdown",
                    "marker-write",
                    "publish-markdown",
                    "drop-markdown",
                    "publish-json",
                    "drop-json",
                    "marker-remove",
                ],
            ),
        ] {
            let events = Rc::new(RefCell::new(Vec::new()));
            let mut publication = MemoryPublication {
                failure,
                events: Rc::clone(&events),
                marker: false,
            };
            let result = report.write_reports_with(
                (paths & 1 != 0).then_some(Path::new("report.json")),
                (paths & 2 != 0).then_some(Path::new("report.md")),
                &mut publication,
            );
            if let Some(operation) = failure {
                let expected_path = match operation {
                    "prepare-json" | "publish-json" => "report.json",
                    "prepare-markdown" | "publish-markdown" => "report.md",
                    _ => "report.json.metabench-incomplete",
                };
                assert!(
                    matches!(result, Err(Error::ReportIo { path, source }) if path == Path::new(expected_path) && source.kind() == io::ErrorKind::PermissionDenied && source.to_string() == operation)
                );
            } else {
                result.unwrap();
            }
            assert_eq!(*events.borrow(), expected, "{paths}/{failure:?}");
            assert_eq!(
                publication.marker,
                matches!(failure, Some("publish-markdown" | "publish-json" | "marker-remove"))
            );
        }
    }

    #[test]
    fn november_atomic_report_bytes_forward_exact_payload_and_preserve_prepare_or_publish_errors() {
        for failure in [None, Some("prepare"), Some("publish")] {
            let events = Rc::new(RefCell::new(Vec::new()));
            let result = write_atomic_bytes_with(
                Path::new("report"),
                b"exact\0bytes\n",
                |path, contents| {
                    assert_eq!(path, Path::new("report"));
                    assert_eq!(contents, b"exact\0bytes\n");
                    events.borrow_mut().push("prepare".into());
                    if failure == Some("prepare") {
                        Err(Error::ReportIo {
                            path: path.to_owned(),
                            source: io::Error::other("prepare refused"),
                        })
                    } else {
                        Ok(MemoryPrepared {
                            kind: "bytes",
                            path: path.to_owned(),
                            events: Rc::clone(&events),
                        })
                    }
                },
                |prepared| {
                    assert_eq!(prepared.path, Path::new("report"));
                    events.borrow_mut().push("publish".into());
                    if failure == Some("publish") {
                        Err(Error::ReportIo {
                            path: prepared.path.clone(),
                            source: io::Error::other("publish refused"),
                        })
                    } else {
                        Ok(())
                    }
                },
            );
            if let Some(operation) = failure {
                assert!(
                    matches!(result, Err(Error::ReportIo { path, source }) if path == Path::new("report") && source.to_string() == format!("{operation} refused"))
                );
            } else {
                result.unwrap();
            }
            assert_eq!(
                *events.borrow(),
                if failure == Some("prepare") {
                    vec!["prepare"]
                } else {
                    vec!["prepare", "publish", "drop-bytes"]
                }
            );
        }
    }

    struct TestHostScope(Option<TestHostSources>);

    impl TestHostScope {
        fn new() -> Self {
            Self(TEST_HOST_SOURCES.replace(Some(TestHostSources {
                timestamp: || "2001-02-03T04:05:06Z".into(),
                output: deterministic_host_output,
                environment: |name| {
                    assert_eq!(name, "PROCESSOR_IDENTIFIER");
                    Ok("november fallback CPU".into())
                },
                #[cfg(target_os = "linux")]
                cpuinfo: "model name : november CPU\n",
            })))
        }
    }

    impl Drop for TestHostScope {
        fn drop(&mut self) {
            TEST_HOST_SOURCES.set(self.0);
        }
    }

    fn deterministic_host_output(command: &mut Command) -> io::Result<std::process::Output> {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;
        use std::process::{ExitStatus, Output};

        let arguments = command.get_args().collect::<Vec<_>>();
        if command.get_program() == "error-probe" {
            assert!(arguments.is_empty());
            return Err(io::Error::other("probe denied"));
        }
        let registry = command.get_program() == "reg";
        if registry {
            assert_eq!(
                arguments,
                [
                    "query",
                    r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                    "/v",
                    "ProcessorNameString"
                ]
            );
        } else if command.get_program() == "sysctl" {
            assert_eq!(arguments[0], "-n");
            assert!(arguments[1] == "machdep.cpu.brand_string" || arguments[1] == "hw.model");
            assert_eq!(arguments.len(), 2);
        } else {
            assert_eq!(command.get_program(), "november-probe");
            assert_eq!(arguments, ["--data"]);
        }
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: if registry {
                b"ProcessorNameString REG_SZ november CPU\n".to_vec()
            } else {
                b"november CPU\n".to_vec()
            },
            stderr: Vec::new(),
        })
    }

    #[test]
    fn november_actual_report_entrypoints_forward_metadata_without_native_observations() {
        let _scope = TestHostScope::new();
        let backends = vec!["criterion".to_owned(), "allocations".to_owned()];
        let report = BenchmarkReport::from_results(std::iter::empty::<NativeResult>(), backends.clone(), "owned-run".into()).unwrap();
        assert!(report.entries.is_empty());
        assert_eq!(report.metadata.measured_backends, backends);
        assert_eq!(report.metadata.artifact_root, "owned-run");
        assert_eq!(report.metadata.timestamp_utc, "2001-02-03T04:05:06Z");
        assert_eq!(report.metadata.host_cpu, "november CPU");
        let metadata = EnvironmentMetadata::collect(vec!["vtune".into()], "another-run".into());
        assert_eq!(metadata.measured_backends, ["vtune"]);
        assert_eq!(metadata.artifact_root, "another-run");
        assert_eq!(host_cpu(), "november CPU");
        assert_eq!(command_value("november-probe", &["--data"]).as_deref(), Some("november CPU"));
        assert_eq!(command_value("error-probe", &[]), None);
        assert_eq!(host_environment("PROCESSOR_IDENTIFIER").unwrap(), "november fallback CPU");
    }

    #[test]
    fn gamma_round2_console_width_fallback_is_exactly_120_columns() {
        let mut report = gamma_report(
            "perf",
            "cycles",
            Metric::integer("perf", "cycles", 17, MetricDirection::LowerIsBetter),
        );
        report.entries[0].results.get_mut("perf").unwrap().metrics.insert(
            "instructions".into(),
            Metric::integer("perf", "instructions", 23, MetricDirection::LowerIsBetter),
        );
        for (required_width, included) in [(120, true), (121, false)] {
            let identity_length = (10..150)
                .find(|length| {
                    report.entries[0].identity = "x".repeat(*length);
                    let metrics = report.table_metrics();
                    let rows = report.table_rows(&metrics, render_metric_console);
                    console_projected_width(&metrics, &rows, 2) == required_width
                })
                .unwrap();
            report.entries[0].identity = "x".repeat(identity_length);
            assert_eq!(report.render_console_table_with_size(None).contains("HW Cycles"), included);
            assert!(
                report
                    .render_console_table_with_size(Some((terminal_size::Width(300), terminal_size::Height(20))))
                    .contains("HW Cycles")
            );
            let narrow = report.render_console_table_with_size(Some((terminal_size::Width(1), terminal_size::Height(20))));
            assert!(narrow.contains("HW Instr"));
            assert!(!narrow.contains("HW Cycles"));
        }
    }

    #[test]
    fn gamma_round2_report_reader_validates_policy_schema_and_publication_before_reading() {
        let path = Path::new("report.json");
        let expected = report("case", 7.0, MetricDirection::LowerIsBetter);
        let bytes = serde_json::to_vec(&expected).unwrap();
        let actual = BenchmarkReport::read_json_with(
            path,
            |candidate| {
                assert_eq!(candidate, path);
                false
            },
            |candidate, limit, format| {
                assert_eq!(candidate, path);
                assert_eq!(limit, 64 * 1024 * 1024);
                assert_eq!(format, "JSON");
                Ok(bytes)
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(expected).unwrap());
        assert!(
            matches!(BenchmarkReport::read_json_with(path, |_| true, |_, _, _| panic!("incomplete report")),
            Err(Error::ReportFormat { path: failed, message }) if failed == path && message == "publication is incomplete; marker report.json.metabench-incomplete is present")
        );
        assert!(
            matches!(BenchmarkReport::read_json_with(path, |_| false, |_, _, _| Err(Error::ArtifactIo { path: path.into(), source: io::Error::new(io::ErrorKind::PermissionDenied, "read refused") })),
            Err(Error::ReportIo { path: failed, source }) if failed == path && source.kind() == io::ErrorKind::PermissionDenied)
        );
        assert!(
            matches!(BenchmarkReport::read_json_with(path, |_| false, |_, _, _| Ok(b"invalid JSON".to_vec())),
            Err(Error::ReportJson { path: failed, .. }) if failed == path)
        );
        let mut incompatible = report("case", 7.0, MetricDirection::LowerIsBetter);
        incompatible.schema_version = 8;
        assert!(
            matches!(BenchmarkReport::read_json_with(path, |_| false, |_, _, _| Ok(serde_json::to_vec(&incompatible).unwrap())),
            Err(Error::UnsupportedReportSchema { path: failed, found: 8, expected: 9 }) if failed == path)
        );
    }

    #[test]
    fn gamma_round2_environment_metadata_retains_inputs_and_nonempty_flags() {
        for flags in [None, Some(""), Some("-Ctarget-cpu=example")] {
            let metadata = EnvironmentMetadata::collect_with(
                vec!["perf".into(), "alloc_tracker".into()],
                "raw artifacts".into(),
                || "fixed timestamp".into(),
                || "fixed CPU".into(),
                flags,
            );
            assert_eq!(metadata.timestamp_utc, "fixed timestamp");
            assert_eq!(metadata.target_triple, env!("METABENCH_TARGET"));
            assert_eq!(metadata.rustc_version, env!("METABENCH_RUSTC_VERSION"));
            assert_eq!(metadata.host_cpu, "fixed CPU");
            assert_eq!(metadata.profile, env!("METABENCH_PROFILE"));
            assert_eq!(
                metadata.compiler_flags.as_deref(),
                match flags {
                    Some("") => None,
                    other => other,
                }
            );
            assert_eq!(metadata.measured_backends, ["perf", "alloc_tracker"]);
            assert_eq!(metadata.artifact_root, "raw artifacts");
        }
    }

    #[test]
    fn gamma_round2_windows_cpu_query_preserves_registry_env_and_fallback_precedence() {
        for (registry, environment, expected) in [
            (
                Some("ProcessorNameString REG_SZ Registry CPU"),
                Ok("Environment CPU".to_owned()),
                "Registry CPU",
            ),
            (None, Ok("Environment CPU".to_owned()), "Environment CPU"),
            (
                Some("ProcessorNameString REG_SZ  "),
                Err(env::VarError::NotPresent),
                "example architecture (CPU model unavailable)",
            ),
        ] {
            let environment_called = std::cell::Cell::new(false);
            let actual = windows_host_cpu(
                |program, arguments| {
                    assert_eq!(program, "reg");
                    assert_eq!(
                        arguments,
                        [
                            "query",
                            r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                            "/v",
                            "ProcessorNameString"
                        ]
                    );
                    registry.map(str::to_owned)
                },
                |name| {
                    assert_eq!(name, "PROCESSOR_IDENTIFIER");
                    environment_called.set(true);
                    environment
                },
                "example",
            );
            assert!(environment_called.get());
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn gamma_round2_metadata_commands_validate_status_utf8_and_trim_without_spawning() {
        use std::ffi::OsStr;
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt as _;
        use std::process::{ExitStatus, Output};

        for (status, stdout, expected) in [
            (0, b"  CPU name \r\n".to_vec(), Some("CPU name")),
            (0, b" \t\r\n".to_vec(), None),
            (0, vec![0xff], None),
            (1, b"CPU name".to_vec(), None),
        ] {
            let actual = command_value_with("metadata-tool", &["first", "space argument"], |command| {
                assert_eq!(command.get_program(), "metadata-tool");
                assert_eq!(command.get_args().collect::<Vec<_>>(), ["first", "space argument"].map(OsStr::new));
                Ok(Output {
                    status: ExitStatus::from_raw(status),
                    stdout,
                    stderr: Vec::new(),
                })
            });
            assert_eq!(actual.as_deref(), expected);
        }
        assert!(command_value_with("metadata-tool", &[], |_| Err(io::Error::other("not installed"))).is_none());
    }

    #[test]
    fn gamma_round2_report_assembly_preserves_metadata_cases_sources_and_initial_state() {
        let result = |identity: &str, source: &str| NativeResult {
            identity: identity.into(),
            native_identity: format!("native::{identity}"),
            source: source.into(),
            metrics: BTreeMap::from([(
                "cycles".into(),
                Metric::integer(source, "cycles", 17, MetricDirection::LowerIsBetter),
            )]),
            raw_artifacts: vec![format!("{identity}-{source}.json")],
        };
        let report = BenchmarkReport::from_results_with_metadata(
            [
                result("z-case", "perf"),
                result("a-case", "perf"),
                result("z-case", "alloc_tracker"),
            ],
            vec!["perf".into(), "allocations".into()],
            "artifact-root".into(),
            |backends, root| {
                assert_eq!(backends, ["perf", "allocations"]);
                assert_eq!(root, "artifact-root");
                let mut value = metadata();
                value.measured_backends = backends;
                value.artifact_root = root;
                value
            },
        )
        .unwrap();
        assert_eq!(report.schema_version, REPORT_SCHEMA_VERSION);
        assert_eq!(report.metadata.measured_backends, ["perf", "allocations"]);
        assert_eq!(report.metadata.artifact_root, "artifact-root");
        assert_eq!(
            report.entries.iter().map(|entry| entry.identity.as_str()).collect::<Vec<_>>(),
            ["a-case", "z-case"]
        );
        for entry in &report.entries {
            assert!(entry.prior.is_none());
            assert_eq!(entry.status, BenchmarkStatus::Uncompared);
            assert_eq!(entry.results["perf"].metrics["cycles"].value.as_f64().to_bits(), 17.0_f64.to_bits());
            assert_eq!(entry.results["perf"].raw_artifacts, [format!("{}-perf.json", entry.identity)]);
        }
        assert_eq!(report.entries[1].results.len(), 2);
        assert_eq!(
            report.entries[1].results["alloc_tracker"].raw_artifacts,
            ["z-case-alloc_tracker.json"]
        );
    }

    #[test]
    fn gamma_round2_report_assembly_returns_duplicate_and_validation_errors() {
        let result = || NativeResult {
            identity: "case".into(),
            native_identity: "native::case".into(),
            source: "perf".into(),
            metrics: BTreeMap::new(),
            raw_artifacts: Vec::new(),
        };
        assert!(matches!(
            BenchmarkReport::from_results_with_metadata([result(), result()], Vec::new(), String::new(), |_, _| panic!("duplicates fail before collecting metadata")),
            Err(Error::DuplicateReportBenchmark(identity)) if identity == "case (perf)"
        ));
        let invalid = NativeResult {
            source: "criterion".into(),
            metrics: BTreeMap::from([(
                "time".into(),
                Metric::float("criterion", "time", f64::NAN, MetricDirection::LowerIsBetter),
            )]),
            ..result()
        };
        assert!(matches!(
            BenchmarkReport::from_results_with_metadata([invalid], Vec::new(), String::new(), |_, _| metadata()),
            Err(Error::ReportFormat { path, message })
                if path == Path::new("<report>") && message == "invalid estimate or confidence metadata for case (criterion/time)"
        ));
    }

    #[test]
    fn gamma_round2_changes_inside_the_threshold_remain_stable() {
        for change in [-4.0, -1.0, 0.0, 1.0, 4.0] {
            assert_eq!(classify(&[change], 5.0), BenchmarkStatus::Stable, "{change}");
        }
        assert_eq!(classify(&[-5.0], 5.0), BenchmarkStatus::Improved);
        assert_eq!(classify(&[5.0], 5.0), BenchmarkStatus::Regressed);
    }

    #[test]
    fn november_metric_selection_stops_at_first_overflow_including_zero_and_equal_widths() {
        fn width(_metrics: &[TableMetric], _rows: &[Vec<String>], count: usize) -> usize {
            assert_ne!(count, 0, "a nonempty table always retains its first metric");
            [0, 7, 11, 19][count]
        }
        let metrics = [
            TableMetric::new("perf".into(), "cycles".into()),
            TableMetric::new("perf".into(), "instructions".into()),
            TableMetric::new("perf".into(), "cache-misses".into()),
        ];
        let rows = vec![vec!["case".into(), "17".into(), "23".into(), "29".into(), "STABLE".into()]];
        for (available, count) in [(0, 1), (6, 1), (7, 1), (10, 1), (11, 2), (18, 2), (19, 3), (20, 3)] {
            assert_eq!(select_metric_count(&metrics, &rows, available, width), count);
            assert_eq!(project_rows(&rows, count)[0].len(), count + 2);
        }
        assert_eq!(
            select_metric_count(&metrics, &rows, 8, |_, _, count| [0, 9, 0, 0][count]),
            1,
            "later smaller widths must not resume selection after the first overflowing prefix"
        );
        assert_eq!(select_metric_count(&[], &[], 0, |_, _, _| panic!("empty table")), 0);
    }

    #[test]
    fn november_zero_threshold_keeps_positive_zero_and_detects_only_negative_improvements() {
        for (changes, expected) in [
            (&[][..], BenchmarkStatus::Uncompared),
            (&[0.0][..], BenchmarkStatus::Stable),
            (&[-0.0][..], BenchmarkStatus::Stable),
            (&[-f64::EPSILON][..], BenchmarkStatus::Improved),
            (&[f64::EPSILON][..], BenchmarkStatus::Regressed),
            (&[-f64::EPSILON, f64::EPSILON][..], BenchmarkStatus::Regressed),
        ] {
            assert_eq!(classify(changes, 0.0), expected, "{changes:?}");
        }
    }

    #[test]
    fn gamma_round2_console_rendering_preserves_the_single_metric_at_any_width() {
        let report = gamma_report(
            "perf",
            "cycles",
            Metric::integer("perf", "cycles", 17, MetricDirection::LowerIsBetter),
        );
        let console = report.render_console_table();
        for required in ["case", "17", "HW Cycles", "UNCOMPARED"] {
            assert!(console.contains(required), "{required}: {console}");
        }
    }

    #[test]
    fn gamma_round2_console_always_retains_one_metric_at_tiny_width() {
        let metrics = vec![
            TableMetric::new("perf".into(), "cycles".into()),
            TableMetric::new("perf".into(), "instructions".into()),
        ];
        let rows = vec![vec!["case".into(), "17".into(), "23".into(), "STABLE".into()]];
        assert_eq!(select_metric_count(&metrics, &rows, 0, console_projected_width), 1);
        assert_eq!(select_metric_count(&metrics, &rows, 0, markdown_projected_width), 1);
        assert_eq!(project_rows(&rows, 1), [vec!["case", "17", "STABLE"]]);
    }

    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use super::*;

    fn gamma_report(engine: &str, name: &str, metric: Metric) -> BenchmarkReport {
        BenchmarkReport {
            schema_version: REPORT_SCHEMA_VERSION,
            metadata: metadata(),
            entries: vec![BenchmarkEntry {
                identity: "case".into(),
                results: BTreeMap::from([(
                    engine.into(),
                    EngineResult {
                        metrics: BTreeMap::from([(name.into(), metric)]),
                        raw_artifacts: vec!["raw.json".into()],
                    },
                )]),
                prior: None,
                status: BenchmarkStatus::Uncompared,
            }],
        }
    }

    #[test]
    fn gamma_survivor_metric_defaults_and_status() {
        for metric in [
            Metric::integer("perf", "cycles", 17, MetricDirection::LowerIsBetter),
            Metric::float("perf", "cycles", 1.25, MetricDirection::HigherIsBetter),
        ] {
            assert_eq!(metric.display_name, "HW Cycles");
            assert_eq!(metric.unit, None);
            assert_eq!(metric.statistic, None);
            assert_eq!(metric.lower_bound, None);
            assert_eq!(metric.upper_bound, None);
            assert_eq!(metric.confidence_level, None);
            assert_eq!(metric.change_percentage, None);
        }
        for (status, text) in [
            (BenchmarkStatus::Uncompared, "UNCOMPARED"),
            (BenchmarkStatus::Stable, "STABLE"),
            (BenchmarkStatus::Improved, "IMPROVED"),
            (BenchmarkStatus::Regressed, "REGRESSED"),
        ] {
            assert_eq!(status.to_string(), text);
        }
    }

    #[test]
    fn gamma_survivor_percentage_and_threshold_boundaries() {
        for (old, new, expected) in [
            (0.0, 0.0, Some(0.0)),
            (0.0, 0.5, Some(100.0)),
            (1.0, 2.0, Some(100.0)),
            (4.0, 3.0, Some(-25.0)),
            (f64::NAN, 2.0, None),
            (2.0, f64::INFINITY, None),
            (f64::NEG_INFINITY, 2.0, None),
            (2.0, f64::NAN, None),
        ] {
            assert_eq!(percentage_shift(old, new), expected);
        }
        for (changes, threshold, expected) in [
            (vec![], 0.0, BenchmarkStatus::Uncompared),
            (vec![0.0], 0.0, BenchmarkStatus::Stable),
            (vec![0.5], 0.0, BenchmarkStatus::Regressed),
            (vec![-0.5], 0.0, BenchmarkStatus::Improved),
            (vec![0.5], 0.5, BenchmarkStatus::Regressed),
            (vec![-0.5], 0.5, BenchmarkStatus::Improved),
            (vec![5.0], 5.0, BenchmarkStatus::Regressed),
            (vec![-5.0], 5.0, BenchmarkStatus::Improved),
            (vec![-6.0, 6.0], 5.0, BenchmarkStatus::Regressed),
            (vec![-4.9, 4.9], 5.0, BenchmarkStatus::Stable),
        ] {
            assert_eq!(classify(&changes, threshold), expected);
        }
    }

    #[test]
    fn gamma_survivor_baseline_validation_and_changes() {
        let metric = Metric::integer("perf", "cycles", 100, MetricDirection::LowerIsBetter);
        let baseline = gamma_report("perf", "cycles", metric.clone());
        for threshold in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                baseline.clone().apply_baseline(&baseline, threshold),
                Err(Error::InvalidThreshold(_))
            ));
        }
        let mut wrong_schema = baseline.clone();
        wrong_schema.schema_version = 0;
        assert!(matches!(
            baseline.clone().apply_baseline(&wrong_schema, 5.0),
            Err(Error::UnsupportedReportSchema { path, found: 0, expected: REPORT_SCHEMA_VERSION })
                if path == Path::new("<in-memory baseline>")
        ));
        let mut current = gamma_report(
            "perf",
            "cycles",
            Metric::integer("perf", "cycles", 105, MetricDirection::LowerIsBetter),
        );
        current.apply_baseline(&baseline, 5.0).unwrap();
        assert_eq!(current.entries[0].status, BenchmarkStatus::Regressed);
        assert_eq!(current.entries[0].results["perf"].metrics["cycles"].change_percentage, Some(5.0));
        assert_eq!(
            current.entries[0].prior.as_ref().unwrap()["perf"].metrics["cycles"].value,
            metric.value
        );

        for difference in 0..4 {
            let mut old = baseline.clone();
            let previous = old.entries[0].results.get_mut("perf").unwrap().metrics.get_mut("cycles").unwrap();
            match difference {
                0 => previous.unit = Some("ns".into()),
                1 => previous.statistic = Some("mean".into()),
                2 => previous.direction = MetricDirection::HigherIsBetter,
                _ => {
                    previous.direction = MetricDirection::Unknown;
                }
            }
            let mut current = baseline.clone();
            let current_metric = current.entries[0]
                .results
                .get_mut("perf")
                .unwrap()
                .metrics
                .get_mut("cycles")
                .unwrap();
            current_metric.change_percentage = Some(99.0);
            if difference == 3 {
                current_metric.direction = MetricDirection::Unknown;
            }
            current.apply_baseline(&old, 5.0).unwrap();
            assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);
            assert_eq!(current.entries[0].results["perf"].metrics["cycles"].change_percentage, None);
        }
    }

    #[test]
    fn gamma_survivor_baseline_skips_do_not_end_iteration() {
        let mut old = gamma_report("z", "z", Metric::integer("z", "z", 100, MetricDirection::HigherIsBetter));
        let mut current = old.clone();
        current.entries[0].results.get_mut("z").unwrap().metrics.get_mut("z").unwrap().value = MetricValue::Integer(90);
        current.entries.insert(
            0,
            BenchmarkEntry {
                identity: "absent".into(),
                results: BTreeMap::new(),
                prior: None,
                status: BenchmarkStatus::Uncompared,
            },
        );
        current.entries[1].results.insert(
            "absent".into(),
            EngineResult {
                metrics: BTreeMap::new(),
                raw_artifacts: vec![],
            },
        );
        current.entries[1]
            .results
            .get_mut("z")
            .unwrap()
            .metrics
            .insert("absent".into(), Metric::integer("z", "absent", 0, MetricDirection::Unknown));
        old.entries[0].results.get_mut("z").unwrap().metrics.insert(
            "incompatible".into(),
            Metric::integer("z", "incompatible", 0, MetricDirection::Unknown),
        );
        current.entries[1].results.get_mut("z").unwrap().metrics.insert(
            "incompatible".into(),
            Metric::integer("z", "incompatible", 0, MetricDirection::Unknown),
        );
        current.apply_baseline(&old, 5.0).unwrap();
        assert_eq!(current.entries[1].status, BenchmarkStatus::Regressed);
        assert_eq!(current.entries[1].results["z"].metrics["z"].change_percentage, Some(-10.0));
    }

    #[test]
    fn gamma_survivor_report_validation_is_engine_and_metric_specific() {
        for engine in ["criterion", "custom"] {
            for name in ["time", "median", "count"] {
                for (value, lower, upper, confidence, statistic) in [
                    (-1.0, Some(0.0), Some(1.0), Some(0.95), Some("mean")),
                    (f64::NAN, Some(0.0), Some(1.0), Some(0.95), Some("mean")),
                    (1.0, Some(-2.0), Some(-1.0), Some(0.95), Some("mean")),
                    (1.0, None, None, None, Some("mean")),
                    (1.0, None, None, Some(0.95), Some("mean")),
                    (1.0, Some(0.0), Some(2.0), Some(0.95), None),
                    (1.0, Some(0.0), Some(2.0), Some(0.95), Some(" ")),
                ] {
                    let mut metric = Metric::float(engine, name, value, MetricDirection::Unknown);
                    metric.lower_bound = lower;
                    metric.upper_bound = upper;
                    metric.confidence_level = confidence;
                    metric.statistic = statistic.map(str::to_owned);
                    let timing = engine == "criterion" && matches!(name, "time" | "median");
                    let expected_error = (timing && (!value.is_finite() || value < 0.0 || lower.is_some_and(|v| v < 0.0)))
                        || (engine == "criterion" && name == "time" && (lower.is_none() || statistic.is_none_or(|v| v.trim().is_empty())))
                        || (lower.is_none() && confidence.is_some());
                    let result = validate_report(&gamma_report(engine, name, metric));
                    assert_eq!(result.is_err(), expected_error, "{engine}/{name}: {value}");
                    if let Err(error) = result {
                        assert!(matches!(error, Error::ReportFormat { path, message }
                            if path == Path::new("<report>") && message == format!("invalid estimate or confidence metadata for case ({engine}/{name})")));
                    }
                }
            }
        }
    }

    #[test]
    fn gamma_survivor_report_grids_and_metric_precision() {
        assert_eq!(escape_table("\u{2028}\u{2029}\n|"), "&#8232;&#8233;&#10;&#124;");
        let headers = vec!["A".into(), "N".into(), "S".into()];
        let rows = vec![vec!["é".into(), "12".into(), "ok".into()]];
        assert_eq!(markdown_column_widths(&headers, &rows), [3, 3, 3]);
        let mut output = Vec::new();
        write_markdown_grid(&mut output, &headers, &rows).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "| A   |   N | S   |\n| --- | --: | --- |\n| é   |  12 | ok  |\n"
        );
        assert_eq!(render_console_grid(&headers, &rows), "A  N   S \n-  --  --\né  12  ok\n");
        for count in 0..5 {
            for index in 0..5 {
                assert_eq!(
                    is_markdown_metric_column(index, count),
                    index != 0 && index < count.saturating_sub(1)
                );
            }
        }
        let mut metric = Metric::float("custom", "count", 1.23456, MetricDirection::Unknown);
        metric.unit = Some("ns".into());
        metric.lower_bound = Some(0.12345);
        metric.upper_bound = Some(2.34567);
        metric.confidence_level = Some(0.95);
        metric.statistic = Some("mean".into());
        assert_eq!(
            render_metric_markdown("count", &metric),
            "1.2346 ns (0.1235..2.3457) 95.0% CI [mean]"
        );
        assert_eq!(render_metric_markdown("median", &metric), "1.23 ns (0.12..2.35) 95.0% CI [mean]");
    }

    #[test]
    fn gamma_survivor_report_width_boundaries() {
        let metrics = vec![
            TableMetric::new("perf".into(), "cycles".into()),
            TableMetric::new("perf".into(), "instructions".into()),
        ];
        let rows = vec![vec!["case".into(), "1".into(), "2".into(), "STABLE".into()]];
        assert_eq!(table_headers(&metrics), ["Benchmark", "HW Cycles", "HW Instr", "Status"]);
        assert_eq!(project_rows(&rows, 1), [vec!["case", "1", "STABLE"]]);
        for projected in [console_projected_width, markdown_projected_width] {
            let width = projected(&metrics, &rows, 2);
            assert_eq!(select_metric_count(&metrics, &rows, width, projected), 2);
            assert_eq!(select_metric_count(&metrics, &rows, width - 1, projected), 1);
            assert_eq!(select_metric_count(&[], &[], 0, projected), 0);
        }
        assert_eq!(console_projected_width(&metrics, &rows, 2), 38);
        assert_eq!(markdown_projected_width(&metrics, &rows, 2), 45);
        let mut report = gamma_report("perf", "cycles", Metric::integer("perf", "cycles", 1, MetricDirection::Unknown));
        report.entries[0].results.get_mut("perf").unwrap().metrics.insert(
            "instructions".into(),
            Metric::integer("perf", "instructions", 2, MetricDirection::Unknown),
        );
        let metrics = report.table_metrics();
        let rows = report.table_rows(&metrics, render_metric_console);
        let width = console_projected_width(&metrics, &rows, 2);
        assert!(report.render_console_table_with_width(width).contains("HW Cycles"));
        assert!(!report.render_console_table_with_width(width - 1).contains("HW Cycles"));
        for (identity_width, two_columns) in [(140, true), (141, false)] {
            report.entries[0].identity = "x".repeat(identity_width);
            let mut output = Vec::new();
            report.write_markdown(&mut output).unwrap();
            assert_eq!(String::from_utf8(output).unwrap().contains("HW Cycles"), two_columns);
        }
    }

    #[test]
    fn gamma_survivor_report_prior_labels_and_markdown_environment() {
        let mut report = gamma_report(
            "perf",
            "cycles",
            Metric::integer("perf", "cycles", 1, MetricDirection::LowerIsBetter),
        );
        let mut prior = report.entries[0].results.clone();
        prior.get_mut("perf").unwrap().metrics.get_mut("cycles").unwrap().display_name = "stale".into();
        report.entries[0].prior = Some(prior);
        report.refresh_display_names();
        assert_eq!(
            report.entries[0].prior.as_ref().unwrap()["perf"].metrics["cycles"].display_name,
            "HW Cycles"
        );
        let metrics = vec![
            TableMetric::new("perf".into(), "cycles".into()),
            TableMetric::new("missing".into(), "count".into()),
        ];
        assert_eq!(
            report.table_rows(&metrics, render_metric_console),
            [vec!["case", "1", "-", "UNCOMPARED"], vec!["case (PRIOR)", "1", "-", "-"]]
        );
        let mut output = Vec::new();
        report.write_markdown(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("# Benchmark Report\n\n## Environment\n\n| Field"));
        assert!(output.contains("| Value"));
        assert!(output.contains("Raw artifacts"));
        assert!(output.contains("artifacts"));
        assert!(output.contains("Compiler flags"));
        assert!(output.contains("target&#45;cpu=native"));
        assert!(output.contains("\n## Results\n\n"));
        assert!(output.ends_with("\n\n"));
    }

    #[test]
    fn gamma_survivor_report_writer_errors_are_returned() {
        struct FailsAfter(usize);
        impl io::Write for FailsAfter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if self.0 == 0 {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "scripted"));
                }
                let count = buf.len().min(self.0);
                self.0 -= count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let report = gamma_report("perf", "cycles", Metric::integer("perf", "cycles", 1, MetricDirection::Unknown));
        let mut output = Vec::new();
        report.write_markdown(&mut output).unwrap();
        for budget in 0..output.len() {
            let error = report.write_markdown(&mut FailsAfter(budget)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        }
    }

    #[test]
    fn gamma_survivor_report_paths_and_cpu_fields() {
        assert_eq!(report_parent(Path::new("report.json")), Path::new("."));
        assert_eq!(report_parent(&Path::new("dir").join("report.json")), Path::new("dir"));
        assert_eq!(
            incomplete_marker(Path::new("report.json")),
            Path::new("report.json.metabench-incomplete")
        );
        for field in ["model name", "Model", "Hardware", "Processor"] {
            assert_eq!(linux_cpu_identity(&format!("ignored\n{field}: Cpu X")), Some("Cpu X".into()));
        }
        assert_eq!(
            linux_cpu_identity("Processor: fallback\nmodel name: preferred"),
            Some("preferred".into())
        );
    }

    #[test]
    fn report_admission_errors_retain_report_context_and_existing_causes() {
        let path = PathBuf::from("baseline.json");
        assert!(matches!(
            BenchmarkReport::report_read_error(Error::ArtifactIo {
                path: path.clone(),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "read denied"),
            }),
            Error::ReportIo { path: failed, source }
                if failed == path && source.kind() == io::ErrorKind::PermissionDenied
        ));
        assert!(matches!(
            BenchmarkReport::report_read_error(Error::ArtifactFormat { path: path.clone(), message: "not regular".into() }),
            Error::ReportFormat { path: failed, message } if failed == path && message == "not regular"
        ));
        assert!(matches!(
            BenchmarkReport::report_read_error(Error::Metadata("original error".into())),
            Error::Metadata(message) if message == "original error"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn baseline_admission_rejects_links_and_fifos_without_waiting_for_a_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        let link = directory.path().join("link.json");
        fs::write(&path, b"{}").unwrap();
        symlink(&path, &link).unwrap();
        assert!(matches!(BenchmarkReport::read_json(&link), Err(Error::ReportIo { .. })));
        #[cfg(target_os = "linux")]
        {
            let fifo = directory.path().join("fifo.json");
            nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR).unwrap();
            assert!(matches!(
                BenchmarkReport::read_json(&fifo),
                Err(Error::ReportFormat { message, .. }) if message.contains("regular file")
            ));
        }
    }

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
        let mut time = Metric::float("criterion", "time", value, direction);
        time.statistic = Some("mean".to_owned());
        time.lower_bound = Some(value);
        time.upper_bound = Some(value);
        time.confidence_level = Some(0.95);
        EngineResult {
            metrics: BTreeMap::from([("time".to_owned(), time)]),
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
            .zip(["time", "Ir"])
            .map(|(source, name)| {
                let mut metric = Metric::integer(source, name, 1, MetricDirection::LowerIsBetter);
                if source == "criterion" {
                    metric.statistic = Some("mean".to_owned());
                    metric.lower_bound = Some(1.0);
                    metric.upper_bound = Some(1.0);
                    metric.confidence_level = Some(0.95);
                }
                NativeResult {
                    identity: "group/bench".to_owned(),
                    native_identity: source.to_owned(),
                    source: source.to_owned(),
                    metrics: BTreeMap::from([(name.to_owned(), metric)]),
                    raw_artifacts: Vec::new(),
                }
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
            ("bench", MetricDirection::HigherIsBetter, None, BenchmarkStatus::Uncompared),
            ("bench", MetricDirection::LowerIsBetter, Some("ns"), BenchmarkStatus::Uncompared),
            ("bench", MetricDirection::LowerIsBetter, None, BenchmarkStatus::Stable),
        ] {
            let mut current = report(identity, 101.0, direction);
            current.entries[0]
                .results
                .get_mut("criterion")
                .unwrap()
                .metrics
                .get_mut("time")
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
        assert!(console.contains("b|ench\\n測試 (PRIOR)"));

        let mut markdown = Vec::new();
        current.write_markdown(&mut markdown).unwrap();
        let markdown = String::from_utf8(markdown).unwrap();
        assert!(markdown.contains("b&#124;ench&#10;測試"));
        assert!(markdown.contains("Benchmark"));
        assert!(markdown.contains("Time"));
        assert!(markdown.contains("Allocs"));
        assert!(markdown.contains("10&#46;00"));
        assert!(markdown.contains("b&#124;ench&#10;測試 &#40;PRIOR&#41;"));
        assert!(markdown.contains("8&#46;00"));
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
        assert!(time_cell.trim().starts_with("10&#46;00"));
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
        let mut time = Metric::float("criterion", "time", 12.3456, MetricDirection::LowerIsBetter);
        time.unit = Some("ns".to_owned());
        time.lower_bound = Some(11.2345);
        time.upper_bound = Some(13.4567);

        assert_eq!(render_metric_console("time", &time), "12.35 ns");
        assert_eq!(render_metric_markdown("time", &time), "12.35 ns (11.23..13.46)");
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
    fn different_timing_statistics_are_not_compared() {
        let mut baseline = report("bench", 10.0, MetricDirection::LowerIsBetter);
        let mut current = report("bench", 100.0, MetricDirection::LowerIsBetter);
        baseline.entries[0]
            .results
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("time")
            .unwrap()
            .statistic = Some("mean".to_owned());
        current.entries[0]
            .results
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("time")
            .unwrap()
            .statistic = Some("slope".to_owned());
        current.apply_baseline(&baseline, 5.0).unwrap();
        assert_eq!(current.entries[0].status, BenchmarkStatus::Uncompared);
        assert_eq!(current.entries[0].results["criterion"].metrics["time"].change_percentage, None);
    }

    #[test]
    fn environment_compatibility_ignores_run_location_but_not_measurement_conditions() {
        let base = report("bench", 10.0, MetricDirection::LowerIsBetter).metadata;
        let mut other = base.clone();
        other.timestamp_utc = "different time".to_owned();
        other.artifact_root = "different output directory".to_owned();
        other.measured_backends = vec!["different subset".to_owned()];
        assert!(base.comparable_to(&other));
        for field in 0..5 {
            let mut other = base.clone();
            match field {
                0 => other.target_triple.push_str("-different"),
                1 => other.rustc_version.push_str("-different"),
                2 => other.host_cpu.push_str("-different"),
                3 => other.profile.push_str("-different"),
                _ => other.compiler_flags = Some("-different".to_owned()),
            }
            assert!(!base.comparable_to(&other));
        }
    }

    #[test]
    fn compiler_metadata_uses_the_build_identity() {
        if env::var_os("METABENCH_TEST_AMBIENT_COMPILER").is_some() {
            let current = EnvironmentMetadata::collect(vec![], "artifacts".to_owned());
            assert_eq!(current.rustc_version, env!("METABENCH_RUSTC_VERSION"));
            assert!(current.rustc_version.starts_with("rustc "));
            let mut other = current.clone();
            other.rustc_version = "rustc other build".to_owned();
            assert!(!current.comparable_to(&other));
            return;
        }
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        for version in ["one", "two"] {
            let ambient = directory.path().join(format!("ambient-rustc-{version}"));
            #[cfg(unix)]
            {
                fs::write(&ambient, format!("#!/bin/sh\nprintf 'rustc ambient {version}\\n'\n")).unwrap();
                fs::set_permissions(&ambient, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let status = Command::new(env::current_exe().unwrap())
                .args(["--exact", "report::tests::compiler_metadata_uses_the_build_identity"])
                .env("METABENCH_TEST_AMBIENT_COMPILER", "1")
                .env("RUSTC", &ambient)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    fn primary_estimate_report() -> BenchmarkReport {
        serde_json::from_str(
            r#"{
                "schema_version": 9,
                "metadata": {
                    "timestamp_utc": "2026-01-02T03:04:05Z", "target_triple": "test-target",
                    "rustc_version": "rustc test", "host_cpu": "test cpu", "profile": "release",
                    "compiler_flags": null, "measured_backends": ["criterion"], "artifact_root": "artifacts"
                },
                "entries": [{
                    "identity": "bench", "status": "uncompared",
                    "results": {"criterion": {"metrics": {"time": {
                        "value": 12.35, "unit": "ns", "direction": "lower_is_better",
                        "lower_bound": 11.23, "upper_bound": 13.46,
                        "confidence_level": 0.95, "statistic": "slope"
                    }}}},
                    "prior": {"criterion": {"metrics": {"time": {
                        "value": 20.0, "unit": "cycles", "direction": "lower_is_better",
                        "lower_bound": 10.0, "upper_bound": 30.0,
                        "confidence_level": 0.99, "statistic": "mean"
                    }}}}
                }]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn primary_estimate_wire_fields_and_markdown_cells_are_preserved() {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let path = directory.path().join("report.json");
        let source = primary_estimate_report();
        fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
        let actual = BenchmarkReport::read_json(&path).unwrap();
        let wire = serde_json::to_value(&actual).unwrap();
        for (row, statistic, value, unit, lower, upper, confidence) in [
            ("results", "slope", 12.35, "ns", 11.23, 13.46, 0.95),
            ("prior", "mean", 20.0, "cycles", 10.0, 30.0, 0.99),
        ] {
            let metric = &wire["entries"][0][row]["criterion"]["metrics"]["time"];
            assert_eq!(metric["statistic"], statistic);
            assert_eq!(metric["confidence_level"], confidence);
            assert_eq!(metric["value"], value);
            assert_eq!(metric["unit"], unit);
            assert_eq!(metric["lower_bound"], lower);
            assert_eq!(metric["upper_bound"], upper);
        }
        let mut output = Vec::new();
        actual.write_markdown(&mut output).unwrap();
        let markdown = String::from_utf8(output).unwrap();
        let rows = markdown.split_once("## Results\n\n").unwrap().1.lines().collect::<Vec<_>>();
        assert_eq!(
            rows[2].split('|').nth(2).unwrap().trim(),
            "12&#46;35 ns &#40;11&#46;23&#46;&#46;13&#46;46&#41; 95&#46;0% CI &#91;slope&#93;"
        );
        assert_eq!(
            rows[3].split('|').nth(2).unwrap().trim(),
            "20&#46;00 cycles &#40;10&#46;00&#46;&#46;30&#46;00&#41; 99&#46;0% CI &#91;mean&#93;"
        );
    }

    #[test]
    fn schema_nine_rejects_incoherent_confidence_in_current_and_prior_metrics() {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let path = directory.path().join("report.json");
        for row in ["results", "prior"] {
            for invalid in [
                serde_json::json!({"statistic": null}),
                serde_json::json!({"statistic": ""}),
                serde_json::json!({"statistic": " "}),
                serde_json::json!({"confidence_level": null}),
                serde_json::json!({"confidence_level": 0.0}),
                serde_json::json!({"confidence_level": 1.0}),
                serde_json::json!({"confidence_level": 2.0}),
                serde_json::json!({"confidence_level": -0.1}),
                serde_json::json!({"confidence_level": 0.95, "lower_bound": null, "upper_bound": null}),
                serde_json::json!({"lower_bound": null}),
                serde_json::json!({"upper_bound": null}),
                serde_json::json!({"lower_bound": 40.0}),
                serde_json::json!({"lower_bound": -1.0}),
                serde_json::json!({"value": -1.0}),
                serde_json::json!({"lower_bound": null, "upper_bound": null, "confidence_level": null}),
            ] {
                let mut wire = serde_json::to_value(primary_estimate_report()).unwrap();
                let metric = wire["entries"][0][row]["criterion"]["metrics"]["time"].as_object_mut().unwrap();
                metric.extend(invalid.as_object().unwrap().clone());
                fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
                assert!(matches!(BenchmarkReport::read_json(&path), Err(Error::ReportFormat { .. })));
                let mut wire = serde_json::to_value(primary_estimate_report()).unwrap();
                let metric = wire["entries"][0][row]["criterion"]["metrics"]["time"].as_object_mut().unwrap();
                for field in ["lower_bound", "upper_bound", "confidence_level"] {
                    metric.remove(field);
                }
                fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
                assert!(matches!(BenchmarkReport::read_json(&path), Err(Error::ReportFormat { .. })));
            }
            let mut wire = serde_json::to_value(primary_estimate_report()).unwrap();
            wire["entries"][0][row]["criterion"]["metrics"]["time"]
                .as_object_mut()
                .unwrap()
                .remove("statistic");
            fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
            assert!(matches!(BenchmarkReport::read_json(&path), Err(Error::ReportFormat { .. })));
        }
        let mut valid = primary_estimate_report();
        valid.entries[0]
            .results
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("time")
            .unwrap()
            .value = MetricValue::Float(100.0);
        valid.entries[0]
            .prior
            .as_mut()
            .unwrap()
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("time")
            .unwrap()
            .value = MetricValue::Float(1.0);
        fs::write(&path, serde_json::to_vec(&valid).unwrap()).unwrap();
        let actual = BenchmarkReport::read_json(&path).unwrap();
        assert_eq!(
            actual.entries[0].results["criterion"].metrics["time"].value,
            MetricValue::Float(100.0)
        );
        assert_eq!(
            actual.entries[0].prior.as_ref().unwrap()["criterion"].metrics["time"].value,
            MetricValue::Float(1.0)
        );
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for component in 0..4 {
                let mut baseline = valid.clone();
                let metric = baseline.entries[0]
                    .results
                    .get_mut("criterion")
                    .unwrap()
                    .metrics
                    .get_mut("time")
                    .unwrap();
                match component {
                    0 => metric.value = MetricValue::Float(invalid),
                    1 => metric.lower_bound = Some(invalid),
                    2 => metric.upper_bound = Some(invalid),
                    _ => metric.confidence_level = Some(invalid),
                }
                let mut current = report("bench", 5.0, MetricDirection::LowerIsBetter);
                assert!(matches!(current.apply_baseline(&baseline, 5.0), Err(Error::ReportFormat { .. })));
            }
        }
    }

    #[test]
    fn timing_estimates_accept_zero_and_equal_interval_bounds() {
        for (value, lower, upper) in [(0.0, 0.0, 0.0), (1.0, 0.0, 2.0), (1.0, 1.0, 1.0)] {
            let mut current = report("bench", value, MetricDirection::LowerIsBetter);
            let time = current.entries[0]
                .results
                .get_mut("criterion")
                .unwrap()
                .metrics
                .get_mut("time")
                .unwrap();
            time.lower_bound = Some(lower);
            time.upper_bound = Some(upper);
            super::validate_report(&current).unwrap();
            current.entries[0].prior = Some(current.entries[0].results.clone());
            super::validate_report(&current).unwrap();
        }
    }

    #[test]
    fn invalid_metric_diagnostics_escape_untrusted_identity_engine_and_name() {
        let mut report = report("identity\n\u{1b}", 1.0, MetricDirection::LowerIsBetter);
        let mut result = report.entries[0].results.remove("criterion").unwrap();
        let mut metric = result.metrics.remove("time").unwrap();
        metric.lower_bound = Some(2.0);
        metric.upper_bound = Some(1.0);
        result.metrics.insert("metric\r\u{202e}".into(), metric);
        report.entries[0].results.insert("engine\t\u{200f}".into(), result);
        let message = validate_report(&report).unwrap_err().to_string();
        assert!(message.contains(r"identity\n\u{1b}"));
        assert!(message.contains(r"engine\t\u{200f}"));
        assert!(message.contains(r"metric\r\u{202e}"));
        for character in ['\n', '\r', '\t', '\u{1b}', '\u{200f}', '\u{202e}'] {
            assert!(!message.contains(character));
        }
    }

    #[test]
    fn markdown_encodes_untrusted_text_without_changing_json() {
        let text = "<img src=x onerror='alert(1)'> [link](javascript:bad) `code` *bold* _em_ \\ | &lt;\r\n\u{1b}\u{85}\u{202e}";
        let mut report = primary_estimate_report();
        report.entries[0].identity = text.to_owned();
        report.metadata.artifact_root = text.to_owned();
        report.metadata.compiler_flags = Some(text.to_owned());
        for result in report.entries[0].results.values_mut() {
            for metric in result.metrics.values_mut() {
                metric.unit = Some(text.to_owned());
                metric.statistic = Some(text.to_owned());
            }
        }
        report.entries[0]
            .prior
            .as_mut()
            .unwrap()
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("time")
            .unwrap()
            .statistic = Some(text.to_owned());
        let mut output = Vec::new();
        report.write_markdown(&mut output).unwrap();
        let markdown = String::from_utf8(output).unwrap();
        assert!(markdown.contains("&#60;img src=x onerror=&#39;alert&#40;1&#41;&#39;&#62;"));
        assert!(markdown.contains("&#91;link&#93;&#40;javascript:bad&#41;"));
        assert!(
            markdown
                .contains("&#96;code&#96; &#42;bold&#42; &#95;em&#95; &#92; &#124; &#38;lt;&#13;&#10;&#27;&#133;&#92;u&#123;202e&#125;")
        );
        assert!(!markdown.contains("<img"));
        assert!(!markdown.contains('\r'));
        assert!(!markdown.contains('\u{1b}'));
        let prior = markdown.split_once("## Results\n\n").unwrap().1.lines().nth(3).unwrap();
        assert!(prior.contains("99&#46;0% CI &#91;&#60;img src=x onerror=&#39;alert&#40;1&#41;&#39;&#62;"));
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["entries"][0]["identity"], text);
        assert_eq!(wire["metadata"]["artifact_root"], text);
        assert_eq!(wire["entries"][0]["results"]["criterion"]["metrics"]["time"]["unit"], text);
        assert_eq!(wire["entries"][0]["results"]["criterion"]["metrics"]["time"]["statistic"], text);
        assert_eq!(wire["entries"][0]["prior"]["criterion"]["metrics"]["time"]["statistic"], text);
    }

    #[test]
    fn markdown_encodes_all_terminal_directional_marks() {
        for character in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}',
            '\u{2068}', '\u{2069}',
        ] {
            let encoded = escape_table(&format!("before{character}after"));
            assert_eq!(encoded, format!("before&#92;u&#123;{:x}&#125;after", u32::from(character)));
            assert!(!encoded.contains(character));
            assert!(!encoded.contains(&format!("&#{};", u32::from(character))));
        }
    }

    #[test]
    fn console_encodes_baseline_cells_without_changing_raw_strings() {
        let text = "bench\u{1b}[31m\r\nforged\t\u{85}\u{2028}\u{202e}";
        let mut baseline = primary_estimate_report();
        baseline.entries[0].identity = text.to_owned();
        for result in baseline.entries[0].results.values_mut() {
            for metric in result.metrics.values_mut() {
                metric.unit = Some(text.to_owned());
                metric.statistic = Some(text.to_owned());
            }
        }
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let path = directory.path().join("baseline.json");
        fs::write(&path, serde_json::to_vec(&baseline).unwrap()).unwrap();
        let baseline = BenchmarkReport::read_json(&path).unwrap();
        let mut current = baseline.clone();
        current.apply_baseline(&baseline, 5.0).unwrap();
        let console = current.render_console_table_with_width(usize::MAX);
        assert!(console.contains(r"bench\u{1b}[31m\r\nforged\t\u{85}\u{2028}\u{202e} (PRIOR)"));
        assert!(console.contains(r"12.35 bench\u{1b}[31m\r\nforged\t\u{85}\u{2028}\u{202e}"));
        assert_eq!(console.lines().count(), 4);
        assert!(!console.chars().any(|character| character.is_control() && character != '\n'));
        assert!(!console.contains('\u{2028}'));
        assert!(!console.contains('\u{202e}'));
        assert!(
            console
                .lines()
                .all(|line| UnicodeWidthStr::width(line) == UnicodeWidthStr::width(console.lines().next().unwrap()))
        );
        let wire = serde_json::to_value(&current).unwrap();
        assert_eq!(wire["entries"][0]["identity"], text);
        for row in ["results", "prior"] {
            assert_eq!(wire["entries"][0][row]["criterion"]["metrics"]["time"]["unit"], text);
            assert_eq!(wire["entries"][0][row]["criterion"]["metrics"]["time"]["statistic"], text);
        }

        let console = render_console_grid(&["counter\u{1b}\r\n\u{202e}".to_owned()], &[vec!["safe".to_owned()]]);
        assert!(console.lines().next().unwrap().contains(r"counter\u{1b}\r\n\u{202e}"));
        assert_eq!(console.lines().count(), 3);
        assert!(!console.contains('\u{1b}'));
        assert!(!console.contains('\r'));
        assert!(!console.contains('\u{202e}'));
    }

    #[test]
    fn report_schema_round_trip_and_parser_boundaries_are_deterministic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let expected = report("bench", 1.25, MetricDirection::LowerIsBetter);
        fs::write(&path, serde_json::to_vec_pretty(&expected).unwrap()).unwrap();
        let actual = BenchmarkReport::read_json(&path).unwrap();
        assert_eq!(actual.schema_version, 9);
        assert_eq!(actual.entries[0].identity, "bench");
        assert_eq!(
            actual.entries[0].results["criterion"].metrics["time"].value,
            MetricValue::Float(1.25)
        );
        assert_eq!(actual.entries[0].results["criterion"].metrics["time"].display_name, "Time");

        let mut stale_name = expected.clone();
        stale_name.entries[0]
            .results
            .get_mut("criterion")
            .unwrap()
            .metrics
            .get_mut("time")
            .unwrap()
            .display_name = "Old Time Label".to_owned();
        fs::write(&path, serde_json::to_vec(&stale_name).unwrap()).unwrap();
        let refreshed = BenchmarkReport::read_json(&path).unwrap();
        assert_eq!(refreshed.entries[0].results["criterion"].metrics["time"].display_name, "Time");

        let valid = String::from_utf8(serde_json::to_vec(&expected).unwrap()).unwrap();
        for invalid in [
            valid.replacen(r#""schema_version":9"#, r#""schema_version":8"#, 1),
            valid.replacen(r#""schema_version":9"#, r#""schema_version":9,"schema_version":9"#, 1),
            valid.replacen("1.25", "1e9999", 1),
            format!("{valid} trailing"),
        ] {
            fs::write(&path, invalid).unwrap();
            BenchmarkReport::read_json(&path).unwrap_err();
        }
    }
}
