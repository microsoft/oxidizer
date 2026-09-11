// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract stubs for automatic response decompression.

#[test]
#[ignore = "contract stub"]
fn fetch_decompresses_configured_response() {
    // Enable a response codec and assert both standard and minimal pipelines decode it.
}

#[test]
#[ignore = "contract stub"]
fn fetch_keeps_decompression_disabled_by_default() {
    // Compile codecs without configuring the client and assert headers and bytes remain unchanged.
}

#[test]
#[ignore = "contract stub"]
fn fetch_applies_decompression_limits() {
    // Configure output and stream limits and assert the body fails lazily at the boundary.
}
