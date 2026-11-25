// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the instrumentation-scope attributes of the `fetch` meter, which the
//! instruments of `CustomContext::meter` carry.

use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::{HttpClientBuilder, HttpRequest, HttpResponse, HttpResponseBuilder};
use http::StatusCode;
use layered::Service;
use observed::Sink;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tick::Clock;

const RUNTIME_ATTRIBUTE: &str = "fetch.runtime";
const TRANSPORT_ATTRIBUTE: &str = "fetch.transport";

/// Returns `attribute_key` from the scope that recorded `metric_name`.
fn scope_attribute_for_metric(exporter: &InMemoryMetricExporter, metric_name: &str, attribute_key: &str) -> Option<String> {
    let resource_metrics = exporter.get_finished_metrics().expect("finished metrics must be retrievable");

    for resource_metric in &resource_metrics {
        for scope_metric in resource_metric.scope_metrics() {
            if scope_metric.metrics().any(|metric| metric.name() == metric_name)
                && let Some(value) = scope_metric
                    .scope()
                    .attributes()
                    .find(|attribute| attribute.key.as_str() == attribute_key)
            {
                return Some(value.value.as_str().into_owned());
            }
        }
    }

    None
}

/// Returns the `fetch.runtime` and `fetch.transport` values for `metric_name`.
fn runtime_and_transport_for_metric(exporter: &InMemoryMetricExporter, metric_name: &str) -> (Option<String>, Option<String>) {
    (
        scope_attribute_for_metric(exporter, metric_name, RUNTIME_ATTRIBUTE),
        scope_attribute_for_metric(exporter, metric_name, TRANSPORT_ATTRIBUTE),
    )
}

fn exporter_and_provider() -> (InMemoryMetricExporter, SdkMeterProvider) {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
    (exporter, provider)
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn custom_transport_instrument_inherits_scope() {
    let (exporter, provider) = exporter_and_provider();

    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        memory: OpaqueMemory::new(GlobalPool::new()),
        sink: Sink::noop(),
        extras: (),
    };

    // A transport metric recorded through cx.meter uses the same scope as built-in metrics.
    let client = create_builder(
        "instrumented-runtime",
        "instrumented",
        |cx: CustomContext| {
            let counter = cx.meter.u64_counter("custom.transport.requests").build();
            counter.add(1, &[]);
            OkHandler {
                body_builder: cx.body_builder,
            }
        },
        Isolation::Shared,
        deps,
    )
    .insecure_allow_http()
    .meter_provider(provider.clone())
    .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        runtime_and_transport_for_metric(&exporter, "custom.transport.requests"),
        (Some("instrumented-runtime".to_owned()), Some("instrumented".to_owned())),
        "an instrument recorded on cx.meter must inherit the runtime/transport scope"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_name_set_before_meter_provider_reaches_meter_scope() {
    let (exporter, provider) = exporter_and_provider();

    let client = counting_builder()
        .insecure_allow_http()
        .name("named_before_provider")
        .meter_provider(provider.clone())
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        scope_attribute_for_metric(&exporter, COUNTER, "http.client.name"),
        Some("named_before_provider".to_owned()),
        "a client name set before meter_provider must be reported as the http.client.name scope attribute"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_name_set_after_meter_provider_reaches_meter_scope() {
    let (exporter, provider) = exporter_and_provider();

    // Set the provider before the name. The client creates the scope at build time, so the later
    // name still applies.
    let client = counting_builder()
        .insecure_allow_http()
        .meter_provider(provider.clone())
        .name("named_after_provider")
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        scope_attribute_for_metric(&exporter, COUNTER, "http.client.name"),
        Some("named_after_provider".to_owned()),
        "a client name set after meter_provider must be reported as the http.client.name scope attribute"
    );
}

/// The counter that the transport of [`counting_builder`] records on its meter.
const COUNTER: &str = "custom.transport.requests";

/// A builder whose transport records [`COUNTER`] on the `fetch` meter for each handler it creates.
fn counting_builder() -> HttpClientBuilder {
    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        memory: OpaqueMemory::new(GlobalPool::new()),
        sink: Sink::noop(),
        extras: (),
    };
    create_builder(
        "counting-runtime",
        "counting",
        |cx: CustomContext| {
            cx.meter.u64_counter(COUNTER).build().add(1, &[]);
            OkHandler {
                body_builder: cx.body_builder,
            }
        },
        Isolation::Shared,
        deps,
    )
}

/// Returns a canned `200 OK`.
struct OkHandler {
    body_builder: http_extensions::HttpBodyBuilder,
}

impl Service<HttpRequest> for OkHandler {
    type Out = fetch::Result<HttpResponse>;

    fn execute(&self, _request: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        std::future::ready(HttpResponseBuilder::new(&self.body_builder).status(StatusCode::OK).build())
    }
}

testing_aids::init_tracing!();
