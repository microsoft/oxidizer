// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public OTLP destinations deliver telemetry to a local collector.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![cfg(test)]

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::routing::post;
use axum::{Router, serve};
use observed_destination_core::{LoggerProviderBuilderExt, MeterProviderBuilderExt};
use observed_destination_otlp::{OtlpDestinationProtocol, otlp_logs, otlp_metrics};
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::metrics::MeterProvider;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{LogsService, LogsServiceServer};
use opentelemetry_proto::tonic::collector::logs::v1::{ExportLogsServiceRequest, ExportLogsServiceResponse};
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::{MetricsService, MetricsServiceServer};
use opentelemetry_proto::tonic::collector::metrics::v1::{ExportMetricsServiceRequest, ExportMetricsServiceResponse};
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{SdkMeterProvider, Temporality};
use prost::Message;
use rstest::rstest;
use rustls::crypto::aws_lc_rs::default_provider;
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::task::JoinHandle;
use tonic::service::Routes;
use tonic::{Request, Response, Status, async_trait};

fn initialize_crypto_provider() {
    static INIT: Once = Once::new();

    // Workspace feature unification can enable reqwest's provider-less Rustls backend.
    INIT.call_once(|| {
        default_provider().install_default().unwrap();
    });
}

#[rstest]
#[case::http(OtlpDestinationProtocol::Http)]
#[case::grpc(OtlpDestinationProtocol::Grpc)]
#[cfg_attr(
    miri,
    ignore = "OTLP delivery requires Tokio I/O, HTTP/gRPC clients, and SDK resource detection (current_exe)"
)]
fn metric_registration_delivers_measurement(#[case] protocol: OtlpDestinationProtocol) {
    initialize_crypto_provider();
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let _guard = runtime.enter();
    let (collector, endpoint, server) = runtime.block_on(Collector::start(protocol));
    let endpoint = match protocol {
        OtlpDestinationProtocol::Http => format!("{endpoint}/v1/metrics"),
        OtlpDestinationProtocol::Grpc => endpoint,
    };
    let destination = otlp_metrics(endpoint)
        .protocol(protocol)
        .temporality(Temporality::Delta)
        .export_interval(Duration::from_mins(1))
        .export_timeout(Duration::from_secs(5))
        .build();
    // Registration must also be safe when called from an async task.
    let provider = runtime.block_on(async { SdkMeterProvider::builder().register(destination).unwrap().build() });
    let counter = provider.meter("registration").u64_counter("requests").build();

    counter.add(7, &[]);

    // Runtime workers keep collector and gRPC I/O running while the SDK blocks.
    provider.force_flush().unwrap();
    provider.shutdown().unwrap();
    runtime.block_on(async move { drop(provider) });
    server.abort();
    runtime.block_on(server).unwrap_err();

    let requests = collector.metrics.lock().unwrap();
    let metric = requests
        .iter()
        .flat_map(|request| &request.resource_metrics)
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
        .find(|metric| metric.name == "requests")
        .unwrap();
    let Some(metric::Data::Sum(sum)) = &metric.data else {
        panic!("expected a counter sum");
    };

    assert_eq!(sum.data_points[0].value, Some(number_data_point::Value::AsInt(7)));
}

#[rstest]
#[case::http(OtlpDestinationProtocol::Http)]
#[case::grpc(OtlpDestinationProtocol::Grpc)]
#[cfg_attr(
    miri,
    ignore = "OTLP delivery requires Tokio I/O, HTTP/gRPC clients, and SDK resource detection (current_exe)"
)]
fn log_registration_delivers_record(#[case] protocol: OtlpDestinationProtocol) {
    initialize_crypto_provider();
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let _guard = runtime.enter();
    let (collector, endpoint, server) = runtime.block_on(Collector::start(protocol));
    let endpoint = match protocol {
        OtlpDestinationProtocol::Http => format!("{endpoint}/v1/logs"),
        OtlpDestinationProtocol::Grpc => endpoint,
    };
    let destination = otlp_logs(endpoint)
        .protocol(protocol)
        .export_timeout(Duration::from_secs(5))
        .build();
    let provider = runtime.block_on(async { SdkLoggerProvider::builder().register(destination).unwrap().build() });
    let logger = provider.logger("registration");
    let mut record = logger.create_log_record();
    record.set_body("collector test".into());
    record.set_severity_number(Severity::Info);

    logger.emit(record);
    provider.force_flush().unwrap();
    provider.shutdown().unwrap();
    runtime.block_on(async move { drop(provider) });
    server.abort();
    runtime.block_on(server).unwrap_err();

    let requests = collector.logs.lock().unwrap();
    let record = requests
        .iter()
        .flat_map(|request| &request.resource_logs)
        .flat_map(|resource| &resource.scope_logs)
        .flat_map(|scope| &scope.log_records)
        .next()
        .unwrap();

    assert_eq!(
        record.body.as_ref().unwrap().value,
        Some(any_value::Value::StringValue("collector test".into()))
    );
}

#[derive(Clone, Default)]
struct Collector {
    logs: Arc<Mutex<Vec<ExportLogsServiceRequest>>>,
    metrics: Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
}

impl Collector {
    async fn start(protocol: OtlpDestinationProtocol) -> (Self, String, JoinHandle<()>) {
        let collector = Self::default();

        // Expose only the selected transport so incorrect protocol wiring fails.
        let router = match protocol {
            OtlpDestinationProtocol::Http => Router::new()
                .route(
                    "/v1/logs",
                    post(|State(collector): State<Self>, body: Bytes| async move {
                        collector.logs.lock().unwrap().push(ExportLogsServiceRequest::decode(body).unwrap());
                        (
                            [(CONTENT_TYPE, "application/x-protobuf")],
                            ExportLogsServiceResponse::default().encode_to_vec(),
                        )
                    }),
                )
                .route(
                    "/v1/metrics",
                    post(|State(collector): State<Self>, body: Bytes| async move {
                        collector
                            .metrics
                            .lock()
                            .unwrap()
                            .push(ExportMetricsServiceRequest::decode(body).unwrap());
                        (
                            [(CONTENT_TYPE, "application/x-protobuf")],
                            ExportMetricsServiceResponse::default().encode_to_vec(),
                        )
                    }),
                )
                .with_state(collector.clone()),
            OtlpDestinationProtocol::Grpc => Routes::new(LogsServiceServer::new(collector.clone()))
                .add_service(MetricsServiceServer::new(collector.clone()))
                .into_axum_router(),
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            serve(listener, router).await.unwrap();
        });

        (collector, endpoint, server)
    }
}

#[async_trait]
impl LogsService for Collector {
    async fn export(&self, request: Request<ExportLogsServiceRequest>) -> Result<Response<ExportLogsServiceResponse>, Status> {
        self.logs.lock().unwrap().push(request.into_inner());
        Ok(Response::new(ExportLogsServiceResponse::default()))
    }
}

#[async_trait]
impl MetricsService for Collector {
    async fn export(&self, request: Request<ExportMetricsServiceRequest>) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        self.metrics.lock().unwrap().push(request.into_inner());
        Ok(Response::new(ExportMetricsServiceResponse::default()))
    }
}
