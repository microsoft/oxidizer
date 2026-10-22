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

//! Runs performance suites and rebuilds `docs/PERF.md` and `docs/PERF-INTERNAL.md`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::{env, fs};

use clap::Parser;
use ohno::{AppError, app_err, bail};

/// Run the routerama benchmark suites and rebuild the two performance reports.
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

    /// Skip the gungraun (Callgrind) suite; `docs/PERF-INTERNAL.md` is left untouched.
    ///
    /// The customer-facing `docs/PERF.md` carries no instruction counts and is unaffected.
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

/// Writes a Mermaid `xychart-beta` bar chart for one comparison group.
///
/// The chart accompanies the table it precedes and never replaces it, so the exact figures stay
/// available. Series with fewer than two measured bars are skipped, because a one-bar chart says
/// less than the row it would duplicate.
fn write_chart<'a>(
    out: &mut String,
    title: &str,
    y_label: &str,
    series: impl IntoIterator<Item = (&'a str, Option<f64>)>,
) {
    let points: Vec<(&str, f64)> = series
        .into_iter()
        .filter_map(|(label, value)| value.map(|value| (label, value)))
        .collect();
    if points.len() < 2 {
        return;
    }

    // Mermaid draws no bar at all when the axis starts above zero, so the range is always
    // anchored at zero and padded above the tallest bar to leave room for the plot frame.
    let max = points.iter().map(|&(_, value)| value).fold(0.0_f64, f64::max);
    let top = if max > 0.0 { max * 1.1 } else { 1.0 };

    out.push_str("```mermaid\nxychart-beta\n");
    let _ = writeln!(out, "    title \"{title}\"");
    out.push_str("    x-axis [");
    for (index, (label, _)) in points.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "\"{label}\"");
    }
    out.push_str("]\n");
    let _ = writeln!(out, "    y-axis \"{y_label}\" 0 --> {top:.0}");
    out.push_str("    bar [");
    for (index, (_, value)) in points.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{value:.0}");
    }
    out.push_str("]\n```\n\n");
}

/// One region of a report file that this script owns and rewrites in place.
struct Block {
    /// The name appearing in the `BEGIN GENERATED` / `END GENERATED` markers.
    name: &'static str,
    /// The Markdown that replaces whatever currently sits between those markers.
    body: String,
}

/// Builds the query-codec wall-clock tables for the customer-facing report.
fn query_time_tables(crit: &[(String, f64)]) -> String {
    let mut out = String::new();
    for (group_index, group) in QUERY_GROUPS.iter().enumerate() {
        let baseline = group.variants.first().expect("every query group has a Routerama baseline");
        let baseline_time = lookup_time_by_key(crit, baseline.criterion);

        let _ = writeln!(out, "### {}\n", group.title);
        write_chart(
            &mut out,
            &format!("{} — time (lower is better)", group.title),
            "Time (ns)",
            group.variants.iter().map(|variant| (variant.label, lookup_time_by_key(crit, variant.criterion))),
        );
        out.push_str("| Implementation | Time | Time vs Routerama |\n");
        out.push_str("|---|---:|---:|\n");
        for variant in group.variants {
            let time = lookup_time_by_key(crit, variant.criterion);
            let _ = writeln!(out, "| `{}` | {} | {} |", variant.label, fmt_ns(time), fmt_ratio(time, baseline_time));
        }
        if group_index + 1 != QUERY_GROUPS.len() {
            out.push('\n');
        }
    }
    out
}

