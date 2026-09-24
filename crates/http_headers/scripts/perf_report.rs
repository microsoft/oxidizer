#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
---

//! Render the per-header metabench results in `docs/PERF.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

use serde::Deserialize;

const SCHEMA_VERSION: u32 = 8;
const DEFAULT_ARTIFACT: &str = "target/metabench/http_headers_per_header/report.json";
const DEFAULT_REPORT: &str = "crates/http_headers/docs/PERF.md";

const ROWS: &[(&str, &str, bool)] = &[
    ("accept", "Accept", false),
    ("accept_encoding", "Accept-Encoding", false),
    ("accept_language", "Accept-Language", false),
    ("accept_ranges", "Accept-Ranges", true),
    (
        "access_control_allow_credentials",
        "Access-Control-Allow-Credentials",
        true,
    ),
    (
        "access_control_allow_headers",
        "Access-Control-Allow-Headers",
        true,
    ),
    (
        "access_control_allow_methods",
        "Access-Control-Allow-Methods",
        true,
    ),
    (
        "access_control_allow_origin",
        "Access-Control-Allow-Origin",
        true,
    ),
    (
        "access_control_expose_headers",
        "Access-Control-Expose-Headers",
        true,
    ),
    ("access_control_max_age", "Access-Control-Max-Age", true),
    (
        "access_control_request_headers",
        "Access-Control-Request-Headers",
        true,
    ),
    (
        "access_control_request_method",
        "Access-Control-Request-Method",
        true,
    ),
    ("allow", "Allow", true),
    ("authorization_basic", "Authorization (Basic)", true),
    ("authorization_bearer", "Authorization (Bearer)", true),
    ("cache_control", "Cache-Control", true),
    ("content_length", "Content-Length", true),
    ("content_range", "Content-Range", true),
    ("content_security_policy", "Content-Security-Policy", false),
    ("content_type", "Content-Type", true),
    ("etag", "ETag", true),
    ("host", "Host", true),
    ("if_match", "If-Match", true),
    ("if_modified_since", "If-Modified-Since", true),
    ("if_none_match", "If-None-Match", true),
    ("if_range", "If-Range", true),
    ("if_unmodified_since", "If-Unmodified-Since", true),
    ("last_modified", "Last-Modified", true),
    ("location", "Location", true),
    ("range", "Range", true),
    ("referrer_policy", "Referrer-Policy", true),
    ("sec_websocket_accept", "Sec-WebSocket-Accept", true),
    (
        "sec_websocket_extensions",
        "Sec-WebSocket-Extensions",
        false,
    ),
    ("sec_websocket_key", "Sec-WebSocket-Key", true),
    ("sec_websocket_protocol", "Sec-WebSocket-Protocol", false),
    ("sec_websocket_version", "Sec-WebSocket-Version", true),
    ("server", "Server", true),
    ("set_cookie", "Set-Cookie", true),
    (
        "strict_transport_security",
        "Strict-Transport-Security",
        true,
    ),
    ("user_agent", "User-Agent", true),
    ("vary", "Vary", true),
    ("x_content_type_options", "X-Content-Type-Options", false),
];

/// Benchmarks the harness measures but the report omits, because they cover a
/// secondary shape of a header the report already lists.
const UNREPORTED: &[&str] = &["sec_websocket_version_advertisement"];

#[derive(Deserialize)]
struct Report {
    schema_version: u32,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    identity: String,
    results: BTreeMap<String, EngineResult>,
}

#[derive(Deserialize)]
struct EngineResult {
    metrics: BTreeMap<String, Metric>,
}

