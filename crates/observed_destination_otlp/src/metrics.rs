// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! OTLP metric destination.

use std::time::Duration;

use observed_destination_core::{DestinationResult, IntoMetricDestination, MetricDestination};
use opentelemetry_otlp::{
    HttpExporterBuilderSet, MetricExporter, MetricExporterBuilder, OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, Protocol, WithExportConfig,
};
use opentelemetry_sdk::metrics::{MeterProviderBuilder, PeriodicReader, Temporality};

use crate::http_client::with_blocking_http_client;
use crate::{OtlpDestinationProtocol, with_optional_timeout};

/// Creates a periodic OTLP metric destination, using HTTP/protobuf by default.
///
/// # Examples
///
/// ```
/// use observed_destination_otlp::otlp_metrics;
///
/// let destination = otlp_metrics("http://localhost:4318/v1/metrics").build();
/// ```
pub fn otlp_metrics(endpoint: impl Into<String>) -> OtlpMetricDestinationBuilder<otlp_metric_destination_builder::SetEndpoint> {
    OtlpMetricDestination::builder().endpoint(endpoint.into())
}

/// A metric destination that periodically exports over OTLP gRPC or HTTP/protobuf.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct OtlpMetricDestination {
    /// Collector URL.
    endpoint: String,
    /// Export transport (default: HTTP/protobuf).
    #[builder(default = OtlpDestinationProtocol::Http)]
    protocol: OtlpDestinationProtocol,
    /// Optional export timeout override.
    ///
    /// When unset, uses `OTEL_EXPORTER_OTLP_METRICS_TIMEOUT`, then
    /// `OTEL_EXPORTER_OTLP_TIMEOUT`, then 10 seconds. Environment values are in milliseconds.
    export_timeout: Option<Duration>,
    /// Optional interval between metric exports.
    ///
    /// When unset or zero, uses `OTEL_METRIC_EXPORT_INTERVAL` in milliseconds,
    /// then 60 seconds.
    export_interval: Option<Duration>,
    /// Optional metric aggregation temporality preference.
    ///
    /// When unset, uses `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`,
    /// then cumulative temporality.
    temporality: Option<Temporality>,
}

impl OtlpMetricDestination {
    fn register_with_http_config(
        self,
        builder: MeterProviderBuilder,
        configure_http: impl FnOnce(
            MetricExporterBuilder<HttpExporterBuilderSet>,
            Option<Duration>,
            &str,
        ) -> DestinationResult<MetricExporterBuilder<HttpExporterBuilderSet>>,
    ) -> DestinationResult<MeterProviderBuilder> {
        let mut exporter = MetricExporter::builder();
        if let Some(temporality) = self.temporality {
            exporter = exporter.with_temporality(temporality);
        }
        let exporter = match self.protocol {
            OtlpDestinationProtocol::Http => configure_http(
                exporter
                    .with_http()
                    .with_endpoint(self.endpoint)
                    .with_protocol(Protocol::HttpBinary),
                self.export_timeout,
                OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
            )?
            .build()?,
            OtlpDestinationProtocol::Grpc => with_optional_timeout(
                exporter.with_tonic().with_endpoint(self.endpoint).with_protocol(Protocol::Grpc),
                self.export_timeout,
                WithExportConfig::with_timeout,
            )
            .build()?,
        };

        let mut reader = PeriodicReader::builder(exporter);
        if let Some(interval) = self.export_interval {
            reader = reader.with_interval(interval);
        }

        Ok(builder.with_reader(reader.build()))
    }
}

impl MetricDestination for OtlpMetricDestination {
    fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
        self.register_with_http_config(builder, with_blocking_http_client)
    }
}

impl<S: otlp_metric_destination_builder::IsComplete> IntoMetricDestination for OtlpMetricDestinationBuilder<S> {
    type Destination = OtlpMetricDestination;

    fn into_metric_destination(self) -> OtlpMetricDestination {
        self.build()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::{Error, ErrorKind};

    use observed_destination_core::MeterProviderBuilderExt;
    use opentelemetry_sdk::metrics::SdkMeterProvider;

    use crate::{OtlpDestinationProtocol, otlp_metrics};

    #[test]
    fn metric_registration_propagates_http_client_error() {
        let destination = otlp_metrics("http://localhost:4318/v1/metrics").build();
        let expected = ErrorKind::PermissionDenied;

        let error = destination
            .register_with_http_config(SdkMeterProvider::builder(), |_, _, _| Err(Error::from(expected).into()))
            .unwrap_err();

        assert_eq!(error.downcast::<Error>().unwrap().kind(), expected);
    }

    #[test]
    #[cfg_attr(
        all(miri, windows),
        ignore = "Reqwest's Tokio runtime requires CreateIoCompletionPort, which Miri does not support"
    )]
    fn metric_registration_propagates_exporter_error() {
        for protocol in [OtlpDestinationProtocol::Grpc, OtlpDestinationProtocol::Http] {
            let result = SdkMeterProvider::builder().register(otlp_metrics("http://[invalid").protocol(protocol));

            result.unwrap_err();
        }
    }
}
