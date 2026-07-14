#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
ohno = { path = "../../ohno", features = ["app-err"] }
routerama = { path = "../../routerama", default-features = false, features = ["build"] }
http_path_template = { path = "../../http_path_template" }
prettyplease = "0.2"
syn = { version = "2", features = ["full", "parsing"] }
---

//! Run the criterion + gungraun "compare routers" suites and rebuild
//! `docs/PERF.md`.
//!
//! On Linux the script runs both the criterion (wall-clock) and gungraun
//! (Callgrind instruction counts) suites; gungraun requires `valgrind`. On
//! Windows valgrind is unavailable, so only the criterion suite runs and the
//! gungraun columns show "—".
//!
//! Usage:
//!   `scripts/perf_report.rs`                                    — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                             — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`  — custom criterion settings
//!   `scripts/perf_report.rs --no-gungraun`                      — criterion only
//!
//! The two suites are aligned 1:1: each criterion `compare_routers/<variant>`
//! corresponds to a gungraun `compare_routers::<variant>`. `VARIANTS` lists the
//! routers in report order; update it if a router is added or removed from the
//! benches.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::Parser;
use ohno::{AppError, app_err, bail};

/// Run the routerama benchmark suites and rebuild `docs/PERF.md`.
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

    /// Skip the gungraun (Callgrind) suite; the instruction-count columns show "—".
    #[arg(long)]
    no_gungraun: bool,

    /// Regenerate the committed benchmark router (`benches/common/generated_router.rs`)
    /// from the route table and exit, without running any benchmarks. Run this
    /// after editing `benches/common/routes_data.rs`.
    #[arg(long)]
    regenerate_router: bool,
}

// The benchmark route table (`ROUTES`, `LOOKUPS`), shared with the benches.
include!("../benches/common/routes_data.rs");

/// The benchmark group name shared by both suites.
const GROUP: &str = "compare_routers";

/// The routers compared, in report order. Each name is both the criterion
/// variant (`compare_routers/<name>`) and the gungraun function
/// (`compare_routers::<name>`). `routerama_static` is the build-time generated
/// router and `routerama_dynamic` the runtime one built from the same table; the
/// rest are third-party runtime routers built in a (non-measured) setup step.
const VARIANTS: &[&str] = &[
    "routerama_static",
    "routerama_dynamic",
    "matchit",
    "path_tree",
    "actix_router",
    "regex",
    "route_recognizer",
    "routefinder",
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

/// True for a non-empty, non-indented `group/variant` identifier (the shape
/// criterion emits before `time:`). Progress lines with colons or internal
/// whitespace are rejected.
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
    let id_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    g.chars().all(id_char) && v.chars().all(id_char)
}

/// Parse a criterion log into `{group/variant: median_ns}` pairs. Criterion
/// writes the bench identifier either on its own line just before the `time:`
/// line (long names) or inline before `time:` (short names); both are handled.
fn parse_criterion(text: &str) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(t_idx) = line.find("time:") {
            let head = line[..t_idx].trim();
            let name_inline = if is_bench_name(head) { Some(head.to_owned()) } else { None };
            let name = name_inline.or_else(|| pending.take());
            if let (Some(name), Some(t)) = (name, parse_time_line(line)) {
                out.push((name, t));
            }
            continue;
        }
        if is_bench_name(trimmed) {
            pending = Some(trimmed.to_owned());
        }
    }
    out
}