#[derive(Deserialize)]
struct Metric {
    value: MetricValue,
    unit: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum MetricValue {
    Integer(u64),
    Float(f64),
}

#[derive(Clone, Copy)]
struct Measurements {
    time_ns: f64,
    instructions: u64,
    allocations: u64,
    allocated_bytes: u64,
}

struct Arguments {
    artifact: PathBuf,
    report: PathBuf,
    no_run: bool,
    check: bool,
}

fn main() -> ExitCode {
    if env::args()
        .skip(1)
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        println!("{}", usage());
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("perf_report: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let root = workspace_root()?;
    let arguments = arguments(&root)?;
    if !arguments.no_run && !arguments.check {
        collect(&root, &arguments.artifact)?;
    }
    let report = render(&read_report(&arguments.artifact)?)?;
    if arguments.check {
        let existing = fs::read_to_string(&arguments.report)
            .map_err(|error| format!("cannot read {}: {error}", arguments.report.display()))?;
        if existing != report {
            return Err(format!(
                "{} differs from the metabench report",
                arguments.report.display()
            ));
        }
        return Ok(format!("{} is up to date", arguments.report.display()));
    }
    if let Some(parent) = arguments.report.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(&arguments.report, report)
        .map_err(|error| format!("cannot write {}: {error}", arguments.report.display()))?;
    Ok(format!(
        "wrote {} with {} header rows",
        arguments.report.display(),
        ROWS.len()
    ))
}

fn arguments(root: &Path) -> Result<Arguments, String> {
    let mut artifact = root.join(DEFAULT_ARTIFACT);
    let mut report = root.join(DEFAULT_REPORT);
    let mut no_run = false;
    let mut check = false;
    let mut values = env::args().skip(1);
    while let Some(value) = values.next() {
        match value.as_str() {
            "--artifact" => {
                artifact = absolute(root, values.next().ok_or("missing artifact path")?)
            }
            "--output" => report = absolute(root, values.next().ok_or("missing output path")?),
            "--no-run" => no_run = true,
            "--check" => {
                no_run = true;
                check = true;
            }
            other => return Err(format!("unknown argument `{other}`\n{}", usage())),
        }
    }
    Ok(Arguments {
        artifact,
        report,
        no_run,
        check,
    })
}

fn usage() -> &'static str {
    "usage: perf_report.rs [--no-run|--check] [--artifact PATH] [--output PATH]"
}

fn absolute(root: &Path, value: String) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    let script = env::args()
        .next()
        .ok_or("missing script path")?
        .parse::<PathBuf>()
        .map_err(|error| error.to_string())?
        .canonicalize()
        .map_err(|error| format!("cannot resolve script path: {error}"))?;
    script
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "cannot locate workspace root".to_string())
}

fn collect(root: &Path, artifact: &Path) -> Result<(), String> {
    if let Some(parent) = artifact.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let status = Command::new("cargo")
        .current_dir(root)
        .args([
            "bench",
            "-p",
            "http_headers",
            "--features",
            "benchmarking,http",
            "--bench",
            "http_headers_per_header",
            "--",
            "--criterion",
            "--gungraun",
            "--allocations",
            "--show-engine-output",
            "--no-baseline",
            "--export-json",
        ])
        .arg(artifact)
        .status()
        .map_err(|error| format!("cannot run metabench: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("metabench failed with {status}"))
    }
}

fn read_report(path: &Path) -> Result<Report, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let report: Report = serde_json::from_str(&text)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    if report.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "expected metabench schema {SCHEMA_VERSION}, got {}",
            report.schema_version
        ));
    }
    Ok(report)
}

