#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
---

//! Run the criterion wall-clock and memory-footprint benchmark suites and
//! rebuild `docs/PERF.md`.
//!
//! The published report is a curated set of customer-facing scenarios: the three
//! interning operations (`insert`, `reuse`, `lookup`) single-threaded and
//! concurrently at 1/2/4/8 threads, plus the live-heap footprint of each
//! interner. The crate's Callgrind instruction-count benches
//! (`benches/internity_compare_cg.rs`) are kept for internal optimization work
//! and are not part of this report, so no valgrind installation is required.
//!
//! Usage:
//!   `scripts/perf_report.rs`                                    — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                             — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`  — custom criterion settings
//!
//! internity has a single criterion bench binary (`internity_compare`, with
//! groups `internity_compare/insert` / `internity_compare/reuse` /
//! `internity_compare/lookup` and their `*-concurrent` counterparts) and a
//! memory-footprint binary (`internity_mem`). Every benchmark measures only
//! insert/dedupe or lookup; benchmark setup and result destruction are kept
//! outside the timed region.

use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;

type BoxErr = Box<dyn Error>;

/// Run the criterion + memory benchmark suites and rebuild `docs/PERF.md`.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Use a faster, lower-fidelity run (10 samples, 1s measurement).
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
}

/// `(operation, criterion_group, interner_rows_in_table_order)`.
type Group = (&'static str, &'static str, &'static [&'static str]);

/// Single-threaded tables. The row name is the string passed to
/// `g.bench_function(...)` in `benches/internity_compare.rs`.
const SINGLE_GROUPS: &[Group] = &[
    (
        "insert",
        "internity_compare/insert",
        &[
            "internity",
            "internity-threaded",
            "lasso",
            "string-interner",
            "symbol_table",
            "ustr",
            "string_cache",
        ],
    ),
    (
        "reuse",
        "internity_compare/reuse",
        &[
            "internity",
            "internity-threaded",
            "lasso",
            "string-interner",
            "symbol_table",
            "ustr",
            "string_cache",
        ],
    ),
    (
        "lookup",
        "internity_compare/lookup",
        &[
            "internity",
            "internity-frozen",
            "lasso",
            "string-interner",
            "symbol_table",
            "ustr",
            "string_cache",
        ],
    ),
];

/// Concurrent tables. Each row is measured at every entry of `THREAD_COUNTS`,
/// under the criterion id `<group>/<interner>/<threads>`.
const CONCURRENT_GROUPS: &[Group] = &[
    (
        "insert",
        "internity_compare/insert-concurrent",
        &["internity", "lasso-threaded", "symbol_table"],
    ),
    (
        "reuse",
        "internity_compare/reuse-concurrent",
        &["internity", "lasso-threaded", "symbol_table", "ustr", "string_cache"],
    ),
    (
        "lookup",
        "internity_compare/lookup-concurrent",
        &["internity", "lasso-resolver", "symbol_table", "ustr", "string_cache"],
    ),
];

const THREAD_COUNTS: &[&str] = &["1", "2", "4", "8"];

/// Rows that have no repeatable single-threaded criterion timing in this suite
/// because they are backed by a process-global, persistent table.
const CRITERION_OMITTED: &[&str] = &[
    "internity_compare/insert/ustr",
    "internity_compare/insert/string_cache",
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
/// Format: `time:   [<low> <unit> <median> <unit> <high> <unit>]`. Change-detection
/// lines (`time: [-3% ...]`, three tokens, no units) are ignored.
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

/// True for a `group/variant` identifier. The variant may contain `-`
/// (e.g. `string-interner`) and further `/` segments (e.g. `internity/8`).
/// "Benchmarking foo/bar" progress lines are rejected by the whitespace check.
fn is_bench_name(s: &str) -> bool {
    if s.is_empty() || s.contains(':') || s.contains(char::is_whitespace) {
        return false;
    }
    let Some((g, v)) = s.split_once('/') else {
        return false;
    };
    if g.is_empty() || v.is_empty() {
        return false;
    }
    let id = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    g.chars().all(id) && v.chars().all(|c| id(c) || c == '/')
}

/// Parse a criterion log and return `{group/variant: median_ns}`.
///
/// The identifier appears either on its own line just before the `time:` line
/// (long names) or inline before `time:` (short names). Both are handled.
fn parse_criterion(text: &str) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(t_idx) = line.find("time:") {
            let head = line[..t_idx].trim();
            let name = if is_bench_name(head) {
                Some(head.to_string())
            } else {
                pending.take()
            };
            if let (Some(name), Some(t)) = (name, parse_time_line(line)) {
                out.push((name, t));
            }
            continue;
        }
        if is_bench_name(trimmed) {
            pending = Some(trimmed.to_string());
        }
    }
    out
}