fn lookup_time(crit: &[(String, f64)], variant: &str) -> Option<f64> {
    let key = format!("{GROUP}/{variant}");
    crit.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// One gungraun benchmark's parsed metrics.
struct GungEntry {
    name: String,
    metrics: Vec<(String, u64)>,
}

/// Parse the gungraun (iai-callgrind) text output. Per-bench headers look like
/// `gungraun_routers::compare_routers::<fn>` (plain benches) or
/// `…::<fn> run:(<args>)` (`#[bench::run(...)]` benches); metric lines follow as
/// `  <Metric>: <value>|…`.
fn parse_gungraun(text: &str) -> Vec<GungEntry> {
    let mut out: Vec<GungEntry> = Vec::new();
    let mut cur: Option<GungEntry> = None;
    let header_prefix = "gungraun_routers::";
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(header_prefix) {
            if let Some(after_mod) = rest.find("::") {
                let after = &rest[after_mod + 2..];
                let fn_name = match after.find(char::is_whitespace) {
                    Some(sp) => &after[..sp],
                    None => after,
                };
                let valid = !fn_name.is_empty() && fn_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if valid {
                    if let Some(prev) = cur.take() {
                        out.push(prev);
                    }
                    cur = Some(GungEntry {
                        name: fn_name.to_owned(),
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
                if matches!(key, "Instructions" | "L1 Hits" | "LL Hits" | "RAM Hits" | "Bcm") {
                    let after = trimmed[colon + 1..].trim_start();
                    let num: String = after.chars().take_while(char::is_ascii_digit).collect();
                    if let Ok(v) = num.parse::<u64>() {
                        entry.metrics.push((key.to_owned(), v));
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

fn build_report(crit: &[(String, f64)], gung: &[GungEntry]) -> String {
    let mut out = String::new();
    out.push_str("# Routerama Performance Report\n\n");
    out.push_str("Generated by `scripts/perf_report.rs`:\n");
    out.push_str("- `cargo bench --bench criterion_routers` — criterion wall-clock timings.\n");
    out.push_str("- `cargo bench --bench gungraun_routers` — Callgrind instruction-precise counts.\n\n");
    out.push_str(
        "**Workload:** one full sweep of the shared request-path lookups (see \
         `benches/common/routes_data.rs`) against each router. Every router is \
         built from the same route table (literal segments plus single-segment \
         `{var}` parameters — the common subset all of them express) in a setup \
         step that is excluded from the measured region; `routerama_static`'s \
         router is the build-time generated `resolve`, so it has no construction \
         cost, while `routerama_dynamic` is the runtime router built from the \
         same table.  \n",
    );
    out.push_str(
        "**Apples-to-apples:** every router is driven to the same end state — the \
         HTTP method (verb) validated against the request and every captured path \
         variable extracted into a `&str`. `routerama` reaches this in one step \
         (a typed enum variant with the method already matched); the third-party \
         routers only *select* a route, so the harness explicitly checks the \
         method and pulls out each parameter afterwards. `regex` selects the \
         winner with a `RegexSet` and then re-scans it with the winning `Regex` \
         to capture (two passes), so it does structurally more work — read it as \
         an upper bound for a regex-based router reaching the same end state.  \n",
    );
    out.push_str(
        "Criterion median is reported (default 30 samples, 1 s warm-up, 2 s \
         measurement; override with `--samples` / `--measurement-time` / \
         `--warm-up-time`).  \n",
    );
    out.push_str("Instructions, branch misses, and memory accesses are per full lookup sweep.  \n");
    out.push_str("Memory accesses = L1 Hits + LL Hits + RAM Hits (Callgrind D-cache references).\n\n");

    let _ = writeln!(out, "## `{GROUP}`\n");
    out.push_str("| Resolver | Time (criterion) | Instructions | Branch misses | Mem accesses |\n");
    out.push_str("|---|---:|---:|---:|---:|\n");
    for variant in VARIANTS {
        let t = lookup_time(crit, variant);
        let instr = gung_metric(gung, variant, "Instructions");
        let bcm = gung_metric(gung, variant, "Bcm");
        let mem = {
            let l1 = gung_metric(gung, variant, "L1 Hits");
            let ll = gung_metric(gung, variant, "LL Hits");
            let ram = gung_metric(gung, variant, "RAM Hits");
            match (l1, ll, ram) {
                (Some(l1), Some(ll), Some(ram)) => Some(l1 + ll + ram),
                _ => None,
            }
        };
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

    out.push_str("## routerama vs. the field (instructions)\n\n");
    out.push_str(
        "Instruction count of each router's full lookup sweep relative to \
         `routerama_static`'s generated router (lower is better; \
         `routerama_static` = 1.00×).\n\n",
    );
    out.push_str("| Resolver | Instructions | Relative to routerama_static |\n");
    out.push_str("|---|---:|---:|\n");
    let base = gung_metric(gung, "routerama_static", "Instructions");
    for variant in VARIANTS {
        let instr = gung_metric(gung, variant, "Instructions");
        let rel = match (instr, base) {
            (Some(i), Some(b)) if b != 0 => {
                #[expect(clippy::cast_precision_loss, reason = "instruction counts stay well under 2^53")]
                let ratio = i as f64 / b as f64;
                format!("{ratio:.2}×")
            }
            _ => "—".into(),
        };
        let _ = writeln!(out, "| `{variant}` | {} | {} |", fmt_int(instr), rel);
    }
    out.push('\n');
    out
}

/// The `routerama` crate root (the parent of this script's `scripts/` directory).
fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts/ always has a parent crate directory")
        .to_path_buf()
}

/// The header prepended to the generated router fixture.
const ROUTER_HEADER: &str = "\
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// GENERATED FILE — do not edit by hand.
//
// This is the `routerama` resolver for the benchmark route table in
// `common/routes_data.rs`, produced by `routerama::Generator`. It is
// committed (rather than emitted from a `build.rs`) so the benchmarks build
// without a build script or codegen step. Regenerate it after editing
// `routes_data.rs` with `scripts/perf_report.rs --regenerate-router`; the
// `generated_router_matches_the_route_table` test fails if it drifts.

";

/// Regenerate `benches/common/generated_router.rs` from the `ROUTES` table using
/// `routerama::Generator`, so the committed benchmark router stays in sync
/// with the route table.
fn regenerate_router(crate_dir: &Path) -> Result<(), AppError> {
    use http_path_template::{Grammar, PathTemplate};
    use routerama::{Generator, HttpMethod, Route};

    let _ = LOOKUPS;
    let rules = ROUTES.iter().map(|(name, template)| {
        let parsed = PathTemplate::parse(template, Grammar::default()).unwrap_or_else(|e| panic!("route template {template:?} is invalid: {e}"));
        Route::new(*name, HttpMethod::Get, parsed)
    });
    let mut generator = Generator::new();
    generator.add_all(rules);
    let tokens = generator.generate();
    let file: syn::File = syn::parse2(tokens).map_err(|e| app_err!("generated router is not valid Rust: {e}"))?;
    let body = prettyplease::unparse(&file);

    let out_path = crate_dir.join("benches").join("common").join("generated_router.rs");
    fs::write(&out_path, format!("{ROUTER_HEADER}{body}")).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;
    println!("Regenerated {}", out_path.display());
    Ok(())
}

/// Whether `valgrind` is available on PATH.
fn have_valgrind() -> bool {
    Command::new("valgrind")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run `cargo bench --bench <name> -- <args>` from `cwd`, capturing combined
/// stdout+stderr (criterion writes summaries to stdout; gungraun to stderr).
fn run_bench(cwd: &Path, bench: &str, extra: &[&str], label: &str) -> Result<String, AppError> {
    println!("==> Running {label}");
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(cwd).arg("bench").arg("--bench").arg(bench);
    // The `routerama_dynamic` variant lives behind the `dynamic` feature in both
    // bench binaries; enable it so the report includes the runtime router.
    cmd.arg("--features").arg("dynamic");
    if !extra.is_empty() {
        cmd.arg("--");
        cmd.args(extra);
    }
    let out = cmd.output().map_err(|e| app_err!("failed to spawn cargo bench --bench {bench}: {e}"))?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        let _ = std::io::stderr().write_all(combined.as_bytes());
        bail!("cargo bench --bench {bench} failed with status {}", out.status);
    }
    Ok(combined)
}

fn run(args: &Args) -> Result<(), AppError> {
    let crate_dir = crate_root();

    if args.regenerate_router {
        return regenerate_router(&crate_dir);
    }

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
    } else if have_valgrind() {
        true
    } else {
        bail!(
            "valgrind is required for the gungraun benchmarks; install it or rerun with \
             --no-gungraun to skip them"
        );
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
        "criterion_routers",
        &crit_args,
        &format!("criterion_routers: {samples} samples, {meas}s measurement"),
    )?;
    let gung_log = if run_gungraun {
        run_bench(&crate_dir, "gungraun_routers", &[], "gungraun_routers")?
    } else {
        String::new()
    };

    println!("==> Building docs/PERF.md");
    let crit = parse_criterion(&crit_log);
    let gung = parse_gungraun(&gung_log);

    let report = build_report(&crit, &gung);
    let out_path = crate_dir.join("docs").join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;

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
