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

//! Run criterion + gungraun benchmark suites and rebuild `docs/PERF.md`.
//!
//! On Linux the script runs both criterion (wall-clock) and gungraun
//! (Callgrind instruction counts) suites; gungraun requires `valgrind` to
//! be installed.  On Windows, valgrind is unavailable so only the criterion
//! suites are run and the gungraun columns in the report show "—".
//!
//! Usage:
//!   `scripts/perf_report.rs`                                       — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                                — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`     — custom criterion settings
//!   `scripts/perf_report.rs --cpu 4`                               — pin benchmark processes to CPU 4
//!
//! Criterion variants with a Callgrind counterpart are aligned through the
//! group tables below. If a bench is added or removed, update the matching
//! group table.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::{env, fs};

use clap::Parser;
use ohno::{AppError, app_err, bail};

/// Run criterion + gungraun benchmark suites and rebuild `docs/PERF.md`.
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

    /// Number of independently warmed runs for each Serde and teardown variant
    /// (default: 3).
    #[arg(long, default_value_t = 3)]
    serde_repetitions: u32,

    /// Force-skip the gungraun (Callgrind) benches even when `valgrind` is
    /// available. Always implied on Windows.
    #[arg(long)]
    no_gungraun: bool,
}

/// `(criterion_variant, Some(gungraun_fn) | None)`.
type Variant = (&'static str, Option<&'static str>);

/// `(group_name, variants_in_definition_order)`.
type Group = (&'static str, &'static [Variant]);

/// Ordered (group, variants) — must match the criterion bench order.
///
/// Each variant is `(criterion_variant, Some(gungraun_fn))` or
/// `(criterion_variant, None)` if the variant has no gungraun counterpart
/// (those columns will show "—"). The criterion variant name is the string
/// passed to `g.bench_function(...)` in `benches/criterion_*.rs`; the
/// gungraun function name is the `fn` name in `benches/gungraun_*.rs` (its
/// `library_benchmark` symbol).
const GROUPS: &[Group] = &[
    (
        "arena_creation",
        &[("multitude_new", Some("multitude_new")), ("bumpalo_new", Some("bumpalo_new"))],
    ),
    (
        "alloc_u64",
        &[
            ("alloc", Some("alloc")),
            ("bumpalo_alloc", Some("bumpalo_alloc")),
            ("alloc_with", Some("alloc_with")),
            ("bumpalo_alloc_with", Some("bumpalo_alloc_with")),
            ("alloc_box", Some("alloc_box")),
            ("alloc_box_with", Some("alloc_box_with")),
            ("alloc_uninit_box", Some("alloc_uninit_box")),
            ("alloc_zeroed_box", Some("alloc_zeroed_box")),
            ("alloc_arc", Some("alloc_arc")),
            ("alloc_arc_with", Some("alloc_arc_with")),
            ("alloc_uninit_arc", Some("alloc_uninit_arc")),
            ("alloc_zeroed_arc", Some("alloc_zeroed_arc")),
            ("alloc_rc", Some("alloc_rc")),
            ("alloc_rc_with", Some("alloc_rc_with")),
            ("alloc_uninit_rc", Some("alloc_uninit_rc")),
            ("alloc_zeroed_rc", Some("alloc_zeroed_rc")),
        ],
    ),
    (
        "alloc_str",
        &[
            ("alloc_str", Some("alloc_str")),
            ("bumpalo_alloc_str", Some("bumpalo_alloc_str")),
            ("alloc_str_box", Some("alloc_str_box")),
            ("alloc_str_arc", Some("alloc_str_arc")),
            ("alloc_str_rc", Some("alloc_str_rc")),
        ],
    ),
    (
        "alloc_slice",
        &[
            ("alloc_slice_copy", Some("alloc_slice_copy")),
            ("bumpalo_alloc_slice_copy", Some("bumpalo_alloc_slice_copy")),
            ("alloc_slice_clone", Some("alloc_slice_clone")),
            ("bumpalo_alloc_slice_clone", Some("bumpalo_alloc_slice_clone")),
            ("alloc_slice_fill_with", Some("alloc_slice_fill_with")),
            ("bumpalo_alloc_slice_fill_with", Some("bumpalo_alloc_slice_fill_with")),
            ("alloc_slice_fill_iter", Some("alloc_slice_fill_iter")),
            ("bumpalo_alloc_slice_fill_iter", Some("bumpalo_alloc_slice_fill_iter")),
            ("alloc_slice_copy_box", Some("alloc_slice_copy_box")),
            ("alloc_slice_clone_box", Some("alloc_slice_clone_box")),
            ("alloc_slice_fill_with_box", Some("alloc_slice_fill_with_box")),
            ("alloc_slice_fill_iter_box", Some("alloc_slice_fill_iter_box")),
            ("alloc_uninit_slice_box", Some("alloc_uninit_slice_box")),
            ("alloc_zeroed_slice_box", Some("alloc_zeroed_slice_box")),
            ("alloc_slice_copy_arc", Some("alloc_slice_copy_arc")),
            ("alloc_slice_clone_arc", Some("alloc_slice_clone_arc")),
            ("alloc_slice_fill_with_arc", Some("alloc_slice_fill_with_arc")),
            ("alloc_slice_fill_iter_arc", Some("alloc_slice_fill_iter_arc")),
            ("alloc_uninit_slice_arc", Some("alloc_uninit_slice_arc")),
            ("alloc_zeroed_slice_arc", Some("alloc_zeroed_slice_arc")),
            ("alloc_slice_copy_rc", Some("alloc_slice_copy_rc")),
            ("alloc_slice_clone_rc", Some("alloc_slice_clone_rc")),
            ("alloc_slice_fill_with_rc", Some("alloc_slice_fill_with_rc")),
            ("alloc_slice_fill_iter_rc", Some("alloc_slice_fill_iter_rc")),
            ("alloc_uninit_slice_rc", Some("alloc_uninit_slice_rc")),
            ("alloc_zeroed_slice_rc", Some("alloc_zeroed_slice_rc")),
        ],
    ),
    (
        "string_builder",
        &[
            ("alloc_string", Some("alloc_string")),
            ("bumpalo_string_new_in", Some("bumpalo_string_new_in")),
            ("alloc_string_with_capacity", Some("alloc_string_with_capacity")),
            ("bumpalo_string_with_capacity_in", Some("bumpalo_string_with_capacity_in")),
        ],
    ),
    (
        "vec_builder",
        &[
            ("alloc_vec", Some("alloc_vec")),
            ("bumpalo_vec_new_in", Some("bumpalo_vec_new_in")),
            ("alloc_vec_with_capacity", Some("alloc_vec_with_capacity")),
            ("bumpalo_vec_with_capacity_in", Some("bumpalo_vec_with_capacity_in")),
        ],
    ),
    (
        "allocator_grow",
        &[
            ("in_place", Some("allocator_grow_in_place")),
            ("zeroed_in_place", Some("allocator_grow_zeroed_in_place")),
            ("shrink_in_place", Some("allocator_shrink_in_place")),
        ],
    ),
    // Criterion-only whole-lifecycle comparison (allocate a mixed working set,
    // then release it): `multitude` arena (bulk reset) vs the system allocator.
    // No gungraun counterpart, so the instruction-count columns show "—".
    ("arena_vs_allocator", &[("arena", None), ("system", None)]),
    (
        "drop",
        &[
            ("box_u64", Some("box_u64")),
            ("rc_u64", Some("rc_u64")),
            ("arc_u64", Some("arc_u64")),
            ("box_droppy", Some("box_droppy")),
            ("rc_droppy", Some("rc_droppy")),
            ("arc_droppy", Some("arc_droppy")),
            ("str_box", Some("str_box")),
            ("str_rc", Some("str_rc")),
            ("str_arc", Some("str_arc")),
            ("slice_box_u64", Some("slice_box_u64")),
            ("slice_rc_u64", Some("slice_rc_u64")),
            ("slice_arc_u64", Some("slice_arc_u64")),
            ("slice_box_droppy", Some("slice_box_droppy")),
            ("slice_rc_droppy", Some("slice_rc_droppy")),
            ("slice_arc_droppy", Some("slice_arc_droppy")),
            ("alloc", Some("alloc")),
        ],
    ),
    ("clone", &[("rc_u64", Some("clone_rc_u64")), ("arc_u64", Some("clone_arc_u64"))]),
];