fn lookup_time(crit: &[(String, f64)], key: &str) -> Option<f64> {
    crit.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

fn validate_criterion(crit: &[(String, f64)]) -> Result<(), BoxErr> {
    let mut missing = Vec::new();
    for (_, group, rows) in SINGLE_GROUPS {
        for row in *rows {
            let key = format!("{group}/{row}");
            if !CRITERION_OMITTED.contains(&key.as_str()) && lookup_time(crit, &key).is_none() {
                missing.push(key);
            }
        }
    }
    for (_, group, rows) in CONCURRENT_GROUPS {
        for row in *rows {
            for threads in THREAD_COUNTS {
                let key = format!("{group}/{row}/{threads}");
                if lookup_time(crit, &key).is_none() {
                    missing.push(key);
                }
            }
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("missing required criterion timings: {}", missing.join(", ")).into())
    }
}

fn fmt_ns(ns: Option<f64>) -> String {
    match ns {
        None => "—".into(),
        Some(ns) if ns < 1000.0 => format!("{ns:.0} ns"),
        Some(ns) if ns < 1e6 => format!("{:.2} µs", ns / 1e3),
        Some(ns) => format!("{:.2} ms", ns / 1e6),
    }
}

/// Formats one row's speed relative to the `internity` reference in the same
/// table: `+42%` means this row is 42% slower than internity; `-30%` faster.
fn fmt_delta(this: Option<f64>, reference: Option<f64>, is_ref: bool) -> String {
    if is_ref {
        return "ref".into();
    }
    match (this, reference) {
        (Some(t), Some(r)) if r != 0.0 => format!("{:+.1}%", (t / r - 1.0) * 100.0),
        _ => "—".into(),
    }
}

fn header() -> String {
    let mut out = String::new();
    out.push_str("# internity Performance Report\n\n");
    out.push_str(
        "Generated by [`scripts/perf_report.rs`](../scripts/perf_report.rs). Re-run it to \
         refresh these numbers.\n\n",
    );
    out.push_str(
        "All timing figures are wall-clock medians measured by Criterion. They are\n\
         machine-dependent: the ratios between rows are the durable signal, not the\n\
         absolute values.\n\n",
    );
    out.push_str(
        "This report is a curated set of customer-facing scenarios. The crate also carries\n\
         internal Callgrind instruction-count benches (`benches/internity_compare_cg.rs`)\n\
         used for optimization work, which are not published here; run them with\n\
         `cargo bench --bench internity_compare_cg`.\n\n",
    );
    out.push_str(
        "**Workload:** a corpus of ≈6000 identifier-like strings, exercised through three \
         customer operations — `insert` (interning a string for the first time), `reuse` \
         (interning a string that is already interned) and `lookup` (resolving a handle back \
         to its string).\n\n",
    );
    out.push_str(
        "**Methodology:** every timed region measures only insert/dedupe or lookup — \
         benchmark setup and result destruction are kept outside the elapsed-time boundary. \
         Refcounted `string_cache` atoms are retained across dedupe/lookup rounds so hits \
         are measured against populated dynamic entries. `lookup` uses the same random order \
         for all crates. The concurrent flavors run the operation on `n` threads and are \
         barrier-timed, so thread spawn/join is excluded and only the parallel work is \
         counted.  \n",
    );
    out.push_str(
        "**Δ vs internity:** `+x%` = that row is x% slower than internity on the same \
         workload; `-x%` = faster.  \n",
    );
    out.push_str(
        "The process-global, cache-backed rows (`ustr`, `string_cache`) keep their table \
         alive for the whole process, so they have no repeatable first-time `insert` timing \
         and are absent from the single-threaded `insert` table.\n\n",
    );
    out
}

fn render_single(out: &mut String, crit: &[(String, f64)]) {
    out.push_str("## Single-threaded interning\n\n");
    out.push_str(
        "One table per operation, comparing internity against every other interner measured \
         for that operation on a single thread.\n\n",
    );

    for (op, group, rows) in SINGLE_GROUPS {
        let _ = writeln!(out, "### `{op}` — single-threaded\n");
        out.push_str("| Interner | Time | Δ vs internity |\n");
        out.push_str("|---|---:|---:|\n");
        let reference = lookup_time(crit, &format!("{group}/internity"));
        for row in *rows {
            let key = format!("{group}/{row}");
            let time = lookup_time(crit, &key);
            if time.is_none() {
                continue;
            }
            let _ = writeln!(
                out,
                "| `{row}` | {} | {} |",
                fmt_ns(time),
                fmt_delta(time, reference, *row == "internity"),
            );
        }
        out.push('\n');
    }
}

/// Render each concurrent operation as a single table: one row per interner, one
/// column per thread count, and a trailing delta at the highest thread count.
fn render_concurrent(out: &mut String, crit: &[(String, f64)]) {
    let top = THREAD_COUNTS
        .last()
        .expect("THREAD_COUNTS is a non-empty compile-time constant");

    out.push_str("## Concurrent scaling\n\n");
    out.push_str(
        "One table per operation, with a column per thread count. Each cell is the \
         wall-clock median of the whole parallel phase at that thread count, so the total \
         work grows with the thread count and the numbers show how each interner holds up \
         under contention rather than a per-thread speedup. The trailing column is the \
         delta against internity at the highest thread count, which is where the designs \
         separate most; per-thread-count deltas are omitted to keep the tables readable.\n\n",
    );

    for (op, group, rows) in CONCURRENT_GROUPS {
        let _ = writeln!(out, "### `{op}` — concurrent\n");
        out.push_str("| Interner |");
        for threads in THREAD_COUNTS {
            let _ = write!(out, " {threads} thr |");
        }
        let _ = writeln!(out, " Δ vs internity @ {top} thr |");
        out.push_str("|---|");
        for _ in THREAD_COUNTS {
            out.push_str("---:|");
        }
        out.push_str("---:|\n");

        let reference = lookup_time(crit, &format!("{group}/internity/{top}"));
        for row in *rows {
            let _ = write!(out, "| `{row}` |");
            for threads in THREAD_COUNTS {
                let time = lookup_time(crit, &format!("{group}/{row}/{threads}"));
                let _ = write!(out, " {} |", fmt_ns(time));
            }
            let top_time = lookup_time(crit, &format!("{group}/{row}/{top}"));
            let _ = writeln!(
                out,
                " {} |",
                fmt_delta(top_time, reference, *row == "internity")
            );
        }
        out.push('\n');
    }
}

/// Formats the `internity_mem` bench's stdout table into a `## Memory footprint` section.
/// The bench prints an aligned table to stdout; `run_bench` appends cargo's status
/// lines (stderr) after it, so we take from the `Corpus:` line up to (but not
/// including) the first cargo-status line.
fn memory_section(mem_log: &str) -> Result<String, BoxErr> {
    let start = mem_log
        .find("Corpus:")
        .ok_or("memory footprint output did not contain a `Corpus:` section")?;
    let is_cargo_status = |l: &str| {
        let t = l.trim_start();
        ["Compiling", "Finished", "Running", "Blocking", "warning", "error", "note:"]
            .iter()
            .any(|p| t.starts_with(p))
    };
    let mut table = String::new();
    for line in mem_log[start..].lines() {
        if is_cargo_status(line) {
            break;
        }
        table.push_str(line);
        table.push('\n');
    }
    let table = table.trim_end();
    if table.is_empty() {
        return Err("memory footprint section was empty".into());
    }
    Ok(format!(
        "## Memory footprint\n\n\
         Live heap bytes held by each interner, measured with a tracking global \
         allocator over the same corpus (`cargo bench --bench internity_mem`). `insert` is the \
         filled interner; `lookup` is the read structure the lookup benchmark resolves \
         against (the frozen read form where a crate has one). Lower is better.\n\n\
         ```text\n{table}\n```\n"
    ))
}

fn build_report(crit: &[(String, f64)]) -> String {
    let mut out = header();
    render_single(&mut out, crit);
    render_concurrent(&mut out, crit);
    out
}

/// Locate the crate root. With `cargo +nightly -Zscript`, `CARGO_MANIFEST_DIR`
/// is the directory holding this script (`scripts/`); its parent is the crate.
fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts/ always has a parent crate directory")
        .to_path_buf()
}

