#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2021"
---

//! Runs the benchmark suites and regenerates `docs/PERF.md`. Gungraun supplies
//! instruction counts; criterion and graph churn supply wall-clock results.
//!
//! The gungraun suites need `valgrind` on PATH; pass `--no-gungraun` to skip
//! them. Pass `--no-wallclock` to skip the criterion + graph-churn suites, and
//! `--fast` for a quicker, lower-fidelity criterion run.
//!
//! Usage:
//!   `scripts/perf_report.rs`                — full run
//!   `scripts/perf_report.rs --fast`         — quick criterion settings
//!   `scripts/perf_report.rs --no-gungraun`  — wall-clock only

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::str::from_utf8;

/// Operations per criterion iteration (must match `N` in `benches/criterion/main.rs`).
const N: f64 = 1000.0;

/// The aligned allocation operations: `(name, pretty label)`. `name` is both the
/// criterion variant (`alloc/<name>`) and the gungraun fn (in group `alloc`).
const ALLOC_OPS: &[(&str, &str)] = &[
    ("box_val", "`Box` — `alloc_box`"),
    ("box_with", "`Box` — `alloc_box_with`"),
    ("box_uninit", "`Box` — `alloc_uninit_box`"),
    ("box_unsize", "`Box` — allocate, unsize, and free"),
    ("arc_val", "`Arc` — `alloc_arc`"),
    ("arc_with", "`Arc` — `alloc_arc_with`"),
    ("arc_uninit", "`Arc` — `alloc_uninit_arc`"),
    ("arc_unsize", "`Arc` — allocate, unsize, and free"),
    ("alloc_val", "`Alloc` — `alloc`"),
    ("alloc_with", "`Alloc` — `alloc_with`"),
    ("alloc_uninit", "`Alloc` — `alloc_uninit`"),
    ("rc_val", "`Rc` — `alloc_rc`"),
    ("rc_with", "`Rc` — `alloc_rc_with`"),
    ("rc_uninit", "`Rc` — `alloc_uninit_rc`"),
];

/// The aligned clone operations (criterion group `clone`, gungraun group `clone`).
const CLONE_OPS: &[(&str, &str)] = &[
    ("arc_clone", "`Arc` clone + drop"),
    ("rc_clone", "`Rc` clone + drop"),
];

/// Comparable owning handles that erase a concrete pooled value to `dyn Marker`.
const DYN_BOX_OPS: &[(&str, &str)] = &[
    ("plurality_box", "plurality — `Box<dyn Trait>`"),
    (
        "infinity_pinned",
        "infinity-pool — `PinnedPool` / `PooledMut<dyn Trait>`",
    ),
    (
        "infinity_local_pinned",
        "infinity-pool — `LocalPinnedPool` / `LocalPooledMut<dyn Trait>`",
    ),
    (
        "infinity_blind",
        "infinity-pool — `BlindPool` / `BlindPooledMut<dyn Trait>` (heterogeneous)",
    ),
    (
        "infinity_local_blind",
        "infinity-pool — `LocalBlindPool` / `LocalBlindPooledMut<dyn Trait>` (heterogeneous)",
    ),
    ("std_box", "standard library — `Box<dyn Trait>`"),
];

/// Pretty labels for the cross-crate `pool_comparison` benchmark fns.
const COMPARISON_LABELS: &[(&str, &str)] = &[
    ("plurality_box", "plurality — `Box`"),
    ("plurality_alloc", "plurality — `Alloc`"),
    ("slab_insert_remove", "slab"),
    ("sharded_slab_insert_remove", "sharded-slab"),
    ("slotmap_insert_remove", "slotmap"),
    ("object_pool_pull", "object-pool"),
    ("opool_get", "opool"),
    ("deadpool_get", "deadpool"),
    ("infinity_pinned", "infinity-pool — `PinnedPool`"),
    ("infinity_raw", "infinity-pool — `RawPinnedPool`"),
];

/// One parsed gungraun benchmark.
struct Gung {
    func: String,
    instructions: Option<u64>,
    mem: Option<u64>,
    cycles: Option<u64>,
}