const SERDE_GROUPS: &[Group] = &[
    (
        "multitude_serde/typed",
        &[
            ("arena_owned", Some("typed_arena_owned")),
            ("serde_json_owned", Some("typed_serde_json_owned")),
        ],
    ),
    (
        "multitude_serde/dynamic",
        &[
            ("arena_value", Some("dynamic_arena_value")),
            ("serde_json_value", Some("dynamic_serde_json_value")),
        ],
    ),
    (
        "multitude_serde/typed_lifecycle",
        &[
            ("serde_json", Some("lifecycle_serde_json")),
            ("multitude", Some("lifecycle_multitude")),
            ("bumpalo", Some("lifecycle_bumpalo")),
        ],
    ),
    (
        "multitude_serde/batch_lifecycle",
        &[
            ("serde_json", Some("batch_lifecycle_serde_json")),
            ("multitude", Some("batch_lifecycle_multitude")),
            ("bumpalo", Some("batch_lifecycle_bumpalo")),
        ],
    ),
];

const TEARDOWN_GROUPS: &[Group] = &[
    (
        "multitude_teardown/free_1",
        &[
            ("standard", Some("free_1_standard")),
            ("multitude", Some("free_1_multitude")),
            ("bumpalo", Some("free_1_bumpalo")),
        ],
    ),
    (
        "multitude_teardown/free_32",
        &[
            ("standard", Some("free_32_standard")),
            ("multitude", Some("free_32_multitude")),
            ("bumpalo", Some("free_32_bumpalo")),
        ],
    ),
    (
        "multitude_teardown/free_1000",
        &[
            ("standard", Some("free_1000_standard")),
            ("multitude", Some("free_1000_multitude")),
            ("bumpalo", Some("free_1000_bumpalo")),
        ],
    ),
];

