// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Snapshot tests for what the two entry points expand to.
//!
//! For a procedural macro the generated code *is* the behaviour, so each case snapshots the
//! whole expansion rather than probing it with substring assertions. Every test drives
//! `event` / `derive_enrichment`, so the snapshots include the re-emitted struct as well as
//! the generated impl.
//!
//! insta cannot run under miri: `insta::_macro_support::get_cargo_workspace` reads from disk
//! and miri's isolation rejects that, so this whole file is compiled out under miri. The
//! assertions that carry no snapshot live in `entry_points.rs` and do run under miri.

#![cfg(not(miri))]

use observed_macros_impl::{derive_enrichment, event};
use testing_aids::{render_expansion, tokenize};

fn expand_event(attr: &str, item: &str) -> String {
    render_expansion(event(tokenize(attr), tokenize(item)).expect("the event attribute expands"))
}

fn expand_enrichment(item: &str) -> String {
    render_expansion(derive_enrichment(tokenize(item)).expect("the derive expands"))
}

// ============================================================================================
// #[event(...)]
// ============================================================================================

#[test]
fn an_event_re_emits_its_struct_without_the_attributes_it_consumed() {
    // `#[info]`, `#[dimension]`, `#[unredacted]` and `#[data_class]` are consumed by the
    // macro and must not survive into the re-emitted struct, while unrelated attributes
    // (`#[derive]`, `#[allow]`) have to be left exactly where the author put them.
    insta::assert_snapshot!(expand_event(
        r#""http.request""#,
        r#"
        #[derive(Debug)]
        #[info("{method} handled as {status}", name = "http.request.log")]
        struct HttpRequest {
            #[dimension(log = "method")]
            method: ClassifiedString,
            #[allow(dead_code)]
            #[unredacted]
            status: i64,
            #[data_class(DataTaxonomy::Euii)]
            user: String,
            #[dimension(log = exclude)]
            request_id: ClassifiedString,
        }
        "#,
    ));
}

#[test]
fn dimension_routes_the_log_and_metric_signals_independently() {
    // Every routing the attribute can express, in one struct: the log key defaults to the
    // field name, can be renamed, or dropped; the metric dimension stays opt-in and keys
    // itself either by the field name or by an explicit one.
    insta::assert_snapshot!(expand_event(
        r#""request""#,
        r#"
        #[info]
        struct Request {
            #[dimension]
            explicit_default: ClassifiedString,
            #[dimension(log = "renamed")]
            renamed: ClassifiedString,
            #[dimension(log = exclude)]
            hidden: ClassifiedString,
            #[dimension(metric)]
            #[unredacted]
            shard: i64,
            #[dimension(metric = "region.key")]
            #[unredacted]
            region: i64,
            #[dimension(log = "http.status", metric = "status")]
            #[unredacted]
            status: i64,
            #[dimension(log = exclude, metric)]
            #[unredacted]
            internal: i64,
        }
        "#,
    ));
}

#[test]
fn every_instrument_kind_records_the_field_it_names() {
    insta::assert_snapshot!(expand_event(
        r#""operation""#,
        r#"
        #[info]
        #[counter(hits, name = "operation.hits", desc = "completed operations", unit = "1")]
        #[updown_counter(queue_delta)]
        #[gauge(level)]
        #[histogram(duration_ms, unit = "ms")]
        struct Operation {
            #[unredacted]
            hits: u64,
            #[unredacted]
            queue_delta: i64,
            #[unredacted]
            level: f64,
            #[unredacted]
            duration_ms: f32,
        }
        "#,
    ));
}

#[test]
fn a_fieldless_counter_becomes_the_event_level_metric() {
    // No severity attribute, so the event carries a metric and nothing else: `shard` is
    // routed to neither signal and must therefore generate no visit at all.
    insta::assert_snapshot!(expand_event(
        r#""heartbeat""#,
        r#"
        #[counter(name = "heartbeat.total", desc = "heartbeats", unit = "1")]
        struct Heartbeat {
            #[unredacted]
            shard: i64,
        }
        "#,
    ));
}

#[test]
fn an_event_with_no_signal_at_all_visits_nothing() {
    insta::assert_snapshot!(expand_event(
        r#""silent""#,
        r"
        struct Silent {
            tenant: ClassifiedString,
        }
        ",
    ));
}

#[test]
fn a_disabled_event_records_that_in_its_description() {
    insta::assert_snapshot!(expand_event(
        r#""audit.write", disabled"#,
        r"
        #[warning]
        struct AuditWrite {
            #[unredacted]
            attempts: i64,
        }
        ",
    ));
}