fn main() -> ExitCode {
    let mut no_gungraun = false;
    let mut no_wallclock = false;
    let mut fast = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--no-gungraun" => no_gungraun = true,
            "--no-wallclock" => no_wallclock = true,
            "--fast" => fast = true,
            "-h" | "--help" => {
                println!("usage: scripts/perf_report.rs [--fast] [--no-gungraun] [--no-wallclock]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    match run(no_gungraun, no_wallclock, fast) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(no_gungraun: bool, no_wallclock: bool, fast: bool) -> Result<(), String> {
    let crate_dir = crate_root();

    let run_gungraun = if no_gungraun {
        eprintln!("note: --no-gungraun set; Callgrind columns will show \"—\".");
        false
    } else if !have_valgrind() {
        return Err(
            "valgrind is required for the gungraun benchmarks; install it or rerun \
             with --no-gungraun to skip them"
                .into(),
        );
    } else {
        true
    };

    let (gung_log, comparison_log) = if run_gungraun {
        (
            run_bench(&crate_dir, "gungraun", &[], "gungraun")?,
            run_bench(&crate_dir, "pool_comparison", &[], "gungraun: pool_comparison")?,
        )
    } else {
        (String::new(), String::new())
    };

    let (crit_log, graph_log) = if no_wallclock {
        eprintln!("note: --no-wallclock set; wall-clock columns/sections omitted.");
        (String::new(), String::new())
    } else {
        let (warm, meas, samples) = if fast { ("0.5", "1", "20") } else { ("1", "2", "50") };
        let crit_args = [
            "--warm-up-time", warm, "--measurement-time", meas, "--sample-size", samples,
        ];
        (
            run_bench(&crate_dir, "criterion", &crit_args, "wall-clock: criterion")?,
            run_bench(&crate_dir, "graph_churn", &[], "wall-clock: graph_churn")?,
        )
    };

    println!("==> Building docs/PERF.md");
    let gung = parse_gungraun(&gung_log, "gungraun");
    let cmp = parse_gungraun(&comparison_log, "pool_comparison");
    let crit = parse_criterion(&crit_log);
    let mut report = build_report(&gung, &gung, &cmp, &crit, &graph_log);
    report.truncate(report.trim_end().len());
    report.push('\n');

    let docs = crate_dir.join("docs");
    fs::create_dir_all(&docs).map_err(|e| format!("creating {}: {e}", docs.display()))?;
    let out_path = docs.join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| format!("writing {}: {e}", out_path.display()))?;
    println!(
        "==> Done. Wrote {} ({} gungraun, {} comparison, {} criterion benches).",
        out_path.display(),
        gung.len(),
        cmp.len(),
        crit.len()
    );
    Ok(())
}

// ── running benches ──────────────────────────────────────────────────────

/// The crate root is the parent of this script's `scripts/` directory.
fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts/ always has a parent crate directory")
        .to_path_buf()
}

fn have_valgrind() -> bool {
    Command::new("valgrind")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run_bench(cwd: &Path, bench: &str, extra: &[&str], label: &str) -> Result<String, String> {
    println!("==> Running {label}");
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(cwd).args(["bench", "--bench", bench]);
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
        let _ = io::stderr().write_all(combined.as_bytes());
        return Err(format!("cargo bench --bench {bench} failed ({})", out.status));
    }
    Ok(combined)
}

// ── parsing gungraun (Callgrind) output ──────────────────────────────────

/// Parse gungraun text output. Headers look like
/// `<target>::<group>::<fn> <case>:(...)`; metric lines follow as
/// `  <Metric>: <new>|<old> (...)`.
fn parse_gungraun(text: &str, target: &str) -> Vec<Gung> {
    let header = format!("{target}::");
    let mut out: Vec<Gung> = Vec::new();
    let (mut l1, mut ll, mut ram) = (None, None, None);

    fn flush(out: &mut [Gung], l1: &mut Option<u64>, ll: &mut Option<u64>, ram: &mut Option<u64>) {
        if let (Some(e), Some(a), Some(b), Some(c)) = (out.last_mut(), *l1, *ll, *ram) {
            e.mem = Some(a + b + c);
        }
        *l1 = None;
        *ll = None;
        *ram = None;
    }

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&header) {
            if let Some((_group, after)) = rest.split_once("::") {
                let func = after.split(char::is_whitespace).next().unwrap_or("");
                if !func.is_empty() && func.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    flush(&mut out, &mut l1, &mut ll, &mut ram);
                    out.push(Gung { func: func.to_string(), instructions: None, mem: None, cycles: None });
                    continue;
                }
            }
        }
        if let Some((key, value)) = metric_line(line.trim_start()) {
            match key {
                "Instructions" => {
                    if let Some(e) = out.last_mut() {
                        e.instructions = Some(value);
                    }
                }
                "L1 Hits" => l1 = Some(value),
                "LL Hits" => ll = Some(value),
                "RAM Hits" => ram = Some(value),
                "Estimated Cycles" => {
                    if let Some(e) = out.last_mut() {
                        e.cycles = Some(value);
                    }
                }
                _ => {}
            }
        }
    }
    flush(&mut out, &mut l1, &mut ll, &mut ram);
    out
}

