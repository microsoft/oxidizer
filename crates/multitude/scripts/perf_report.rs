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
//!
//! Both bench suites must be aligned 1:1: each criterion `<group>/<variant>`
//! corresponds to a gungraun `<group>_<variant>`. The variant order in
//! `GROUPS` mirrors the order benches are defined in `benches/criterion_*.rs`
//! and `benches/gungraun_*.rs`; if a bench is added or removed, update
//! `GROUPS` to match.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

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
        &[
            ("multitude_new", Some("multitude_new")),
            ("bumpalo_new", Some("bumpalo_new")),
        ],
    ),
    (
        "alloc_u64",
        &[
            ("alloc", Some("alloc")),
            ("alloc_with", Some("alloc_with")),
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
            ("bumpalo_alloc", Some("bumpalo_alloc")),
            ("bumpalo_alloc_with", Some("bumpalo_alloc_with")),
        ],
    ),
    (
        "alloc_str",
        &[
            ("alloc_str", Some("alloc_str")),
            ("alloc_str_box", Some("alloc_str_box")),
            ("alloc_str_arc", Some("alloc_str_arc")),
            ("alloc_str_rc", Some("alloc_str_rc")),
            ("bumpalo_alloc_str", Some("bumpalo_alloc_str")),
        ],
    ),
    (
        "alloc_slice",
        &[
            ("alloc_slice_copy", Some("alloc_slice_copy")),
            ("alloc_slice_clone", Some("alloc_slice_clone")),
            ("alloc_slice_fill_with", Some("alloc_slice_fill_with")),
            ("alloc_slice_fill_iter", Some("alloc_slice_fill_iter")),
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
            ("bumpalo_alloc_slice_copy", Some("bumpalo_alloc_slice_copy")),
            ("bumpalo_alloc_slice_clone", Some("bumpalo_alloc_slice_clone")),
            ("bumpalo_alloc_slice_fill_with", Some("bumpalo_alloc_slice_fill_with")),
            ("bumpalo_alloc_slice_fill_iter", Some("bumpalo_alloc_slice_fill_iter")),
        ],
    ),
    (
        "string_builder",
        &[
            ("alloc_string", Some("alloc_string")),
            ("alloc_string_with_capacity", Some("alloc_string_with_capacity")),
            ("bumpalo_string_new_in", Some("bumpalo_string_new_in")),
            ("bumpalo_string_with_capacity_in", Some("bumpalo_string_with_capacity_in")),
        ],
    ),
    (
        "vec_builder",
        &[
            ("alloc_vec", Some("alloc_vec")),
            ("alloc_vec_with_capacity", Some("alloc_vec_with_capacity")),
            ("bumpalo_vec_new_in", Some("bumpalo_vec_new_in")),
            ("bumpalo_vec_with_capacity_in", Some("bumpalo_vec_with_capacity_in")),
        ],
    ),
    // Criterion-only whole-lifecycle comparison (allocate a mixed working set,
    // then release it): `multitude` arena (bulk reset) vs the system allocator.
    // No gungraun counterpart, so the instruction-count columns show "—".
    (
        "arena_vs_allocator",
        &[("arena", None), ("system", None)],
    ),
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
    let Some((g, v)) = s.split_once('/') else { return false; };
    if g.is_empty() || v.is_empty() {
        return false;
    }
    let id_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    g.chars().all(id_char) && v.chars().all(id_char)
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

    let expected_keys: std::collections::HashSet<String> =
        expected.iter().map(|(g, v)| format!("{g}/{v}")).collect();
    let got_keys: std::collections::HashSet<String> = out.iter().map(|(k, _)| k.clone()).collect();
    for missing in expected_keys.difference(&got_keys) {
        eprintln!("warning: criterion log missing expected bench {missing}");
    }
    for extra in got_keys.difference(&expected_keys) {
        eprintln!("warning: criterion log has unexpected bench {extra}");
    }
    out
}

