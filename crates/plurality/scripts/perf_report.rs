#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
---

//! Runs the customer-facing benchmark suites and regenerates `docs/PERF.md`.
//!
//! The published report is wall-clock only: it covers a curated set of
//! scenarios a user of the crate would recognize, and every comparison is
//! measured against the alternatives a user would otherwise reach for. The
//! Callgrind instruction-count suites inside the consolidated `plurality`
//! bench target and the exhaustive per-API micro-benchmarks stay in the crate
//! for optimization work and are deliberately not published here.
//!
//! This report requires Linux: it includes the `pool_comparison` group,
//! which is Linux-only because several third-party pooling crates it
//! compares against are unavailable on other platforms.
//!
//! Usage:
//!   `scripts/perf_report.rs`          — full run
//!   `scripts/perf_report.rs --fast`   — quick, lower-fidelity criterion settings

use std::collections::{BTreeMap, btree_map::Entry};
use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::Deserialize;

const N: f64 = 1000.0;
const GRAPH_ALLOCATIONS: f64 = 1_000_000.0;

const HANDLE_OPS: &[(&str, &str)] = &[
    ("alloc/alloc_val", "`Alloc<'pool, T>` — borrowed, unique owner"),
    ("alloc/box_val", "`Box<T>` — `'static`, unique owner, `Send`"),
    ("alloc/rc_val", "`Rc<T>` — shared, non-atomic refcount"),
    ("alloc/arc_val", "`Arc<T>` — shared, atomic refcount, `Send + Sync`"),
];

const CLONE_OPS: &[(&str, &str)] = &[
    ("clone/rc_clone", "`Rc<T>` clone + drop"),
    ("clone/arc_clone", "`Arc<T>` clone + drop"),
];

const TYPE_ERASURE_OPS: &[(&str, &str)] = &[
    ("alloc/box_val", "`Pool<T>` — one type"),
    ("alloc/multi_box_val", "`MultiPool` — one layout"),
    ("alloc/multi_box_val_spread", "`MultiPool` — sixteen layouts"),
];

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

const GRAPH_OPS: &[(&str, &str)] = &[
    ("graph_churn/std_box_mimalloc", "std::Box + mimalloc"),
    ("graph_churn/plurality_pool", "plurality::Pool"),
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
    if !cfg!(target_os = "linux") {
        return Err(
            "this report requires Linux: the `pool_comparison` group it depends on is Linux-only \
             (see crates/plurality/benches/plurality.rs), so several third-party pooling crates used \
             only in that comparison are unavailable on other platforms"
                .into(),
        );
    }

    let crate_dir = crate_root();
    let target_dir = crate_dir.join("target");
    fs::create_dir_all(&target_dir).map_err(|e| format!("creating {}: {e}", target_dir.display()))?;
    let json_path = target_dir.join("plurality-perf-report.json");
    let markdown_path = target_dir.join("plurality-perf-report.md");

    let (warm, meas, samples) = if fast { ("0.5", "1", "20") } else { ("1", "2", "50") };

    let mut args = vec![
        OsString::from("--"),
        OsString::from("--criterion"),
        OsString::from("--no-baseline"),
        OsString::from("--export-json"),
        json_path.as_os_str().to_owned(),
        OsString::from("--export-md"),
        markdown_path.as_os_str().to_owned(),
    ];
    for forwarded in [
        "--warm-up-time",
        warm,
        "--measurement-time",
        meas,
        "--sample-size",
        samples,
    ] {
        args.push(OsString::from("--criterion-arg"));
        args.push(OsString::from(forwarded));
    }

    println!("==> Running cargo bench --bench plurality");
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(&crate_dir)
        .arg("bench")
        .arg("--bench")
        .arg("plurality")
        .args(&args);
    let output = cmd
        .output()
        .map_err(|e| format!("failed to spawn cargo bench --bench plurality: {e}"))?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        eprint!("{combined}");
        return Err(format!("cargo bench --bench plurality failed ({})", output.status));
    }

    println!("==> Building docs/PERF.md");
    let json = fs::read_to_string(&json_path).map_err(|e| format!("reading {}: {e}", json_path.display()))?;
    let report: JsonReport = serde_json::from_str(&json).map_err(|e| format!("parsing {}: {e}", json_path.display()))?;
    let report = index_report(report)?;

    let expected = HANDLE_OPS
        .iter()
        .chain(CLONE_OPS)
        .chain(TYPE_ERASURE_OPS)
        .chain(COMPARISON_OPS)
        .chain(DYN_BOX_OPS)
        .chain(GRAPH_OPS)
        .map(|(key, _)| *key);
    let missing: Vec<&str> = expected.filter(|key| criterion_time_ns(&report, key).is_none()).collect();
    if !missing.is_empty() {
        return Err(format!(
            "report is missing {} expected criterion benchmark(s): {}",
            missing.len(),
            missing.join(", ")
        ));
    }

    let mut markdown = build_report(&report);
    markdown.truncate(markdown.trim_end().len());
    markdown.push('\n');

    let out_path = crate_dir.join("docs").join("PERF.md");
    fs::write(&out_path, &markdown).map_err(|e| format!("writing {}: {e}", out_path.display()))?;
    println!("==> Done. Wrote {} ({} metabench identities).", out_path.display(), report.len());
    Ok(())
}

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts/ always has a parent crate directory")
        .to_path_buf()
}

