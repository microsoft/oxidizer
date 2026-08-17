#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
ohno = { path = "../../ohno", features = ["app-err"] }
---

//! Run the curated criterion benchmark scenarios and rebuild `docs/PERF.md`.
//!
//! The report is wall-clock only: it runs the criterion bench targets that back
//! the customer-facing scenarios and emits differential tables for them. The
//! crate's remaining micro-benchmarks and its Callgrind instruction-count
//! suites are not part of this report; run them directly with `cargo bench`.
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

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

use clap::Parser;
use ohno::{AppError, app_err, bail};

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
    (
        "multitude_serde/typed_lifecycle",
        &["serde_json", "multitude", "bumpalo"],
    ),
    (
        "multitude_serde/batch_lifecycle",
        &["serde_json", "multitude", "bumpalo"],
    ),
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
    (
        "multitude_record_batch/errors",
        &["malformed_standard", "malformed_arena"],
    ),
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
    (
        "Sized value (`alloc`)",
        "criterion_alloc/alloc_u64",
        "alloc",
        "bumpalo_alloc",
    ),
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
    (
        "Arena, raw-value scan, per-record output",
        "arena_raw_each_reset_global_select",
    ),
    (
        "Arena, raw-value scan, indexed selection",
        "arena_raw_index_reset_global_select",
    ),
];

fn unit_to_ns(unit: &str) -> Option<f64> {
    match unit {
        "ps" => Some(1e-3),
        "ns" => Some(1.0),
        "µs" | "us" => Some(1e3),
        "ms" => Some(1e6),
        "s" => Some(1e9),
        _ => None,
    }
}

/// Extract the median time from a criterion `time:` summary line.
///
/// Format: `time:   [<low> <unit> <median> <unit> <high> <unit>]`.
fn parse_time_line(line: &str) -> Option<f64> {
    let idx = line.find("time:")?;
    let rest = &line[idx + "time:".len()..];
    let open = rest.find('[')?;
    let close = rest.find(']')?;
    let inside = &rest[open + 1..close];
    let toks: Vec<&str> = inside.split_whitespace().collect();
    if toks.len() != 6 {
        return None;
    }
    let median: f64 = toks[2].parse().ok()?;
    let scale = unit_to_ns(toks[3])?;
    Some(median * scale)
}

/// True for a non-empty, non-indented `group/variant` identifier (the
/// shape criterion emits on its own line or inline before `time:`).
/// "Benchmarking foo/bar: ..." progress lines are filtered out by the
/// no-colon and no-internal-whitespace checks.
fn is_bench_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.contains(':') || s.contains(char::is_whitespace) {
        return false;
    }
    let id_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut segments = s.split('/');
    let Some(first) = segments.next() else {
        return false;
    };
    if first.is_empty() || !first.chars().all(id_char) {
        return false;
    }
    let mut descendants = 0;
    for segment in segments {
        if segment.is_empty() || !segment.chars().all(id_char) {
            return false;
        }
        descendants += 1;
    }
    descendants > 0
}

/// Parse a criterion log and return `{group/variant: median_ns}`.
///
/// Criterion writes the bench identifier either on its own line just
/// before the `time:` line (long names) or on the same line as `time:`
/// separated by whitespace (short names). Both shapes are handled.
///
/// Fails if any expected row is absent, so a report is never emitted with
/// blank cells where a measurement was meant to be.
fn parse_criterion(text: &str, expected: &[(&str, &str)]) -> Result<Vec<(String, f64)>, AppError> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        // Same-line form: `group/variant  time:   [...]`.
        if let Some(t_idx) = line.find("time:") {
            let head = line[..t_idx].trim();
            let name_inline = if is_bench_name(head) { Some(head.to_string()) } else { None };
            let name = name_inline.or_else(|| pending.take());
            if let (Some(name), Some(t)) = (name, parse_time_line(line)) {
                out.push((name, t));
            }
            continue;
        }
        // Bare-name line: stash for the next `time:` we see.
        if is_bench_name(trimmed) {
            pending = Some(trimmed.to_string());
        }
    }

    let expected_keys: HashSet<String> = expected.iter().map(|(g, v)| format!("{g}/{v}")).collect();
    let got_keys: HashSet<String> = out.iter().map(|(k, _)| k.clone()).collect();
    let mut missing: Vec<&String> = expected_keys.difference(&got_keys).collect();
    if !missing.is_empty() {
        missing.sort();
        let names: Vec<&str> = missing.iter().map(|name| name.as_str()).collect();
        bail!("criterion log is missing expected benches: {}", names.join(", "));
    }
    for extra in got_keys.difference(&expected_keys) {
        eprintln!("warning: criterion log has unexpected bench {extra}");
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
         suites (`benches/*_cg.rs`), which are used for optimization work and are not\n\
         published here; run them with `cargo bench` in this crate.\n\n",
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
            let _ = writeln!(
                out,
                "| {count} | {label} | {} | {} |",
                fmt_ns(time),
                fmt_delta(time, standard_time),
            );
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

/// Run a benchmark and capture stdout and stderr in one log.
fn run_bench(cwd: &Path, bench: &str, features: &[&str], extra: &[&str], label: &str, cpu: Option<u32>) -> Result<String, AppError> {
    println!("==> Running {label}");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = if let Some(cpu) = cpu {
        let mut cmd = Command::new("taskset");
        cmd.arg("--cpu-list").arg(cpu.to_string()).arg(cargo);
        cmd
    } else {
        Command::new(cargo)
    };
    cmd.current_dir(cwd).arg("bench").arg("--bench").arg(bench);
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    if !extra.is_empty() {
        cmd.arg("--");
        cmd.args(extra);
    }
    let out = cmd
        .output()
        .map_err(|e| app_err!("failed to spawn cargo bench --bench {bench}: {e}"))?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        // Mirror the captured log to stderr so users can debug failures.
        let _ = std::io::stderr().write_all(combined.as_bytes());
        bail!("cargo bench --bench {bench} failed with status {}", out.status);
    }
    Ok(combined)
}

