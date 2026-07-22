// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests verifying that the `fetch.runtime` and `fetch.transport`
//! instrumentation-scope attributes are attached to the meter of every HTTP
//! client and therefore land on the metrics that client emits.

use bytes::Bytes;
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::tokio::TokioDeps;
use fetch::{HttpClient, HttpRequest, HttpResponse, HttpResponseBuilder};
use http::StatusCode;
use layered::Service;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tick::Clock;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RUNTIME_ATTRIBUTE: &str = "fetch.runtime";
const TRANSPORT_ATTRIBUTE: &str = "fetch.transport";

/// Returns the value of scope attribute `attribute_key` on the scope that
/// recorded `metric_name`, if any.
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

/// Returns the `(fetch.runtime, fetch.transport)` scope attributes of the scope
/// that recorded `metric_name`.
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

async fn serve(body: impl Into<Bytes>) -> MockServer {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/hello-world"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.into().to_vec()))
        .mount(&mock_server)
        .await;

    mock_server
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn fake_transport_scope_attribute() {
    let (exporter, provider) = exporter_and_provider();

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .meter_provider(provider.clone())
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        runtime_and_transport_for_metric(&exporter, "http.client.request.duration"),
        (Some("fake".to_owned()), Some("fake".to_owned())),
        "the fake client must report fetch.runtime=fake and fetch.transport=fake on its request metric"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn custom_transport_scope_attribute() {
    let (exporter, provider) = exporter_and_provider();

    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        global_pool: bytesbuf::mem::GlobalPool::new(),
        extras: (),
    };

    let client = create_builder(
        "my-runtime",
        "my-custom",
        |cx: CustomContext| OkHandler {
            body_builder: cx.body_builder,
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
        runtime_and_transport_for_metric(&exporter, "http.client.request.duration"),
        (Some("my-runtime".to_owned()), Some("my-custom".to_owned())),
        "a custom transport must report the runtime and transport names passed to create_builder"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn custom_transport_instrument_inherits_scope() {
    let (exporter, provider) = exporter_and_provider();

    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        global_pool: bytesbuf::mem::GlobalPool::new(),
        extras: (),
    };

    // The transport records its own instrument against `cx.meter`; it must
    // inherit the same runtime/transport scope as the built-in fetch metrics.
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
async fn tokio_transport_connection_metric_scope_attribute() {
    let (exporter, provider) = exporter_and_provider();

    let client = HttpClient::builder_tokio(TokioDeps::default())
        .insecure_allow_http()
        .meter_provider(provider.clone())
        .build();

    let server = serve(Bytes::from_static(b"hello")).await;
    client.get(server.uri() + "/hello-world").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        runtime_and_transport_for_metric(&exporter, "http.client.connection.setup.duration"),
        (Some("tokio".to_owned()), Some("hyper".to_owned())),
        "the hyper connection metric must carry the tokio runtime and hyper transport scope"
    );
    assert_eq!(
        runtime_and_transport_for_metric(&exporter, "http.client.request.duration"),
        (Some("tokio".to_owned()), Some("hyper".to_owned())),
        "the request metric must carry the tokio runtime and hyper transport scope"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_name_set_before_meter_provider_is_reported() {
    let (exporter, provider) = exporter_and_provider();

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .name("named_before_provider")
        .meter_provider(provider.clone())
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        scope_attribute_for_metric(&exporter, "http.client.request.duration", "http.client.name"),
        Some("named_before_provider".to_owned()),
        "a client name set before meter_provider must be reported as the http.client.name scope attribute"
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_name_set_after_meter_provider_is_reported() {
    let (exporter, provider) = exporter_and_provider();

    // Setting the meter provider *before* the name exercises the deferred-scope path: the
    // custom meter is only materialized at build time, so the later name still applies.
    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .meter_provider(provider.clone())
        .name("named_after_provider")
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(
        scope_attribute_for_metric(&exporter, "http.client.request.duration", "http.client.name"),
        Some("named_after_provider".to_owned()),
        "a client name set after meter_provider must be reported as the http.client.name scope attribute"
    );
}

/// Transport handler that returns a canned `200 OK`.
struct OkHandler {
    body_builder: http_extensions::HttpBodyBuilder,
}

impl Service<HttpRequest> for OkHandler {
    type Out = fetch::Result<HttpResponse>;

    async fn execute(&self, _request: HttpRequest) -> Self::Out {
        HttpResponseBuilder::new(&self.body_builder).status(StatusCode::OK).build()
    }
}