const RECORD_BATCH_GROUPS: &[Group] = &[
    (
        "multitude_record_batch/decode",
        &[
            ("standard_vec", Some("decode_standard_vec")),
            ("arena_box_slice", Some("decode_arena_box_slice")),
            ("arena_vec_baseline", Some("decode_arena_vec_baseline")),
        ],
    ),
    (
        "multitude_record_batch/strings",
        &[
            ("standard_vec_unescaped", Some("strings_standard_vec_unescaped")),
            ("standard_vec_escaped", Some("strings_standard_vec_escaped")),
            ("arena_vec_unescaped", Some("strings_arena_vec_unescaped")),
            ("arena_vec_escaped", Some("strings_arena_vec_escaped")),
        ],
    ),
    (
        "multitude_record_batch/reuse",
        &[
            ("repeated_no_reset", Some("reuse_repeated_no_reset")),
            ("reset_recreate", Some("reuse_reset_recreate")),
        ],
    ),
    (
        "multitude_record_batch/sparse_retention",
        &[
            ("standard_one_in_eight", Some("sparse_retention_standard_one_in_eight")),
            ("arena_one_in_eight", Some("sparse_retention_arena_one_in_eight")),
        ],
    ),
    (
        "multitude_record_batch/lazy_raw_strings",
        &[
            ("eager_sparse_escaped", Some("lazy_raw_strings_eager_sparse_escaped")),
            ("lazy_sparse_escaped", Some("lazy_raw_strings_lazy_sparse_escaped")),
        ],
    ),
    (
        "multitude_record_batch/errors",
        &[
            ("malformed_standard", Some("errors_malformed_standard")),
            ("malformed_arena", Some("errors_malformed_arena")),
            ("resource_limited_arena", Some("errors_resource_limited_arena")),
        ],
    ),
    (
        "multitude_record_batch/refresh_workload",
        &[
            ("standard_selective", Some("refresh_workload_standard_selective")),
            ("arena_vec_reset_selective", Some("refresh_workload_arena_vec_reset_selective")),
            ("arena_each_reset_selective", Some("refresh_workload_arena_each_reset_selective")),
            (
                "arena_raw_each_reset_index_selective",
                Some("refresh_workload_arena_raw_each_reset_index_selective"),
            ),
        ],
    ),
];

/// `(workload, criterion_group, arena_variant, standard_variant)`.
const SERDE_COMPARISONS: &[(&str, &str, &str, &str)] = &[
    ("Typed record", "multitude_serde/typed", "arena_owned", "serde_json_owned"),
    ("Dynamic value", "multitude_serde/dynamic", "arena_value", "serde_json_value"),
];

const SERDE_LIFECYCLE_COMPARISONS: &[(&str, &str, &str)] = &[
    ("Standard Serde", "serde_json", "lifecycle_serde_json"),
    ("Multitude", "multitude", "lifecycle_multitude"),
    ("Bumpalo (manual seed)", "bumpalo", "lifecycle_bumpalo"),
];

const SERDE_BATCH_LIFECYCLE_COMPARISONS: &[(&str, &str, &str)] = &[
    ("Standard Serde", "serde_json", "batch_lifecycle_serde_json"),
    ("Multitude", "multitude", "batch_lifecycle_multitude"),
    ("Bumpalo (manual seed)", "bumpalo", "batch_lifecycle_bumpalo"),
];

const COMPARISONS: &[(&str, &str, &str)] = &[
    ("alloc_u64", "alloc", "bumpalo_alloc"),
    ("alloc_str", "alloc_str", "bumpalo_alloc_str"),
    ("alloc_slice", "alloc_slice_copy", "bumpalo_alloc_slice_copy"),
    ("alloc_slice", "alloc_slice_clone", "bumpalo_alloc_slice_clone"),
    ("alloc_slice", "alloc_slice_fill_with", "bumpalo_alloc_slice_fill_with"),
    ("alloc_slice", "alloc_slice_fill_iter", "bumpalo_alloc_slice_fill_iter"),
    ("string_builder", "alloc_string", "bumpalo_string_new_in"),
    ("string_builder", "alloc_string_with_capacity", "bumpalo_string_with_capacity_in"),
    ("vec_builder", "alloc_vec", "bumpalo_vec_new_in"),
    ("vec_builder", "alloc_vec_with_capacity", "bumpalo_vec_with_capacity_in"),
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
/// `expected` is used only for a sanity-check warning when names are
/// missing or extra; it does not gate which entries get returned.
fn parse_criterion(text: &str, expected: &[(&str, &str)]) -> Vec<(String, f64)> {
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
    for missing in expected_keys.difference(&got_keys) {
        eprintln!("warning: criterion log missing expected bench {missing}");
    }
    for extra in got_keys.difference(&expected_keys) {
        eprintln!("warning: criterion log has unexpected bench {extra}");
    }
    out
}

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

/// One gungraun benchmark's parsed metrics.
struct GungEntry {
    name: String,
    metrics: Vec<(String, u64)>,
}

/// Parse the iai-callgrind / gungraun text output.
///
/// Per-bench header looks like
/// `<prefix>::<module>::<fn> run:(<args>)` for `#[bench::run(...)]`
/// benches, and `<prefix>::<module>::<fn>` (no trailing run-clause) for
/// plain `#[library_benchmark]` benches. Both shapes are accepted.
/// Metric lines follow as `  <Metric>: <value>|...`.
fn parse_gungraun(text: &str, prefix: &str) -> Vec<GungEntry> {
    let mut out: Vec<GungEntry> = Vec::new();
    let mut cur: Option<GungEntry> = None;
    let header_prefix = format!("{prefix}::");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&header_prefix) {
            if let Some(after_mod) = rest.find("::") {
                let after = &rest[after_mod + 2..];
                // `after` is either "<fn>" alone or "<fn> run:(...)";
                // split on the first whitespace to isolate the fn name.
                let fn_name = match after.find(char::is_whitespace) {
                    Some(sp) => &after[..sp],
                    None => after,
                };
                // Reject obvious non-identifier shapes so unrelated lines
                // that happen to start with the prefix can't slip through.
                let valid = !fn_name.is_empty() && fn_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if valid {
                    if let Some(prev) = cur.take() {
                        out.push(prev);
                    }
                    cur = Some(GungEntry {
                        name: fn_name.to_string(),
                        metrics: Vec::new(),
                    });
                    continue;
                }
            }
        }
        if let Some(entry) = cur.as_mut() {
            // Lines look like `  Instructions: 12345|...`.
            let trimmed = line.trim_start();
            if let Some(colon) = trimmed.find(':') {
                let key = &trimmed[..colon];
                if matches!(key, "Instructions" | "L1 Hits" | "LL Hits" | "RAM Hits" | "Bcm") {
                    let after = trimmed[colon + 1..].trim_start();
                    let num: String = after.chars().take_while(char::is_ascii_digit).collect();
                    if let Ok(v) = num.parse::<u64>() {
                        entry.metrics.push((key.to_string(), v));
                    }
                }
            }
        }
    }
    if let Some(entry) = cur.take() {
        out.push(entry);
    }
    out
}