/// A criterion filter matching exactly the published variants of `group`.
fn group_filter(group: &str, variants: &[&str]) -> String {
    format!("^{group}/({})$", variants.join("|"))
}

/// Run each benchmark group in an independent process.
///
/// Grouping the implementations being compared in one process keeps their
/// measurements close enough to limit host-load and frequency drift. Every
/// Criterion iteration still creates fresh inputs and freshly warmed state.
fn run_groups(
    cwd: &Path,
    bench: &str,
    features: &[&str],
    groups: &[Group],
    common_args: &[&str],
    cpu: Option<u32>,
) -> Result<String, AppError> {
    let mut combined = String::new();
    for (group, variants) in groups {
        let filter = group_filter(group, variants);
        let mut args = Vec::with_capacity(common_args.len() + 1);
        args.push(filter.as_str());
        args.extend_from_slice(common_args);
        combined.push_str(&run_bench(cwd, bench, features, &args, &format!("{bench} ({group})"), cpu)?);
    }
    Ok(combined)
}

/// Run every benchmark group in an independent process, alternating group order
/// between rounds.
///
/// Keeping compared variants in one process makes their CPU state adjacent;
/// alternating group order and reporting the median across rounds reduces
/// longer-term host-load and frequency bias.
fn run_repeated_variants(
    cwd: &Path,
    bench: &str,
    features: &[&str],
    groups: &[Group],
    common_args: &[&str],
    repetitions: u32,
    cpu: Option<u32>,
) -> Result<String, AppError> {
    let mut combined = String::new();
    for round in 0..repetitions {
        let indices: Vec<usize> = if round.is_multiple_of(2) {
            (0..groups.len()).collect()
        } else {
            (0..groups.len()).rev().collect()
        };
        for index in indices {
            let (group, variants) = groups[index];
            let filter = group_filter(group, variants);
            let mut args = Vec::with_capacity(common_args.len() + 1);
            args.push(filter.as_str());
            args.extend_from_slice(common_args);
            combined.push_str(&run_bench(
                cwd,
                bench,
                features,
                &args,
                &format!("{bench} ({group}, round {}/{repetitions})", round + 1),
                cpu,
            )?);
        }
    }
    Ok(combined)
}

fn keys(groups: &[Group]) -> Vec<(&str, &str)> {
    groups
        .iter()
        .flat_map(|(group, variants)| variants.iter().map(move |variant| (*group, *variant)))
        .collect()
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
    let samples_arg = samples.to_string();
    let measurement_arg = measurement_secs.to_string();
    let warmup_arg = warmup_secs.to_string();

    let crit_args = vec![
        "--warm-up-time",
        warmup_arg.as_str(),
        "--measurement-time",
        measurement_arg.as_str(),
        "--sample-size",
        samples_arg.as_str(),
    ];

    let arena_vs_allocator_log = run_groups(
        &crate_dir,
        "criterion_arena_vs_allocator",
        &[],
        ARENA_VS_ALLOCATOR_GROUPS,
        &crit_args,
        args.cpu,
    )?;
    let alloc_log = run_repeated_variants(
        &crate_dir,
        "criterion_alloc",
        &[],
        ALLOC_GROUPS,
        &crit_args,
        args.comparison_repetitions,
        args.cpu,
    )?;
    let teardown_log = run_repeated_variants(
        &crate_dir,
        "multitude_teardown",
        &[],
        TEARDOWN_GROUPS,
        &crit_args,
        args.comparison_repetitions,
        args.cpu,
    )?;
    let serde_log = run_repeated_variants(
        &crate_dir,
        "multitude_serde",
        &["serde_json"],
        SERDE_GROUPS,
        &crit_args,
        args.comparison_repetitions,
        args.cpu,
    )?;
    let record_batch_log = run_groups(
        &crate_dir,
        "multitude_record_batch",
        &["serde_json"],
        RECORD_BATCH_GROUPS,
        &crit_args,
        args.cpu,
    )?;

    println!("==> Building docs/PERF.md");

    let mut crit = parse_criterion(&arena_vs_allocator_log, &keys(ARENA_VS_ALLOCATOR_GROUPS))?;
    crit.extend(parse_criterion(&alloc_log, &keys(ALLOC_GROUPS))?);
    crit.extend(parse_criterion(&teardown_log, &keys(TEARDOWN_GROUPS))?);
    crit.extend(parse_criterion(&serde_log, &keys(SERDE_GROUPS))?);
    crit.extend(parse_criterion(&record_batch_log, &keys(RECORD_BATCH_GROUPS))?);

    let report = build_report(
        &crit,
        args.comparison_repetitions,
        samples,
        warmup_secs,
        measurement_secs,
        args.cpu,
    );
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
