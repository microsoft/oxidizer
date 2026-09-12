#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
ohno = { path = "../../ohno", features = ["app-err"] }
prettyplease = "0.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
syn = { version = "2", features = ["full", "parsing"] }
---

//! Run the consolidated routerama metabench suite and rebuild `docs/PERF.md`.
//!
//! Runs Criterion and, where Valgrind is available, Gungraun.
//!
//! Usage:
//!   `scripts/perf_report.rs`                                    — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                             — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`  — custom criterion settings
//!   `scripts/perf_report.rs --no-gungraun`                      — criterion only
//!
//! Criterion and Gungraun variants are paired through shared metabench identities.

use std::collections::{BTreeMap, btree_map::Entry};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::{env, fs};

use clap::Parser;
use ohno::{AppError, app_err, bail};
use serde::Deserialize;

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

    /// Regenerate the committed benchmark router (`benches/common/bench_router.rs`)
    /// from the route table and exit, without running any benchmarks. Run this
    /// after editing `benches/common/routes_data.rs`.
    #[arg(long)]
    regenerate_router: bool,
}

// The benchmark route table (`ROUTES`, `LOOKUPS`), shared with the benches.
include!("../benches/common/routes_data.rs");

/// The benchmark group name shared by the router comparison rows.
const GROUP: &str = "compare_routers";

/// The routers compared, in report order.
const VARIANTS: &[&str] = &[
    "compare_routers/routerama_static",
    "compare_routers/routerama_dynamic",
    "compare_routers/matchit",
    "compare_routers/path_tree",
    "compare_routers/regex",
    "compare_routers/route_recognizer",
];

struct QueryVariant {
    label: &'static str,
    identity: &'static str,
}

struct QueryGroup {
    title: &'static str,
    variants: &'static [QueryVariant],
}