fn gung_metric(g: &[GungEntry], name: &str, key: &str) -> Option<u64> {
    let entry = g.iter().find(|e| e.name == name)?;
    entry.metrics.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

fn fmt_ns(ns: Option<f64>) -> String {
    match ns {
        None => "—".into(),
        Some(ns) if ns < 1000.0 => format!("{ns:.0} ns"),
        Some(ns) if ns < 1e6 => format!("{:.2} µs", ns / 1e3),
        Some(ns) => format!("{:.2} ms", ns / 1e6),
    }
}

fn fmt_int(n: Option<u64>) -> String {
    match n {
        None => "—".into(),
        Some(n) => {
            // Insert commas as thousands separators.
            let s = n.to_string();
            let bytes = s.as_bytes();
            let mut out = String::with_capacity(s.len() + s.len() / 3);
            let first = bytes.len() % 3;
            if first > 0 {
                out.push_str(&s[..first]);
            }
            for (i, chunk) in bytes[first..].chunks(3).enumerate() {
                if !(i == 0 && first == 0) {
                    out.push(',');
                }
                out.push_str(std::str::from_utf8(chunk).expect("ASCII digits from u64::to_string"));
            }
            out
        }
    }
}

fn fmt_float_delta(candidate: Option<f64>, baseline: Option<f64>) -> String {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) if baseline != 0.0 => {
            format!("{:+.1}%", (candidate / baseline - 1.0) * 100.0)
        }
        _ => "—".into(),
    }
}

fn fmt_int_delta(candidate: Option<u64>, baseline: Option<u64>) -> String {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) if baseline != 0 => {
            #[expect(clippy::cast_precision_loss, reason = "benchmark instruction counts stay well under 2^53")]
            let pct = (candidate as f64 / baseline as f64 - 1.0) * 100.0;
            format!("{pct:+.1}%")
        }
        _ => "—".into(),
    }
}

fn gung_for(group: &str, variant: &str, g_alloc: &[GungEntry]) -> Option<u64> {
    for (g, vs) in GROUPS {
        if *g != group {
            continue;
        }
        for (v, gname) in *vs {
            if *v == variant {
                return gname.and_then(|n| gung_metric(g_alloc, n, "Instructions"));
            }
        }
    }
    None
}

