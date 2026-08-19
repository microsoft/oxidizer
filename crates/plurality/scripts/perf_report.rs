#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2021"
---

//! Runs the customer-facing benchmark suites and regenerates `docs/PERF.md`.
//!
//! The published report is wall-clock only: it covers a curated set of
//! scenarios a user of the crate would recognize, and every comparison is
//! measured against the alternatives a user would otherwise reach for. The
//! Callgrind instruction-count suites under `benches/*_cg.rs` and the
//! exhaustive per-API micro-benchmarks stay in the crate for optimization work
//! and are deliberately not published here.
//!
//! Usage:
//!   `scripts/perf_report.rs`          — full run
//!   `scripts/perf_report.rs --fast`   — quick, lower-fidelity criterion settings

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Operations per criterion iteration. Must match `N` in
/// `benches/criterion/main.rs` and `benches/pool_comparison.rs`.
const N: f64 = 1000.0;

/// Allocation of one value through each handle type, as one allocate-then-free
/// against a pre-warmed pool: `(criterion variant, pretty label)`.
const HANDLE_OPS: &[(&str, &str)] = &[
    ("alloc/alloc_val", "`Alloc<'pool, T>` — borrowed, unique owner"),
    ("alloc/box_val", "`Box<T>` — `'static`, unique owner, `Send`"),
    ("alloc/rc_val", "`Rc<T>` — shared, non-atomic refcount"),
    ("alloc/arc_val", "`Arc<T>` — shared, atomic refcount, `Send + Sync`"),
];

/// Clone-and-drop of an existing shared handle.
const CLONE_OPS: &[(&str, &str)] = &[
    ("clone/rc_clone", "`Rc<T>` clone + drop"),
    ("clone/arc_clone", "`Arc<T>` clone + drop"),
];

/// The cost of serving many types from one pool, against the typed pool doing
/// the same work. The first entry is the baseline every other row is compared
/// against.
const TYPE_ERASURE_OPS: &[(&str, &str)] = &[
    ("alloc/box_val", "`Pool<T>` — one type"),
    ("alloc/multi_box_val", "`MultiPool` — one layout"),
    ("alloc/multi_box_val_spread", "`MultiPool` — sixteen layouts"),
];

/// The cross-crate allocate+free comparison. The first entry is the baseline
/// every other row is compared against.
const COMPARISON_OPS: &[(&str, &str)] = &[
    ("pool_comparison/churn/plurality_box", "plurality — `Box`"),
    ("pool_comparison/churn/plurality_alloc", "plurality — `Alloc`"),
    ("pool_comparison/churn/slab_insert_remove", "slab"),
    ("pool_comparison/churn/slotmap_insert_remove", "slotmap"),
    ("pool_comparison/churn/sharded_slab_insert_remove", "sharded-slab"),
    ("pool_comparison/churn/object_pool_pull", "object-pool"),
    ("pool_comparison/churn/opool_get", "opool"),
    ("pool_comparison/churn/deadpool_get", "deadpool"),
    ("pool_comparison/churn/infinity_pinned", "infinity-pool — `PinnedPool`"),
    ("pool_comparison/churn/infinity_raw", "infinity-pool — `RawPinnedPool`"),
];

/// Comparable owning handles that erase a concrete pooled value to `dyn Marker`.
/// The first entry is the baseline every other row is compared against.
const DYN_BOX_OPS: &[(&str, &str)] = &[
    ("dyn_box/plurality_box", "plurality — `Box<dyn Trait>`"),
    (
        "dyn_box/plurality_multi_box",
        "plurality — `MultiPool` / `Box<dyn Trait>` (heterogeneous)",
    ),
    (
        "dyn_box/infinity_pinned",
        "infinity-pool — `PinnedPool` / `PooledMut<dyn Trait>`",
    ),
    (
        "dyn_box/infinity_local_pinned",
        "infinity-pool — `LocalPinnedPool` / `LocalPooledMut<dyn Trait>`",
    ),
    (
        "dyn_box/infinity_blind",
        "infinity-pool — `BlindPool` / `BlindPooledMut<dyn Trait>` (heterogeneous)",
    ),
    (
        "dyn_box/infinity_local_blind",
        "infinity-pool — `LocalBlindPool` / `LocalBlindPooledMut<dyn Trait>` (heterogeneous)",
    ),
    ("dyn_box/std_box", "standard library — `Box<dyn Trait>`"),
];

