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
syn = { version = "2", features = ["full", "parsing"] }
---

//! Run the criterion + gungraun router and query-codec suites and rebuild
//! `docs/PERF.md`.
//!
//! Runs Criterion and, where Valgrind is available, Gungraun.
//!
//! Usage:
//!   `scripts/perf_report.rs`                                    — full run (30 samples, 2s measurement)
//!   `scripts/perf_report.rs --fast`                             — quick run (10 samples, 1s)
//!   `scripts/perf_report.rs --samples 50 --measurement-time 3`  — custom criterion settings
//!   `scripts/perf_report.rs --no-gungraun`                      — criterion only
//!
//! Criterion and Gungraun variants are paired through `VARIANTS`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::{env, fs};

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

    /// Regenerate the committed benchmark router (`benches/common/bench_router.rs`)
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
/// (`compare_routers::<name>`). `routerama_static` is the compile-time
/// `#[resolver]` router and `routerama_dynamic` the run-time one built from the
/// same table; both coerce captures to typed fields. The rest are third-party
/// runtime routers driven to the same typed end state in a (non-measured) setup
/// step.
const VARIANTS: &[&str] = &[
    "routerama_static",
    "routerama_dynamic",
    "matchit",
    "path_tree",
    "regex",
    "route_recognizer",
];

