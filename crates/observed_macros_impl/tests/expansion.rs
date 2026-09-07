// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Snapshot tests for both entry points: what they expand to, and what they reject.
//!
//! For a procedural macro the generated code *is* the behaviour, so every case snapshots the
//! whole expansion -- or the whole diagnostic -- rather than probing it with a substring
//! assertion. A substring assertion marks the branch covered while proving almost nothing
//! about it: the rest of the output is free to be wrong. A snapshot pins all of it, so a
//! branch whose behaviour changes shows up in the diff whether or not anyone thought to
//! assert on that part.
//!
//! A table-driven case joins its rows into one snapshot, so a test still owns exactly one
//! snapshot. Every row records the whole source the macro was given before what it produced,
//! so a snapshot reads on its own and a diff still names the row that moved. A one-line
//! diagnostic is asserted inline instead, where the input is already next to it.
//!
//! insta cannot run under miri: `insta::_macro_support::get_cargo_workspace` reads from disk
//! and miri's isolation rejects that, so this whole file is compiled out under miri. The
//! crate is on the miri exclusion list in `main.yml`, so that costs no CI coverage.

#![cfg(not(miri))]

use observed_macros_impl::{derive_enrichment, event};
use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::quote;
use testing_aids::{render_expansion, tokenize};

fn expand_event(attr: &str, item: &str) -> String {
    render_expansion(&event(tokenize(attr), tokenize(item)).expect("the event attribute expands"))
}

fn expand_enrichment(item: &str) -> String {
    render_expansion(&derive_enrichment(tokenize(item)).expect("the derive expands"))
}

fn event_error(attr: &str, item: &str) -> String {
    event(tokenize(attr), tokenize(item))
        .expect_err("the event attribute is rejected")
        .to_string()
}

fn enrichment_error(item: &str) -> String {
    derive_enrichment(tokenize(item)).expect_err("the derive is rejected").to_string()
}

/// The source a user would write to reach `#[event(...)]`, for the input side of a report row.
fn event_source(attr: &str, item: &str) -> String {
    format!("#[event({attr})]\n{}", item.trim())
}

/// The source a user would write to reach `#[derive(Enrichment)]`.
fn enrichment_source(item: &str) -> String {
    format!("#[derive(Enrichment)]\n{}", item.trim())
}