const QUERY_GROUPS: &[QueryGroup] = &[
    QueryGroup {
        title: "Parse: common query",
        variants: &[
            QueryVariant {
                label: "routerama",
                identity: "routerama_query/parse_common/routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                identity: "routerama_query/parse_common/serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                identity: "routerama_query/parse_common/serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Parse: escaped query",
        variants: &[
            QueryVariant {
                label: "routerama",
                identity: "routerama_query/parse_escaped/routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                identity: "routerama_query/parse_escaped/serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                identity: "routerama_query/parse_escaped/serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Parse: repeated values",
        variants: &[
            QueryVariant {
                label: "routerama",
                identity: "routerama_query/parse_repeated/routerama",
            },
            QueryVariant {
                label: "serde_html_form",
                identity: "routerama_query/parse_repeated/serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Parse: long ASCII value",
        variants: &[
            QueryVariant {
                label: "routerama",
                identity: "routerama_query/parse_long_ascii/routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                identity: "routerama_query/parse_long_ascii/serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                identity: "routerama_query/parse_long_ascii/serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Produce: caller-provided buffer",
        variants: &[
            QueryVariant {
                label: "routerama",
                identity: "routerama_query/produce_common/routerama_reserved",
            },
            QueryVariant {
                label: "serde_html_form",
                identity: "routerama_query/produce_common/serde_html_form_reserved",
            },
        ],
    },
    QueryGroup {
        title: "Produce: allocating",
        variants: &[
            QueryVariant {
                label: "routerama",
                identity: "routerama_query/produce_common_allocating/routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                identity: "routerama_query/produce_common_allocating/serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                identity: "routerama_query/produce_common_allocating/serde_html_form",
            },
        ],
    },
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

    fn as_u64(self) -> Option<u64> {
        match self {
            Self::Integer(value) => Some(value),
            Self::Float(_) => None,
        }
    }
}

type ReportIndex = BTreeMap<String, JsonEntry>;

fn index_report(report: JsonReport) -> Result<ReportIndex, AppError> {
    let mut entries = BTreeMap::new();
    for entry in report.entries {
        match entries.entry(entry.identity.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            Entry::Occupied(_) => bail!("report contains duplicate benchmark '{}'", entry.identity),
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

fn callgrind_metric(report: &ReportIndex, identity: &str, key: &str) -> Option<u64> {
    [identity, &format!("{identity}/run")]
        .into_iter()
        .filter_map(|candidate| report.get(candidate))
        .find_map(|entry| {
            let engine = entry
                .results
                .iter()
                .find(|(name, result)| name.starts_with("gungraun.") && result.metrics.contains_key(key))?;
            engine.1.metrics.get(key)?.value.as_u64()
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
            for (index, chunk) in bytes[first..].chunks(3).enumerate() {
                if !(index == 0 && first == 0) {
                    out.push(',');
                }
                out.push_str(std::str::from_utf8(chunk).expect("ASCII digits from u64::to_string"));
            }
            out
        }
    }
}

fn fmt_ratio(value: Option<f64>, baseline: Option<f64>) -> String {
    match (value, baseline) {
        (Some(value), Some(baseline)) if baseline != 0.0 => format!("{:.2}×", value / baseline),
        _ => "—".into(),
    }
}

fn write_query_report(out: &mut String, report: &ReportIndex) {
    out.push_str("\n## Query codecs\n\n");
    out.push_str(
        "Each table compares complete typed parsing or canonical production of \
         the same schema and values. Ratios are relative to Routerama; lower is \
         better. Reserved production reuses a caller-provided `String`, while \
         allocating production returns a new `String`.\n\n",
    );
    for (group_index, group) in QUERY_GROUPS.iter().enumerate() {
        let baseline = group.variants.first().expect("every query group has a Routerama baseline");
        let baseline_time = criterion_time_ns(report, baseline.identity);
        let baseline_instructions = callgrind_metric(report, baseline.identity, "Ir");

        let _ = writeln!(out, "### {}\n", group.title);
        out.push_str("| Implementation | Time | Time vs Routerama | Instructions | Instructions vs Routerama |\n");
        out.push_str("|---|---:|---:|---:|---:|\n");
        for variant in group.variants {
            let time = criterion_time_ns(report, variant.identity);
            let instructions = callgrind_metric(report, variant.identity, "Ir");
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} |",
                variant.label,
                fmt_ns(time),
                fmt_ratio(time, baseline_time),
                fmt_int(instructions),
                fmt_ratio(
                    instructions.map(|value| value as f64),
                    baseline_instructions.map(|value| value as f64)
                ),
            );
        }
        if group_index + 1 != QUERY_GROUPS.len() {
            out.push('\n');
        }
    }
}

fn build_report(report: &ReportIndex) -> String {
    let mut out = String::new();
    out.push_str("# Routerama Performance Report\n\n");
    out.push_str("Generated by `scripts/perf_report.rs`:\n");
    out.push_str(
        "- `cargo bench --bench routerama` — consolidated Criterion and Gungraun \
         router/query suite; this script forwards `--criterion` and, when \
         available, `--gungraun` to the same benchmark binary.\n\n",
    );
    out.push_str(
        "**Workload:** one full sweep of the shared request-path lookups (see \
         `benches/common/routes_data.rs`) against each router. Every router is \
         built from the same route table (literal segments plus single-segment \
         `{var}` parameters — the common subset all of them express) in a setup \
         step that is excluded from the measured region. Setup also performs one \
         full unmeasured lookup sweep to initialize lazy matcher and allocator \
         state before measuring the steady-state hot path; `routerama_static` is \
         the compile-time `#[resolver]` router, so it has no construction cost, \
         while `routerama_dynamic` is the run-time router registered from the \
         same table.\n\n",
    );
    out.push_str(
        "**Apples-to-apples:** every router is driven to the same *typed* end \
         state — the HTTP method (verb) validated against the request and every \
         captured path variable coerced into its declared type (`u32` parsed, \
         `String` percent-decoded and owned, `&str` borrowed). `routerama` \
         reaches this in one step (a typed enum variant with the method already \
         matched and every field coerced); the third-party routers only *select* \
         a route, so the harness explicitly checks the method and coerces each \
         parameter the same way afterwards. `regex` selects the winner with a \
         `RegexSet` and then re-scans it with the winning `Regex` to capture (two \
         passes), so it does structurally more work — read it as an upper bound \
         for a regex-based router reaching the same end state.  \n",
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
    for identity in VARIANTS {
        let variant = identity.strip_prefix("compare_routers/").expect("variant identity stays within compare_routers");
        let time = criterion_time_ns(report, identity);
        let instructions = callgrind_metric(report, identity, "Ir");
        let branch_misses = callgrind_metric(report, identity, "Bcm");
        let memory_accesses = match (
            callgrind_metric(report, identity, "L1hits"),
            callgrind_metric(report, identity, "LLhits"),
            callgrind_metric(report, identity, "RamHits"),
        ) {
            (Some(l1), Some(ll), Some(ram)) => Some(l1 + ll + ram),
            _ => None,
        };
        let _ = writeln!(
            out,
            "| `{variant}` | {} | {} | {} | {} |",
            fmt_ns(time),
            fmt_int(instructions),
            fmt_int(branch_misses),
            fmt_int(memory_accesses),
        );
    }
    write_query_report(&mut out, report);
    out
}

/// The `routerama` crate root (the parent of this script's `scripts/` directory).
fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts/ always has a parent crate directory")
        .to_path_buf()
}

/// The header prepended to the generated benchmark router.
const ROUTER_HEADER: &str = "\
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// GENERATED FILE — do not edit by hand. Regenerate after editing
// `routes_data.rs` with `scripts/perf_report.rs --regenerate-router`.
//
// Static and dynamic typed routers generated from `routes_data.rs`.

";

/// The `{name}` captures of a `template`, in left-to-right order.
fn capture_names(template: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        names.push(&after[..close]);
        rest = &after[close + 1..];
    }
    names
}

/// `UpperCamelCase` route name to the `snake_case` `add_<variant>` method stem.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The static-router field type for a capture of type `ty` (may borrow).
fn static_field_ty(ty: Ty) -> &'static str {
    match ty {
        Ty::Str => "&'p str",
        Ty::U32 => "u32",
        Ty::Owned => "String",
    }
}

/// The dynamic-router field type for a capture of type `ty` (always owned).
fn dynamic_field_ty(ty: Ty) -> &'static str {
    match ty {
        Ty::Str | Ty::Owned => "String",
        Ty::U32 => "u32",
    }
}

/// Emits one `#[resolver]` variant (`Name` or `Name { field: Type, .. }`),
/// typing each field via `field_ty`.
fn emit_variant(out: &mut String, name: &str, template: &str, tys: &[Ty], field_ty: fn(Ty) -> &'static str) {
    let names = capture_names(template);
    if names.is_empty() {
        let _ = writeln!(out, "    {name},");
        return;
    }
    let fields: Vec<String> = names
        .iter()
        .zip(tys.iter().copied())
        .map(|(field, ty)| format!("{field}: {}", field_ty(ty)))
        .collect();
    let _ = writeln!(out, "    {name} {{ {} }},", fields.join(", "));
}

/// Regenerates `benches/common/bench_router.rs` from the `ROUTES` table: two
/// `#[resolver]` routers (static + dynamic) whose fields carry the capture types
/// the table declares, so the committed benchmark router stays in sync.
fn regenerate_router(crate_dir: &Path) -> Result<(), AppError> {
    let _ = LOOKUPS;
    let mut code = String::new();

    code.push_str("/// Static typed router: `#[resolver]` bakes the trie at compile time and\n");
    code.push_str("/// coerces each capture into its field type.\n");
    code.push_str("#[::routerama::resolver]\n#[derive(Debug)]\nenum BenchRoute<'p> {\n");
    for (name, template, tys) in ROUTES {
        let _ = writeln!(code, "    #[route(GET, {template:?})]");
        emit_variant(&mut code, name, template, tys, static_field_ty);
    }
    code.push_str("}\n\n");

    code.push_str("/// Dynamic typed router: the same routes registered at run time through the\n");
    code.push_str("/// generated builder. Dynamic captures are always owned.\n");
    code.push_str("#[::routerama::resolver]\n#[derive(Debug)]\nenum BenchDynRoute {\n");
    for (name, _template, tys) in ROUTES {
        emit_variant(&mut code, name, _template, tys, dynamic_field_ty);
    }
    code.push_str("}\n\n");

    code.push_str("/// Builds the dynamic typed router by registering every benchmark route at\n");
    code.push_str("/// run time (part of the non-measured setup step).\n");
    code.push_str("#[expect(clippy::too_many_lines, reason = \"one fluent call per benchmark route\")]\n");
    code.push_str("fn build_bench_dyn() -> BenchDynRouteResolver {\n    BenchDynRoute::builder()\n");
    for (name, template, _tys) in ROUTES {
        let _ = writeln!(
            code,
            "        .add_{}(::routerama::HttpMethod::GET, {template:?})",
            snake_case(name)
        );
    }
    code.push_str("        .build()\n        .expect(\"every dynamic bench route registers\")\n}\n");

    let file: syn::File = syn::parse_str(&code).map_err(|e| app_err!("generated benchmark resolver is not valid Rust: {e}"))?;
    let body = prettyplease::unparse(&file);

    let out_path = crate_dir.join("benches").join("common").join("bench_router.rs");
    fs::write(&out_path, format!("{ROUTER_HEADER}{body}"))
        .map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;
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
        .is_ok_and(|status| status.success())
}

fn run_benchmark(crate_dir: &Path, arguments: &[OsString]) -> Result<(), AppError> {
    println!("==> Running cargo bench --bench routerama");
    let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.current_dir(crate_dir).arg("bench").arg("--bench").arg("routerama");
    command.args(arguments);
    let output = command
        .output()
        .map_err(|e| app_err!("failed to spawn cargo bench --bench routerama: {e}"))?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        let _ = std::io::stderr().write_all(combined.as_bytes());
        bail!("cargo bench --bench routerama failed with status {}", output.status);
    }
    Ok(())
}

fn run(args: &Args) -> Result<(), AppError> {
    let crate_dir = crate_root();

    if args.regenerate_router {
        return regenerate_router(&crate_dir);
    }

    let run_gungraun = if !cfg!(target_os = "linux") {
        if !args.no_gungraun {
            eprintln!(
                "note: skipping gungraun benches; metabench only supports Gungraun on Linux. \
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

    let (default_samples, default_measurement) = if args.fast { (10, 1) } else { (30, 2) };
    let samples = args.samples.unwrap_or(default_samples).to_string();
    let measurement = args.measurement_time.unwrap_or(default_measurement).to_string();
    let warmup = args.warm_up_time.unwrap_or(1).to_string();

    let target_dir = crate_dir.join("target");
    fs::create_dir_all(&target_dir).map_err(|e| app_err!("creating {}: {e}", target_dir.display()))?;
    let json_path = target_dir.join("routerama-perf-report.json");
    let markdown_path = target_dir.join("routerama-perf-report.md");

    let mut bench_args = vec![OsString::from("--")];
    bench_args.push(OsString::from("--criterion"));
    if run_gungraun {
        bench_args.push(OsString::from("--gungraun"));
    }
    for forwarded in [
        "--warm-up-time",
        warmup.as_str(),
        "--measurement-time",
        measurement.as_str(),
        "--sample-size",
        samples.as_str(),
    ] {
        bench_args.push(OsString::from("--criterion-arg"));
        bench_args.push(OsString::from(forwarded));
    }
    bench_args.push(OsString::from("--no-baseline"));
    bench_args.push(OsString::from("--export-json"));
    bench_args.push(json_path.as_os_str().to_owned());
    bench_args.push(OsString::from("--export-md"));
    bench_args.push(markdown_path.as_os_str().to_owned());

    run_benchmark(&crate_dir, &bench_args)?;

    println!("==> Building docs/PERF.md");
    let json = fs::read_to_string(&json_path).map_err(|e| app_err!("reading {}: {e}", json_path.display()))?;
    let report: JsonReport = serde_json::from_str(&json).map_err(|e| app_err!("parsing {}: {e}", json_path.display()))?;
    let report = index_report(report)?;

    let expected = VARIANTS
        .iter()
        .copied()
        .chain(QUERY_GROUPS.iter().flat_map(|group| group.variants.iter().map(|variant| variant.identity)));
    let missing: Vec<&str> = expected.filter(|key| criterion_time_ns(&report, key).is_none()).collect();
    if !missing.is_empty() {
        bail!(
            "report is missing {} expected criterion benchmark(s): {}",
            missing.len(),
            missing.join(", ")
        );
    }

    let markdown = build_report(&report);
    let out_path = crate_dir.join("docs").join("PERF.md");
    fs::write(&out_path, &markdown).map_err(|e| app_err!("writing {}: {e}", out_path.display()))?;

    println!(
        "Wrote {} ({} metabench identities)",
        out_path.display(),
        report.len(),
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