fn metric_line(trimmed: &str) -> Option<(&str, u64)> {
    let colon = trimmed.find(':')?;
    let key = trimmed[..colon].trim();
    if !matches!(key, "Instructions" | "L1 Hits" | "LL Hits" | "RAM Hits" | "Estimated Cycles") {
        return None;
    }
    let after = trimmed[colon + 1..].trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok().map(|v| (key, v))
}

fn gung_for<'a>(g: &'a [Gung], func: &str) -> Option<&'a Gung> {
    g.iter().find(|e| e.func == func)
}

// ── parsing criterion output ─────────────────────────────────────────────

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

/// Extract the median (middle value) from a criterion `time: [low u med u hi u]`.
fn parse_time_line(line: &str) -> Option<f64> {
    let idx = line.find("time:")?;
    let rest = &line[idx + "time:".len()..];
    let open = rest.find('[')?;
    let close = rest.find(']')?;
    let toks: Vec<&str> = rest[open + 1..close].split_whitespace().collect();
    if toks.len() != 6 {
        return None;
    }
    Some(toks[2].parse::<f64>().ok()? * unit_to_ns(toks[3])?)
}

fn is_bench_name(s: &str) -> bool {
    if s.is_empty() || s.contains(':') || s.contains(char::is_whitespace) {
        return false;
    }
    let Some((g, v)) = s.split_once('/') else { return false };
    !g.is_empty()
        && !v.is_empty()
        && g.chars().chain(v.chars()).all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse a criterion log into `{group/variant: median_ns_for_N_ops}`.
fn parse_criterion(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        if let Some(t_idx) = line.find("time:") {
            let head = line[..t_idx].trim();
            let name = if is_bench_name(head) { Some(head.to_string()) } else { pending.take() };
            if let (Some(name), Some(t)) = (name, parse_time_line(line)) {
                out.push((name, t));
            }
            continue;
        }
        if is_bench_name(line.trim()) {
            pending = Some(line.trim().to_string());
        }
    }
    out
}

fn crit_per_op(crit: &[(String, f64)], key: &str) -> Option<f64> {
    crit.iter().find(|(k, _)| k == key).map(|(_, v)| *v / N)
}

// ── parsing graph_churn ──────────────────────────────────────────────────

fn parse_graph(text: &str) -> (Vec<(String, f64, f64, f64)>, Option<String>) {
    let mut rows = Vec::new();
    let mut summary = None;
    for line in text.lines() {
        if line.contains("Malloc/s") {
            let (name, nums) = split_name_and_numbers(line);
            if nums.len() >= 3 {
                rows.push((name, nums[0], nums[1], nums[2]));
            }
        } else if let Some(idx) = line.find("=>") {
            summary = Some(line[idx + 2..].trim().to_string());
        }
    }
    (rows, summary)
}

fn split_name_and_numbers(s: &str) -> (String, Vec<f64>) {
    let mut name = Vec::new();
    let mut nums = Vec::new();
    for tok in s.split_whitespace() {
        if let Ok(n) = tok.parse::<f64>() {
            nums.push(n);
        } else if nums.is_empty() {
            name.push(tok);
        }
    }
    (name.join(" "), nums)
}

// ── formatting ───────────────────────────────────────────────────────────

fn fmt_int(n: Option<u64>) -> String {
    let Some(n) = n else { return "—".into() };
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
        out.push_str(from_utf8(chunk).expect("ASCII digits"));
    }
    out
}

fn fmt_ns(ns: Option<f64>) -> String {
    match ns {
        None => "—".into(),
        Some(ns) if ns < 1000.0 => format!("{ns:.2} ns"),
        Some(ns) if ns < 1e6 => format!("{:.2} µs", ns / 1e3),
        Some(ns) => format!("{:.2} ms", ns / 1e6),
    }
}

