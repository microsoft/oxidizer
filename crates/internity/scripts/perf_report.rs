#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
---

//! Run the criterion + gungraun benchmark suites and rebuild `docs/PERF.md`.
//!
//! On Linux the script runs both criterion (wall-clock) and gungraun
//! (Callgrind instruction counts) suites; gungraun requires `valgrind` to be
//! installed. When valgrind is unavailable the gungraun columns show "—".
//!
//! Usage:
//!   `scripts/perf_report.rs`                                    — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                             — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`  — custom criterion settings
//!
//! internity has a single criterion bench binary (`compare`, with groups
//! `insert`/`reuse`/`lookup` and their `*-concurrent` counterparts) and a single
//! gungraun binary (`counts`, functions `insert_*` / `reuse_*` / `lookup_*`). The
//! `GROUPS` table below maps each criterion `<group>/<variant>` to its gungraun
//! counterpart (or `None` for multi-threaded / competitor rows, which have no
//! instruction-count column). Every benchmark measures only insert/dedupe or
//! lookup — interner construction and drop are excluded from the timed region.

use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::Parser;

type BoxErr = Box<dyn Error>;

/// Run the criterion + gungraun benchmark suites and rebuild `docs/PERF.md`.
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

    /// Force-skip the gungraun (Callgrind) benches even when `valgrind` is
    /// available.
    #[arg(long)]
    no_gungraun: bool,
}

/// `(criterion_variant, Some(gungraun_fn) | None)`.
type Variant = (&'static str, Option<&'static str>);

/// `(group_name, variants_in_table_order)`.
type Group = (&'static str, &'static [Variant]);

