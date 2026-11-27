// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract test stubs for Kubernetes TCP readiness.

#[test]
#[ignore = "stub"]
fn tcp_probe_follows_health_and_releases_port() {
    // Start with an unhealthy aggregate and assert that the readiness address is closed.
    // Publish Healthy and assert that real TCP connections succeed.
    // Accept more than one connection and assert that the address stays open.
    // Publish Degraded and Unhealthy and assert that the address is closed.
    // Publish Healthy again and assert that the address opens again.
    // Cancel the server and assert that a new listener can bind the same address.
}

#[test]
#[ignore = "stub"]
fn tcp_probe_reports_bind_failure() {
    // Keep a listener bound to the readiness address.
    // Start the probe with a healthy aggregate.
    // Assert that the probe returns AddrInUse instead of waiting.
}
