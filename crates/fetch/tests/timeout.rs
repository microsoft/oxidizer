// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for request timeout behavior.

use std::time::Duration;

use fetch::fake::FakeDeps;
use fetch::{HttpClient, Recovery, RecoveryInfo};
use http_extensions::FakeHandler;
use ohno::Labeled;
use tick::ClockControl;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn response_timeout() {
    let handler = FakeHandler::never_completes();
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(handler, FakeDeps { clock }).minimal_pipeline().build();

    let err = client
        .get("https://example.com")
        .response_timeout(Duration::from_secs(10))
        .fetch()
        .await
        .unwrap_err();

    assert_eq!(err.recovery(), RecoveryInfo::retry());
    assert_eq!(err.label(), "response_timeout");
}