/// Builds the query-codec instruction-count tables for the internal report.
fn query_instruction_tables(gung: &[GungEntry]) -> String {
    let mut out = String::new();
    for (group_index, group) in QUERY_GROUPS.iter().enumerate() {
        let baseline = group.variants.first().expect("every query group has a Routerama baseline");
        let baseline_instructions = gung_metric(gung, baseline.gungraun, "Instructions");

        let _ = writeln!(out, "### {}\n", group.title);
        out.push_str("| Implementation | Instructions | Instructions vs Routerama |\n");
        out.push_str("|---|---:|---:|\n");
        for variant in group.variants {
            let instructions = gung_metric(gung, variant.gungraun, "Instructions");
            let _ = writeln!(
                out,
                "| `{}` | {} | {} |",
                variant.label,
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
    out
}

/// Builds the generated blocks of the customer-facing `docs/PERF.md`.
///
/// Only wall-clock comparisons appear here. Instruction counts and the engineering records that
/// accompany them are not a comparative claim, so they live in `docs/PERF-INTERNAL.md` instead.
fn build_report(crit: &[(String, f64)]) -> Vec<Block> {
    let mut resolver = String::new();
    let baseline_time = lookup_time(crit, VARIANTS.first().expect("VARIANTS lists the Routerama baseline first"));
    write_chart(
        &mut resolver,
        "Full lookup sweep — time (lower is better)",
        "Time (ns)",
        VARIANTS.iter().map(|variant| (*variant, lookup_time(crit, variant))),
    );
    resolver.push_str("| Resolver | Time (criterion) | Time vs Routerama |\n");
    resolver.push_str("|---|---:|---:|\n");
    for variant in VARIANTS {
        let time = lookup_time(crit, variant);
        let _ = writeln!(resolver, "| `{variant}` | {} | {} |", fmt_ns(time), fmt_ratio(time, baseline_time));
    }

    vec![
        Block { name: "resolver-comparison", body: resolver },
        Block { name: "query-codecs", body: query_time_tables(crit) },
    ]
}

/// Builds the generated blocks of the internal `docs/PERF-INTERNAL.md`.
fn build_internal_report(gung: &[GungEntry]) -> Vec<Block> {
    let mut resolver = String::new();
    resolver.push_str("| Resolver | Instructions | Branch misses | Total reads+writes |\n");
    resolver.push_str("|---|---:|---:|---:|\n");
    for variant in VARIANTS {
        let _ = writeln!(
            resolver,
            "| `{variant}` | {} | {} | {} |",
            fmt_int(gung_metric(gung, variant, "Instructions")),
            fmt_int(gung_metric(gung, variant, "Bcm")),
            fmt_int(gung_metric(gung, variant, "Total read+write")),
        );
    }

    vec![
        Block { name: "resolver-instructions", body: resolver },
        Block { name: "query-instructions", body: query_instruction_tables(gung) },
    ]
}

/// Replaces each generated block in `existing` with its freshly measured body.
///
/// Everything outside the markers is hand-written prose and is copied through untouched, so a
/// regeneration can never silently discard an explanation, a dated provenance note, or a caveat.
/// A block named here but absent from the file is an error rather than a silent no-op, because
/// that means the measurement it carries would vanish from the report.
fn splice_blocks(existing: &str, blocks: &[Block]) -> Result<String, AppError> {
    let mut out = existing.to_string();
    for block in blocks {
        let begin_marker = format!("<!-- BEGIN GENERATED: {} -->", block.name);
        let end_marker = format!("<!-- END GENERATED: {} -->", block.name);

        let begin = out
            .find(&begin_marker)
            .ok_or_else(|| app_err!("report is missing the marker `{begin_marker}`"))?;
        let body_start = begin + begin_marker.len();
        let end = out[body_start..]
            .find(&end_marker)
            .map(|offset| body_start + offset)
            .ok_or_else(|| app_err!("report is missing the marker `{end_marker}` after `{begin_marker}`"))?;

        out.replace_range(body_start..end, &format!("\n\n{}\n", block.body.trim_end()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::Block;
    use super::splice_blocks;
    use super::write_chart;

    #[test]
    fn splicing_replaces_block_bodies_and_preserves_surrounding_prose() {
        let existing = "\
# Report

Hand-written preface.

## Resolvers

A dated provenance note that must survive.

<!-- BEGIN GENERATED: resolver-comparison -->

stale table

<!-- END GENERATED: resolver-comparison -->

A caveat written by hand after the table.
";
        let blocks = [Block {
            name: "resolver-comparison",
            body: "| a |\n|---|\n| 1 |\n".to_string(),
        }];

        assert_eq!(
            splice_blocks(existing, &blocks).unwrap(),
            "\
# Report

Hand-written preface.

## Resolvers

A dated provenance note that must survive.

<!-- BEGIN GENERATED: resolver-comparison -->

| a |
|---|
| 1 |
<!-- END GENERATED: resolver-comparison -->

A caveat written by hand after the table.
"
        );
    }

    #[test]
    fn splicing_is_idempotent() {
        let existing = "<!-- BEGIN GENERATED: x -->\n\nold\n<!-- END GENERATED: x -->\n";
        let blocks = [Block { name: "x", body: "new\n".to_string() }];

        let once = splice_blocks(existing, &blocks).unwrap();
        let twice = splice_blocks(&once, &blocks).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn a_missing_marker_is_an_error_rather_than_a_silent_no_op() {
        let blocks = [Block { name: "absent", body: String::new() }];
        assert!(splice_blocks("# Report\n", &blocks).is_err());
    }

    #[test]
    fn an_unterminated_block_is_an_error() {
        let blocks = [Block { name: "x", body: String::new() }];
        assert!(splice_blocks("<!-- BEGIN GENERATED: x -->\n", &blocks).is_err());
    }

    #[test]
    fn chart_is_skipped_when_fewer_than_two_bars_are_measured() {
        let mut out = String::new();
        write_chart(&mut out, "t", "y", [("a", Some(1.0)), ("b", None)]);
        assert!(out.is_empty());
    }

    #[test]
    fn chart_anchors_the_axis_at_zero_and_pads_above_the_tallest_bar() {
        let mut out = String::new();
        write_chart(&mut out, "Lookup", "Time (ns)", [("a", Some(100.0)), ("b", Some(200.0))]);
        assert!(out.starts_with("```mermaid\nxychart-beta\n"), "{out}");
        assert!(out.contains("    title \"Lookup\"\n"), "{out}");
        assert!(out.contains("    x-axis [\"a\", \"b\"]\n"), "{out}");
        assert!(out.contains("    y-axis \"Time (ns)\" 0 --> 220\n"), "{out}");
        assert!(out.contains("    bar [100, 200]\n"), "{out}");
        assert!(out.ends_with("```\n\n"), "{out}");
    }
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
    code.push_str("#[::routerama::resolve::resolver]\n#[derive(Debug)]\nenum BenchRoute<'p> {\n");
    for (name, template, tys) in ROUTES {
        let _ = writeln!(code, "    #[route(GET, {template:?})]");
        emit_variant(&mut code, name, template, tys, static_field_ty);
    }
    code.push_str("}\n\n");

    code.push_str("/// Dynamic typed router: the same routes registered at run time through the\n");
    code.push_str("/// generated builder. Dynamic captures are always owned.\n");
    code.push_str("#[::routerama::resolve::resolver]\n#[derive(Debug)]\nenum BenchDynRoute {\n");
    for (name, template, tys) in ROUTES {
        code.push_str("    #[route(dynamic)]\n");
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
            "        .add_{}(::routerama::resolve::HttpMethod::GET, {template:?})",
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
fn run_bench(cwd: &Path, bench: &str, features: &str, extra: &[&str], label: &str) -> Result<String, AppError> {
    println!("==> Running {label}");
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(cwd)
        .arg("bench")
        .arg("--features")
        .arg(features)
        .arg("--bench")
        .arg(bench);
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
                 docs/PERF-INTERNAL.md will not be regenerated."
            );
        }
        false
    } else if args.no_gungraun {
        eprintln!("note: --no-gungraun set; docs/PERF-INTERNAL.md will not be regenerated.");
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
        "resolve",
        &crit_args,
        &format!("criterion_routers: {samples} samples, {meas}s measurement"),
    )?;
    let query_crit_log = run_bench(
        &crate_dir,
        "routerama_query",
        "query",
        &crit_args,
        &format!("routerama_query: {samples} samples, {meas}s measurement"),
    )?;
    let mut gung_log = if run_gungraun {
        run_bench(&crate_dir, "gungraun_routers", "resolve", &[], "gungraun_routers")?
    } else {
        String::new()
    };
    if run_gungraun {
        gung_log.push_str(&run_bench(
            &crate_dir,
            "routerama_query_cg",
            "query",
            &[],
            "routerama_query_cg",
        )?);
    }

    println!("==> Building docs/PERF.md");
    let crit = parse_criterion(&(crit_log + &query_crit_log));
    let gung = parse_gungraun(&gung_log);

    let docs_dir = crate_dir.join("docs");
    write_merged(&docs_dir.join("PERF.md"), &build_report(&crit))?;

    if run_gungraun {
        println!("==> Building docs/PERF-INTERNAL.md");
        write_merged(&docs_dir.join("PERF-INTERNAL.md"), &build_internal_report(&gung))?;
    } else {
        println!("==> Skipping docs/PERF-INTERNAL.md (no gungraun run); docs/PERF.md is unaffected.");
    }

    println!(
        "Wrote docs/PERF.md ({} criterion, {} gungraun benches)",
        crit.len(),
        gung.len(),
    );
    println!("==> Done.");
    Ok(())
}

/// Regenerates one report in place, preserving every hand-written line it already holds.
fn write_merged(out_path: &Path, blocks: &[Block]) -> Result<(), AppError> {
    let existing =
        fs::read_to_string(out_path).map_err(|e| app_err!("reading {}: {e}", out_path.display()))?;
    let report = splice_blocks(&existing, blocks)
        .map_err(|e| app_err!("regenerating {}: {e}", out_path.display()))?;
    fs::write(out_path, &report).map_err(|e| app_err!("writing {}: {e}", out_path.display()))
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
