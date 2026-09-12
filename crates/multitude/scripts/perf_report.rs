#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
ohno = { path = "../../ohno", features = ["app-err"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
---

//! Run the curated criterion benchmark scenarios and rebuild `docs/PERF.md`.
//!
//! The report is wall-clock only: it runs the criterion scenarios that back the
//! customer-facing tables and emits differential tables for them. The crate's
//! remaining micro-benchmarks and its Callgrind instruction-count suites are
//! not part of this report; run them directly with
//! `cargo bench --bench multitude --features serde_json`.
//!
//! Usage:
//!   `scripts/perf_report.rs`                                       — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                                — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`     — custom criterion settings
//!   `scripts/perf_report.rs --comparison-repetitions 5`            — repeat paired comparisons
//!   `scripts/perf_report.rs --cpu 4`                               — pin benchmark processes to CPU 4
//!
//! The group tables below select which benchmark variants are measured and
//! published. If a published scenario is added or removed, update them.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

use clap::Parser;
use ohno::{AppError, app_err, bail};
use serde::Deserialize;

/// Run the curated criterion benchmark scenarios and rebuild `docs/PERF.md`.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Use a faster, lower-fidelity run (10 samples, 1s measurement).
    /// Explicit `--samples` / `--measurement-time` / `--warm-up-time` flags
    /// still override the individual values when combined with `--fast`.
    #[arg(long)]
    fast: bool,

    /// Number of samples for criterion (default: 30, or 10 with `--fast`).
    #[arg(long)]
    samples: Option<u32>,

    /// Criterion measurement time, in seconds (default: 2, or 1 with `--fast`).
    #[arg(long)]
    measurement_time: Option<u32>,

    /// Criterion warm-up time, in seconds (default: 1).
    #[arg(long)]
    warm_up_time: Option<u32>,

    /// Pin every benchmark process to this logical CPU (Linux only).
    #[arg(long)]
    cpu: Option<u32>,

    /// Number of independently warmed runs for each differential allocation,
    /// Serde, and teardown group (default: 3).
    #[arg(long, alias = "serde-repetitions", default_value_t = 3)]
    comparison_repetitions: u32,
}

/// `(criterion_group, published_variants_in_report_order)`.
type Group = (&'static str, &'static [&'static str]);

/// Whole-lifecycle comparison of the arena against the system allocator.
const ARENA_VS_ALLOCATOR_GROUPS: &[Group] = &[("criterion_arena_vs_allocator/arena_vs_allocator", &["arena", "system"])];

/// The `criterion_alloc` variants backing the head-to-head bumpalo table.
///
/// Only these variants are measured; the bench target carries many more
/// per-API variants that are used for internal optimization work.
const ALLOC_GROUPS: &[Group] = &[
    ("criterion_alloc/alloc_u64", &["alloc", "bumpalo_alloc"]),
    ("criterion_alloc/alloc_str", &["alloc_str", "bumpalo_alloc_str"]),
    (
        "criterion_alloc/alloc_slice",
        &[
            "alloc_slice_copy",
            "bumpalo_alloc_slice_copy",
            "alloc_slice_clone",
            "bumpalo_alloc_slice_clone",
            "alloc_slice_fill_with",
            "bumpalo_alloc_slice_fill_with",
            "alloc_slice_fill_iter",
            "bumpalo_alloc_slice_fill_iter",
        ],
    ),
    (
        "criterion_alloc/string_builder",
        &[
            "alloc_string",
            "bumpalo_string_new_in",
            "alloc_string_with_capacity",
            "bumpalo_string_with_capacity_in",
        ],
    ),
    (
        "criterion_alloc/vec_builder",
        &[
            "alloc_vec",
            "bumpalo_vec_new_in",
            "alloc_vec_with_capacity",
            "bumpalo_vec_with_capacity_in",
        ],
    ),
];

const TEARDOWN_GROUPS: &[Group] = &[
    (
        "multitude_teardown/free_1",
        &[
            "standard",
            "multitude",
            "bumpalo",
            "multitude_reset_allocate",
            "bumpalo_reset_allocate",
        ],
    ),
    (
        "multitude_teardown/free_32",
        &[
            "standard",
            "multitude",
            "bumpalo",
            "multitude_reset_allocate",
            "bumpalo_reset_allocate",
        ],
    ),
    (
        "multitude_teardown/free_1000",
        &[
            "standard",
            "multitude",
            "bumpalo",
            "multitude_reset_allocate",
            "bumpalo_reset_allocate",
        ],
    ),
];

const SERDE_GROUPS: &[Group] = &[
    ("multitude_serde/typed", &["arena_owned", "serde_json_owned"]),
    ("multitude_serde/dynamic", &["arena_value", "serde_json_value"]),
    ("multitude_serde/typed_lifecycle", &["serde_json", "multitude", "bumpalo"]),
    ("multitude_serde/batch_lifecycle", &["serde_json", "multitude", "bumpalo"]),
];

/// The curated record-batch scenarios.
///
/// The bench target additionally covers reuse, lazy raw-string, arena-vector
/// baseline, and resource-limit variants that stay internal.
const RECORD_BATCH_GROUPS: &[Group] = &[
    ("multitude_record_batch/decode", &["standard_vec", "arena_box_slice"]),
    (
        "multitude_record_batch/strings",
        &[
            "standard_vec_unescaped",
            "arena_vec_unescaped",
            "standard_vec_escaped",
            "arena_vec_escaped",
        ],
    ),
    (
        "multitude_record_batch/sparse_retention",
        &["standard_one_in_eight", "arena_one_in_eight"],
    ),
    ("multitude_record_batch/errors", &["malformed_standard", "malformed_arena"]),
    (
        "multitude_record_batch/refresh_workload",
        &[
            "standard_global_select",
            "arena_vec_reset_global_select",
            "arena_each_reset_global_select",
            "arena_raw_each_reset_global_select",
            "arena_raw_index_reset_global_select",
        ],
    ),
];

/// `(workload_label, criterion_group, multitude_variant, bumpalo_variant)`.
const BUMPALO_COMPARISONS: &[(&str, &str, &str, &str)] = &[
    ("Sized value (`alloc`)", "criterion_alloc/alloc_u64", "alloc", "bumpalo_alloc"),
    (
        "String copy (`alloc_str`)",
        "criterion_alloc/alloc_str",
        "alloc_str",
        "bumpalo_alloc_str",
    ),
    (
        "Slice copy (`alloc_slice_copy`)",
        "criterion_alloc/alloc_slice",
        "alloc_slice_copy",
        "bumpalo_alloc_slice_copy",
    ),
    (
        "Slice clone (`alloc_slice_clone`)",
        "criterion_alloc/alloc_slice",
        "alloc_slice_clone",
        "bumpalo_alloc_slice_clone",
    ),
    (
        "Slice from closure (`alloc_slice_fill_with`)",
        "criterion_alloc/alloc_slice",
        "alloc_slice_fill_with",
        "bumpalo_alloc_slice_fill_with",
    ),
    (
        "Slice from iterator (`alloc_slice_fill_iter`)",
        "criterion_alloc/alloc_slice",
        "alloc_slice_fill_iter",
        "bumpalo_alloc_slice_fill_iter",
    ),
    (
        "Growable string (`alloc_string`)",
        "criterion_alloc/string_builder",
        "alloc_string",
        "bumpalo_string_new_in",
    ),
    (
        "Growable string, preallocated (`alloc_string_with_capacity`)",
        "criterion_alloc/string_builder",
        "alloc_string_with_capacity",
        "bumpalo_string_with_capacity_in",
    ),
    (
        "Growable vector (`alloc_vec`)",
        "criterion_alloc/vec_builder",
        "alloc_vec",
        "bumpalo_vec_new_in",
    ),
    (
        "Growable vector, preallocated (`alloc_vec_with_capacity`)",
        "criterion_alloc/vec_builder",
        "alloc_vec_with_capacity",
        "bumpalo_vec_with_capacity_in",
    ),
];

/// `(workload_label, criterion_group, arena_variant, standard_variant)`.
const SERDE_COMPARISONS: &[(&str, &str, &str, &str)] = &[
    ("Typed record", "multitude_serde/typed", "arena_owned", "serde_json_owned"),
    ("Dynamic value", "multitude_serde/dynamic", "arena_value", "serde_json_value"),
];

/// `(implementation_label, criterion_variant)`; the first row is the baseline.
const SERDE_LIFECYCLE_COMPARISONS: &[(&str, &str)] = &[
    ("Standard Serde", "serde_json"),
    ("Multitude", "multitude"),
    ("Bumpalo (manual seed)", "bumpalo"),
];

/// `(workload_label, criterion_group, standard_variant, arena_variant)`.
const RECORD_BATCH_COMPARISONS: &[(&str, &str, &str, &str)] = &[
    (
        "Decode a batch of wide records",
        "multitude_record_batch/decode",
        "standard_vec",
        "arena_box_slice",
    ),
    (
        "String fields, no escapes",
        "multitude_record_batch/strings",
        "standard_vec_unescaped",
        "arena_vec_unescaped",
    ),
    (
        "String fields, escaped",
        "multitude_record_batch/strings",
        "standard_vec_escaped",
        "arena_vec_escaped",
    ),
    (
        "Retain one record in eight",
        "multitude_record_batch/sparse_retention",
        "standard_one_in_eight",
        "arena_one_in_eight",
    ),
    (
        "Malformed input (error path)",
        "multitude_record_batch/errors",
        "malformed_standard",
        "malformed_arena",
    ),
];

/// `(implementation_label, criterion_variant)`; the first row is the baseline.
const REFRESH_COMPARISONS: &[(&str, &str)] = &[
    ("Standard collections", "standard_global_select"),
    ("Arena, `Vec` output, reset per refresh", "arena_vec_reset_global_select"),
    ("Arena, per-record output, reset per refresh", "arena_each_reset_global_select"),
    ("Arena, raw-value scan, per-record output", "arena_raw_each_reset_global_select"),
    ("Arena, raw-value scan, indexed selection", "arena_raw_index_reset_global_select"),
];

#[derive(Deserialize)]
struct JsonReport {
    entries: Vec<JsonEntry>,
}

#[derive(Deserialize)]
struct JsonEntry {
    identity: String,
    results: BTreeMap<String, JsonEngineResult>,
}

#[derive(Deserialize)]
struct JsonEngineResult {
    metrics: BTreeMap<String, JsonMetric>,
}

#[derive(Deserialize)]
struct JsonMetric {
    value: JsonMetricValue,
    unit: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum JsonMetricValue {
    Integer(u64),
    Float(f64),
}

impl JsonMetricValue {
    fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

type ReportIndex = BTreeMap<String, JsonEntry>;

fn index_report(report: JsonReport) -> Result<ReportIndex, AppError> {
    let mut entries = BTreeMap::new();
    for entry in report.entries {
        match entries.entry(entry.identity.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            Entry::Occupied(_) => bail!("report contains duplicate benchmark '{}'", entry.identity),
        }
    }
    Ok(entries)
}

fn criterion_time_ns(report: &ReportIndex, identity: &str) -> Option<f64> {
    let metric = report.get(identity)?.results.get("criterion")?.metrics.get("median")?;
    let value = metric.value.as_f64();
    match metric.unit.as_deref()? {
        "ps" => Some(value * 1e-3),
        "ns" => Some(value),
        "µs" | "us" => Some(value * 1e3),
        "ms" => Some(value * 1e6),
        "s" => Some(value * 1e9),
        _ => None,
    }
}

fn parse_report(path: &Path, expected: &[(&str, &str)]) -> Result<Vec<(String, f64)>, AppError> {
    let json = fs::read_to_string(path).map_err(|e| app_err!("reading {}: {e}", path.display()))?;
    let report: JsonReport = serde_json::from_str(&json).map_err(|e| app_err!("parsing {}: {e}", path.display()))?;
    let report = index_report(report)?;

    let expected_keys: HashSet<String> = expected.iter().map(|(group, variant)| format!("{group}/{variant}")).collect();
    let mut out = Vec::with_capacity(expected_keys.len());
    for key in &expected_keys {
        let Some(value) = criterion_time_ns(&report, key) else {
            bail!("report {} is missing criterion median for {key}", path.display());
        };
        out.push((key.clone(), value));
    }

    for extra in report.keys().filter(|key| !expected_keys.contains(*key)) {
        eprintln!("warning: report {} has unexpected benchmark {extra}", path.display());
    }

    Ok(out)
}

/// Median of every measurement recorded for `key`.
fn lookup_time(crit: &[(String, f64)], key: &str) -> Option<f64> {
    let mut values: Vec<f64> = crit
        .iter()
        .filter_map(|(candidate, value)| (candidate == key).then_some(*value))
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

fn fmt_ns(ns: Option<f64>) -> String {
    match ns {
        None => "—".into(),
        Some(ns) if ns < 1000.0 => format!("{ns:.0} ns"),
        Some(ns) if ns < 1e6 => format!("{:.2} µs", ns / 1e3),
        Some(ns) => format!("{:.2} ms", ns / 1e6),
    }
}

/// Percentage change of `candidate` relative to `baseline`.
fn fmt_delta(candidate: Option<f64>, baseline: Option<f64>) -> String {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) if baseline != 0.0 => {
            format!("{:+.1}%", (candidate / baseline - 1.0) * 100.0)
        }
        _ => "—".into(),
    }
}

/// How many times faster `candidate` is than `baseline`.
fn fmt_speedup(candidate: Option<f64>, baseline: Option<f64>) -> String {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) if candidate != 0.0 => {
            format!("{:.2}×", baseline / candidate)
        }
        _ => "—".into(),
    }
}

fn build_report(
    crit: &[(String, f64)],
    comparison_repetitions: u32,
    criterion_samples: u32,
    criterion_warmup_secs: u32,
    criterion_measurement_secs: u32,
    cpu: Option<u32>,
) -> String {
    let mut out = String::new();
    out.push_str("# Multitude Performance Report\n\n");
    out.push_str(
        "Generated by [`scripts/perf_report.rs`](../scripts/perf_report.rs). \
         Re-run it to refresh these numbers.\n\n",
    );
    out.push_str(
        "All figures are wall-clock medians measured by Criterion. They are \
         machine-dependent:\nthe ratios between rows are the durable signal, not \
         the absolute values.\n\n",
    );
    out.push_str(
        "This report is a curated set of customer-facing scenarios. The crate also carries a\n\
         larger suite of internal micro-benchmarks, including Callgrind instruction-count\n\
         suites, which are used for optimization work and are not published here; run them\n\
         with `cargo bench --bench multitude --features serde_json` in this crate.\n\n",
    );

    out.push_str("## How these numbers were produced\n\n");
    let _ = writeln!(
        out,
        "Criterion medians use {criterion_samples} samples, a {criterion_warmup_secs} s \
         warm-up, and a {criterion_measurement_secs} s measurement."
    );
    if let Some(cpu) = cpu {
        let _ = writeln!(out, "Benchmark processes were pinned to logical CPU {cpu}.");
    }
    out.push_str(
        "Each comparison group runs in a freshly warmed process. Compared implementations \
         are invoked adjacently to limit host-load and frequency drift.\n",
    );
    if comparison_repetitions == 1 {
        out.push_str("Differential allocation, Serde, and teardown timings come from one independently warmed paired run.\n\n");
    } else {
        let _ = writeln!(
            out,
            "Differential allocation, Serde, and teardown timings are the median of \
             {comparison_repetitions} independently warmed paired runs. Compared \
             variants run in the same process, with group order alternated between rounds.\n"
        );
    }

    out.push_str("## Arena vs. the system allocator\n\n");
    out.push_str(
        "One pass allocates a mixed working set — 1,000 `u64` values, 1,000 \
         32-byte slices, and 1,000 short strings, plus the vectors holding them — \
         and then releases all of it. The `arena` row takes every allocation from \
         one warmed `Arena` and releases the whole generation with a single \
         `Arena::reset`; the system row does the same work with `Box` and `Vec` on \
         the global allocator ([mimalloc](https://github.com/microsoft/mimalloc)) \
         and pays one free per object. Arena warm-up and the source data are built \
         outside the measured region, so only allocation and release traffic is \
         timed.\n\n",
    );
    let arena_time = lookup_time(crit, "criterion_arena_vs_allocator/arena_vs_allocator/arena");
    let system_time = lookup_time(crit, "criterion_arena_vs_allocator/arena_vs_allocator/system");
    out.push_str("| Workload | Multitude arena | System allocator (mimalloc) | Δ vs system allocator | Speedup |\n");
    out.push_str("|---|---:|---:|---:|---:|\n");
    let _ = writeln!(
        out,
        "| Allocate and release 3,000 mixed objects | {} | {} | {} | {} |",
        fmt_ns(arena_time),
        fmt_ns(system_time),
        fmt_delta(arena_time, system_time),
        fmt_speedup(arena_time, system_time),
    );
    out.push('\n');

    out.push_str("## Multitude vs. Bumpalo, head-to-head\n\n");
    out.push_str(
        "Identical workloads run against `multitude` and \
         [`bumpalo`](https://crates.io/crates/bumpalo); the `multitude` API chosen \
         in each row is the closest semantic equivalent to bumpalo's plain \
         bump-allocation. Each row performs 1,000 allocations per measurement, \
         with a slice element count of 8. Δ is `multitude` relative to `bumpalo`; \
         negative values favor `multitude`.\n\n",
    );
    out.push_str("| Workload | Multitude | Bumpalo | Δ |\n");
    out.push_str("|---|---:|---:|---:|\n");
    for (label, group, multitude_variant, bumpalo_variant) in BUMPALO_COMPARISONS {
        let multitude_time = lookup_time(crit, &format!("{group}/{multitude_variant}"));
        let bumpalo_time = lookup_time(crit, &format!("{group}/{bumpalo_variant}"));
        let _ = writeln!(
            out,
            "| {label} | {} | {} | {} |",
            fmt_ns(multitude_time),
            fmt_ns(bumpalo_time),
            fmt_delta(multitude_time, bumpalo_time),
        );
    }
    out.push('\n');

    out.push_str("## Allocation teardown\n\n");
    out.push_str(
        "Setup is outside the measured region: each implementation starts with the \
         same number of independent 64-byte, non-dropping payloads and only \
         release is timed. The standard path frees individually boxed values; \
         `multitude` leaks its arena-local `Alloc<T>` handles before measurement \
         and releases the generation with `Arena::reset`; bumpalo likewise \
         measures only `Bump::reset`. Non-dropping payloads make bulk reset \
         semantically equivalent across the two arena implementations.\n\n",
    );
    out.push_str("| Allocations | Implementation | Time | Δ vs standard allocator |\n");
    out.push_str("|---:|---|---:|---:|\n");
    for (count, group) in [
        (1, "multitude_teardown/free_1"),
        (32, "multitude_teardown/free_32"),
        (1_000, "multitude_teardown/free_1000"),
    ] {
        let standard_time = lookup_time(crit, &format!("{group}/standard"));
        for (label, variant) in [
            ("Standard allocator", "standard"),
            ("Multitude", "multitude"),
            ("Bumpalo", "bumpalo"),
        ] {
            let time = lookup_time(crit, &format!("{group}/{variant}"));
            let _ = writeln!(out, "| {count} | {label} | {} | {} |", fmt_ns(time), fmt_delta(time, standard_time),);
        }
    }
    out.push('\n');

    out.push_str("### Reset plus the next allocation\n\n");
    out.push_str(
        "This extends the pure-reset diagnostic through the first 64-byte \
         allocation of the next generation. Both allocators start with the \
         same warmed state, and backing-allocation assertions enforce that the \
         measured boundary only rewinds and reuses existing storage.\n\n",
    );
    out.push_str("| Previous allocations | Multitude | Bumpalo | Δ |\n");
    out.push_str("|---:|---:|---:|---:|\n");
    for (count, group) in [
        (1, "multitude_teardown/free_1"),
        (32, "multitude_teardown/free_32"),
        (1_000, "multitude_teardown/free_1000"),
    ] {
        let multitude = lookup_time(crit, &format!("{group}/multitude_reset_allocate"));
        let bumpalo = lookup_time(crit, &format!("{group}/bumpalo_reset_allocate"));
        let _ = writeln!(
            out,
            "| {count} | {} | {} | {} |",
            fmt_ns(multitude),
            fmt_ns(bumpalo),
            fmt_delta(multitude, bumpalo),
        );
    }
    out.push('\n');

    out.push_str("## Serde deserialization\n\n");
    out.push_str(
        "The arena and standard paths deserialize the same JSON document into \
         equivalent typed or dynamic values. Both run against warmed allocator \
         state; arena backing storage is preallocated and faulted in during setup. \
         Allocator setup and result teardown are outside the measured region. Δ \
         reports the arena relative to standard `serde_json`; negative values \
         favor the arena.\n\n",
    );
    out.push_str("| Workload | Arena | Standard `serde_json` | Δ |\n");
    out.push_str("|---|---:|---:|---:|\n");
    for (label, group, arena_variant, standard_variant) in SERDE_COMPARISONS {
        let arena = lookup_time(crit, &format!("{group}/{arena_variant}"));
        let standard = lookup_time(crit, &format!("{group}/{standard_variant}"));
        let _ = writeln!(
            out,
            "| {label} | {} | {} | {} |",
            fmt_ns(arena),
            fmt_ns(standard),
            fmt_delta(arena, standard),
        );
    }
    out.push('\n');

    out.push_str("### Reused-allocator lifecycle\n\n");
    out.push_str(
        "This is the shape of a server that reuses one allocator per request: \
         deserialize, consume the result, then perform whatever cleanup the next \
         request needs. Standard Serde drops its owned output; `multitude` drops \
         its owning arena pointers and resets the arena; bumpalo drops its \
         arena-borrowed output and resets the bump allocator. Bumpalo has no \
         built-in deserialization support, so its row uses a hand-written \
         `DeserializeSeed` that copies all strings and sequence storage into the \
         bump arena. Allocator construction stays outside the measured region.\n\n",
    );
    out.push_str("#### One record\n\n");
    out.push_str("| Implementation | Time | Δ vs standard Serde |\n");
    out.push_str("|---|---:|---:|\n");
    let lifecycle_group = "multitude_serde/typed_lifecycle";
    let lifecycle_standard = lookup_time(crit, &format!("{lifecycle_group}/serde_json"));
    for (label, variant) in SERDE_LIFECYCLE_COMPARISONS {
        let time = lookup_time(crit, &format!("{lifecycle_group}/{variant}"));
        let _ = writeln!(out, "| {label} | {} | {} |", fmt_ns(time), fmt_delta(time, lifecycle_standard));
    }
    out.push('\n');

    out.push_str("#### 32-record batch\n\n");
    out.push_str(
        "The same complete lifecycle for 32 independent JSON documents in one \
         reusable allocator generation. All implementations use an outer standard \
         `Vec`, so its allocation and destruction are included equally.\n\n",
    );
    out.push_str("| Implementation | Time | Δ vs standard Serde |\n");
    out.push_str("|---|---:|---:|\n");
    let batch_group = "multitude_serde/batch_lifecycle";
    let batch_standard = lookup_time(crit, &format!("{batch_group}/serde_json"));
    for (label, variant) in SERDE_LIFECYCLE_COMPARISONS {
        let time = lookup_time(crit, &format!("{batch_group}/{variant}"));
        let _ = writeln!(out, "| {label} | {} | {} |", fmt_ns(time), fmt_delta(time, batch_standard));
    }
    out.push('\n');

    out.push_str("## Record-batch decoding\n\n");
    out.push_str(
        "A synthetic batch of 16 wide records, decoded either into standard \
         collections or into arena-backed storage. Comparable standard and arena \
         paths include output destruction and storage reclamation in every \
         measured iteration, so nothing is deferred out of the measurement. Δ \
         reports the arena relative to the standard path; negative values favor \
         the arena.\n\n",
    );
    out.push_str("| Workload | Standard | Arena | Δ |\n");
    out.push_str("|---|---:|---:|---:|\n");
    for (label, group, standard_variant, arena_variant) in RECORD_BATCH_COMPARISONS {
        let standard = lookup_time(crit, &format!("{group}/{standard_variant}"));
        let arena = lookup_time(crit, &format!("{group}/{arena_variant}"));
        let _ = writeln!(
            out,
            "| {label} | {} | {} | {} |",
            fmt_ns(standard),
            fmt_ns(arena),
            fmt_delta(arena, standard),
        );
    }
    out.push('\n');

    out.push_str("### Reset-per-refresh workload\n\n");
    out.push_str(
        "The most end-to-end scenario in this report: each iteration parses 1,000 \
         escaped-string records with rich filter headers, makes one global \
         top-candidate selection, materializes 32 owned records, and keeps the \
         previously retained generation alive until its replacement is ready. The \
         arena rows differ in how output is shaped and when the arena is reset; \
         the raw-value rows scan every element as `&RawValue` and re-parse only \
         the selected records.\n\n",
    );
    out.push_str("| Implementation | Time | Δ vs standard collections |\n");
    out.push_str("|---|---:|---:|\n");
    let refresh_group = "multitude_record_batch/refresh_workload";
    let refresh_standard = lookup_time(crit, &format!("{refresh_group}/standard_global_select"));
    for (label, variant) in REFRESH_COMPARISONS {
        let time = lookup_time(crit, &format!("{refresh_group}/{variant}"));
        let _ = writeln!(out, "| {label} | {} | {} |", fmt_ns(time), fmt_delta(time, refresh_standard));
    }

    out
}

/// Locate the `multitude` crate root (the directory containing this script's
/// parent). With `cargo +nightly -Zscript`, `CARGO_MANIFEST_DIR` is the
/// directory holding the script file (i.e. `crates/multitude/scripts`).
fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts/ always has a parent crate directory")
        .to_path_buf()
}

/// A criterion filter matching exactly the published variants of `group`.
fn group_filter(group: &str, variants: &[&str]) -> String {
    format!("^{group}/({})$", variants.join("|"))
}

fn bench_arguments(filter: &str, json_path: &Path, warmup_secs: u32, measurement_secs: u32, samples: u32) -> Vec<OsString> {
    let warmup = warmup_secs.to_string();
    let measurement = measurement_secs.to_string();
    let sample_size = samples.to_string();
    let mut args = vec![OsString::from("--")];
    args.push(OsString::from("--criterion"));
    args.push(OsString::from("--no-baseline"));
    args.push(OsString::from("--export-json"));
    args.push(json_path.as_os_str().to_owned());
    for forwarded in [
        filter,
        "--warm-up-time",
        warmup.as_str(),
        "--measurement-time",
        measurement.as_str(),
        "--sample-size",
        sample_size.as_str(),
    ] {
        args.push(OsString::from("--criterion-arg"));
        args.push(OsString::from(forwarded));
    }
    args
}

/// Run a benchmark and write its metabench JSON report.
fn run_bench(
    cwd: &Path,
    filter: &str,
    json_path: &Path,
    label: &str,
    cpu: Option<u32>,
    warmup_secs: u32,
    measurement_secs: u32,
    samples: u32,
) -> Result<(), AppError> {
    println!("==> Running {label}");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = if let Some(cpu) = cpu {
        let mut cmd = Command::new("taskset");
        cmd.arg("--cpu-list").arg(cpu.to_string()).arg(cargo);
        cmd
    } else {
        Command::new(cargo)
    };
    cmd.current_dir(cwd)
        .arg("bench")
        .arg("--bench")
        .arg("multitude")
        .arg("--features")
        .arg("serde_json")
        .args(bench_arguments(filter, json_path, warmup_secs, measurement_secs, samples));
    let status = cmd
        .status()
        .map_err(|e| app_err!("failed to spawn cargo bench --bench multitude: {e}"))?;
    if !status.success() {
        bail!("cargo bench --bench multitude failed with status {status}");
    }
    Ok(())
}

fn run_group(
    cwd: &Path,
    target_dir: &Path,
    group: &str,
    variants: &[&str],
    label: &str,
    cpu: Option<u32>,
    warmup_secs: u32,
    measurement_secs: u32,
    samples: u32,
) -> Result<Vec<(String, f64)>, AppError> {
    let filter = group_filter(group, variants);
    let json_path = target_dir.join("multitude-perf-report.json");
    run_bench(cwd, &filter, &json_path, label, cpu, warmup_secs, measurement_secs, samples)?;
    let expected: Vec<(&str, &str)> = variants.iter().copied().map(|variant| (group, variant)).collect();
    parse_report(&json_path, &expected)
}

fn run_groups(
    cwd: &Path,
    target_dir: &Path,
    groups: &[Group],
    cpu: Option<u32>,
    warmup_secs: u32,
    measurement_secs: u32,
    samples: u32,
) -> Result<Vec<(String, f64)>, AppError> {
    let mut combined = Vec::new();
    for (group, variants) in groups {
        combined.extend(run_group(
            cwd,
            target_dir,
            group,
            variants,
            &format!("multitude ({group})"),
            cpu,
            warmup_secs,
            measurement_secs,
            samples,
        )?);
    }
    Ok(combined)
}

fn run_repeated_variants(
    cwd: &Path,
    target_dir: &Path,
    groups: &[Group],
    repetitions: u32,
    cpu: Option<u32>,
    warmup_secs: u32,
    measurement_secs: u32,
    samples: u32,
) -> Result<Vec<(String, f64)>, AppError> {
    let mut combined = Vec::new();
    for round in 0..repetitions {
        let indices: Vec<usize> = if round.is_multiple_of(2) {
            (0..groups.len()).collect()
        } else {
            (0..groups.len()).rev().collect()
        };
        for index in indices {
            let (group, variants) = groups[index];
            combined.extend(run_group(
                cwd,
                target_dir,
                group,
                variants,
                &format!("multitude ({group}, round {}/{repetitions})", round + 1),
                cpu,
                warmup_secs,
                measurement_secs,
                samples,
            )?);
        }
    }
    Ok(combined)
}

fn run(args: &Args) -> Result<(), AppError> {
    let crate_dir = crate_root();

    if args.cpu.is_some() && !cfg!(target_os = "linux") {
        bail!("--cpu is only supported on Linux");
    }

    let (default_samples, default_measurement_secs) = if args.fast { (10, 1) } else { (30, 2) };
    let samples = args.samples.unwrap_or(default_samples);
    let measurement_secs = args.measurement_time.unwrap_or(default_measurement_secs);
    let warmup_secs = args.warm_up_time.unwrap_or(1);

    let target_dir = crate_dir.join("target");
    fs::create_dir_all(&target_dir).map_err(|e| app_err!("creating {}: {e}", target_dir.display()))?;

    let mut crit = run_groups(
        &crate_dir,
        &target_dir,
        ARENA_VS_ALLOCATOR_GROUPS,
        args.cpu,
        warmup_secs,
        measurement_secs,
        samples,
    )?;
    crit.extend(run_repeated_variants(
        &crate_dir,
        &target_dir,
        ALLOC_GROUPS,
        args.comparison_repetitions,
        args.cpu,
        warmup_secs,
        measurement_secs,
        samples,
    )?);
    crit.extend(run_repeated_variants(
        &crate_dir,
        &target_dir,
        TEARDOWN_GROUPS,
        args.comparison_repetitions,
        args.cpu,
        warmup_secs,
        measurement_secs,
        samples,
    )?);
    crit.extend(run_repeated_variants(
        &crate_dir,
        &target_dir,
        SERDE_GROUPS,
        args.comparison_repetitions,
        args.cpu,
        warmup_secs,
        measurement_secs,
        samples,
    )?);
    crit.extend(run_groups(
        &crate_dir,
        &target_dir,
        RECORD_BATCH_GROUPS,
        args.cpu,
        warmup_secs,
        measurement_secs,
        samples,
    )?);

    println!("==> Building docs/PERF.md");
    let report = build_report(&crit, args.comparison_repetitions, samples, warmup_secs, measurement_secs, args.cpu);
    let out_path = crate_dir.join("docs").join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;

    println!("Wrote {} ({} criterion measurements)", out_path.display(), crit.len());
    println!("==> Done. Report written to docs/PERF.md");
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