#[test]
fn a_unit_struct_event_expands_to_an_empty_visitor() {
    insta::assert_snapshot!(expand_event(r#""no.fields""#, "#[info] struct NoFields;"));
}

#[test]
fn a_generic_event_spells_out_the_bounds_its_body_relies_on() {
    // Each redaction path expands to a different call and therefore needs different
    // bounds, and only fields that mention a type parameter get any. The pre-existing
    // `where` clause has to be extended rather than replaced, and the const parameter
    // has to survive into the `TypeId` arguments.
    insta::assert_snapshot!(expand_event(
        r#""generic""#,
        r"
        #[info]
        struct Generic<'a, T, const N: usize>
        where
            T: Clone,
        {
            #[unredacted]
            unredacted_value: T,
            #[data_class(DataTaxonomy::Euii)]
            classified: T,
            #[data_class(DataTaxonomy::Euii)]
            classified_ref: &'a T,
            defaulted: T,
            optional: Option<T>,
            matrix: [T; N],
            #[unredacted]
            concrete: i64,
        }
        ",
    ));
}

#[test]
fn a_borrowed_string_is_copied_but_a_static_one_is_not() {
    // `Value` owns its data, so a non-`'static` `&str` has to be copied into an
    // `Arc<str>`. `&'static str` is stored as-is, and a reference to a non-string type
    // goes through the ordinary conversion.
    insta::assert_snapshot!(expand_event(
        r#""strings""#,
        r"
        #[info]
        struct Strings<'a> {
            #[unredacted]
            borrowed: &'a str,
            #[unredacted]
            qualified: &'a std::primitive::str,
            #[unredacted]
            immortal: &'static str,
            #[unredacted]
            numeric_ref: &'a u64,
            #[unredacted]
            owned: String,
            redactable_ref: &'a Redactable,
            parenthesized: (&'a Redactable),
            #[data_class(DataTaxonomy::Euii)]
            classified_ref: &'a Classified,
        }
        ",
    ));
}

#[test]
fn an_optional_field_follows_its_if_none_setting() {
    insta::assert_snapshot!(expand_event(
        r#""optional""#,
        r#"
        #[info]
        struct Optional<'a> {
            default_placeholder: Option<ClassifiedString>,
            #[if_none(drop)]
            dropped: Option<ClassifiedString>,
            #[if_none("missing")]
            custom_placeholder: Option<ClassifiedString>,
            #[dimension(metric = "opt.shard")]
            #[unredacted]
            dimension: Option<i64>,
            #[unredacted]
            borrowed_inner: Option<&'a str>,
            #[if_none(drop)]
            #[data_class(DataTaxonomy::Euii)]
            classified: Option<String>,
        }
        "#,
    ));
}

#[test]
fn a_raw_identifier_field_is_reported_under_its_domain_name() {
    // `r#type` addresses the field, but telemetry must see `type` rather than Rust's
    // raw-identifier escape.
    insta::assert_snapshot!(expand_event(
        r#""raw""#,
        r"
        #[info]
        struct Raw {
            r#type: ClassifiedString,
            #[dimension(metric)]
            #[unredacted]
            r#match: i64,
        }
        ",
    ));
}

// ============================================================================================
// #[derive(Enrichment)]
// ============================================================================================

#[test]
fn an_enrichment_pushes_one_entry_per_field() {
    insta::assert_snapshot!(expand_enrichment(
        r#"
        struct RequestContext {
            method: ClassifiedString,
            #[unredacted]
            status: i64,
            #[data_class(DataTaxonomy::Euii)]
            username: String,
            #[dimension(log = "http.method")]
            renamed: ClassifiedString,
            #[dimension(log = exclude)]
            internal: ClassifiedString,
        }
        "#,
    ));
}

#[test]
fn an_enrichment_routes_the_log_and_metric_signals_independently() {
    insta::assert_snapshot!(expand_enrichment(
        r#"
        struct RequestContext<'a> {
            #[dimension(metric)]
            #[unredacted]
            shard: i64,
            #[dimension(metric = "region.key")]
            #[unredacted]
            region: i64,
            #[dimension(log = "http.status", metric = "status")]
            #[unredacted]
            status: i64,
            #[dimension(log = exclude, metric)]
            #[unredacted]
            internal: i64,
            #[unredacted]
            borrowed: &'a str,
        }
        "#,
    ));
}

#[test]
fn an_optional_enrichment_field_follows_its_if_none_setting() {
    insta::assert_snapshot!(expand_enrichment(
        r#"
        struct RequestContext {
            tenant: ClassifiedString,
            default_placeholder: Option<ClassifiedString>,
            #[if_none(drop)]
            dropped: Option<ClassifiedString>,
            #[if_none("missing")]
            custom_placeholder: Option<ClassifiedString>,
        }
        "#,
    ));
}

#[test]
fn a_generic_enrichment_spells_out_the_bounds_its_body_relies_on() {
    // Values are moved rather than cloned, so no `Clone` bound appears; the redacted
    // paths store the value as a trait object and so add `Send + Sync + 'static`.
    insta::assert_snapshot!(expand_enrichment(
        r"
        struct GenericCtx<'a, T> {
            #[unredacted]
            unredacted_value: T,
            #[data_class(DataTaxonomy::Euii)]
            classified: T,
            defaulted: T,
            optional: Option<T>,
            #[unredacted]
            label: &'a str,
            #[unredacted]
            concrete: i64,
        }
        ",
    ));
}
