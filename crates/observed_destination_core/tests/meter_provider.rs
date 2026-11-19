// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public meter provider contracts.

#![cfg(test)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use observed_destination_core::FlushableMetricsProvider;
use opentelemetry_sdk::error::OTelSdkError;
use opentelemetry_sdk::metrics::SdkMeterProvider;

#[test]
fn sdk_flush_errors_are_preserved_in_destination_results() {
    let sdk = SdkMeterProvider::builder().build();
    let provider: &dyn FlushableMetricsProvider = &sdk;
    provider.force_flush().unwrap();
    sdk.shutdown().unwrap();

    let error = provider.force_flush().unwrap_err();
    assert!(matches!(error.downcast_ref::<OTelSdkError>(), Some(OTelSdkError::AlreadyShutdown)));
}