/// Ordered (group, variants). The criterion variant name is the string passed
/// to `g.bench_function(...)` / `BenchmarkId::new(...)` in `benches/compare.rs`;
/// the gungraun function name is the `fn` in `benches/counts.rs`. Competitor
/// rows have no gungraun counterpart (`None`) and show "—".
const GROUPS: &[Group] = &[
    (
        "insert",
        &[
            ("internity", Some("insert_internity")),
            ("internity-threaded", Some("insert_internity_threaded")),
            ("lasso", Some("insert_lasso")),
            ("string-interner", Some("insert_string_interner")),
            ("symbol_table", Some("insert_symbol_table")),
            ("ustr", Some("insert_ustr")),
            ("string_cache", Some("insert_string_cache")),
        ],
    ),
    (
        "insert-concurrent",
        &[
            ("internity/1", None),
            ("internity/2", None),
            ("internity/4", None),
            ("internity/8", None),
            ("lasso-threaded/1", None),
            ("lasso-threaded/2", None),
            ("lasso-threaded/4", None),
            ("lasso-threaded/8", None),
            ("symbol_table/1", None),
            ("symbol_table/2", None),
            ("symbol_table/4", None),
            ("symbol_table/8", None),
        ],
    ),
    (
        "reuse",
        &[
            ("internity", Some("reuse_internity")),
            ("internity-threaded", Some("reuse_internity_threaded")),
            ("lasso", Some("reuse_lasso")),
            ("string-interner", Some("reuse_string_interner")),
            ("symbol_table", Some("reuse_symbol_table")),
            ("ustr", Some("reuse_ustr")),
            ("string_cache", Some("reuse_string_cache")),
        ],
    ),
    (
        "reuse-concurrent",
        &[
            ("internity/1", None),
            ("internity/2", None),
            ("internity/4", None),
            ("internity/8", None),
            ("lasso-threaded/1", None),
            ("lasso-threaded/2", None),
            ("lasso-threaded/4", None),
            ("lasso-threaded/8", None),
            ("symbol_table/1", None),
            ("symbol_table/2", None),
            ("symbol_table/4", None),
            ("symbol_table/8", None),
            ("ustr/1", None),
            ("ustr/2", None),
            ("ustr/4", None),
            ("ustr/8", None),
            ("string_cache/1", None),
            ("string_cache/2", None),
            ("string_cache/4", None),
            ("string_cache/8", None),
        ],
    ),
    (
        "lookup",
        &[
            ("internity", Some("lookup_internity")),
            ("internity-frozen", Some("lookup_internity_frozen")),
            ("lasso", Some("lookup_lasso")),
            ("string-interner", Some("lookup_string_interner")),
            ("symbol_table", Some("lookup_symbol_table")),
            ("ustr", Some("lookup_ustr")),
            ("string_cache", Some("lookup_string_cache")),
        ],
    ),
    (
        "lookup-concurrent",
        &[
            ("internity/1", None),
            ("internity/2", None),
            ("internity/4", None),
            ("internity/8", None),
            ("lasso-threaded/1", None),
            ("lasso-threaded/2", None),
            ("lasso-threaded/4", None),
            ("lasso-threaded/8", None),
            ("symbol_table/1", None),
            ("symbol_table/2", None),
            ("symbol_table/4", None),
            ("symbol_table/8", None),
            ("ustr/1", None),
            ("ustr/2", None),
            ("ustr/4", None),
            ("ustr/8", None),
            ("string_cache/1", None),
            ("string_cache/2", None),
            ("string_cache/4", None),
            ("string_cache/8", None),
        ],
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

/// One gungraun benchmark's parsed metrics.
struct GungEntry {
    name: String,
    metrics: Vec<(String, u64)>,
}

/// Parse the gungraun / iai-callgrind text output. Per-bench headers look like
/// `<prefix>::<module>::<fn> <case>:(...)`; metric lines follow as
/// `  <Metric>: <value>|...`.
fn parse_gungraun(text: &str, prefix: &str) -> Vec<GungEntry> {
    let mut out: Vec<GungEntry> = Vec::new();
    let mut cur: Option<GungEntry> = None;
    let header_prefix = format!("{prefix}::");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&header_prefix) {
            if let Some(after_mod) = rest.find("::") {
                let after = &rest[after_mod + 2..];
                let fn_name = match after.find(char::is_whitespace) {
                    Some(sp) => &after[..sp],
                    None => after,
                };
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
            let trimmed = line.trim_start();
            if let Some(colon) = trimmed.find(':') {
                let key = &trimmed[..colon];
                if matches!(key, "Instructions" | "L1 Hits" | "LL Hits" | "RAM Hits") {
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

fn gung_instructions(g: &[GungEntry], name: Option<&str>) -> Option<u64> {
    gung_metric(g, name?, "Instructions")
}

fn gung_mem(g: &[GungEntry], name: Option<&str>) -> Option<u64> {
    let name = name?;
    let l1 = gung_metric(g, name, "L1 Hits")?;
    let ll = gung_metric(g, name, "LL Hits")?;
    let ram = gung_metric(g, name, "RAM Hits")?;
    Some(l1 + ll + ram)
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
                out.push_str(std::str::from_utf8(chunk).expect("ASCII digits"));
            }
            out
        }
    }
}

/// The `internity` variant to compare against for a given variant: the same
/// thread-count suffix for concurrent rows, else the plain `internity` row.
fn internity_ref(variant: &str) -> String {
    match variant.split_once('/') {
        Some((_, suffix)) => format!("internity/{suffix}"),
        None => "internity".to_string(),
    }
}

/// Formats one variant's speed relative to the `internity` reference in the same
/// group: `+42%` means this variant is 42% slower than internity; `-30%` faster.
fn fmt_delta(this: Option<f64>, reference: Option<f64>, is_ref: bool) -> String {
    if is_ref {
        return "ref".into();
    }
    match (this, reference) {
        (Some(t), Some(r)) if r != 0.0 => format!("{:+.1}%", (t / r - 1.0) * 100.0),
        _ => "—".into(),
    }
}

fn build_report(crit: &[(String, f64)], gung: &[GungEntry]) -> String {
    let mut out = String::new();
    out.push_str("# internity Performance Report\n\n");
    out.push_str("Generated by `scripts/perf_report.rs`. Head-to-head against the main Rust\n");
    out.push_str("interners.\n\n");
    out.push_str(
        "- `cargo bench --bench compare` — criterion wall-clock timings for three \
         operations (`insert`, `reuse`, `lookup`), each in a **single-threaded** \
         flavor and **multi-threaded** flavors at 1/2/4/8 threads.\n",
    );
    out.push_str(
        "- `cargo bench --bench counts` — gungraun / Callgrind instruction-precise \
         counts for internity's hot paths (single-op, i.e. the single-threaded view).\n",
    );
    out.push_str(
        "- `cargo bench --bench mem` — live heap footprint of each interner \
         (tracking global allocator).\n\n",
    );
    out.push_str(
        "**Methodology:** every timed region measures only insert/dedupe or lookup — \
         interner construction and drop are excluded. `lookup` uses the same random \
         order for all crates. Multi-threaded flavors run the op on `n` threads, \
         barrier-timed so only the parallel work is counted (no thread spawn/join). \
         Corpus ≈ 6000 identifier-like strings.  \n",
    );
    out.push_str(
        "**Δ vs internity:** `+x%` = that row is x% slower than internity on the same \
         workload; `-x%` = faster. Instruction/memory columns are per single \
         operation (one insert or lookup) and only appear on the single-threaded \
         tables; multi-threaded tables are criterion-only (no instruction-count \
         analog).  \n",
    );
    out.push_str("Mem accesses = L1 + LL + RAM hits (Callgrind D-cache references).  \n");
    out.push_str(
        "The process-global caches (`ustr`, `string_cache`) can't be reset between \
         criterion iterations, so they have no repeatable single-threaded `insert` \
         (time shows \"—\"); their single-insert cost is still measured \
         deterministically in the Instructions column.\n\n",
    );

    for (group, variants) in GROUPS {
        // Concurrent groups are rendered as one table per thread count.
        if group.ends_with("-concurrent") {
            render_concurrent(&mut out, group, crit, variants);
            continue;
        }

        let _ = writeln!(out, "## `{group}` — single-threaded\n");
        out.push_str("| Variant | Time | Δ vs internity | Instructions | Mem accesses |\n");
        out.push_str("|---|---:|---:|---:|---:|\n");
        for (variant, gung_name) in *variants {
            let key = format!("{group}/{variant}");
            let t = lookup_time(crit, &key);
            let is_ref = *variant == "internity" || variant.starts_with("internity/");
            let reference = lookup_time(crit, &format!("{group}/{}", internity_ref(variant)));
            let instr = gung_instructions(gung, *gung_name);
            let mem = gung_mem(gung, *gung_name);
            let _ = writeln!(
                out,
                "| `{variant}` | {} | {} | {} | {} |",
                fmt_ns(t),
                fmt_delta(t, reference, is_ref),
                fmt_int(instr),
                fmt_int(mem),
            );
        }
        out.push('\n');
    }

    out
}

/// Formats the `mem` bench's stdout table into a `## Memory footprint` section.
/// The bench prints an aligned table to stdout; `run_bench` appends cargo's status
/// lines (stderr) after it, so we take from the `Corpus:` line up to (but not
/// including) the first cargo-status line.
fn memory_section(mem_log: &str) -> String {
    let start = match mem_log.find("Corpus:") {
        Some(i) => i,
        None => return String::new(),
    };
    let is_cargo_status = |l: &str| {
        let t = l.trim_start();
        ["Compiling", "Finished", "Running", "warning", "error", "note:"]
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
    format!(
        "## Memory footprint\n\n\
         Live heap bytes held by each interner, measured with a tracking global \
         allocator over the same corpus (`cargo bench --bench mem`). `insert` is the \
         filled interner; `lookup` is the read structure the lookup benchmark resolves \
         against (the frozen read form where a crate has one).\n\n\
         ```text\n{table}\n```\n"
    )
}

/// Render a concurrent group as one table per thread count. Variant names are
/// `<crate>/<threads>`; rows are the crates (compared to internity at the same
/// thread count), with a separate table per distinct thread count. `group` is
/// e.g. `insert-concurrent`; its label drops the `-concurrent` suffix.
fn render_concurrent(out: &mut String, group: &str, crit: &[(String, f64)], variants: &[Variant]) {
    let op = group.strip_suffix("-concurrent").unwrap_or(group);

    // Collect crates and thread counts in first-seen order.
    let mut crates: Vec<&str> = Vec::new();
    let mut threads: Vec<&str> = Vec::new();
    for (variant, _) in variants {
        if let Some((c, t)) = variant.split_once('/') {
            if !crates.contains(&c) {
                crates.push(c);
            }
            if !threads.contains(&t) {
                threads.push(t);
            }
        }
    }

    for t in &threads {
        let label = if *t == "1" {
            "1 thread".to_string()
        } else {
            format!("{t} threads")
        };
        let _ = writeln!(out, "## `{op}` — multi-threaded, {label}\n");
        out.push_str("| Interner | Time | Δ vs internity |\n");
        out.push_str("|---|---:|---:|\n");
        let reference = lookup_time(crit, &format!("{group}/internity/{t}"));
        for c in &crates {
            let time = lookup_time(crit, &format!("{group}/{c}/{t}"));
            let is_ref = *c == "internity";
            let _ = writeln!(
                out,
                "| `{c}` | {} | {} |",
                fmt_ns(time),
                fmt_delta(time, reference, is_ref),
            );
        }
        out.push('\n');
    }
}

/// Locate the crate root. With `cargo +nightly -Zscript`, `CARGO_MANIFEST_DIR`
/// is the directory holding this script (`scripts/`); its parent is the crate.
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

    let run_gungraun = if args.no_gungraun {
        eprintln!("note: --no-gungraun set; gungraun columns will show \"—\".");
        false
    } else if have_valgrind() {
        true
    } else {
        eprintln!(
            "note: valgrind not found; skipping gungraun benches. \
             Instruction/memory columns will show \"—\". Rerun with valgrind installed \
             for the full report."
        );
        false
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

    let crit_log = run_bench(
        &crate_dir,
        "compare",
        &crit_args,
        &format!("criterion compare: {samples} samples, {meas}s measurement"),
    )?;
    let gung_log = if run_gungraun {
        run_bench(&crate_dir, "counts", &[], "gungraun counts")?
    } else {
        String::new()
    };
    let mem_log = run_bench(&crate_dir, "mem", &[], "memory footprint")?;

    println!("==> Building docs/PERF.md");
    let crit = parse_criterion(&crit_log);
    let gung = parse_gungraun(&gung_log, "counts");

    // Warn about any GROUPS entry that produced no criterion timing, except the
    // global caches which intentionally have no repeatable wall-clock fill.
    const CRITERION_OMITTED: &[&str] = &["insert/ustr", "insert/string_cache"];
    for (group, variants) in GROUPS {
        for (variant, _) in *variants {
            let key = format!("{group}/{variant}");
            if lookup_time(&crit, &key).is_none() && !CRITERION_OMITTED.contains(&key.as_str()) {
                eprintln!("warning: no criterion timing parsed for {key}");
            }
        }
    }

    let report = build_report(&crit, &gung);
    let report = format!("{report}{}", memory_section(&mem_log));
    let docs_dir = crate_dir.join("docs");
    fs::create_dir_all(&docs_dir).map_err(|e| format!("creating {}: {e}", docs_dir.display()))?;
    let out_path = docs_dir.join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| format!("writing {}: {e}", out_path.display()))?;

    println!(
        "Wrote {} ({} criterion, {} gungraun benches)",
        out_path.display(),
        crit.len(),
        gung.len(),
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