fn main() -> ExitCode {
    let mut fast = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--fast" => fast = true,
            "-h" | "--help" => {
                println!("usage: scripts/perf_report.rs [--fast]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    match run(fast) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(fast: bool) -> Result<(), String> {
    let crate_dir = crate_root();

    let (warm, meas, samples) = if fast { ("0.5", "1", "20") } else { ("1", "2", "50") };
    let crit_args = [
        "--warm-up-time", warm, "--measurement-time", meas, "--sample-size", samples,
    ];

    let crit_log = run_bench(&crate_dir, "criterion", &crit_args, "wall-clock: criterion")?;
    let cmp_log = run_bench(&crate_dir, "pool_comparison", &crit_args, "wall-clock: pool_comparison")?;
    let graph_log = run_bench(&crate_dir, "graph_churn", &[], "wall-clock: graph_churn")?;

    println!("==> Building docs/PERF.md");
    let mut crit = parse_criterion(&crit_log);
    crit.extend(parse_criterion(&cmp_log));

    let expected = HANDLE_OPS
        .iter()
        .chain(CLONE_OPS)
        .chain(TYPE_ERASURE_OPS)
        .chain(COMPARISON_OPS)
        .chain(DYN_BOX_OPS)
        .map(|(key, _)| *key);
    let missing: Vec<&str> = expected.filter(|key| crit_per_op(&crit, key).is_none()).collect();
    if !missing.is_empty() {
        return Err(format!(
            "criterion output is missing {} expected benchmark(s): {}",
            missing.len(),
            missing.join(", ")
        ));
    }

    let graph = parse_graph(&graph_log);
    if graph.rows.is_empty() {
        return Err("graph_churn produced no throughput rows".into());
    }

    let mut report = build_report(&crit, &graph);
    report.truncate(report.trim_end().len());
    report.push('\n');

    let docs = crate_dir.join("docs");
    fs::create_dir_all(&docs).map_err(|e| format!("creating {}: {e}", docs.display()))?;
    let out_path = docs.join("PERF.md");
    fs::write(&out_path, &report).map_err(|e| format!("writing {}: {e}", out_path.display()))?;
    println!("==> Done. Wrote {} ({} criterion benches).", out_path.display(), crit.len());
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

/// A criterion benchmark id is a `/`-separated path of identifier-like segments.
fn is_bench_name(s: &str) -> bool {
    if s.is_empty() || s.contains(':') || s.contains(char::is_whitespace) {
        return false;
    }
    let mut segments = s.split('/');
    let multi_segment = s.contains('/');
    multi_segment
        && segments.all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// Parse a criterion log into `{bench id: median_ns_for_N_ops}`.
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

/// One backend replayed through the shared graph-churn op stream.
struct GraphRow {
    name: String,
    secs: f64,
    ns_per_alloc: f64,
    mallocs_per_sec: f64,
}

struct Graph {
    rows: Vec<GraphRow>,
    summary: Option<String>,
}

fn parse_graph(text: &str) -> Graph {
    let mut rows = Vec::new();
    let mut summary = None;
    for line in text.lines() {
        if line.contains("Malloc/s") {
            let (name, nums) = split_name_and_numbers(line);
            if nums.len() >= 3 {
                rows.push(GraphRow {
                    name,
                    secs: nums[0],
                    ns_per_alloc: nums[1],
                    mallocs_per_sec: nums[2],
                });
            }
        } else if let Some(idx) = line.find("=>") {
            summary = Some(line[idx + 2..].trim().to_string());
        }
    }
    Graph { rows, summary }
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

// ── report ───────────────────────────────────────────────────────────────

fn build_report(crit: &[(String, f64)], graph: &Graph) -> String {
    let mut out = String::new();
    out.push_str("# Plurality Performance Report\n\n");
    out.push_str(
        "Generated by [`scripts/perf_report.rs`](../scripts/perf_report.rs). \
         Re-run it to refresh these numbers.\n\n",
    );
    out.push_str(
        "All figures are wall-clock medians measured by Criterion. They are \
         machine-dependent: the ratios between rows are the durable signal, not \
         the absolute values.\n\n",
    );
    out.push_str(
        "This report is a curated set of customer-facing scenarios. The crate \
         also carries a larger suite of internal micro-benchmarks, including \
         Callgrind instruction-count suites (`benches/*_cg.rs`), which are used \
         for optimization work and are not published here; run them with \
         `cargo bench` in this crate.\n\n",
    );
    out.push_str(
        "Every measured body allocates one ~32-byte, `Drop`-free value and \
         releases it again. Each pool is pre-warmed with 1,024 slots and every \
         slot is released before measurement starts, so the timed region only \
         ever reuses a slot and never grows the pool. Payload construction is \
         inside the timed region for every implementation, including the \
         guard-returning pools, which write the payload through their guard.\n\n",
    );

    emit_handle_costs(&mut out, crit);
    emit_type_erasure(&mut out, crit);
    emit_comparison(&mut out, crit);
    emit_graph_churn(&mut out, graph);
    emit_dyn_box(&mut out, crit);

    out
}

fn emit_handle_costs(out: &mut String, crit: &[(String, f64)]) {
    out.push_str("## Cost of a handle\n\n");
    out.push_str(
        "What one allocate-then-free costs through each of the four handle \
         types, so you can price the capability you need. The handles differ in \
         ownership and thread affinity, not in how the slot itself is found: \
         [`Alloc`] borrows the pool and is the cheapest, [`Box`] is `'static` \
         and `Send`, and the two shared handles additionally maintain a \
         reference count. `cargo bench --bench criterion`.\n\n",
    );
    out.push_str("| Handle | Allocate + free |\n|---|---:|\n");
    for (key, label) in HANDLE_OPS {
        let _ = writeln!(out, "| {} | {} |", label, fmt_ns(crit_per_op(crit, key)));
    }
    out.push('\n');
    out.push_str(
        "Sharing an existing value is cheaper still, since no slot changes \
         hands:\n\n",
    );
    out.push_str("| Operation | Time |\n|---|---:|\n");
    for (key, label) in CLONE_OPS {
        let _ = writeln!(out, "| {} | {} |", label, fmt_ns(crit_per_op(crit, key)));
    }
    out.push('\n');
    out.push_str("[`Alloc`]: https://docs.rs/plurality/latest/plurality/struct.Alloc.html\n");
    out.push_str("[`Box`]: https://docs.rs/plurality/latest/plurality/struct.Box.html\n\n");
}

fn emit_type_erasure(out: &mut String, crit: &[(String, f64)]) {
    let baseline = crit_per_op(crit, TYPE_ERASURE_OPS[0].0);

    out.push_str("## Cost of serving many types from one pool\n\n");
    out.push_str(
        "What dropping the element type costs. [`MultiPool`] accepts values of \
         any type, and finds the right slot size by looking the value's layout \
         up in a directory of the layouts it has seen; a `Pool<T>` knows its \
         slot size at compile time and looks nothing up. The lookup is a linear \
         scan, so the rows below hold the number of distinct layouts at one and \
         at sixteen, the latter with the measured layout registered last so the \
         scan runs its full length. Price type erasure at the step from the \
         first row to either of the others, not at the difference between them: \
         the longer scan executes materially more instructions, but the \
         processor overlaps it with the pool's own pointer chasing, and what \
         remains is smaller than the effect of heap and code placement, which \
         this benchmark does not control. `cargo bench --bench criterion`.\n\n",
    );
    out.push_str("| Pool | Allocate + free | Δ vs `Pool<T>` |\n|---|---:|---:|\n");
    for (key, label) in TYPE_ERASURE_OPS {
        let time = crit_per_op(crit, key);
        let _ = writeln!(out, "| {} | {} | {} |", label, fmt_ns(time), fmt_ratio(time, baseline));
    }
    out.push('\n');
    out.push_str(
        "[`MultiPool`]: https://docs.rs/plurality/latest/plurality/struct.MultiPool.html\n\n",
    );
}

fn emit_comparison(out: &mut String, crit: &[(String, f64)]) {
    let baseline = crit_per_op(crit, COMPARISON_OPS[0].0);

    out.push_str("## Against other pooling crates\n\n");
    out.push_str(
        "The same allocate-and-free workload run against every pooling crate we \
         found with a comparable model. This ranks raw single-thread cost, not \
         capability: `slab` and `slotmap` are single-threaded and hand back \
         keys rather than pointers, `sharded-slab` and `deadpool` pay for \
         concurrency and async readiness, and the guard-returning pools \
         (`object-pool`, `opool`) borrow from the pool rather than owning. \
         `plurality — Alloc` is the fair analogue to the guard-returning pools; \
         `plurality — Box` is the owned, `Send` handle that none of the \
         key-based pools offer. `cargo bench --bench pool_comparison`.\n\n",
    );
    out.push_str("| Pool | Allocate + free | Δ vs plurality `Box` |\n|---|---:|---:|\n");
    for (key, label) in COMPARISON_OPS {
        let time = crit_per_op(crit, key);
        let _ = writeln!(out, "| {} | {} | {} |", label, fmt_ns(time), fmt_ratio(time, baseline));
    }
    out.push('\n');
}

fn emit_graph_churn(out: &mut String, graph: &Graph) {
    out.push_str("## Against the system allocator, under churn\n\n");
    out.push_str(
        "The scenario a pool actually exists for: 1,000,000 node allocations \
         with a realistic add/remove pattern over a large live set, replayed \
         identically against `plurality::Pool` and `std::Box` on mimalloc. Both \
         backends are verified to have performed the same work by a shared \
         checksum. Unlike the microbenchmarks above, this measures a broad live \
         set and so includes locality effects. \
         `cargo bench --bench graph_churn`.\n\n",
    );
    out.push_str("| Backend | Total | ns / alloc | Mallocs/s (millions) |\n|---|---:|---:|---:|\n");
    for row in &graph.rows {
        let _ = writeln!(
            out,
            "| {} | {:.4} s | {:.2} | {:.2} |",
            row.name, row.secs, row.ns_per_alloc, row.mallocs_per_sec
        );
    }
    out.push('\n');
    if let Some(summary) = &graph.summary {
        let _ = writeln!(out, "**{summary}.**\n");
    }
}

fn emit_dyn_box(out: &mut String, crit: &[(String, f64)]) {
    let baseline = crit_per_op(crit, DYN_BOX_OPS[0].0);

    out.push_str("## Owning `dyn Trait` handles\n\n");
    out.push_str(
        "Each row allocates the same concrete 32-byte value, converts its \
         owning handle to `dyn Trait`, performs one virtual call, and drops the \
         handle — the shape you get when a pool backs a heterogeneous \
         collection of trait objects. Before measurement every pool \
         materializes a 1,024-object working set using its default layout \
         policy, drops every object, and executes the exact operation once, so \
         growth, layout-map creation, and first-use effects stay outside the \
         timed region; an allocation-tracking test confirms 1,024 consecutive \
         executions of every pooled measured body perform zero system \
         allocations. The standard-library setup is warmed the same way, but \
         its measured body necessarily performs one heap allocation through the \
         process's default system allocator.\n\n",
    );
    out.push_str(
        "infinity-pool is the only other crate found with reusable owning \
         `?Sized` handles, but no one variant matches plurality on both axes: \
         plurality combines `Send` handles and cross-thread drops with \
         single-threaded, lock-free allocation; infinity-pool's `PinnedPool` \
         variants support concurrent, lock-based allocation with `Send` \
         handles, while their faster `Local` variants make both pool and \
         handles single-threaded. The rows marked heterogeneous accept values \
         of any type in one pool and therefore pay for more capability; each \
         row here also unsizes a handle and makes a virtual call, so the cost \
         of type erasure alone is the one measured above rather than the \
         difference between these rows. Other \
         surveyed pool crates return keys or pool-borrowing guards rather than \
         owning fat-pointer handles. `cargo bench --bench criterion`.\n\n",
    );
    out.push_str("| Handle | Allocate, call, free | Δ vs plurality |\n|---|---:|---:|\n");
    for (key, label) in DYN_BOX_OPS {
        let time = crit_per_op(crit, key);
        let _ = writeln!(out, "| {} | {} | {} |", label, fmt_ns(time), fmt_ratio(time, baseline));
    }
    out.push('\n');
    out.push_str(
        "The standard-library row is an allocator best case: every allocation \
         is the same size and is immediately freed, so allocator thread caches \
         are maximally effective. The churn benchmark above measures a broader \
         live set and locality effects.\n\n",
    );
}