fn fmt_ratio(value: Option<f64>, baseline: Option<f64>) -> String {
    match (value, baseline) {
        (Some(value), Some(baseline)) if baseline != 0.0 => format!("{:.2}×", value / baseline),
        _ => "—".into(),
    }
}

fn label_for(func: &str, table: &[(&str, &str)]) -> String {
    table
        .iter()
        .find(|(f, _)| *f == func)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_else(|| format!("`{func}`"))
}

// ── report ───────────────────────────────────────────────────────────────

fn build_report(
    gung_alloc: &[Gung],
    gung_dyn_box: &[Gung],
    cmp: &[Gung],
    crit: &[(String, f64)],
    graph_log: &str,
) -> String {
    let mut out = String::new();
    out.push_str("# Plurality Performance Report\n\n");
    out.push_str(
        "Generated by [`scripts/perf_report.rs`](../scripts/perf_report.rs). \
         Re-run it to refresh these numbers.\n\n",
    );
    out.push_str(
        "- **Time / op** is the criterion wall-clock median divided by the loop \
         count (machine-dependent).\n",
    );
    out.push_str(
        "- **Instructions / mem accesses / est. cycles** are Callgrind counts \
         (via [gungraun]) for a single operation — deterministic and noise-free, \
         the right signal for tracking regressions. *Mem accesses* = L1 + LL + \
         RAM hits; *est. cycles* is Callgrind's cache-miss-weighted model.\n\n",
    );
    out.push_str(
        "The wall-clock and Callgrind halves run the **same** per-operation \
         bodies under `benches/`; the only difference is that criterion \
         loops each body where gungraun runs it once.\n\n",
    );
    out.push_str("[gungraun]: https://github.com/gungraun/gungraun\n\n");

    // ── Allocation functions (aligned criterion + gungraun) ──
    out.push_str("## Allocation functions\n\n");
    out.push_str(
        "Every allocation function, measured as one allocate-then-free against a \
         pre-warmed pool (steady-state slot reuse, no growth). \
         `cargo bench --bench criterion` + `--bench gungraun`.\n\n",
    );
    emit_aligned_table(&mut out, ALLOC_OPS, "alloc", gung_alloc, crit);

    out.push_str("### Clone + drop (shared handles)\n\n");
    emit_aligned_table(&mut out, CLONE_OPS, "clone", gung_alloc, crit);

    // ── Cross-crate comparison ──
    out.push_str("## Cross-crate comparison (allocate + free)\n\n");
    out.push_str(
        "From `cargo bench --bench pool_comparison`: 10,000 allocate+free \
         iterations against each pre-warmed pool. `slab`/`slotmap` are \
         single-threaded; `sharded-slab`/`deadpool` pay concurrency/async \
         overhead — so this ranks raw single-thread cost, not capability. \
         (`plurality — Alloc` is the fair analogue to the guard-returning pools; \
         `plurality — Box` is the owned, `Send` handle.)\n\n",
    );
    out.push_str("| Pool | Instructions | Mem accesses | Est. cycles |\n");
    out.push_str("|---|---:|---:|---:|\n");
    if cmp.is_empty() {
        out.push_str("| _(Callgrind benches skipped)_ | — | — | — |\n");
    } else {
        let mut seen = Vec::new();
        for (func, _) in COMPARISON_LABELS {
            if let Some(g) = gung_for(cmp, func) {
                emit_cmp_row(&mut out, g);
                seen.push(*func);
            }
        }
        for g in cmp {
            if !seen.contains(&g.func.as_str()) {
                emit_cmp_row(&mut out, g);
            }
        }
    }
    out.push('\n');

    // ── Graph churn ──
    let (grows, gsummary) = parse_graph(graph_log);
    if !grows.is_empty() {
        out.push_str("## Graph churn throughput (wall-clock)\n\n");
        out.push_str(
            "From `cargo bench --bench graph_churn`: 1,000,000 node allocations \
             with a realistic add/remove pattern, replayed identically against \
             `plurality::Pool` and `std::Box` + mimalloc (verified by a shared \
             checksum).\n\n",
        );
        out.push_str("| Backend | Total | ns / alloc | Malloc/s |\n");
        out.push_str("|---|---:|---:|---:|\n");
        for (name, secs, ns, mops) in &grows {
            let _ = writeln!(out, "| {name} | {secs:.4} s | {ns:.2} | {mops:.2} |");
        }
        out.push('\n');
        if let Some(s) = gsummary {
            let _ = writeln!(out, "**{s}.**");
        }
        out.push('\n');
    }

    // ── Owning fat-pointer comparison ──
    out.push_str("## Owning fat-pointer comparison\n\n");
    out.push_str(
        "Each row allocates the same concrete 32-byte value, converts its owning \
         handle to `dyn Trait`, performs one virtual call, and drops the handle. \
         Before measurement, every pool materializes a 1,024-object working set \
         using its default layout policy, drops every object, and executes the \
         exact operation once. This keeps growth, layout-map creation, and \
         first-use effects outside the timed region; an allocation-tracking test \
         confirms 1,024 consecutive executions of every pooled measured body \
         perform zero system allocations. The standard-library setup is warmed \
         the same way, but its measured body necessarily performs one heap \
         allocation through the process's default system allocator. \
         infinity-pool is the only other crate found with reusable owning \
         `?Sized` handles, but no one variant matches plurality on both axes: \
         plurality combines `Send` handles and cross-thread drops with \
         single-threaded, lock-free allocation; infinity-pool's `PinnedPool` \
         variants support concurrent, lock-based allocation with `Send` handles, \
         while their faster `Local` variants make both pool and handles \
         single-threaded. The `BlindPool` rows additionally support heterogeneous \
         layouts and therefore pay for more capability. Other \
         surveyed pool crates return keys or pool-borrowing guards rather than \
         owning fat-pointer handles. \
         `cargo bench --bench criterion` + \
         `--bench gungraun`.\n\n",
    );
    emit_dyn_box_table(&mut out, gung_dyn_box, crit);
    out.push_str(
        "The standard-library row is an allocator best case: every allocation is \
         the same size and is immediately freed, so allocator thread caches are \
         maximally effective. The graph-churn benchmark above measures a broader \
         live set and locality effects.\n\n",
    );

    out
}

