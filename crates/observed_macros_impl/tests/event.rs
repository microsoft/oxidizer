// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(missing_docs, reason = "Test code")]

use observed_macros_impl::internals::*;
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident};
use syn::{Error, ItemStruct, Result};

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    /// Expands a new-syntax event struct (including its `#[event(...)]` attribute)
    /// into just the generated `Event` impl, mirroring what the attribute macro
    /// emits alongside the re-emitted struct.
    fn run(input: &str) -> Result<TokenStream> {
        let item: TokenStream = input.parse().expect("failed to tokenize input");
        let mut item_struct: ItemStruct = syn::parse2(item)?;
        let mut event_args: Option<EventArgs> = None;
        let mut kept = Vec::with_capacity(item_struct.attrs.len());
        for attr in item_struct.attrs.drain(..) {
            if attr.path().is_ident("event") {
                event_args = Some(attr.parse_args::<EventArgs>()?);
            } else {
                kept.push(attr);
            }
        }
        item_struct.attrs = kept;
        let args = event_args.ok_or_else(|| Error::new_spanned(&item_struct.ident, "missing `#[event(...)]` attribute"))?;
        generate_event(
            &item_struct.ident,
            &item_struct.generics,
            &item_struct.attrs,
            &item_struct.fields,
            &args,
        )
    }

    fn parse_and_generate(input: &str) -> String {
        let tokens = run(input).expect("failed to generate");
        let file = syn::parse2(tokens).expect("failed to parse generated code");
        prettyplease::unparse(&file)
    }

    fn parse_and_expect_error(input: &str) -> String {
        run(input).expect_err("expected generation to fail").to_string()
    }

    #[test]
    fn test_basic_event() {
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
            struct HttpRequest {
                method: ClassifiedString,
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_message() {
        let output = parse_and_generate(
            r#"
            #[event("request.failed")]
            #[warning("Request failed")]
            struct RequestFailed {
                reason: ClassifiedString,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_field_attrs() {
        let output = parse_and_generate(
            r#"
            #[event("my.event")]
            #[info]
            struct MyEvent {
                #[dimension(log = "custom_key")]
                request_id: ClassifiedString,
                #[unredacted]
                latency: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_field_metric() {
        let output = parse_and_generate(
            r#"
            #[event("outgoing_request")]
            #[info("Outgoing request")]
            #[histogram(duration, name = "request_duration", unit = "ms")]
            struct OutgoingRequest {
                method: ClassifiedString,
                request_id: ClassifiedString,
                operation: ClassifiedString,
                #[unredacted]
                duration: f64,
                #[dimension(log = exclude)]
                #[unredacted]
                internal_tag: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_all_attributes() {
        let output = parse_and_generate(
            r#"
            #[event("http.outgoing_request")]
            #[error("Outgoing HTTP request")]
            #[counter(name = "http.request.count")]
            #[histogram(duration, name = "request_duration")]
            struct FullEvent {
                #[dimension(log = "http.method")]
                method: ClassifiedString,
                #[dimension(metric = "op")]
                #[unredacted]
                operation: i64,
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_disabled_event() {
        let output = parse_and_generate(
            r#"
            #[event("debug.diagnostics", disabled)]
            #[debug("Internal diagnostics")]
            #[gauge(queue_depth_metric, name = "debug.queue_depth")]
            struct DebugDiagnostics {
                #[unredacted]
                queue_depth: i64,
                #[unredacted]
                queue_depth_metric: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_data_class() {
        let output = parse_and_generate(
            r#"
            #[event("user.login")]
            #[info]
            struct UserLogin {
                #[data_class(DataTaxonomy::Euii)]
                username: String,
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_counter_with_unsigned_field() {
        let output = parse_and_generate(
            r#"
            #[event("bytes.received")]
            #[counter(bytes, name = "bytes.received.total", unit = "By")]
            struct BytesReceived {
                #[unredacted]
                bytes: u64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_updowncounter_with_signed_field() {
        let output = parse_and_generate(
            r#"
            #[event("queue.delta")]
            #[updown_counter(delta, name = "queue.size.delta")]
            struct QueueDelta {
                #[unredacted]
                delta: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_metric_only() {
        // A fieldless counter records `1` per emission (event-level metric).
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[counter]
            struct CountEvent {
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_field_metric_only() {
        let output = parse_and_generate(
            r#"
            #[event("system.memory")]
            #[gauge(bytes, name = "system.memory.usage")]
            struct GaugeEvent {
                #[unredacted]
                bytes: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_enum() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            enum BadEvent { A, B }
            "#,
        );
        assert!(err.contains("struct"), "{err}");
    }

    #[test]
    fn test_no_signal() {
        let output = parse_and_generate(
            r#"
            #[event("no.signal")]
            struct NoSignal { x: String }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_data_class_and_unredacted() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[data_class(Euii)]
                #[unredacted]
                x: String,
            }
            "#,
        );
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_event_with_lifetime() {
        let output = parse_and_generate(
            r#"
            #[event("borrowed.event")]
            #[info]
            struct BorrowedEvent<'a> {
                #[unredacted]
                message: &'a str,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_type_parameter() {
        let output = parse_and_generate(
            r#"
            #[event("generic.event")]
            #[info]
            struct GenericEvent<T> {
                #[unredacted]
                value: T,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_where_clause() {
        let output = parse_and_generate(
            r#"
            #[event("bounded.event")]
            #[info]
            struct BoundedEvent<T> where T: Clone {
                #[unredacted]
                value: T,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_duplicate_log_setting() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension(log = "a", log = "b")]
                x: String,
            }
            "#,
        );
        assert!(err.contains("duplicate `log`"), "{err}");
    }

    #[test]
    fn test_error_duplicate_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension(metric = "a")]
                #[dimension(metric = "b")]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn test_error_missing_event_name() {
        // Without an `#[event(...)]` attribute there is no event to generate.
        let err = parse_and_expect_error(
            r"
            #[info]
            struct MissingEventName {
                #[unredacted]
                x: i64,
            }
            ",
        );
        assert!(err.contains("event"), "{err}");
    }

    #[test]
    fn test_log_name_override() {
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info(name = "http.request.log")]
            struct HttpRequest {
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_duplicate_metric_setting() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension(metric = "a", metric = "b")]
                x: String,
            }
            "#,
        );
        assert!(err.contains("duplicate `metric`"), "{err}");
    }

    #[test]
    fn test_error_duplicate_exclude_setting() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension(log = exclude, log = exclude)]
                x: String,
            }
            "#,
        );
        assert!(err.contains("duplicate `log`"), "{err}");
    }

    #[test]
    fn test_error_unredacted_with_args() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[unredacted(foo)]
                x: String,
            }
            "#,
        );
        assert!(err.contains("does not accept arguments"), "{err}");
    }

    /// `#[unredacted]` is a marker, so a name-value form must be rejected
    /// rather than silently selecting the privacy bypass.
    #[test]
    fn test_error_unredacted_with_value() {
        for payload in ["false", "\"no\"", "0"] {
            let err = parse_and_expect_error(&format!(
                r#"
                #[event("bad")]
                #[info]
                struct BadEvent {{
                    #[unredacted = {payload}]
                    x: String,
                }}
                "#,
            ));
            assert!(err.contains("does not accept arguments"), "{err}");
        }
    }

    #[test]
    fn test_error_log_unknown_option_rejected() {
        // A log-severity attribute only accepts a message and `name = "..."`.
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info(target = "svc")]
            struct BadEvent {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("target"), "{err}");
    }

    #[test]
    fn test_error_message_references_nonexistent_attr() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info("Hello {missing}")]
            struct BadEvent {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn test_error_message_references_field_name_not_attr_name() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info("Value: {my_field}")]
            struct BadEvent {
                #[dimension(log = "custom_name")]
                #[unredacted]
                my_field: i64,
            }
            "#,
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn test_message_references_renamed_attr_ok() {
        let _output = parse_and_generate(
            r#"
            #[event("good")]
            #[info("Value: {custom_name}")]
            struct GoodEvent {
                #[dimension(log = "custom_name")]
                #[unredacted]
                my_field: i64,
            }
            "#,
        );
    }

    #[test]
    fn bare_severity_generates_without_body() {
        // A severity attribute with no arguments (`#[info]`) opts into logging
        // with no message body.
        let _output = parse_and_generate(
            r#"
            #[event("e")]
            #[info]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
    }

    #[test]
    fn metric_error_names_the_instrument_kind() {
        // A fieldless non-counter metric is rejected, and the message must
        // spell out the offending kind via `InstrumentKindValue::attr_name`.
        let err = parse_and_expect_error(
            r#"
            #[event("e")]
            #[gauge]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("gauge"), "error should name the kind: {err}");
    }

    #[test]
    fn message_with_unknown_placeholder_is_rejected() {
        // Guards the `{`-offset arithmetic in `validate_message_placeholders`:
        // the placeholder must be extracted exactly so an unknown one errors.
        let err = parse_and_expect_error(
            r#"
            #[event("e")]
            #[info("Value: {nonexistent}")]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("`{nonexistent}`"), "{err}");
    }

    #[test]
    fn test_error_message_references_excluded_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info("Tag: {tag}")]
            struct BadEvent {
                #[dimension(log = exclude)]
                #[unredacted]
                tag: i64,
            }
            "#,
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn test_unit_struct_event() {
        let output = parse_and_generate(
            r#"
            #[event("workload.disabled")]
            #[info]
            struct NoV2Workloads;
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_reference_to_redactable_type() {
        let output = parse_and_generate(
            r#"
            #[event("borrowed.classified")]
            #[info]
            struct BorrowedClassified<'a> {
                name: &'a PiiString,
                #[unredacted]
                count: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_parenthesized_reference() {
        let output = parse_and_generate(
            r#"
            #[event("paren.ref")]
            #[info]
            struct ParenRef<'a> {
                name: (&'a PiiString),
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_filled_when_none() {
        // By default a `None` `Option<T>` is filled with the `"n/a"` placeholder.
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
            struct HttpRequest {
                method: ClassifiedString,
                user_agent: Option<ClassifiedString>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_drop_when_none() {
        // `#[if_none(drop)]` omits the field entirely when `None`.
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
            struct HttpRequest {
                method: ClassifiedString,
                #[if_none(drop)]
                user_agent: Option<ClassifiedString>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_custom_fill_when_none() {
        // `#[if_none("...")]` records a custom placeholder.
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
            struct HttpRequest {
                method: ClassifiedString,
                #[if_none("unknown")]
                user_agent: Option<ClassifiedString>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_dimension_filled_when_none() {
        // `Option<T>` metric dimension without a value: filled with `"n/a"` when `None`.
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[counter]
            struct CountEvent {
                #[dimension(metric = "region")]
                #[unredacted]
                region: Option<String>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_if_none_on_non_option() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[if_none(drop)]
                #[unredacted]
                count: i64,
            }
            "#,
        );
        assert!(err.contains("only valid on `Option<T>`"), "{err}");
    }

    #[test]
    fn test_error_counter_signed_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[counter(count)]
            struct BadEvent {
                #[unredacted]
                count: i64,
            }
            "#,
        );
        assert!(err.contains("unsigned integer"), "{err}");
    }

    #[test]
    fn test_error_updowncounter_unsigned_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[updown_counter(delta)]
            struct BadEvent {
                #[unredacted]
                delta: u64,
            }
            "#,
        );
        assert!(err.contains("signed integer"), "{err}");
    }

    #[test]
    fn test_error_counter_non_integer_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[counter(count)]
            struct BadEvent {
                #[unredacted]
                count: f64,
            }
            "#,
        );
        assert!(err.contains("unsigned integer"), "{err}");
    }

    #[test]
    fn test_error_metric_field_not_found() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[histogram(nope)]
            struct BadEvent {
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn test_error_gauge_requires_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[gauge(name = "x")]
            struct BadEvent {
                #[unredacted]
                value: f64,
            }
            "#,
        );
        assert!(err.contains("requires a field"), "{err}");
    }

    #[test]
    fn test_error_updowncounter_requires_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[updown_counter(name = "x")]
            struct BadEvent {
                #[unredacted]
                value: i64,
            }
            "#,
        );
        assert!(err.contains("requires a field"), "{err}");
    }

    #[test]
    fn test_error_instrument_on_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            struct BadEvent {
                #[counter(x)]
                #[unredacted]
                x: u64,
            }
            "#,
        );
        assert!(err.contains("struct-level attribute"), "{err}");
    }

    #[test]
    fn test_error_field_both_metric_and_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[histogram(duration)]
            struct BadEvent {
                #[dimension(metric = "duration")]
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("cannot be both"), "{err}");
    }

    #[test]
    fn test_error_duplicate_metric_on_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[histogram(duration)]
            #[gauge(duration)]
            struct BadEvent {
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("already has a metric"), "{err}");
    }

    #[test]
    fn test_error_duplicate_event_metric() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[counter]
            #[counter(name = "other")]
            struct BadEvent {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("only one event-level"), "{err}");
    }

    #[test]
    fn test_error_dimension_positional_rejected() {
        // The positional string form `#[dimension("region")]` is no longer
        // supported; users must write `#[dimension(log = "region")]`.
        let err = parse_and_expect_error(
            r#"
            #[event("http.request.count")]
            #[info]
            #[counter]
            struct CountEvent {
                #[dimension("region")]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        assert!(err.contains("positional string name is not supported"), "{err}");
    }

    #[test]
    fn test_dimension_bare_logs_under_own_name() {
        // A bare `#[dimension]` logs the field under its own name and opts out of
        // the metric signal - equivalent to the field's default routing.
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
            struct HttpRequest {
                #[dimension]
                #[unredacted]
                internal_flag: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_metric_bare_opts_in_own_name() {
        // Bare `metric` opts the field in as a metric dimension keyed by the field
        // name, while it remains logged under its own name.
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[info]
            #[counter]
            struct CountEvent {
                #[dimension(metric)]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_exclude_log_with_bare_metric() {
        // `log = exclude, metric` drops the field from logs while opting it in as a
        // metric dimension keyed by the field name.
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[info]
            #[counter]
            struct CountEvent {
                #[dimension(log = exclude, metric)]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_separate_log_and_metric_keys() {
        // `log` and `metric` can name the two signals independently.
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[info]
            #[counter]
            struct CountEvent {
                #[dimension(log = "http.status_code", metric = "status")]
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_metric_only_excludes_log() {
        // `log = exclude` removes the field from the log while `metric` keeps it
        // as a metric dimension.
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[info]
            #[counter]
            struct CountEvent {
                #[dimension(log = exclude, metric = "region")]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_metric_keeps_default_log() {
        // With only `metric` set, the field is still logged under its own name.
        let output = parse_and_generate(
            r#"
            #[event("http.request.count")]
            #[info]
            #[counter]
            struct CountEvent {
                #[dimension(metric = "region")]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_dimension_bare_exclude() {
        // The bare `#[dimension(exclude)]` shorthand is not supported; use
        // `#[dimension(log = exclude)]` instead.
        let err = parse_and_expect_error(
            r#"
            #[event("http.request")]
            #[info]
            struct HttpRequest {
                #[dimension(exclude)]
                #[unredacted]
                internal_flag: i64,
            }
            "#,
        );
        assert!(err.contains("log = exclude"), "{err}");
    }

    #[test]
    fn test_metric_value_field_can_exclude_from_log() {
        // A metric value field may still control its own log routing: `log =
        // exclude` removes the value from the log without making it a dimension.
        let output = parse_and_generate(
            r#"
            #[event("outgoing_request")]
            #[info]
            #[histogram(duration)]
            struct OutgoingRequest {
                method: ClassifiedString,
                #[dimension(log = exclude)]
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_empty_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension()]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("requires at least one"), "{err}");
    }

    #[test]
    fn test_error_dimension_log_specified_twice() {
        // Two `log` items in one `#[dimension(...)]` is a duplicate-`log` error.
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension(log = "a", log = "b")]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("duplicate `log`"), "{err}");
    }

    #[test]
    fn test_error_dimension_bad_log_value() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[dimension(log = nope)]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("string key or the `exclude`"), "{err}");
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, not(miri)))]
mod coverage_tests {
    use super::*;

    /// Expands `input` (a new-syntax event struct including its `#[event(...)]`
    /// attribute) into just the generated impl, surfacing any error.
    #[cfg_attr(test, mutants::skip)] // test-only helper: mutating it can't change observable behavior
    fn expand(input: &str) -> Result<TokenStream> {
        let item: TokenStream = input.parse().expect("failed to tokenize input");
        let mut item_struct: ItemStruct = syn::parse2(item)?;
        let mut event_args: Option<EventArgs> = None;
        let mut kept = Vec::with_capacity(item_struct.attrs.len());
        for attr in item_struct.attrs.drain(..) {
            if attr.path().is_ident("event") {
                event_args = Some(attr.parse_args::<EventArgs>()?);
            } else {
                kept.push(attr);
            }
        }
        item_struct.attrs = kept;
        let args = event_args.ok_or_else(|| Error::new_spanned(&item_struct.ident, "missing `#[event(...)]` attribute"))?;
        generate_event(
            &item_struct.ident,
            &item_struct.generics,
            &item_struct.attrs,
            &item_struct.fields,
            &args,
        )
    }

    /// Parses `input` and asserts generation fails, returning the error text.
    #[cfg_attr(test, mutants::skip)] // test-only helper: mutating it can't change observable behavior
    fn expect_err(input: &str) -> String {
        expand(input).expect_err("expected generation to fail").to_string()
    }

    /// Parses `input` and asserts generation succeeds.
    #[cfg_attr(test, mutants::skip)] // test-only helper: mutating it can't change observable behavior
    fn expect_ok(input: &str) {
        let tokens = expand(input).expect("expected generation to succeed");
        syn::parse2::<syn::File>(tokens).expect("generated code should parse");
    }

    #[test]
    fn tuple_struct_is_rejected() {
        let _ = expect_err(r#"#[event("e")] struct E(i64);"#);
    }

    #[test]
    fn union_is_rejected() {
        let _ = expect_err(r#"#[event("e")] union E { a: i64 }"#);
    }

    #[test]
    fn duplicate_log_attribute_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] #[warning] struct E { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn message_escaped_brace_and_valid_placeholder() {
        // Escaped `{{` is skipped while a valid `{name}` placeholder resolves.
        expect_ok(r#"#[event("e")] #[info("a {{ b {name}")] struct E { #[unredacted] name: i64 }"#);
    }

    #[test]
    fn const_generic_event_generates() {
        expect_ok(r#"#[event("e")] #[info] struct E<const N: usize> { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn dimension_name_value_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[dimension = 1] #[unredacted] v: i64 }"#);
    }

    #[test]
    fn if_none_without_argument_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[if_none] #[unredacted] v: Option<i64> }"#);
    }

    #[test]
    fn if_none_bad_keyword_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[if_none(bogus)] #[unredacted] v: Option<i64> }"#);
    }

    #[test]
    fn duplicate_if_none_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[if_none(drop)] #[if_none("x")] #[unredacted] v: Option<i64> }"#);
    }

    #[test]
    fn data_class_after_unredacted_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[unredacted] #[data_class(Foo::Bar)] v: i64 }"#);
    }

    #[test]
    fn option_field_with_reference_inner_generates() {
        // A `Option<&T>` field drives the `inner_is_ref` branch of the option
        // visit codegen.
        expect_ok(r#"#[event("e")] #[info] struct E { #[unredacted] v: Option<&'static str> }"#);
    }

    #[test]
    fn message_with_unterminated_brace_is_ignored() {
        // A `{` with no matching `}` is skipped rather than treated as a placeholder.
        expect_ok(r#"#[event("e")] #[info("x {y")] struct E { #[unredacted] y: i64 }"#);
    }

    #[test]
    fn classified_metric_value_field_is_rejected() {
        // A classified value is rendered through the redaction engine as a
        // string, which carries no measurement -- so it must be a compile-time
        // error rather than an instrument that silently records nothing.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[data_class(DC)] n: u64 }"#);
        assert!(msg.contains("#[unredacted]"), "diagnostic should name the fix, got: {msg}");

        // The default (no redaction attribute at all) takes the same path.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { n: u64 }"#);
        assert!(msg.contains("#[unredacted]"), "diagnostic should name the fix, got: {msg}");
    }

    #[test]
    fn non_numeric_metric_value_field_is_rejected() {
        // `gauge`/`histogram` place no signedness constraint, but they still
        // need a number: a non-numeric value produces no measurement at all.
        for kind in ["gauge", "histogram"] {
            let msg = expect_err(&format!(
                r#"#[event("e")] #[info] #[{kind}(n)] struct E {{ #[unredacted] n: PublicString }}"#
            ));
            assert!(msg.contains("numeric"), "diagnostic should say numeric, got: {msg}");
        }
    }

    #[test]
    fn metric_value_field_accepts_supported_widths() {
        // `u64` in particular: it is the natural type for a byte or request
        // counter, and `Value` carries it exactly.
        for ty in ["u8", "u16", "u32", "u64", "usize"] {
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[counter(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
        }
        for ty in ["i8", "i16", "i32", "i64", "isize"] {
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[updown_counter(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
        }
        // Gauge and histogram are signedness- and width-agnostic, floats included.
        for ty in ["u64", "i64", "f32", "f64"] {
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[gauge(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[histogram(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
        }
    }

    #[test]
    fn metric_value_field_rejects_128_bit_widths() {
        // No telemetry backend represents these, so `Value` has no conversion.
        // The diagnostic is specific rather than the generic "not numeric" one.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[unredacted] n: u128 }"#);
        assert!(msg.contains("128-bit"), "diagnostic should call out the width, got: {msg}");
        let msg = expect_err(r#"#[event("e")] #[info] #[updown_counter(n)] struct E { #[unredacted] n: i128 }"#);
        assert!(msg.contains("128-bit"), "diagnostic should call out the width, got: {msg}");
    }

    #[test]
    fn counter_still_requires_unsigned_and_updown_counter_signed() {
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[unredacted] n: i64 }"#);
        assert!(msg.contains("unsigned"), "got: {msg}");
        let msg = expect_err(r#"#[event("e")] #[info] #[updown_counter(n)] struct E { #[unredacted] n: u64 }"#);
        assert!(msg.contains("signed"), "got: {msg}");
        // A float is not an integer, so neither counter accepts one.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[unredacted] n: f64 }"#);
        assert!(msg.contains("unsigned"), "got: {msg}");
    }

    #[test]
    fn numeric_kind_of_non_path_type_is_none() {
        let reference: syn::Type = syn::parse_str("&u64").expect("parse type");
        assert!(numeric_kind(&reference).is_none());
    }

    #[test]
    fn numeric_kind_does_not_see_through_option() {
        // `Option<u64>` must not satisfy an instrument's value-type requirement --
        // an optional field has no measurement to record when it is `None`.
        let optional: syn::Type = syn::parse_str("Option<u64>").expect("parse type");
        assert!(numeric_kind(&optional).is_none());
    }

    #[test]
    fn numeric_kind_rejects_128_bit_widths() {
        // `Value` has no conversion for these, so they are not "numeric" for the
        // purposes of an instrument; `is_128_bit_int` exists to give them a
        // dedicated diagnostic instead of the generic one.
        for spelling in ["u128", "i128"] {
            let ty: syn::Type = syn::parse_str(spelling).expect("parse type");
            assert!(numeric_kind(&ty).is_none(), "{spelling} must not be a supported numeric");
            assert!(is_128_bit_int(&ty), "{spelling} must be recognized for its own diagnostic");
        }
    }

    #[test]
    fn is_128_bit_int_is_false_for_non_path_and_supported_types() {
        let reference: syn::Type = syn::parse_str("&u128").expect("parse type");
        assert!(!is_128_bit_int(&reference));
        let supported: syn::Type = syn::parse_str("u64").expect("parse type");
        assert!(!is_128_bit_int(&supported));
    }

    #[test]
    fn numeric_kind_classifies_floats() {
        for spelling in ["f32", "f64"] {
            let ty: syn::Type = syn::parse_str(spelling).expect("parse type");
            assert!(matches!(numeric_kind(&ty), Some(NumericKind::Float)));
        }
    }

    #[test]
    fn optional_metric_value_field_is_rejected() {
        // An instrument records a measurement on every emission, so its value field
        // cannot be `Option<T>`: `#[if_none(...)]` would fill `None` with a placeholder
        // string, which is not a valid measurement.
        for attr in ["counter(v)", "updown_counter(v)", "gauge(v)", "histogram(v)"] {
            let err = expect_err(&format!(r#"#[event("e")] #[{attr}] struct E {{ #[unredacted] v: Option<u64> }}"#));
            assert!(
                err.contains("a metric value cannot be optional"),
                "unexpected error for `{attr}`: {err}"
            );
        }
    }

    #[test]
    fn optional_metric_dimension_field_is_still_allowed() {
        // Only the metric *value* is constrained; an optional dimension is fine
        // because a placeholder is a meaningful attribute value.
        expect_ok(r#"#[event("e")] #[info] struct E { #[dimension(metric)] #[unredacted] v: Option<u64> }"#);
    }

    #[test]
    fn strip_type_wrappers_unwraps_paren_and_group() {
        // `Paren` comes from source; `Group` is synthesized (it only appears from
        // macro-expanded token streams, never hand-written source).
        let paren: syn::Type = syn::parse_str("(u64)").expect("parse type");
        assert!(matches!(strip_type_wrappers(&paren), syn::Type::Path(_)));

        let inner: syn::Type = syn::parse_str("u64").expect("parse type");
        let grouped = syn::Type::Group(syn::TypeGroup {
            attrs: Vec::new(),
            group_token: syn::token::Group::default(),
            elem: Box::new(inner),
        });
        assert!(matches!(strip_type_wrappers(&grouped), syn::Type::Path(_)));
    }

    #[test]
    fn event_args_requires_a_string_name() {
        let err = syn::parse_str::<EventArgs>("123").expect_err("a non-string event name must be rejected");
        assert!(err.to_string().contains("requires a string event name"), "unexpected error: {err}");
    }

    #[test]
    fn event_args_accepts_trailing_comma() {
        let args = syn::parse_str::<EventArgs>(r#""e","#).expect("a trailing comma is allowed");
        assert_eq!(args.name, "e");
        assert!(!args.disabled);
    }

    #[test]
    fn event_args_rejects_unknown_flag() {
        let err = syn::parse_str::<EventArgs>(r#""e", bogus"#).expect_err("an unknown flag must be rejected");
        assert!(
            err.to_string().contains("unknown `#[event(...)]` flag `bogus`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn log_message_followed_by_named_option_generates() {
        // The comma between the positional message and `name = "..."` drives the
        // separator branch of the log attribute body parser.
        expect_ok(r#"#[event("e")] #[info("m", name = "log.name")] struct E { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn duplicate_log_name_option_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[info("m", name = "a", name = "b")] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("duplicate `name` setting"), "unexpected error: {err}");
    }

    #[test]
    fn unknown_metric_option_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[counter(bogus = "x")] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("unknown metric option `bogus`"), "unexpected error: {err}");
    }

    #[test]
    fn non_ident_struct_attribute_is_ignored() {
        // A multi-segment attribute path has no single ident, so it can be neither
        // a log-severity nor a metric-kind helper and is skipped.
        expect_ok(r#"#[event("e")] #[info] #[some::other] struct E { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn log_attribute_written_as_name_value_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[info = "m"] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("not `= value`"), "unexpected error: {err}");
    }

    #[test]
    fn metric_attribute_written_as_name_value_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[counter = "c"] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("not `= value`"), "unexpected error: {err}");
    }

    #[test]
    fn event_attr_reemits_struct_without_helper_attributes() {
        // The attribute entry point re-emits the struct alongside the generated
        // impl, with every consumed helper attribute stripped and every
        // unrelated attribute left untouched.
        let attr: TokenStream = r#""http.request""#.parse().expect("failed to tokenize attribute");
        let item: TokenStream = r#"#[derive(Debug)] #[info("hi")] struct E { #[allow(dead_code)] #[unredacted] v: i64 }"#
            .parse()
            .expect("failed to tokenize item");

        let expanded = event_attr(attr, item).expect("attribute expansion should succeed");

        let file: syn::File = syn::parse2(expanded).expect("generated code should parse");
        let syn::Item::Struct(reemitted) = &file.items[0] else {
            panic!("the first generated item should be the re-emitted struct");
        };
        let struct_attrs: Vec<_> = reemitted
            .attrs
            .iter()
            .map(|a| a.path().get_ident().map(ToString::to_string))
            .collect();
        assert_eq!(
            struct_attrs,
            vec![Some("derive".to_owned())],
            "`#[info]` should be stripped and `#[derive]` preserved"
        );

        let field_attrs: Vec<_> = reemitted
            .fields
            .iter()
            .flat_map(|f| &f.attrs)
            .map(|a| a.path().get_ident().map(ToString::to_string))
            .collect();
        assert_eq!(
            field_attrs,
            vec![Some("allow".to_owned())],
            "`#[unredacted]` should be stripped and `#[allow]` preserved"
        );
    }

    #[test]
    fn event_attr_propagates_codegen_errors() {
        // The entry point must surface codegen failures rather than re-emitting
        // a struct with no `Event` impl.
        let attr: TokenStream = r#""e""#.parse().expect("failed to tokenize attribute");
        let item: TokenStream = "struct E(i64);".parse().expect("failed to tokenize item");

        let err = event_attr(attr, item).expect_err("a tuple struct must be rejected");

        assert!(err.to_string().contains("named fields"), "unexpected error: {err}");
    }

    #[test]
    fn strip_helper_attrs_clears_tuple_struct_field_attributes() {
        // Tuple structs are rejected by codegen, but the re-emit helper stays
        // total over `Fields` so it never silently leaves helpers behind.
        let item: ItemStruct = syn::parse_str(r"#[info] struct E(#[unredacted] i64);").expect("failed to parse tuple struct");

        let stripped = strip_helper_attrs(item);

        assert!(stripped.attrs.is_empty());
        assert!(stripped.fields.iter().all(|field| field.attrs.is_empty()));
    }

    #[test]
    fn every_severity_attribute_maps_to_its_own_variant() {
        // A dropped arm would leave the attribute unrecognized, silently
        // demoting the event to "no log signal" instead of failing to build.
        for (attribute, expected) in [
            ("trace", "Trace"),
            ("debug", "Debug"),
            ("info", "Info"),
            ("warning", "Warn"),
            ("error", "Error"),
            ("fatal", "Fatal"),
        ] {
            let parsed = SeverityKind::from_ident(&format_ident!("{attribute}")).expect("known severity attribute");
            assert_eq!(
                parsed.to_token_stream().to_string(),
                expected,
                "`#[{attribute}]` mapped to the wrong severity"
            );
        }

        assert!(SeverityKind::from_ident(&format_ident!("bogus")).is_none());
    }
}
