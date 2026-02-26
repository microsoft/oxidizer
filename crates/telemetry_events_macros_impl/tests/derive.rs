// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]

use insta::assert_snapshot;
use quote::quote;
use telemetry_events_macros_impl::derive_event;

fn expand(input: proc_macro2::TokenStream) -> String {
    // Use the canonical ::telemetry_events root in test snapshots.
    let root: syn::Path = syn::parse_quote!(::telemetry_events);
    let ts = derive_event(input, &root);
    // Pretty print if it parses as a file; fall back to raw tokens.
    syn::parse_file(&ts.to_string()).map_or_else(|_| ts.to_string(), |f| prettyplease::unparse(&f))
}

#[test]
#[cfg_attr(miri, ignore)]
fn basic_event() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(
            id = 1,
            name = "simple_event",
            log(name = "simple_event", message = "Simple event with value {value}")
        )]
        struct SimpleEvent {
            #[telemetry_events(include_in_logs)]
            value: i64,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn multiple_logs() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(
            id = 2,
            name = "multi_log",
            log(name = "detailed", message = "Detail: {name} {count}"),
            log(name = "summary", message = "Summary: {count}")
        )]
        struct MultiLogEvent {
            #[telemetry_events(include_in_logs)]
            count: i64,

            #[telemetry_events(include_in_log("detailed"))]
            name: UserName,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn metrics() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(
            id = 3,
            name = "metric_event",
            log(name = "metric_event", message = "Metric event: {kind} {latency}")
        )]
        struct MetricEvent {
            #[telemetry_events(include_in_logs, include_in_metrics)]
            kind: RequestKind,

            #[telemetry_events(include_in_logs, metric(name = "request_latency", kind = InstrumentKind::Histogram))]
            latency: Duration,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn full_outgoing_request() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(
            id = 0,
            name = "outgoing_request",
            log(name = "outgoing_request", message = "Outgoing request of type {request_type} from org {org_id} with duration {duration} and size {size}, request id {request_id} and operation {operation}"),
            log(name = "outgoing_request_summary", message = "Outgoing request with duration {duration} and size {size}")
        )]
        struct OutgoingRequest {
            #[telemetry_events(include_in_logs)]
            org_id: OrganizationId,

            #[telemetry_events(include_in_logs, include_in_metrics)]
            request_type: RequestType,

            #[telemetry_events(include_in_log("outgoing_request"))]
            request_id: RequestId,

            #[telemetry_events(include_in_logs, include_in_metric("outgoing_request_size"))]
            operation: OperationType,

            #[telemetry_events(include_in_logs, metric(name = "outgoing_request_duration", kind = InstrumentKind::Histogram))]
            duration: Duration,

            #[telemetry_events(include_in_logs, metric(name = "outgoing_request_size", kind = InstrumentKind::Histogram))]
            size: i64,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn into_vs_redacted() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(
            id = 4,
            name = "type_test",
            log(name = "type_test", message = "Types: {a} {b} {c} {d}")
        )]
        struct TypeTest {
            #[telemetry_events(include_in_logs)]
            a: i64,

            #[telemetry_events(include_in_logs)]
            b: f64,

            #[telemetry_events(include_in_logs)]
            c: Duration,

            #[telemetry_events(include_in_logs)]
            d: SomeCustomType,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn error_enum_not_supported() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(id = 5, name = "bad")]
        enum BadEnum {
            A,
            B,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn error_union_not_supported() {
    let input = quote! {
        #[derive(Event)]
        #[telemetry_events(id = 6, name = "bad")]
        union BadUnion {
            a: u32,
            b: u64,
        }
    };
    assert_snapshot!(expand(input));
}
