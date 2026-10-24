// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;

use crate::DestinationResult;

/// A complete meter provider that can explicitly flush pending measurements.
///
/// OpenTelemetry's [`MeterProvider`] supplies meters but exposes no flush
/// operation. This contract lets a processor retain the provider itself and
/// flush it before releasing its provider handle and cached instruments.
///
/// Flush applies to the pipeline owned by the provider. A provider
/// that forwards raw measurements synchronously has no local buffer to flush;
/// it does not thereby flush the receiving runtime's aggregation pipeline.
/// Terminal cleanup follows the provider's own ownership and drop semantics,
/// rather than one consumer shutting down a pipeline that others may still use.
pub trait FlushableMetricsProvider: MeterProvider + Debug + Send + Sync {
    /// Flushes the provider's pipeline without terminating it.
    ///
    /// Uses the destination error contract, independently of the provider's SDK.
    ///
    /// # Errors
    ///
    /// Returns an error if the provider has been shut down or if flushing fails.
    fn force_flush(&self) -> DestinationResult<()>;
}

impl FlushableMetricsProvider for SdkMeterProvider {
    fn force_flush(&self) -> DestinationResult<()> {
        Self::force_flush(self).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry_sdk::error::OTelSdkError;

    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "SDK resource detection uses current_exe")]
    fn sdk_flush_errors_are_preserved_in_destination_results() {
        let sdk = SdkMeterProvider::builder().build();
        let provider: &dyn FlushableMetricsProvider = &sdk;
        provider.force_flush().unwrap();
        sdk.shutdown().unwrap();

        let error = provider.force_flush().unwrap_err();
        assert!(matches!(error.downcast_ref::<OTelSdkError>(), Some(OTelSdkError::AlreadyShutdown)));
    }
}
