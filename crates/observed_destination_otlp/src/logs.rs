// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! OTLP log destination.

use std::time::Duration;

use observed_destination_core::{DestinationResult, IntoLogDestination, LogDestination};
use opentelemetry_otlp::{
    HttpExporterBuilderSet, LogExporter, LogExporterBuilder, OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, Protocol, WithExportConfig,
};
use opentelemetry_sdk::logs::{BatchLogProcessor, LoggerProviderBuilder};

use crate::http_client::with_blocking_http_client;
use crate::{OtlpDestinationProtocol, with_optional_timeout};

/// Creates a batched OTLP log destination, using HTTP/protobuf by default.
///
/// # Examples
///
/// ```
/// use observed_destination_otlp::otlp_logs;
///
/// let destination = otlp_logs("http://localhost:4318/v1/logs").build();
/// ```
pub fn otlp_logs(endpoint: impl Into<String>) -> OtlpLogDestinationBuilder<otlp_log_destination_builder::SetEndpoint> {
    OtlpLogDestination::builder().endpoint(endpoint.into())
}

/// A log destination that sends batches over OTLP HTTP/protobuf or gRPC.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct OtlpLogDestination {
    /// Collector URL.
    endpoint: String,
    /// Export transport (default: HTTP/protobuf).
    #[builder(default = OtlpDestinationProtocol::Http)]
    protocol: OtlpDestinationProtocol,
    /// Optional export timeout override.
    ///
    /// When unset, uses `OTEL_EXPORTER_OTLP_LOGS_TIMEOUT`, then
    /// `OTEL_EXPORTER_OTLP_TIMEOUT`, then 10 seconds. Environment values are in milliseconds.
    export_timeout: Option<Duration>,
}

impl OtlpLogDestination {
    fn register_with_http_config(
        self,
        builder: LoggerProviderBuilder,
        configure_http: impl FnOnce(
            LogExporterBuilder<HttpExporterBuilderSet>,
            Option<Duration>,
            &str,
        ) -> DestinationResult<LogExporterBuilder<HttpExporterBuilderSet>>,
    ) -> DestinationResult<LoggerProviderBuilder> {
        let exporter = match self.protocol {
            OtlpDestinationProtocol::Http => configure_http(
                LogExporter::builder()
                    .with_http()
                    .with_endpoint(self.endpoint)
                    .with_protocol(Protocol::HttpBinary),
                self.export_timeout,
                OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
            )?
            .build()?,
            OtlpDestinationProtocol::Grpc => with_optional_timeout(
                LogExporter::builder()
                    .with_tonic()
                    .with_endpoint(self.endpoint)
                    .with_protocol(Protocol::Grpc),
                self.export_timeout,
                WithExportConfig::with_timeout,
            )
            .build()?,
        };

        let processor = BatchLogProcessor::builder(exporter).build();

        Ok(builder.with_log_processor(processor))
    }
}

impl LogDestination for OtlpLogDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        self.register_with_http_config(builder, with_blocking_http_client)
    }
}

impl<S: otlp_log_destination_builder::IsComplete> IntoLogDestination for OtlpLogDestinationBuilder<S> {
    type Destination = OtlpLogDestination;

    fn into_log_destination(self) -> OtlpLogDestination {
        self.build()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::{Error, ErrorKind};

    use observed_destination_core::LoggerProviderBuilderExt;
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use crate::{OtlpDestinationProtocol, otlp_logs};

    #[test]
    fn log_registration_propagates_http_client_error() {
        let destination = otlp_logs("http://localhost:4318/v1/logs").build();
        let expected = ErrorKind::PermissionDenied;

        let error = destination
            .register_with_http_config(SdkLoggerProvider::builder(), |_, _, _| Err(Error::from(expected).into()))
            .unwrap_err();

        assert_eq!(error.downcast::<Error>().unwrap().kind(), expected);
    }

    #[test]
    #[cfg_attr(
        all(miri, windows),
        ignore = "Reqwest's Tokio runtime requires CreateIoCompletionPort, which Miri does not support"
    )]
    fn log_registration_propagates_exporter_error() {
        for protocol in [OtlpDestinationProtocol::Grpc, OtlpDestinationProtocol::Http] {
            let result = SdkLoggerProvider::builder().register(otlp_logs("http://[invalid").protocol(protocol));

            result.unwrap_err();
        }
    }
}