fn emit_aligned_table(
    out: &mut String,
    ops: &[(&str, &str)],
    group: &str,
    gung: &[Gung],
    crit: &[(String, f64)],
) {
    out.push_str("| Operation | Time / op | Instructions | Mem accesses | Est. cycles |\n");
    out.push_str("|---|---:|---:|---:|---:|\n");
    for (name, label) in ops {
        let t = crit_per_op(crit, &format!("{group}/{name}"));
        let g = gung_for(gung, name);
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            label,
            fmt_ns(t),
            fmt_int(g.and_then(|g| g.instructions)),
            fmt_int(g.and_then(|g| g.mem)),
            fmt_int(g.and_then(|g| g.cycles)),
        );
    }
    out.push('\n');
}

fn emit_dyn_box_table(out: &mut String, gung: &[Gung], crit: &[(String, f64)]) {
    let baseline_time = crit_per_op(crit, "dyn_box/plurality_box");
    let baseline_instructions =
        gung_for(gung, "plurality_box").and_then(|entry| entry.instructions);

    out.push_str(
        "| Handle | Time / op | Time vs plurality | Instructions | Instructions vs plurality | Mem accesses | Est. cycles |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for (name, label) in DYN_BOX_OPS {
        let time = crit_per_op(crit, &format!("dyn_box/{name}"));
        let entry = gung_for(gung, name);
        let instructions = entry.and_then(|entry| entry.instructions);
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            label,
            fmt_ns(time),
            fmt_ratio(time, baseline_time),
            fmt_int(instructions),
            fmt_ratio(
                instructions.map(|value| value as f64),
                baseline_instructions.map(|value| value as f64),
            ),
            fmt_int(entry.and_then(|entry| entry.mem)),
            fmt_int(entry.and_then(|entry| entry.cycles)),
        );
    }
    out.push('\n');
}

fn emit_cmp_row(out: &mut String, g: &Gung) {
    let _ = writeln!(
        out,
        "| {} | {} | {} | {} |",
        label_for(&g.func, COMPARISON_LABELS),
        fmt_int(g.instructions),
        fmt_int(g.mem),
        fmt_int(g.cycles),
    );
}