/// Joins the rows of a table-driven case into a single snapshot.
///
/// Each row records the whole source the macro was given and then what it produced, so the
/// snapshot reads on its own: working out what a row covers does not mean opening the test
/// beside it. The source names its own entry point, which is what tells the two rows of one
/// input apart in a case that drives both macros.
fn report(rows: impl IntoIterator<Item = (String, String)>) -> String {
    rows.into_iter()
        .map(|(source, result)| format!("---- input ----\n{}\n\n---- yields ----\n{}\n", source.trim(), result.trim_end()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A struct body carrying one well-formed field, for the cases whose subject is the
/// struct-level attributes rather than the fields.
const ONE_FIELD: &str = "struct Subject { #[unredacted] v: u64 }";

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

// ============================================================================================
// Entry-point contract
// ============================================================================================

#[test]
fn each_entry_point_rejects_input_it_cannot_parse() {
    let output = report(
        ["1 + 1", "enum NotAStruct { A }", "fn not_a_struct() {}"]
            .into_iter()
            .flat_map(|item| {
                [
                    (event_source(r#""e""#, item), event_error(r#""e""#, item)),
                    (enrichment_source(item), enrichment_error(item)),
                ]
            }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn an_attribute_the_macro_does_not_own_is_left_untouched() {
    // A multi-segment path has no single identifier to match against a severity or an
    // instrument name, and an unrecognized field attribute belongs to some other macro.
    // Both must survive onto the re-emitted struct rather than being consumed or rejected.
    insta::assert_snapshot!(expand_event(
        r#""ignored""#,
        r#"
        #[some::other::attr]
        #[derive(Debug)]
        #[info]
        struct Ignored {
            #[serde(rename = "renamed")]
            #[unredacted]
            v: i64,
        }
        "#,
    ));
}

#[test]
fn every_severity_attribute_maps_to_its_own_variant() {
    // A dropped arm would leave the attribute unrecognized, silently demoting the event
    // to "no log signal" instead of failing to build.
    let output = report(
        ["trace", "debug", "info", "warning", "error", "fatal"]
            .into_iter()
            .map(|attribute| {
                let item = format!("#[{attribute}] {ONE_FIELD}");
                (event_source(r#""severity""#, &item), expand_event(r#""severity""#, &item))
            }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn a_macro_substituted_type_is_seen_through_its_invisible_group() {
    // A `macro_rules!` `$ty:ty` fragment reaches the macro wrapped in an invisible
    // (`Delimiter::None`) group, which `syn` parses as `syn::Type::Group`. Every
    // structural check has to look through it, or a type that arrived from a macro is
    // misclassified: no longer numeric, no longer an `Option`, no longer a borrowed `str`.
    let substituted = |source: &str| TokenStream::from(TokenTree::Group(Group::new(Delimiter::None, tokenize(source))));
    let unsigned = substituted("u64");
    let optional = substituted("Option<ClassifiedString>");
    let borrowed = substituted("&'a str");

    let expanded = event(
        quote!("macro.built"),
        quote! {
            #[info]
            #[counter(hits)]
            struct FromMacro<'a> {
                #[unredacted]
                hits: #unsigned,
                #[if_none(drop)]
                maybe: #optional,
                #[unredacted]
                label: #borrowed,
            }
        },
    )
    .expect("a macro-substituted type expands");

    insta::assert_snapshot!(render_expansion(&expanded));
}

#[test]
fn a_field_type_that_only_resembles_an_option_is_treated_as_a_plain_field() {
    // `Option<T>` is matched syntactically, so each of these has to fall through to the
    // non-optional path rather than being mistaken for an optional field.
    let output = report(
        [
            "(u8, u8)",              // not a path at all
            "<i32 as Copy>::Output", // a qualified self
            "Option",                // `Option` with no arguments
            "Option<u8, u16>",       // two generic arguments
            "Option<'a>",            // a lifetime where a type belongs
            "[T; 4]",                // a token group the type-parameter scan descends into
            "(T, u8)",               // a type parameter found before the end of a group
        ]
        .into_iter()
        .map(|ty| {
            let item = format!("#[info] struct Shapes<'a, T> {{ field: {ty} }}");
            (event_source(r#""shapes""#, &item), expand_event(r#""shapes""#, &item))
        }),
    );
    insta::assert_snapshot!(output);
}

// ============================================================================================
// `#[event(...)]` diagnostics
// ============================================================================================

#[test]
fn the_event_attribute_arguments_are_validated() {
    let output = report(
        ["", "bare_ident", "42", r#""e", bogus"#]
            .into_iter()
            .map(|attr| (event_source(attr, ONE_FIELD), event_error(attr, ONE_FIELD))),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn the_event_attribute_accepts_the_disabled_flag_and_a_trailing_comma() {
    let output = report(
        [r#""e""#, r#""e","#, r#""e", disabled"#, r#""e", disabled,"#]
            .into_iter()
            .map(|attr| (event_source(attr, ONE_FIELD), expand_event(attr, ONE_FIELD))),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn an_event_requires_named_fields() {
    insta::assert_snapshot!(
        event_error(r#""e""#, "#[info] struct Tuple(#[unredacted] i64);"),
        @"#[event] can only be applied to structs with named fields"
    );
}

#[test]
fn the_log_severity_attribute_is_validated() {
    let output = report(
        [
            "#[info] #[warning]",
            r#"#[info(bogus = "x")]"#,
            r#"#[info = "x"]"#,
            r#"#[info(name = "a", name = "b")]"#,
        ]
        .into_iter()
        .map(|attrs| {
            let item = format!("{attrs} {ONE_FIELD}");
            (event_source(r#""e""#, &item), event_error(r#""e""#, &item))
        }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn the_instrument_attribute_is_validated() {
    let output = report(
        [
            // Only `counter` may be fieldless; the rest have nothing to measure without one.
            "#[gauge]",
            "#[histogram]",
            "#[updown_counter]",
            "#[counter] #[counter]",
            "#[counter(absent)]",
            r#"#[counter = "x"]"#,
            r#"#[counter(v, bogus = "x")]"#,
            r#"#[counter(v, name = "a", name = "b")]"#,
            r#"#[counter(v, desc = "a", desc = "b")]"#,
            r#"#[counter(v, unit = "a", unit = "b")]"#,
            "#[counter(v)] #[counter(v)]",
        ]
        .into_iter()
        .map(|attrs| {
            let item = format!("{attrs} {ONE_FIELD}");
            (event_source(r#""e""#, &item), event_error(r#""e""#, &item))
        }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn an_instrument_belongs_on_the_struct_not_on_the_field() {
    insta::assert_snapshot!(
        event_error(r#""e""#, "struct Subject { #[counter] v: u64 }"),
        @"a metric instrument is a struct-level attribute; place it on the event struct and name this field positionally, e.g. `#[histogram(duration_ms)]`"
    );
}

#[test]
fn an_instrument_only_accepts_a_field_it_can_take_a_measurement_from() {
    // A metric records a number on every emission. Redaction turns the value into a
    // string, the 128-bit widths have no `Value` conversion, an `Option` can be absent,
    // and `counter` / `updown_counter` additionally fix the signedness.
    let output = report(
        [
            ("#[counter(v)]", "#[unredacted] v: i64"),
            ("#[updown_counter(v)]", "#[unredacted] v: u64"),
            ("#[counter(v)]", "#[unredacted] v: String"),
            ("#[counter(v)]", "#[unredacted] v: (u8, u8)"),
            ("#[counter(v)]", "#[unredacted] v: u128"),
            ("#[updown_counter(v)]", "#[unredacted] v: i128"),
            ("#[counter(v)]", "v: u64"),
            ("#[counter(v)]", "#[unredacted] v: Option<u64>"),
            ("#[counter(v)]", "#[dimension(metric)] #[unredacted] v: u64"),
        ]
        .into_iter()
        .map(|(attr, field)| {
            let item = format!("{attr} struct Subject {{ {field} }}");
            (event_source(r#""e""#, &item), event_error(r#""e""#, &item))
        }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn a_log_message_may_only_reference_an_attribute_that_exists() {
    let subject = |message: &str| {
        format!(
            r#"#[info("{message}")]
struct Subject {{
    #[unredacted]
    status: i64,
    #[dimension(log = "renamed")]
    original: ClassifiedString,
    #[dimension(log = exclude)]
    request_id: ClassifiedString,
}}"#
        )
    };

    // A placeholder naming something that is not a log attribute is rejected. The log key
    // is the `#[dimension(log = ...)]` override rather than the field name, and an excluded
    // field is not an attribute at all, so neither is a valid target.
    let rejected = ["absent", "original", "request_id"].into_iter().map(|unknown| {
        let item = subject(&format!("{{{unknown}}}"));
        (event_source(r#""e""#, &item), event_error(r#""e""#, &item))
    });

    // A message with no placeholder at all needs no validation, and an escaped brace, a
    // known key, an empty placeholder and a dangling brace are all accepted: only a
    // *named* placeholder resolving to nothing is an error.
    let accepted = ["nothing to interpolate", "{{literal}} {status} {} and a dangling {"]
        .into_iter()
        .map(|message| {
            let item = subject(message);
            (event_source(r#""e""#, &item), expand_event(r#""e""#, &item))
        });

    let output = report(rejected.chain(accepted));
    insta::assert_snapshot!(output);
}

#[test]
fn an_instrument_without_a_name_is_named_after_the_event() {
    // Both the event-level counter and a field instrument default their metric name to
    // the event name, so omitting `name = "..."` must not leave an instrument unnamed.
    insta::assert_snapshot!(expand_event(
        r#""heartbeat""#,
        "#[counter] #[gauge(level)] struct Heartbeat { #[unredacted] level: f64 }",
    ));
}

#[test]
fn an_instrument_looks_through_parentheses_around_its_value_type() {
    // Parentheses are transparent to the type, so they must not hide the `u64` from the
    // counter's numeric check. The invisible-group wrapper is covered separately, by
    // `a_macro_substituted_type_is_seen_through_its_invisible_group`.
    insta::assert_snapshot!(expand_event(
        r#""wrapped""#,
        "#[counter(v)] struct Wrapped { #[unredacted] v: (u64) }",
    ));
}

// ============================================================================================
// `#[derive(Enrichment)]` diagnostics
// ============================================================================================

#[test]
fn an_enrichment_requires_a_struct_with_named_fields() {
    let output = report(
        ["enum Bad { A }", "union Bad { a: i64 }", "struct Bad;", "struct Bad(i64);"]
            .into_iter()
            .map(|item| (enrichment_source(item), enrichment_error(item))),
    );
    insta::assert_snapshot!(output);
}

// ============================================================================================
// Field attributes shared by both macros
// ============================================================================================

#[test]
fn both_macros_validate_the_shared_field_attributes_identically() {
    // `#[dimension]`, `#[unredacted]`, `#[data_class]` and `#[if_none]` are parsed by one
    // shared implementation, so every diagnostic has to arrive unchanged through either
    // entry point. Both are in the same snapshot so a divergence shows up as a diff
    // between two adjacent rows.
    let output = report(
        [
            r#"#[dimension("positional")] f: T"#,
            r#"#[dimension = "x"] f: T"#,
            "#[dimension()] f: T",
            "#[dimension(bogus)] f: T",
            "#[dimension(log = bogus)] f: T",
            r#"#[dimension(log = "a", log = "b")] f: T"#,
            "#[dimension(log = exclude, log = exclude)] f: T",
            r#"#[dimension(metric = "a", metric = "b")] f: T"#,
            "#[dimension(metric, metric)] f: T",
            "#[dimension] #[dimension] f: T",
            "#[unredacted(foo)] f: T",
            r#"#[unredacted = "no"] f: T"#,
            "#[unredacted] #[data_class(Euii)] f: T",
            "#[data_class(Euii)] #[unredacted] f: T",
            "#[if_none] f: Option<T>",
            "#[if_none(bogus)] f: Option<T>",
            "#[if_none(drop)] #[if_none(drop)] f: Option<T>",
            "#[if_none(drop)] f: T",
        ]
        .into_iter()
        .flat_map(|field| {
            let event_item = format!("#[info] struct Subject {{ {field} }}");
            let enrichment_item = format!("struct Subject {{ {field} }}");
            [
                (event_source(r#""e""#, &event_item), event_error(r#""e""#, &event_item)),
                (enrichment_source(&enrichment_item), enrichment_error(&enrichment_item)),
            ]
        }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn an_event_field_may_not_hold_a_mutable_reference() {
    // Fields are read through `&self` while the event is visited, so an exclusive borrow
    // can never be handed out. Rejecting it during parsing names the offending field;
    // without the check the input is accepted and fails later inside generated code the
    // author never wrote. The optional form is checked separately because the visit body
    // dereferences the inner type rather than the field type.
    let output = report(
        [
            "#[unredacted] v: &mut u64",
            "#[unredacted] v: Option<&mut u64>",
            "v: &mut Classified",
            "#[unredacted] v: &'a mut u64",
            // A shared reference to a mutable one still cannot be handed out.
            "#[unredacted] v: &&mut u64",
        ]
        .into_iter()
        .map(|field| {
            let item = format!("#[info] struct Subject<'a> {{ {field} }}");
            (event_source(r#""e""#, &item), event_error(r#""e""#, &item))
        }),
    );
    insta::assert_snapshot!(output);
}

#[test]
fn an_event_field_may_hold_a_shared_reference() {
    // The guard above must not catch the shared references the macro has always accepted.
    insta::assert_snapshot!(expand_event(
        r#""shared.refs""#,
        r"
        #[info]
        struct SharedRefs<'a> {
            #[unredacted]
            plain: &'a u64,
            #[unredacted]
            nested: &'a &'a u64,
            #[unredacted]
            optional: Option<&'a u64>,
        }
        ",
    ));
}