/// Run `cargo bench --bench <name> -- <extra>` from `cwd`, capturing combined
/// stdout+stderr (criterion writes its summaries to stdout).
fn run_bench(cwd: &Path, bench: &str, extra: &[&str], label: &str) -> Result<String, BoxErr> {
    println!("==> Running {label}");
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(cwd).arg("bench").arg("--bench").arg(bench);
    if !extra.is_empty() {
        cmd.arg("--");
        cmd.args(extra);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to spawn cargo bench --bench {bench}: {e}"))?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        let _ = std::io::stderr().write_all(combined.as_bytes());
        return Err(format!("cargo bench --bench {bench} failed with status {}", out.status).into());
    }
    Ok(combined)
}

fn run(args: &Args) -> Result<(), BoxErr> {
    let crate_dir = crate_root();

    // The published report's prose is written against the default corpus, and the
    // memory and timing benches must measure the *same* corpus. A non-default
    // `INTERNITY_BENCH_CORPUS_SIZE` would silently desynchronise the prose from the
    // numbers, so reject it here and fail before writing anything.
    if let Ok(value) = std::env::var("INTERNITY_BENCH_CORPUS_SIZE") {
        return Err(format!(
            "INTERNITY_BENCH_CORPUS_SIZE is set to {value:?}; report generation requires the \
             default corpus so the timing, memory, and prose sections stay consistent. \
             Unset it and rerun."
        )
        .into());
    }

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

    let crit_log = run_bench(
        &crate_dir,
        "internity_compare",
        &crit_args,
        &format!("criterion internity_compare: {samples} samples, {meas}s measurement"),
    )?;
    let mem_log = run_bench(&crate_dir, "internity_mem", &[], "memory footprint")?;

    println!("==> Building docs/PERF.md");
    let crit = parse_criterion(&crit_log);
    let mem = memory_section(&mem_log)?;

    validate_criterion(&crit)?;

    let report = build_report(&crit);
    let report = format!("{report}{mem}");
    let docs_dir = crate_dir.join("docs");
    fs::create_dir_all(&docs_dir).map_err(|e| format!("creating {}: {e}", docs_dir.display()))?;
    let out_path = docs_dir.join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| format!("writing {}: {e}", out_path.display()))?;

    println!("Wrote {} ({} criterion benches)", out_path.display(), crit.len());
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