struct QueryVariant {
    label: &'static str,
    criterion: &'static str,
    gungraun: &'static str,
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
                criterion: "routerama_query/parse_common/routerama",
                gungraun: "parse_common_routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                criterion: "routerama_query/parse_common/serde_urlencoded",
                gungraun: "parse_common_serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                criterion: "routerama_query/parse_common/serde_html_form",
                gungraun: "parse_common_serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Parse: escaped query",
        variants: &[
            QueryVariant {
                label: "routerama",
                criterion: "routerama_query/parse_escaped/routerama",
                gungraun: "parse_escaped_routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                criterion: "routerama_query/parse_escaped/serde_urlencoded",
                gungraun: "parse_escaped_serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                criterion: "routerama_query/parse_escaped/serde_html_form",
                gungraun: "parse_escaped_serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Parse: repeated values",
        variants: &[
            QueryVariant {
                label: "routerama",
                criterion: "routerama_query/parse_repeated/routerama",
                gungraun: "parse_repeated_routerama",
            },
            QueryVariant {
                label: "serde_html_form",
                criterion: "routerama_query/parse_repeated/serde_html_form",
                gungraun: "parse_repeated_serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Parse: long ASCII value",
        variants: &[
            QueryVariant {
                label: "routerama",
                criterion: "routerama_query/parse_long_ascii/routerama",
                gungraun: "parse_long_routerama",
            },
            QueryVariant {
                label: "serde_urlencoded",
                criterion: "routerama_query/parse_long_ascii/serde_urlencoded",
                gungraun: "parse_long_serde_urlencoded",
            },
            QueryVariant {
                label: "serde_html_form",
                criterion: "routerama_query/parse_long_ascii/serde_html_form",
                gungraun: "parse_long_serde_html_form",
            },
        ],
    },
    QueryGroup {
        title: "Produce: caller-provided buffer",
        variants: &[
            QueryVariant {
                label: "routerama",
                criterion: "routerama_query/produce_common/routerama_reserved",
                gungraun: "produce_common_routerama_reserved",
            },
            QueryVariant {
                label: "serde_html_form",
                criterion: "routerama_query/produce_common/serde_html_form_reserved",
                gungraun: "produce_common_serde_html_form_reserved",
            },
        ],
    },
    QueryGroup {
        title: "Produce: allocating",
        variants: &[
            QueryVariant {
                label: "routerama",
                criterion: "routerama_query/produce_common_allocating/routerama",
                gungraun: "produce_common_routerama_allocating",
            },
            QueryVariant {
                label: "serde_urlencoded",
                criterion: "routerama_query/produce_common_allocating/serde_urlencoded",
                gungraun: "produce_common_serde_urlencoded_allocating",
            },
            QueryVariant {
                label: "serde_html_form",
                criterion: "routerama_query/produce_common_allocating/serde_html_form",
                gungraun: "produce_common_serde_html_form_allocating",
            },
        ],
    },
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
    if s.is_empty() || !s.contains('/') || s.contains(':') || s.contains(char::is_whitespace) {
        return false;
    }
    let id_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    s.split('/').all(|part| !part.is_empty() && part.chars().all(id_char))
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
    lookup_time_by_key(crit, &key)
}

fn lookup_time_by_key(crit: &[(String, f64)], key: &str) -> Option<f64> {
    crit.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
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
    for line in text.lines() {
        let rest = line
            .strip_prefix("gungraun_routers::")
            .or_else(|| line.strip_prefix("routerama_query_cg::"));
        if let Some(rest) = rest
            && let Some(after_mod) = rest.find("::")
        {
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

fn fmt_ratio(value: Option<f64>, baseline: Option<f64>) -> String {
    match (value, baseline) {
        (Some(value), Some(baseline)) if baseline != 0.0 => format!("{:.2}×", value / baseline),
        _ => "—".into(),
    }
}

fn write_query_report(out: &mut String, crit: &[(String, f64)], gung: &[GungEntry]) {
    out.push_str("\n## Query codecs\n\n");
    out.push_str(
        "Each table compares complete typed parsing or canonical production of \
         the same schema and values. Ratios are relative to Routerama; lower is \
         better. Reserved production reuses a caller-provided `String`, while \
         allocating production returns a new `String`.\n\n",
    );
    for (group_index, group) in QUERY_GROUPS.iter().enumerate() {
        let baseline = group.variants.first().expect("every query group has a Routerama baseline");
        let baseline_time = lookup_time_by_key(crit, baseline.criterion);
        let baseline_instructions = gung_metric(gung, baseline.gungraun, "Instructions");

        let _ = writeln!(out, "### {}\n", group.title);
        out.push_str("| Implementation | Time | Time vs Routerama | Instructions | Instructions vs Routerama |\n");
        out.push_str("|---|---:|---:|---:|---:|\n");
        for variant in group.variants {
            let time = lookup_time_by_key(crit, variant.criterion);
            let instructions = gung_metric(gung, variant.gungraun, "Instructions");
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

fn build_report(crit: &[(String, f64)], gung: &[GungEntry]) -> String {
    let mut out = String::new();
    out.push_str("# Routerama Performance Report\n\n");
    out.push_str("Generated by `scripts/perf_report.rs`:\n");
    out.push_str("- `cargo bench --bench criterion_routers` — criterion wall-clock timings.\n");
    out.push_str("- `cargo bench --bench gungraun_routers` — Callgrind instruction-precise counts.\n");
    out.push_str("- `cargo bench --bench routerama_query` — differential query-codec timings.\n");
    out.push_str("- `cargo bench --bench routerama_query_cg` — differential query-codec instruction counts.\n\n");
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
    write_query_report(&mut out, crit, gung);
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
    for (name, template, tys) in ROUTES {
        emit_variant(&mut code, name, template, tys, dynamic_field_ty);
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
    let query_crit_log = run_bench(
        &crate_dir,
        "routerama_query",
        &crit_args,
        &format!("routerama_query: {samples} samples, {meas}s measurement"),
    )?;
    let mut gung_log = if run_gungraun {
        run_bench(&crate_dir, "gungraun_routers", &[], "gungraun_routers")?
    } else {
        String::new()
    };
    if run_gungraun {
        gung_log.push_str(&run_bench(&crate_dir, "routerama_query_cg", &[], "routerama_query_cg")?);
    }

    println!("==> Building docs/PERF.md");
    let crit = parse_criterion(&(crit_log + &query_crit_log));
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