fn build_report(
    crit: &[(String, f64)],
    g_alloc: &[GungEntry],
    g_drop: &[GungEntry],
    g_teardown: &[GungEntry],
    g_serde: &[GungEntry],
    g_record_batch: &[GungEntry],
    serde_repetitions: u32,
    cpu: Option<u32>,
) -> String {
    let mut out = String::new();
    out.push_str("# Multitude Performance Report\n\n");
    out.push_str("Generated by `scripts/perf_report.rs`:\n");
    out.push_str(
        "- `cargo bench --bench criterion_alloc` and `criterion_drop` — \
         criterion wall-clock timings.\n",
    );
    out.push_str(
        "- `cargo bench --bench criterion_arena_vs_allocator` — criterion \
         wall-clock timing of allocating a mixed working set and then releasing \
         it, comparing the arena (bulk reset) against the system allocator \
         (mimalloc, per-object free).\n",
    );
    out.push_str(
        "- `cargo bench --bench gungraun_alloc` and `gungraun_drop` — \
         Callgrind instruction-precise counts.\n",
    );
    out.push_str(
        "- `cargo bench --bench multitude_serde` and `multitude_serde_cg` — \
         differential wall-clock and instruction-count measurements for \
         standard Serde and arena deserialization.\n",
    );
    out.push_str(
        "- `cargo bench --bench multitude_teardown` and \
         `multitude_teardown_cg` — matched measurements of freeing standard \
         allocations and resetting local-reference arenas.\n\n",
    );
    out.push_str(
        "- `cargo bench --bench multitude_record_batch` and \
         `multitude_record_batch_cg` — wide-record decoding, reuse, sparse \
         retention, error handling, and reset-per-refresh workloads.\n\n",
    );
    out.push_str("**Workload:** N = 1000 operations per measurement; slice element count = 8.  \n");
    out.push_str(
        "Owned slice rows (`Box<[u64]>`, `Arc<[u64]>`, and `Rc<[u64]>` variants) \
         use N = 768 so their metadata-bearing allocations stay within one \
         warmed normal arena chunk.\n",
    );
    out.push_str(
        "Serde single-record rows measure one deserialization of the documented \
         JSON fixture; batch rows process 32 independent documents.\n",
    );
    out.push_str(
        "Record-batch rows normally process 16 wide records. The refresh workload \
         processes 1,000 escaped-string records, retains one in eight, and keeps \
         the previous retained generation alive until its replacement is ready.\n",
    );
    out.push_str(
        "Criterion median is reported (default 30 samples, 1 s warm-up, \
         2 s measurement; override with `--samples` / `--measurement-time` / \
         `--warm-up-time`).  \n",
    );
    if let Some(cpu) = cpu {
        let _ = writeln!(out, "Benchmark processes were pinned to logical CPU {cpu}.");
    }
    out.push_str(
        "Each allocation and drop Criterion group is measured in its own \
         warmed process, with direct Multitude/Bumpalo alternatives adjacent \
         to minimize host-load and frequency drift.\n",
    );
    let repetition_label = if serde_repetitions == 1 { "run" } else { "runs" };
    let _ = writeln!(
        out,
        "Serde and teardown timing is the median of {serde_repetitions} independently warmed \
         {repetition_label}, with variant order alternated between runs."
    );
    out.push_str(
        "Memory accesses = L1 Hits + LL Hits + RAM Hits \
         (Callgrind D-cache references).  \n",
    );
    out.push_str(
        "Bench names are aligned between criterion and gungraun via the group \
         tables in `scripts/perf_report.rs`.\n\n",
    );

    for (group, variants) in GROUPS {
        let _ = writeln!(out, "## `{group}`\n");
        out.push_str(
            "| Variant | Time (criterion) | Instructions | \
             Branch misses | Mem accesses |\n",
        );
        out.push_str("|---|---:|---:|---:|---:|\n");
        let src: &[GungEntry] = if matches!(*group, "drop" | "clone") { g_drop } else { g_alloc };
        for (variant, gung_name) in *variants {
            let t = lookup_time(crit, &format!("{group}/{variant}"));
            let instr = gung_name.and_then(|n| gung_metric(src, n, "Instructions"));
            let bcm = gung_name.and_then(|n| gung_metric(src, n, "Bcm"));
            let mem = gung_name.and_then(|n| {
                let l1 = gung_metric(src, n, "L1 Hits")?;
                let ll = gung_metric(src, n, "LL Hits")?;
                let ram = gung_metric(src, n, "RAM Hits")?;
                Some(l1 + ll + ram)
            });
            let _ = writeln!(
                out,
                "| `{variant}` | {} | {} | {} | {} |",
                fmt_ns(t),
                fmt_int(instr),
                fmt_int(bcm),
                fmt_int(mem),
            );
        }
        out.push('\n');
    }

    out.push_str("## Multitude vs Bumpalo Head-to-Head\n\n");
    out.push_str(
        "Direct comparisons of multitude versus bumpalo on identical \
         workloads (the multitude variant chosen is the closest \
         semantic equivalent to bumpalo's plain bump-allocation).\n\n",
    );
    out.push_str(
        "| Workload | Multitude time | Bumpalo time | Δ time | \
         Multitude instr | Bumpalo instr | Δ instr |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for (group, mvar, bvar) in COMPARISONS {
        let mt = lookup_time(crit, &format!("{group}/{mvar}"));
        let bt = lookup_time(crit, &format!("{group}/{bvar}"));
        let mi = gung_for(group, mvar, g_alloc);
        let bi = gung_for(group, bvar, g_alloc);
        let dt = fmt_float_delta(mt, bt);
        let di = fmt_int_delta(mi, bi);
        let _ = writeln!(
            out,
            "| `{mvar}` vs `{bvar}` | {} | {} | {} | {} | {} | {} |",
            fmt_ns(mt),
            fmt_ns(bt),
            dt,
            fmt_int(mi),
            fmt_int(bi),
            di,
        );
    }
    out.push('\n');

    out.push_str("## Allocation Teardown\n\n");
    out.push_str(
        "Setup is outside the measured region. Each implementation starts with \
         the same number of independent 64-byte, non-dropping payloads. The \
         standard path frees individually boxed values; Multitude allocations \
         use arena-local `Alloc<T>` handles that are leaked before measurement, \
         then release the generation with `Arena::reset`; Bumpalo likewise \
         measures only `Bump::reset`. Non-dropping payloads make bulk reset \
         semantically equivalent across the arena implementations.\n\n",
    );
    out.push_str(
        "| Allocations | Implementation | Time | Δ time vs standard | Instructions | \
         Δ instr vs standard |\n",
    );
    out.push_str("|---:|---|---:|---:|---:|---:|\n");
    for (count, group) in [
        (1, "multitude_teardown/free_1"),
        (32, "multitude_teardown/free_32"),
        (1_000, "multitude_teardown/free_1000"),
    ] {
        let standard_time = lookup_time(crit, &format!("{group}/standard"));
        let standard_gung = TEARDOWN_GROUPS
            .iter()
            .find(|(name, _)| name == &group)
            .and_then(|(_, variants)| variants.first())
            .and_then(|(_, name)| *name);
        let standard_instructions = standard_gung.and_then(|name| gung_metric(g_teardown, name, "Instructions"));
        for (label, variant) in [
            ("Standard allocator", "standard"),
            ("Multitude", "multitude"),
            ("Bumpalo", "bumpalo"),
        ] {
            let time = lookup_time(crit, &format!("{group}/{variant}"));
            let gung_name = TEARDOWN_GROUPS
                .iter()
                .find(|(name, _)| name == &group)
                .and_then(|(_, variants)| variants.iter().find(|(name, _)| name == &variant))
                .and_then(|(_, name)| *name);
            let instructions = gung_name.and_then(|name| gung_metric(g_teardown, name, "Instructions"));
            let _ = writeln!(
                out,
                "| {count} | {label} | {} | {} | {} | {} |",
                fmt_ns(time),
                fmt_float_delta(time, standard_time),
                fmt_int(instructions),
                fmt_int_delta(instructions, standard_instructions),
            );
        }
    }
    out.push('\n');

    out.push_str("## Serde Deserialization\n\n");
    out.push_str(
        "The arena and standard paths deserialize the same JSON into equivalent \
         typed or dynamic values. Criterion and Callgrind invoke the same shared, \
         out-of-line hot-path functions; only their iteration counts differ. Both \
         run against warmed allocator state; arena backing storage is preallocated \
         and faulted in during setup. Allocator setup and result teardown are \
         outside the measured region. Deltas report arena relative to standard \
         Serde; negative values favor the arena.\n\n",
    );
    out.push_str(
        "| Workload | Arena time | Standard time | Δ time | \
         Arena instr | Standard instr | Δ instr |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for (workload, group, arena_variant, standard_variant) in SERDE_COMPARISONS {
        let arena_time = lookup_time(crit, &format!("{group}/{arena_variant}"));
        let standard_time = lookup_time(crit, &format!("{group}/{standard_variant}"));
        let arena_gung = SERDE_GROUPS
            .iter()
            .find(|(name, _)| name == group)
            .and_then(|(_, variants)| variants.iter().find(|(name, _)| name == arena_variant))
            .and_then(|(_, name)| *name);
        let standard_gung = SERDE_GROUPS
            .iter()
            .find(|(name, _)| name == group)
            .and_then(|(_, variants)| variants.iter().find(|(name, _)| name == standard_variant))
            .and_then(|(_, name)| *name);
        let arena_instructions = arena_gung.and_then(|name| gung_metric(g_serde, name, "Instructions"));
        let standard_instructions = standard_gung.and_then(|name| gung_metric(g_serde, name, "Instructions"));
        let _ = writeln!(
            out,
            "| {workload} | {} | {} | {} | {} | {} | {} |",
            fmt_ns(arena_time),
            fmt_ns(standard_time),
            fmt_float_delta(arena_time, standard_time),
            fmt_int(arena_instructions),
            fmt_int(standard_instructions),
            fmt_int_delta(arena_instructions, standard_instructions),
        );
    }
    out.push('\n');

    out.push_str("### Reused Allocator Lifecycle\n\n");
    out.push_str(
        "This scenario deserializes and consumes the typed record, then performs \
         the cleanup a reusable allocator needs before the next request. Standard \
         Serde drops its owned output; Multitude drops its owning arena pointers \
         and resets the arena; Bumpalo drops its arena-borrowed output and resets \
         the bump allocator. Bumpalo has no built-in deserialization support, so \
         its row uses a hand-written `DeserializeSeed` that copies all strings and \
         sequence storage into the bump arena. Allocator construction remains \
         outside the measured region.\n\n",
    );
    out.push_str("#### One record\n\n");
    out.push_str(
        "| Implementation | Time | Δ time vs standard | Instructions | \
         Δ instr vs standard |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|\n");
    let lifecycle_group = "multitude_serde/typed_lifecycle";
    let standard_time = lookup_time(crit, &format!("{lifecycle_group}/serde_json"));
    let standard_instructions = gung_metric(g_serde, "lifecycle_serde_json", "Instructions");
    for (label, variant, gung_name) in SERDE_LIFECYCLE_COMPARISONS {
        let time = lookup_time(crit, &format!("{lifecycle_group}/{variant}"));
        let instructions = gung_metric(g_serde, gung_name, "Instructions");
        let _ = writeln!(
            out,
            "| {label} | {} | {} | {} | {} |",
            fmt_ns(time),
            fmt_float_delta(time, standard_time),
            fmt_int(instructions),
            fmt_int_delta(instructions, standard_instructions),
        );
    }
    out.push('\n');

    out.push_str("#### 32-record batch\n\n");
    out.push_str(
        "This repeats the same complete lifecycle for 32 independent JSON \
         documents in one reusable allocator generation. All implementations \
         use an outer standard `Vec`, so its allocation and destruction are \
         included equally.\n\n",
    );
    out.push_str(
        "| Implementation | Time | Δ time vs standard | Instructions | \
         Δ instr vs standard |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|\n");
    let batch_lifecycle_group = "multitude_serde/batch_lifecycle";
    let batch_standard_time = lookup_time(crit, &format!("{batch_lifecycle_group}/serde_json"));
    let batch_standard_instructions = gung_metric(g_serde, "batch_lifecycle_serde_json", "Instructions");
    for (label, variant, gung_name) in SERDE_BATCH_LIFECYCLE_COMPARISONS {
        let time = lookup_time(crit, &format!("{batch_lifecycle_group}/{variant}"));
        let instructions = gung_metric(g_serde, gung_name, "Instructions");
        let _ = writeln!(
            out,
            "| {label} | {} | {} | {} | {} |",
            fmt_ns(time),
            fmt_float_delta(time, batch_standard_time),
            fmt_int(instructions),
            fmt_int_delta(instructions, batch_standard_instructions),
        );
    }
    out.push('\n');

    out.push_str("## Record-Batch Deserialization\n\n");
    out.push_str(
        "These synthetic wide-record workloads compare standard decoding with \
         arena-backed collection, reuse, and selective-retention paths. The \
         reset-per-refresh workload recreates arena vectors after reset or \
         streams each item directly to the retention callback. Criterion and \
         Callgrind invoke the same shared hot-path functions and equivalent \
         prewarmed state.\n\n",
    );
    for (group, variants) in RECORD_BATCH_GROUPS {
        let title = group
            .strip_prefix("multitude_record_batch/")
            .expect("record-batch report groups use the benchmark prefix");
        let _ = writeln!(out, "### `{title}`\n");
        out.push_str(
            "| Variant | Time (criterion) | Instructions | \
             Branch misses | Mem accesses |\n",
        );
        out.push_str("|---|---:|---:|---:|---:|\n");
        for (variant, gung_name) in *variants {
            let time = lookup_time(crit, &format!("{group}/{variant}"));
            let instructions = gung_name.and_then(|name| gung_metric(g_record_batch, name, "Instructions"));
            let branch_misses = gung_name.and_then(|name| gung_metric(g_record_batch, name, "Bcm"));
            let memory_accesses = gung_name.and_then(|name| {
                let l1 = gung_metric(g_record_batch, name, "L1 Hits")?;
                let ll = gung_metric(g_record_batch, name, "LL Hits")?;
                let ram = gung_metric(g_record_batch, name, "RAM Hits")?;
                Some(l1 + ll + ram)
            });
            let _ = writeln!(
                out,
                "| `{variant}` | {} | {} | {} | {} |",
                fmt_ns(time),
                fmt_int(instructions),
                fmt_int(branch_misses),
                fmt_int(memory_accesses),
            );
        }
        out.push('\n');
    }
    out.pop();
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

/// Check whether `valgrind` is available on PATH.
fn have_valgrind() -> bool {
    Command::new("valgrind")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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

/// Run each benchmark group in an independent process.
///
/// Grouping direct alternatives in one process keeps their measurements close
/// enough to limit host-load and frequency drift. Every Criterion iteration
/// still creates fresh inputs and freshly warmed allocator state.
fn run_groups(
    cwd: &Path,
    bench: &str,
    features: &[&str],
    groups: &[Group],
    common_args: &[&str],
    cpu: Option<u32>,
) -> Result<String, AppError> {
    let mut combined = String::new();
    for (group, _) in groups {
        let filter = format!("^{group}/");
        let mut args = Vec::with_capacity(common_args.len() + 1);
        args.push(filter.as_str());
        args.extend_from_slice(common_args);
        combined.push_str(&run_bench(cwd, bench, features, &args, &format!("{bench} ({group})"), cpu)?);
    }
    Ok(combined)
}

/// Run every benchmark variant independently, alternating order between rounds.
///
/// This is reserved for cross-implementation lifecycle benchmarks whose setup
/// has materially different global allocator effects. Alternating order and
/// reporting the median across rounds reduces host-load and frequency bias.
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
        for (group, variants) in groups {
            let indices: Vec<usize> = if round.is_multiple_of(2) {
                (0..variants.len()).collect()
            } else {
                (0..variants.len()).rev().collect()
            };
            for index in indices {
                let variant = variants[index].0;
                let filter = format!("^{group}/{variant}$");
                let mut args = Vec::with_capacity(common_args.len() + 1);
                args.push(filter.as_str());
                args.extend_from_slice(common_args);
                combined.push_str(&run_bench(
                    cwd,
                    bench,
                    features,
                    &args,
                    &format!("{bench} ({group}/{variant}, round {}/{repetitions})", round + 1),
                    cpu,
                )?);
            }
        }
    }
    Ok(combined)
}