fn index_report(report: JsonReport) -> Result<ReportIndex, String> {
    let mut entries = BTreeMap::new();
    for entry in report.entries {
        match entries.entry(entry.identity.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            Entry::Occupied(_) => return Err(format!("report contains duplicate benchmark '{}'", entry.identity)),
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

fn crit_per_op(report: &ReportIndex, key: &str) -> Option<f64> {
    criterion_time_ns(report, key).map(|ns| ns / N)
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

fn build_report(report: &ReportIndex) -> String {
    let mut out = String::new();
    out.push_str("# Plurality Performance Report\n\n");
    out.push_str("Generated by [`scripts/perf_report.rs`](../scripts/perf_report.rs). Re-run it to refresh these numbers.\n\n");
    out.push_str("All figures are wall-clock medians measured by Criterion. They are machine-dependent: the ratios between rows are the durable signal, not the absolute values.\n\n");
    out.push_str("This report is a curated set of customer-facing scenarios. The crate also carries a larger suite of internal micro-benchmarks, including Callgrind instruction-count suites inside the consolidated `plurality` bench target, which are used for optimization work and are not published here; run them with `cargo bench --bench plurality` in this crate.\n\n");
    out.push_str("Every measured body allocates one ~32-byte, `Drop`-free value and releases it again. Each pool is pre-warmed with 1,024 slots and every slot is released before measurement starts, so the timed region only ever reuses a slot and never grows the pool. Payload construction is inside the timed region for every implementation, including the guard-returning pools, which write the payload through their guard.\n\n");

    emit_handle_costs(&mut out, report);
    emit_type_erasure(&mut out, report);
    emit_comparison(&mut out, report);
    emit_graph_churn(&mut out, report);
    emit_dyn_box(&mut out, report);

    out
}

fn emit_handle_costs(out: &mut String, report: &ReportIndex) {
    out.push_str("## Cost of a handle\n\n");
    out.push_str("What one allocate-then-free costs through each of the four handle types, so you can price the capability you need. The handles differ in ownership and thread affinity, not in how the slot itself is found: [`Alloc`] borrows the pool and is the cheapest, [`Box`] is `'static` and `Send`, and the two shared handles additionally maintain a reference count. `cargo bench --bench plurality`.\n\n");
    out.push_str("| Handle | Allocate + free |\n|---|---:|\n");
    for (key, label) in HANDLE_OPS {
        let _ = writeln!(out, "| {} | {} |", label, fmt_ns(crit_per_op(report, key)));
    }
    out.push('\n');
    out.push_str("Sharing an existing value is cheaper still, since no slot changes hands:\n\n");
    out.push_str("| Operation | Time |\n|---|---:|\n");
    for (key, label) in CLONE_OPS {
        let _ = writeln!(out, "| {} | {} |", label, fmt_ns(crit_per_op(report, key)));
    }
    out.push('\n');
    out.push_str("[`Alloc`]: https://docs.rs/plurality/latest/plurality/struct.Alloc.html\n");
    out.push_str("[`Box`]: https://docs.rs/plurality/latest/plurality/struct.Box.html\n\n");
}

fn emit_type_erasure(out: &mut String, report: &ReportIndex) {
    let baseline = crit_per_op(report, TYPE_ERASURE_OPS[0].0);
    out.push_str("## Cost of serving many types from one pool\n\n");
    out.push_str("What dropping the element type costs. [`MultiPool`] accepts values of any type, and finds the right slot size by looking the value's layout up in a directory of the layouts it has seen; a `Pool<T>` knows its slot size at compile time and looks nothing up. The lookup is a linear scan, so the rows below hold the number of distinct layouts at one and at sixteen, the latter with the measured layout registered last so the scan runs its full length. Price type erasure at the step from the first row to either of the others, not at the difference between them: the longer scan executes materially more instructions, but the processor overlaps it with the pool's own pointer chasing, and what remains is smaller than the effect of heap and code placement, which this benchmark does not control. `cargo bench --bench plurality`.\n\n");
    out.push_str("| Pool | Allocate + free | Δ vs `Pool<T>` |\n|---|---:|---:|\n");
    for (key, label) in TYPE_ERASURE_OPS {
        let time = crit_per_op(report, key);
        let _ = writeln!(out, "| {} | {} | {} |", label, fmt_ns(time), fmt_ratio(time, baseline));
    }
    out.push('\n');
    out.push_str("[`MultiPool`]: https://docs.rs/plurality/latest/plurality/struct.MultiPool.html\n\n");
}

fn emit_comparison(out: &mut String, report: &ReportIndex) {
    let baseline = crit_per_op(report, COMPARISON_OPS[0].0);
    out.push_str("## Against other pooling crates\n\n");
    out.push_str("The same allocate-and-free workload run against every pooling crate we found with a comparable model. This ranks raw single-thread cost, not capability: `slab` and `slotmap` are single-threaded and hand back keys rather than pointers, `sharded-slab` and `deadpool` pay for concurrency and async readiness, and the guard-returning pools (`object-pool`, `opool`) borrow from the pool rather than owning. `plurality — Alloc` is the fair analogue to the guard-returning pools; `plurality — Box` is the owned, `Send` handle that none of the key-based pools offer. `cargo bench --bench plurality`.\n\n");
    out.push_str("| Pool | Allocate + free | Δ vs plurality `Box` |\n|---|---:|---:|\n");
    for (key, label) in COMPARISON_OPS {
        let time = crit_per_op(report, key);
        let _ = writeln!(out, "| {} | {} | {} |", label, fmt_ns(time), fmt_ratio(time, baseline));
    }
    out.push('\n');
}

fn emit_graph_churn(out: &mut String, report: &ReportIndex) {
    let std_total = criterion_time_ns(report, GRAPH_OPS[0].0).map(|ns| ns / 1e9);
    let pool_total = criterion_time_ns(report, GRAPH_OPS[1].0).map(|ns| ns / 1e9);

    out.push_str("## Against the system allocator, under churn\n\n");
    out.push_str("The scenario a pool actually exists for: 1,000,000 node allocations with a realistic add/remove pattern over a large live set, replayed identically against `plurality::Pool` and `std::Box` on mimalloc. Both backends are verified to have performed the same work by a shared checksum. Unlike the microbenchmarks above, this measures a broad live set and so includes locality effects. `cargo bench --bench plurality`.\n\n");
    out.push_str("| Backend | Total | ns / alloc | Mallocs/s (millions) |\n|---|---:|---:|---:|\n");
    for (key, label) in GRAPH_OPS {
        let secs = criterion_time_ns(report, key).map(|ns| ns / 1e9);
        match secs {
            Some(secs) if secs != 0.0 => {
                let _ = writeln!(
                    out,
                    "| {} | {:.4} s | {:.2} | {:.2} |",
                    label,
                    secs,
                    secs * 1e9 / GRAPH_ALLOCATIONS,
                    GRAPH_ALLOCATIONS / secs / 1e6,
                );
            }
            _ => {
                let _ = writeln!(out, "| {} | — | — | — |", label);
            }
        }
    }
    out.push('\n');
    if let (Some(std_total), Some(pool_total)) = (std_total, pool_total) {
        let ratio = std_total / pool_total;
        if ratio >= 1.0 {
            let _ = writeln!(out, "**plurality::Pool is {ratio:.2}x faster than std::Box + mimalloc.**\n");
        } else {
            let _ = writeln!(out, "**plurality::Pool is {:.2}x slower than std::Box + mimalloc.**\n", 1.0 / ratio);
        }
    }
}

fn emit_dyn_box(out: &mut String, report: &ReportIndex) {
    let baseline = crit_per_op(report, DYN_BOX_OPS[0].0);
    out.push_str("## Owning `dyn Trait` handles\n\n");
    out.push_str("Each row allocates the same concrete 32-byte value, converts its owning handle to `dyn Trait`, performs one virtual call, and drops the handle — the shape you get when a pool backs a heterogeneous collection of trait objects. Before measurement every pool materializes a 1,024-object working set using its default layout policy, drops every object, and executes the exact operation once, so growth, layout-map creation, and first-use effects stay outside the timed region; an allocation-tracking test confirms 1,024 consecutive executions of every pooled measured body perform zero system allocations. The standard-library setup is warmed the same way, but its measured body necessarily performs one heap allocation through the process's default system allocator.\n\n");
    out.push_str("infinity-pool is the only other crate found with reusable owning `?Sized` handles, but no one variant matches plurality on both axes: plurality combines `Send` handles and cross-thread drops with single-threaded, lock-free allocation; infinity-pool's `PinnedPool` variants support concurrent, lock-based allocation with `Send` handles, while their faster `Local` variants make both pool and handles single-threaded. The rows marked heterogeneous accept values of any type in one pool and therefore pay for more capability; each row here also unsizes a handle and makes a virtual call, so the cost of type erasure alone is the one measured above rather than the difference between these rows. Other surveyed pool crates return keys or pool-borrowing guards rather than owning fat-pointer handles. `cargo bench --bench plurality`.\n\n");
    out.push_str("| Handle | Allocate, call, free | Δ vs plurality |\n|---|---:|---:|\n");
    for (key, label) in DYN_BOX_OPS {
        let time = crit_per_op(report, key);
        let _ = writeln!(out, "| {} | {} | {} |", label, fmt_ns(time), fmt_ratio(time, baseline));
    }
    out.push('\n');
    out.push_str("The standard-library row is an allocator best case: every allocation is the same size and is immediately freed, so allocator thread caches are maximally effective. The churn benchmark above measures a broader live set and locality effects.\n\n");
}
