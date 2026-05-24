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
const GROUPS: &[Group] = &[(
    "dispatch",
    &[
        ("thunk_void", Some("dispatch_thunk_void")),
        ("thunk_arg_u64", Some("dispatch_thunk_arg_u64")),
        ("spawn_blocking_void", Some("dispatch_spawn_blocking_void")),
        ("spawn_blocking_arg_u64", Some("dispatch_spawn_blocking_arg_u64")),
    ],
)];

/// Direct sync_thunk-vs-`tokio::spawn_blocking` comparisons. Each entry is
/// `(group, sync_thunk_variant, spawn_blocking_variant)`.
const COMPARISONS: &[(&str, &str, &str)] = &[
    ("dispatch", "thunk_void", "spawn_blocking_void"),
    ("dispatch", "thunk_arg_u64", "spawn_blocking_arg_u64"),
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

/// Parse a criterion log and return `{group/variant: median_ns}` aligned
/// 1:1 with `expected` in execution order. In non-TTY mode criterion only
/// writes the per-bench `time:` summary line.
fn parse_criterion(text: &str, expected: &[(&str, &str)]) -> Vec<(String, f64)> {
    let medians: Vec<f64> = text.lines().filter_map(parse_time_line).collect();
    if medians.len() != expected.len() {
        eprintln!(
            "warning: criterion log has {} time entries, expected {}",
            medians.len(),
            expected.len()
        );
    }
    expected
        .iter()
        .zip(medians.iter())
        .map(|((g, v), t)| (format!("{g}/{v}"), *t))
        .collect()
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
/// Per-bench header looks like `<prefix>::<module>::<fn> run ...` and metrics
/// follow as `  <Metric>: <value>|...`.
fn parse_gungraun(text: &str, prefix: &str) -> Vec<GungEntry> {
    let mut out: Vec<GungEntry> = Vec::new();
    let mut cur: Option<GungEntry> = None;
    let header_prefix = format!("{prefix}::");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&header_prefix) {
            if let Some(after_mod) = rest.find("::") {
                let after = &rest[after_mod + 2..];
                if let Some(space) = after.find(' ') {
                    let fn_name = &after[..space];
                    let tail = after[space..].trim_start();
                    if tail.starts_with("run") {
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

fn gung_for(group: &str, variant: &str, g_dispatch: &[GungEntry]) -> Option<u64> {
    for (g, vs) in GROUPS {
        if *g != group {
            continue;
        }
        for (v, gname) in *vs {
            if *v == variant {
                return gname.and_then(|n| gung_metric(g_dispatch, n, "Instructions"));
            }
        }
    }
    None
}

fn build_report(crit: &[(String, f64)], g_dispatch: &[GungEntry]) -> String {
    let mut out = String::new();
    out.push_str("# sync_thunk Performance Report\n\n");
    out.push_str("Generated by `scripts/perf_report.rs`:\n");
    out.push_str(
        "- `cargo bench --bench criterion_dispatch` — \
         criterion wall-clock timings.\n",
    );
    out.push_str(
        "- `cargo bench --bench gungraun_dispatch` — \
         Callgrind instruction-precise counts.\n\n",
    );
    out.push_str(
        "**Workload:** each iteration awaits a single dispatch round-trip \
         (enqueue → worker execute → wake → poll-ready). Criterion batches \
         these inside a `tokio` current-thread runtime; gungraun runs the \
         same batch under Callgrind for instruction-precise counts.  \n",
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
        for (variant, gung_name) in *variants {
            let t = lookup_time(crit, &format!("{group}/{variant}"));
            let instr = gung_name.and_then(|n| gung_metric(g_dispatch, n, "Instructions"));
            let bcm = gung_name.and_then(|n| gung_metric(g_dispatch, n, "Bcm"));
            let mem = gung_name.and_then(|n| {
                let l1 = gung_metric(g_dispatch, n, "L1 Hits")?;
                let ll = gung_metric(g_dispatch, n, "LL Hits")?;
                let ram = gung_metric(g_dispatch, n, "RAM Hits")?;
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

    out.push_str("## sync_thunk vs `tokio::spawn_blocking` Head-to-Head\n\n");
    out.push_str(
        "Direct comparisons of `sync_thunk` versus `tokio::spawn_blocking` \
         on identical workloads. A negative Δ means `sync_thunk` is faster.\n\n",
    );
    out.push_str(
        "| Workload | sync_thunk time | spawn_blocking time | Δ time | \
         sync_thunk instr | spawn_blocking instr | Δ instr |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for (group, mvar, bvar) in COMPARISONS {
        let mt = lookup_time(crit, &format!("{group}/{mvar}"));
        let bt = lookup_time(crit, &format!("{group}/{bvar}"));
        let mi = gung_for(group, mvar, g_dispatch);
        let bi = gung_for(group, bvar, g_dispatch);
        let dt = match (mt, bt) {
            (Some(m), Some(b)) if b != 0.0 => format!("{:+.1}%", (m / b - 1.0) * 100.0),
            _ => "—".into(),
        };
        let di = match (mi, bi) {
            (Some(m), Some(b)) if b != 0 => {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "bench instruction counts stay well under 2^53"
                )]
                let pct = (m as f64 / b as f64 - 1.0) * 100.0;
                format!("{pct:+.1}%")
            }
            _ => "—".into(),
        };
        let _ = writeln!(
            out,
            "| `{group}/{mvar}` vs `{bvar}` | {} | {} | {} | {} | {} | {} |",
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

/// Locate the `sync_thunk` crate root (the directory containing this script's
/// parent). With `cargo +nightly -Zscript`, `CARGO_MANIFEST_DIR` is the
/// directory holding the script file (i.e. `crates/sync_thunk/scripts`).
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

    let crit_dispatch_log = run_bench(
        &crate_dir,
        "criterion_dispatch",
        &crit_args,
        &format!("criterion_dispatch: {samples} samples, {meas}s measurement"),
    )?;
    let gung_dispatch_log = if run_gungraun {
        run_bench(&crate_dir, "gungraun_dispatch", &[], "gungraun_dispatch")?
    } else {
        String::new()
    };

    println!("==> Building docs/PERF.md");

    let dispatch_keys: Vec<(&str, &str)> = GROUPS
        .iter()
        .flat_map(|(g, vs)| vs.iter().map(move |(v, _)| (*g, *v)))
        .collect();

    let crit = parse_criterion(&crit_dispatch_log, &dispatch_keys);
    let g_dispatch = parse_gungraun(&gung_dispatch_log, "gungraun_dispatch");

    let report = build_report(&crit, &g_dispatch);
    let docs_dir = crate_dir.join("docs");
    fs::create_dir_all(&docs_dir).map_err(|e| app_err!("creating {}: {e}", docs_dir.display()))?;
    let out_path = docs_dir.join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;

    println!(
        "Wrote {} ({} criterion, {} gungraun_dispatch benches)",
        out_path.display(),
        crit.len(),
        g_dispatch.len(),
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