fn lookup_time(crit: &[(String, f64)], key: &str) -> Option<f64> {
    crit.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
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
                let valid = !fn_name.is_empty()
                    && fn_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
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

fn build_report(crit: &[(String, f64)], g_alloc: &[GungEntry], g_drop: &[GungEntry]) -> String {
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
         Callgrind instruction-precise counts.\n\n",
    );
    out.push_str(
        "**Workload:** N = 1000 operations per measurement; slice element count = 8.  \n",
    );
    out.push_str(
        "Criterion median is reported (default 30 samples, 1 s warm-up, \
         2 s measurement; override with `--samples` / `--measurement-time` / \
         `--warm-up-time`).  \n",
    );
    out.push_str(
        "Memory accesses = L1 Hits + LL Hits + RAM Hits \
         (Callgrind D-cache references).  \n",
    );
    out.push_str(
        "Bench names are aligned between criterion and gungraun via the \
         `GROUPS` table in `scripts/perf_report.rs`.\n\n",
    );

    for (group, variants) in GROUPS {
        let _ = writeln!(out, "## `{group}`\n");
        out.push_str(
            "| Variant | Time (criterion) | Instructions | \
             Branch misses | Mem accesses |\n",
        );
        out.push_str("|---|---:|---:|---:|---:|\n");
        let src: &[GungEntry] = if *group == "drop" { g_drop } else { g_alloc };
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
        let dt = match (mt, bt) {
            (Some(m), Some(b)) if b != 0.0 => format!("{:+.1}%", (m / b - 1.0) * 100.0),
            _ => "—".into(),
        };
        let di = match (mi, bi) {
            (Some(m), Some(b)) if b != 0 => {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "bench instruction counts (N=1000) stay well under 2^53"
                )]
                let pct = (m as f64 / b as f64 - 1.0) * 100.0;
                format!("{pct:+.1}%")
            }
            _ => "—".into(),
        };
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

/// Run `cargo bench --bench <name> -- <args>` from `cwd`, capturing combined
/// stdout+stderr (criterion writes summaries to stdout; we mirror the
/// previous shell script's behaviour of redirecting both streams into one log).
fn run_bench(cwd: &Path, bench: &str, extra: &[&str], label: &str) -> Result<String, AppError> {
    println!("==> Running {label}");
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(cwd).arg("bench").arg("--bench").arg(bench);
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

fn run(args: &Args) -> Result<(), AppError> {
    let crate_dir = crate_root();

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

    let crit_alloc_log = run_bench(
        &crate_dir,
        "criterion_alloc",
        &crit_args,
        &format!("criterion_alloc: {samples} samples, {meas}s measurement"),
    )?;
    let crit_drop_log = run_bench(
        &crate_dir,
        "criterion_drop",
        &crit_args,
        &format!("criterion_drop: {samples} samples, {meas}s measurement"),
    )?;
    let crit_ava_log = run_bench(
        &crate_dir,
        "criterion_arena_vs_allocator",
        &crit_args,
        &format!("criterion_arena_vs_allocator: {samples} samples, {meas}s measurement"),
    )?;
    let (gung_alloc_log, gung_drop_log) = if run_gungraun {
        (
            run_bench(&crate_dir, "gungraun_alloc", &[], "gungraun_alloc")?,
            run_bench(&crate_dir, "gungraun_drop", &[], "gungraun_drop")?,
        )
    } else {
        (String::new(), String::new())
    };

    println!("==> Building docs/PERF.md");

    // `arena_vs_allocator` is criterion-only and lives in its own bench binary,
    // so it is excluded from the alloc-log keys and parsed from its own log.
    let alloc_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .filter(|(g, _)| *g != "drop" && *g != "arena_vs_allocator")
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let drop_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .filter(|(g, _)| *g == "drop")
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();
    let ava_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .filter(|(g, _)| *g == "arena_vs_allocator")
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();

    let mut crit = parse_criterion(&crit_alloc_log, &alloc_keys);
    crit.extend(parse_criterion(&crit_drop_log, &drop_keys));
    crit.extend(parse_criterion(&crit_ava_log, &ava_keys));
    let g_alloc = parse_gungraun(&gung_alloc_log, "gungraun_alloc");
    let g_drop = parse_gungraun(&gung_drop_log, "gungraun_drop");

    let report = build_report(&crit, &g_alloc, &g_drop);
    let out_path = crate_dir.join("docs").join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;

    println!(
        "Wrote {} ({} criterion, {} gungraun_alloc, {} gungraun_drop benches)",
        out_path.display(),
        crit.len(),
        g_alloc.len(),
        g_drop.len(),
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