fn run(args: &Args) -> Result<(), AppError> {
    let crate_dir = crate_root();

    if args.cpu.is_some() && !cfg!(target_os = "linux") {
        bail!("--cpu is only supported on Linux");
    }

    // Skip gungraun on Windows (no valgrind), when explicitly disabled, or
    // when valgrind isn't available on the host.
    let run_gungraun = if cfg!(windows) {
        if !args.no_gungraun {
            eprintln!(
                "note: skipping gungraun benches; valgrind is unavailable on Windows. \
                 Gungraun columns in docs/PERF.md will show \"—\"."
            );
        }
        false
    } else if args.no_gungraun {
        eprintln!("note: --no-gungraun set; gungraun columns will show \"—\".");
        false
    } else if !have_valgrind() {
        bail!(
            "valgrind is required for the gungraun benchmarks; install it or rerun with \
             --no-gungraun to skip them"
        );
    } else {
        true
    };

    let (def_samples, def_meas) = if args.fast { (10, 1) } else { (30, 2) };
    let samples = args.samples.unwrap_or(def_samples).to_string();
    let meas = args.measurement_time.unwrap_or(def_meas).to_string();
    let warmup = args.warm_up_time.unwrap_or(1).to_string();

    let crit_args = vec![
        "--warm-up-time",
        warmup.as_str(),
        "--measurement-time",
        meas.as_str(),
        "--sample-size",
        samples.as_str(),
    ];

    let alloc_groups: Vec<Group> = GROUPS
        .iter()
        .copied()
        .filter(|(group, _)| !matches!(*group, "drop" | "clone" | "arena_vs_allocator"))
        .collect();
    let crit_alloc_log = run_groups(&crate_dir, "criterion_alloc", &[], &alloc_groups, &crit_args, args.cpu)?;
    let drop_groups: Vec<Group> = GROUPS
        .iter()
        .copied()
        .filter(|(group, _)| matches!(*group, "drop" | "clone"))
        .collect();
    let crit_drop_log = run_groups(&crate_dir, "criterion_drop", &[], &drop_groups, &crit_args, args.cpu)?;
    let arena_vs_allocator_groups: Vec<Group> = GROUPS.iter().copied().filter(|(group, _)| *group == "arena_vs_allocator").collect();
    let crit_ava_log = run_groups(
        &crate_dir,
        "criterion_arena_vs_allocator",
        &[],
        &arena_vs_allocator_groups,
        &crit_args,
        args.cpu,
    )?;
    let crit_teardown_log = run_repeated_variants(
        &crate_dir,
        "multitude_teardown",
        &[],
        TEARDOWN_GROUPS,
        &crit_args,
        args.serde_repetitions,
        args.cpu,
    )?;
    let crit_serde_log = run_repeated_variants(
        &crate_dir,
        "multitude_serde",
        &["serde_json"],
        SERDE_GROUPS,
        &crit_args,
        args.serde_repetitions,
        args.cpu,
    )?;
    let crit_record_batch_log = run_groups(
        &crate_dir,
        "multitude_record_batch",
        &["serde_json"],
        RECORD_BATCH_GROUPS,
        &crit_args,
        args.cpu,
    )?;
    let (gung_alloc_log, gung_drop_log, gung_teardown_log, gung_serde_log, gung_record_batch_log) = if run_gungraun {
        (
            run_bench(&crate_dir, "gungraun_alloc", &[], &[], "gungraun_alloc", args.cpu)?,
            run_bench(&crate_dir, "gungraun_drop", &[], &[], "gungraun_drop", args.cpu)?,
            run_bench(&crate_dir, "multitude_teardown_cg", &[], &[], "multitude_teardown_cg", args.cpu)?,
            run_bench(
                &crate_dir,
                "multitude_serde_cg",
                &["serde_json"],
                &[],
                "multitude_serde_cg",
                args.cpu,
            )?,
            run_bench(
                &crate_dir,
                "multitude_record_batch_cg",
                &["serde_json"],
                &[],
                "multitude_record_batch_cg",
                args.cpu,
            )?,
        )
    } else {
        (String::new(), String::new(), String::new(), String::new(), String::new())
    };

    println!("==> Building docs/PERF.md");

    // `arena_vs_allocator` is criterion-only and lives in its own bench binary,
    // so it is excluded from the alloc-log keys and parsed from its own log.
    let alloc_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .filter(|(g, _)| !matches!(*g, "drop" | "clone" | "arena_vs_allocator"))
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let drop_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .filter(|(g, _)| matches!(*g, "drop" | "clone"))
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let ava_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .filter(|(g, _)| *g == "arena_vs_allocator")
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let serde_keys: Vec<(&str, &str)> = SERDE_GROUPS
        .iter()
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let teardown_keys: Vec<(&str, &str)> = TEARDOWN_GROUPS
        .iter()
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let record_batch_keys: Vec<(&str, &str)> = RECORD_BATCH_GROUPS
        .iter()
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();

    let mut crit = parse_criterion(&crit_alloc_log, &alloc_keys);
    crit.extend(parse_criterion(&crit_drop_log, &drop_keys));
    crit.extend(parse_criterion(&crit_ava_log, &ava_keys));
    crit.extend(parse_criterion(&crit_teardown_log, &teardown_keys));
    crit.extend(parse_criterion(&crit_serde_log, &serde_keys));
    crit.extend(parse_criterion(&crit_record_batch_log, &record_batch_keys));
    let g_alloc = parse_gungraun(&gung_alloc_log, "gungraun_alloc");
    let g_drop = parse_gungraun(&gung_drop_log, "gungraun_drop");
    let g_teardown = parse_gungraun(&gung_teardown_log, "multitude_teardown_cg");
    let g_serde = parse_gungraun(&gung_serde_log, "multitude_serde_cg");
    let g_record_batch = parse_gungraun(&gung_record_batch_log, "multitude_record_batch_cg");

    let report = build_report(
        &crit,
        &g_alloc,
        &g_drop,
        &g_teardown,
        &g_serde,
        &g_record_batch,
        args.serde_repetitions,
        args.cpu,
    );
    let out_path = crate_dir.join("docs").join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;

    println!(
        "Wrote {} ({} criterion, {} gungraun_alloc, {} gungraun_drop, \
         {} multitude_teardown_cg, {} multitude_serde_cg, \
         {} multitude_record_batch_cg benches)",
        out_path.display(),
        crit.len(),
        g_alloc.len(),
        g_drop.len(),
        g_teardown.len(),
        g_serde.len(),
        g_record_batch.len(),
    );
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