fn render(report: &Report) -> Result<String, String> {
    let entries = report
        .entries
        .iter()
        .map(|entry| (entry.identity.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    if entries.len() != report.entries.len() {
        return Err("metabench report contains duplicate identities".to_string());
    }
    let expected = ROWS
        .iter()
        .flat_map(|(id, _, supported)| {
            let mut identities = vec![
                format!("http_headers_per_header/per_header/http_headers_owned/{id}"),
                format!("http_headers_per_header/per_header/http_headers_borrowed/{id}"),
            ];
            if *supported {
                identities.push(format!("http_headers_per_header/per_header/headers/{id}"));
            }
            identities
        })
        .collect::<BTreeSet<_>>();
    let unreported = UNREPORTED
        .iter()
        .flat_map(|id| {
            [
                format!("http_headers_per_header/per_header/http_headers_owned/{id}"),
                format!("http_headers_per_header/per_header/http_headers_borrowed/{id}"),
                format!("http_headers_per_header/per_header/headers/{id}"),
            ]
        })
        .collect::<BTreeSet<_>>();
    let actual = entries
        .keys()
        .map(|identity| (*identity).to_owned())
        .filter(|identity| !unreported.contains(identity))
        .collect();
    if actual != expected {
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let extra = actual.difference(&expected).cloned().collect::<Vec<_>>();
        return Err(format!(
            "metabench inventory mismatch; missing: {missing:?}; extra: {extra:?}"
        ));
    }

    let mut output = String::with_capacity(16 * 1024);
    writeln!(output, "# Performance\n")
        .map_err(|error| format!("cannot render report heading: {error}"))?;
    writeln!(
        output,
        "This table is the comparative snapshot imported with `http_headers` 0.1.0. It\n\
         is not a promise of current timing on different hardware or dependency\n\
         versions; use the repository's metabench targets to measure the current\n\
         checkout.\n"
    )
    .map_err(|error| format!("cannot render report disclaimer: {error}"))?;
    writeln!(
        output,
        "Each cell reports metabench's median wall-clock time, Callgrind instruction\n\
         count, allocation count, and total allocated bytes for one typed decode and\n\
         read. Benchmarks consume the decoded value and force comparable semantic work\n\
         when `headers 0.4.1` defers parsing. Setup uses a prebuilt `HeaderMap` outside\n\
         the measured operation. Criterion uses a 1 s warm-up, 3 s measurement period,\n\
         and 60 samples per arm. Each cell is formatted as time, instructions, then\n\
         allocation count / allocated bytes. `n/a` means `headers 0.4.1` does not\n\
         provide that typed header.\n"
    )
    .map_err(|error| format!("cannot render report description: {error}"))?;
    writeln!(
        output,
        "| Header | `headers 0.4.1` | `http_headers` (owned) | `http_headers` (borrowed) |"
    )
    .and_then(|()| writeln!(output, "|---|---:|---:|---:|"))
    .map_err(|error| format!("cannot render table heading: {error}"))?;

    for (id, header, supported) in ROWS {
        let theirs = if *supported {
            format_measurements(measurements(required_entry(&entries, id, "headers")?)?)
        } else {
            "n/a".to_string()
        };
        let owned = format_measurements(measurements(required_entry(
            &entries,
            id,
            "http_headers_owned",
        )?)?);
        let borrowed = format_measurements(measurements(required_entry(
            &entries,
            id,
            "http_headers_borrowed",
        )?)?);
        writeln!(output, "| {header} | {theirs} | {owned} | {borrowed} |")
            .map_err(|error| format!("cannot render `{id}`: {error}"))?;
    }
    Ok(output)
}

fn required_entry<'a>(
    entries: &'a BTreeMap<&str, &Entry>,
    group: &str,
    benchmark: &str,
) -> Result<&'a Entry, String> {
    let identity = format!("http_headers_per_header/per_header/{benchmark}/{group}");
    entries
        .get(identity.as_str())
        .copied()
        .ok_or_else(|| format!("missing benchmark `{identity}`"))
}

fn measurements(entry: &Entry) -> Result<Measurements, String> {
    let time = metric(entry, "criterion", "median")?;
    if time.unit.as_deref() != Some("ns") {
        return Err(format!(
            "`{}` criterion median has unit {}, expected ns",
            entry.identity,
            time.unit.as_deref().unwrap_or("<missing>")
        ));
    }
    Ok(Measurements {
        time_ns: finite_nonnegative(entry, "criterion/median", float_value(time.value))?,
        instructions: integer_metric(entry, "gungraun.callgrind", "Ir")?,
        allocations: integer_metric(entry, "alloc_tracker", "Allocations")?,
        allocated_bytes: integer_metric(entry, "alloc_tracker", "Allocated bytes")?,
    })
}

fn metric<'a>(entry: &'a Entry, engine: &str, name: &str) -> Result<&'a Metric, String> {
    entry
        .results
        .get(engine)
        .and_then(|result| result.metrics.get(name))
        .ok_or_else(|| format!("`{}` lacks `{engine}/{name}`", entry.identity))
}

fn integer_metric(entry: &Entry, engine: &str, name: &str) -> Result<u64, String> {
    match metric(entry, engine, name)?.value {
        MetricValue::Integer(value) => Ok(value),
        MetricValue::Float(_) => Err(format!(
            "`{}` has non-integer `{engine}/{name}` value",
            entry.identity
        )),
    }
}

fn float_value(value: MetricValue) -> f64 {
    match value {
        MetricValue::Integer(value) => value as f64,
        MetricValue::Float(value) => value,
    }
}

fn finite_nonnegative(entry: &Entry, name: &str, value: f64) -> Result<f64, String> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(format!("`{}` has invalid `{name}` value", entry.identity))
    }
}

fn format_measurements(value: Measurements) -> String {
    format!(
        "{}<br>{} instr<br>{} allocs / {}",
        time(value.time_ns),
        grouped(value.instructions),
        grouped(value.allocations),
        bytes(value.allocated_bytes)
    )
}

fn time(nanoseconds: f64) -> String {
    if nanoseconds >= 1_000.0 {
        format!("{:.2} µs", nanoseconds / 1_000.0)
    } else {
        format!("{nanoseconds:.1} ns")
    }
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(char::from(byte));
    }
    output
}

fn bytes(value: u64) -> String {
    format!("{} B", grouped(value))
}
