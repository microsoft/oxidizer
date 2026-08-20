// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behaviour of the crate's two entry points, `event` and `derive_enrichment`.
//!
//! Everything the macros reject, plus the classification decisions that are easier to state
//! as an assertion than to read out of a snapshot. The expansions themselves are snapshotted
//! in `expansion.rs`; nothing here uses insta, so this file also runs under miri.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use observed_macros_impl::{derive_enrichment, event};
use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::quote;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(source: &str) -> TokenStream {
        source.parse().expect("the source tokenizes")
    }

    fn expand_event(attr: &str, item: &str) -> String {
        event(tokenize(attr), tokenize(item))
            .expect("the event attribute expands")
            .to_string()
    }

    fn event_error(attr: &str, item: &str) -> String {
        event(tokenize(attr), tokenize(item))
            .expect_err("the event attribute is rejected")
            .to_string()
    }

    fn enrichment_error(item: &str) -> String {
        derive_enrichment(tokenize(item)).expect_err("the derive is rejected").to_string()
    }

    /// A struct body carrying one well-formed field, for the cases whose subject is the
    /// struct-level attributes rather than the fields.
    const ONE_FIELD: &str = "struct Subject { #[unredacted] v: u64 }";

    // ============================================================================================
    // Entry-point contract
    // ============================================================================================

    #[test]
    fn each_entry_point_rejects_input_it_cannot_parse() {
        for item in ["1 + 1", "enum NotAStruct { A }", "fn not_a_struct() {}"] {
            _ = event(tokenize(r#""e""#), tokenize(item)).expect_err(item);
            _ = derive_enrichment(tokenize(item)).expect_err(item);
        }
    }

    #[test]
    fn an_attribute_the_macro_does_not_own_is_left_untouched() {
        // A multi-segment path has no single identifier to match against a severity or an
        // instrument name, and an unrecognized field attribute belongs to some other macro.
        // Both must survive onto the re-emitted struct rather than being consumed or rejected.
        let expanded = expand_event(
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
        );

        assert!(expanded.contains("some :: other :: attr"), "{expanded}");
        assert!(expanded.contains("serde"), "{expanded}");
        assert!(!expanded.contains("# [info]"), "{expanded}");
        assert!(!expanded.contains("# [unredacted]"), "{expanded}");
    }

    #[test]
    fn every_severity_attribute_maps_to_its_own_variant() {
        // A dropped arm would leave the attribute unrecognized, silently demoting the event
        // to "no log signal" instead of failing to build.
        for (attribute, variant) in [
            ("trace", "Trace"),
            ("debug", "Debug"),
            ("info", "Info"),
            ("warning", "Warn"),
            ("error", "Error"),
            ("fatal", "Fatal"),
        ] {
            let expanded = expand_event(r#""severity""#, &format!("#[{attribute}] {ONE_FIELD}"));
            assert!(
                expanded.contains(&format!("Severity :: {variant}")),
                "`#[{attribute}]` mapped to the wrong severity: {expanded}"
            );
        }
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
        .expect("a macro-substituted type expands")
        .to_string();

        // `hits` satisfied the counter's unsigned-integer contract, so the numeric check saw
        // through the group to the `u64`.
        assert!(expanded.contains("InstrumentKind :: Counter"), "{expanded}");
        // `maybe` was recognized as an `Option`, so `#[if_none(drop)]` was accepted and the
        // drop arm generated instead of an unconditional visit.
        assert!(expanded.contains("if let :: core :: option :: Option :: Some"), "{expanded}");
        // `label` was recognized as a borrowed `str`, so the copy into an `Arc<str>` is
        // spelled out rather than a plain conversion being emitted.
        assert!(expanded.contains("Arc :: < str >"), "{expanded}");
    }

    #[test]
    fn a_field_type_that_only_resembles_an_option_is_treated_as_a_plain_field() {
        // `Option<T>` is matched syntactically, so each of these has to fall through to the
        // non-optional path rather than being mistaken for an optional field.
        for ty in [
            "(u8, u8)",              // not a path at all
            "<i32 as Copy>::Output", // a qualified self
            "Option",                // `Option` with no arguments
            "Option<u8, u16>",       // two generic arguments
            "Option<'a>",            // a lifetime where a type belongs
            "[T; 4]",                // a token group the type-parameter scan descends into
            "(T, u8)",               // a type parameter found before the end of a group
        ] {
            let expanded = expand_event(r#""shapes""#, &format!("#[info] struct Shapes<'a, T> {{ field: {ty} }}"));
            // Only the optional paths bind `__val`; a plain field never does.
            assert!(!expanded.contains("__val"), "field type `{ty}`: {expanded}");
        }
    }

    // ============================================================================================
    // `#[event(...)]` diagnostics
    // ============================================================================================

    #[test]
    fn the_event_attribute_arguments_are_validated() {
        for (attr, expected) in [
            ("", "requires a string event name"),
            ("bare_ident", "requires a string event name"),
            ("42", "requires a string event name"),
            (r#""e", bogus"#, "unknown `#[event(...)]` flag `bogus`"),
        ] {
            let err = event_error(attr, ONE_FIELD);
            assert!(err.contains(expected), "`#[event({attr})]`: {err}");
        }
    }

    #[test]
    fn the_event_attribute_accepts_the_disabled_flag_and_a_trailing_comma() {
        for attr in [r#""e""#, r#""e","#, r#""e", disabled"#, r#""e", disabled,"#] {
            _ = event(tokenize(attr), tokenize(ONE_FIELD)).unwrap_or_else(|err| panic!("`#[event({attr})]`: {err}"));
        }
    }

    #[test]
    fn an_event_requires_named_fields() {
        let err = event_error(r#""e""#, "#[info] struct Tuple(#[unredacted] i64);");
        assert!(err.contains("named fields"), "{err}");
    }

    #[test]
    fn the_log_severity_attribute_is_validated() {
        for (attrs, expected) in [
            ("#[info] #[warning]", "only one log-severity attribute"),
            (r#"#[info(bogus = "x")]"#, "unknown log option `bogus`"),
            (r#"#[info = "x"]"#, "not `= value`"),
            (r#"#[info(name = "a", name = "b")]"#, "duplicate `name` setting"),
        ] {
            let err = event_error(r#""e""#, &format!("{attrs} {ONE_FIELD}"));
            assert!(err.contains(expected), "`{attrs}`: {err}");
        }
    }

    #[test]
    fn the_instrument_attribute_is_validated() {
        for (attrs, expected) in [
            // Only `counter` may be fieldless; the rest have nothing to measure without one.
            ("#[gauge]", "`#[gauge(...)]` requires a field"),
            ("#[histogram]", "`#[histogram(...)]` requires a field"),
            ("#[updown_counter]", "`#[updown_counter(...)]` requires a field"),
            ("#[counter] #[counter]", "only one event-level metric"),
            ("#[counter(absent)]", "which does not exist in the struct"),
            (r#"#[counter = "x"]"#, "not `= value`"),
            (r#"#[counter(v, bogus = "x")]"#, "unknown metric option `bogus`"),
            (r#"#[counter(v, name = "a", name = "b")]"#, "duplicate `name` setting"),
            (r#"#[counter(v, desc = "a", desc = "b")]"#, "duplicate `desc` setting"),
            (r#"#[counter(v, unit = "a", unit = "b")]"#, "duplicate `unit` setting"),
            ("#[counter(v)] #[counter(v)]", "already has a metric instrument"),
        ] {
            let err = event_error(r#""e""#, &format!("{attrs} {ONE_FIELD}"));
            assert!(err.contains(expected), "`{attrs}`: {err}");
        }
    }

    #[test]
    fn an_instrument_belongs_on_the_struct_not_on_the_field() {
        let err = event_error(r#""e""#, "struct Subject { #[counter] v: u64 }");
        assert!(err.contains("is a struct-level attribute"), "{err}");
    }

    #[test]
    fn an_instrument_only_accepts_a_field_it_can_take_a_measurement_from() {
        // A metric records a number on every emission. Redaction turns the value into a
        // string, the 128-bit widths have no `Value` conversion, an `Option` can be absent,
        // and `counter` / `updown_counter` additionally fix the signedness.
        for (attr, field, expected) in [
            ("#[counter(v)]", "#[unredacted] v: i64", "to be a unsigned integer type"),
            ("#[updown_counter(v)]", "#[unredacted] v: u64", "to be a signed integer type"),
            ("#[counter(v)]", "#[unredacted] v: String", "numeric type that"),
            ("#[counter(v)]", "#[unredacted] v: (u8, u8)", "numeric type that"),
            ("#[counter(v)]", "#[unredacted] v: u128", "does not support 128-bit integers"),
            ("#[updown_counter(v)]", "#[unredacted] v: i128", "does not support 128-bit integers"),
            ("#[counter(v)]", "v: u64", "to be `#[unredacted]`"),
            ("#[counter(v)]", "#[unredacted] v: Option<u64>", "a metric value cannot be optional"),
            (
                "#[counter(v)]",
                "#[dimension(metric)] #[unredacted] v: u64",
                "cannot be both a metric value and a metric dimension",
            ),
        ] {
            let err = event_error(r#""e""#, &format!("{attr} struct Subject {{ {field} }}"));
            assert!(err.contains(expected), "`{attr}` on `{field}`: {err}");
        }
    }

    #[test]
    fn a_log_message_may_only_reference_an_attribute_that_exists() {
        let subject = |message: &str| {
            format!(
                r#"
                #[info("{message}")]
                struct Subject {{
                    #[unredacted]
                    status: i64,
                    #[dimension(log = "renamed")]
                    original: ClassifiedString,
                    #[dimension(log = exclude)]
                    request_id: ClassifiedString,
                }}
                "#
            )
        };

        for unknown in [
            "absent",
            // The log key is the `#[dimension(log = ...)]` override, not the field name.
            "original",
            // An excluded field is not a log attribute, so it is not a valid target either.
            "request_id",
        ] {
            let err = event_error(r#""e""#, &subject(&format!("{{{unknown}}}")));
            assert!(
                err.contains(&format!("references `{{{unknown}}}`")) && err.contains("available attributes: [status, renamed]"),
                "`{{{unknown}}}`: {err}"
            );
        }

        // A message with no placeholder at all needs no validation, and an escaped brace, a
        // known key, an empty placeholder and a dangling brace are all accepted: only a
        // *named* placeholder resolving to nothing is an error.
        for message in ["nothing to interpolate", "{{literal}} {status} {} and a dangling {"] {
            let expanded = expand_event(r#""e""#, &subject(message));
            assert!(expanded.contains("Severity :: Info"), "`{message}`: {expanded}");
        }
    }

    #[test]
    fn an_instrument_without_a_name_is_named_after_the_event() {
        // Both the event-level counter and a field instrument default their metric name to
        // the event name, so omitting `name = "..."` must not leave an instrument unnamed.
        let expanded = expand_event(
            r#""heartbeat""#,
            "#[counter] #[gauge(level)] struct Heartbeat { #[unredacted] level: f64 }",
        );
        assert_eq!(
            expanded.matches(r#"MetricDescription :: new ("heartbeat""#).count(),
            2,
            "{expanded}"
        );
    }

    #[test]
    fn an_instrument_looks_through_parentheses_around_its_value_type() {
        // Parentheses are transparent to the type, so they must not hide the `u64` from the
        // counter's numeric check. The invisible-group wrapper is covered separately, by
        // `a_macro_substituted_type_is_seen_through_its_invisible_group`.
        let expanded = expand_event(r#""wrapped""#, "#[counter(v)] struct Wrapped { #[unredacted] v: (u64) }");
        assert!(expanded.contains("InstrumentKind :: Counter"), "{expanded}");
    }

    // ============================================================================================
    // `#[derive(Enrichment)]` diagnostics
    // ============================================================================================

    #[test]
    fn an_enrichment_requires_a_struct_with_named_fields() {
        for (item, expected) in [
            ("enum Bad { A }", "structs, not enums"),
            ("union Bad { a: i64 }", "structs, not unions"),
            ("struct Bad;", "named fields"),
            ("struct Bad(i64);", "named fields"),
        ] {
            let err = enrichment_error(item);
            assert!(err.contains(expected), "`{item}`: {err}");
        }
    }

    // ============================================================================================
    // Field attributes shared by both macros
    // ============================================================================================

    #[test]
    fn both_macros_validate_the_shared_field_attributes_identically() {
        // `#[dimension]`, `#[unredacted]`, `#[data_class]` and `#[if_none]` are parsed by one
        // shared implementation, so every diagnostic has to arrive unchanged through either
        // entry point.
        for (field, expected) in [
            (r#"#[dimension("positional")] f: T"#, "a positional string name is not supported"),
            (r#"#[dimension = "x"] f: T"#, "does not take a `= value`"),
            ("#[dimension()] f: T", "requires at least one of `log` or `metric`"),
            ("#[dimension(bogus)] f: T", "expected `log = \"...\"`"),
            (
                "#[dimension(log = bogus)] f: T",
                "`log` expects a string key or the `exclude` keyword",
            ),
            (r#"#[dimension(log = "a", log = "b")] f: T"#, "duplicate `log` setting"),
            ("#[dimension(log = exclude, log = exclude)] f: T", "duplicate `log` setting"),
            (r#"#[dimension(metric = "a", metric = "b")] f: T"#, "duplicate `metric` setting"),
            ("#[dimension(metric, metric)] f: T", "duplicate `metric` setting"),
            ("#[dimension] #[dimension] f: T", "duplicate `#[dimension(...)]` attribute"),
            ("#[unredacted(foo)] f: T", "does not accept arguments"),
            (r#"#[unredacted = "no"] f: T"#, "does not accept arguments"),
            ("#[unredacted] #[data_class(Euii)] f: T", "mutually exclusive"),
            ("#[data_class(Euii)] #[unredacted] f: T", "mutually exclusive"),
            ("#[if_none] f: Option<T>", "requires an argument"),
            ("#[if_none(bogus)] f: Option<T>", "expected `drop` or a string literal placeholder"),
            (
                "#[if_none(drop)] #[if_none(drop)] f: Option<T>",
                "duplicate `#[if_none(...)]` attribute",
            ),
            ("#[if_none(drop)] f: T", "only valid on `Option<T>` fields"),
        ] {
            let from_event = event_error(r#""e""#, &format!("#[info] struct Subject {{ {field} }}"));
            assert!(from_event.contains(expected), "`#[event]` on `{field}`: {from_event}");

            let from_enrichment = enrichment_error(&format!("struct Subject {{ {field} }}"));
            assert!(from_enrichment.contains(expected), "`Enrichment` on `{field}`: {from_enrichment}");
        }
    }
}
