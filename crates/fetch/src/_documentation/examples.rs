// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runnable examples for the [`fetch`](crate) crate.
//!
//! The full, runnable source for every example lives in the
//! [`crates/fetch/examples`](https://github.com/microsoft/oxidizer/tree/main/crates/fetch/examples)
//! folder on GitHub. Each example is a standalone binary; the simplest way to
//! run any of them is to enable every feature with `--all-features`.
//!
//! For example:
//!
//! ```sh
//! cargo run -p fetch --all-features --example http_client_tokio
//! ```
//!
//! # Getting started
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`http_client_tokio`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_tokio.rs) | Minimal Tokio-runtime client that issues a couple of GET requests, including one from a spawned task. |
//! | [`http_client_json`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_json.rs) | Deserializes a JSON response into a borrowed struct with [`fetch_json`](crate::HttpRequestBuilder::fetch_json) against a fake handler. |
//! | [`http_client_streaming`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_streaming.rs) | Downloads a response incrementally as a [`Stream`](futures::Stream) and writes each chunk to a file. |
//!
//! # Configuration & pipelines
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`http_client_minimal_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_minimal_pipeline.rs) | Builds a lightweight client with [`minimal_pipeline`](crate::HttpClientBuilder::minimal_pipeline) (no middleware). |
//! | [`http_client_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_advanced.rs) | Extensive configuration: connection keep-alive, pooling, HTTP/2 options, a custom rustls verifier, and resilience tuning. |
//! | [`http_client_customization`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_customization.rs) | Customizes the standard pipeline's timeout, retry, and intercept layers. |
//! | [`http_client_custom_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_custom_pipeline.rs) | Replaces the standard pipeline with a fully custom layer stack via [`custom_pipeline`](crate::HttpClientBuilder::custom_pipeline). |
//! | [`http_client_custom`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_custom.rs) | Plugs a custom transport handler (an echo handler) into [`fetch::custom::create_builder`](crate::custom). |
//! | [`http_client_pooling`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_pooling.rs) | Uses multiple connection pools to work around the single-HTTP/2-connection-per-host limit. |
//! | [`http_client_connection_scaling`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_connection_scaling.rs) | Fires a burst of concurrent requests to show the pool scaling connections up and down. |
//! | [`http_client_api_with_templated_uri`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_api_with_templated_uri.rs) | Wraps a REST API in a typed client using templated URIs for multiple endpoints. |
//! | [`http_client_app`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_app.rs) | Assembles an application with [`fundle`] dependency injection, telemetry, and a custom rustls verifier. |
//!
//! # Resilience
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`http_client_resilience`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_resilience.rs) | Customizes retry behavior (attempts, backoff, recovery) against a fake handler that fails then succeeds. |
//! | [`http_client_breaker`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_breaker.rs) | Demonstrates the per-origin circuit breaker tripping for a failing host while a healthy host keeps working. |
//!
//! # TLS & mutual TLS
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`http_client_native_tls`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_native_tls.rs) | Switches the client to the platform native-TLS backend. |
//! | [`http_client_mtls`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_mtls.rs) | Mutual TLS with the rustls backend using a PEM client identity, following redirects manually. |
//! | [`http_client_native_tls_mtls`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_native_tls_mtls.rs) | Mutual TLS with the native-TLS backend using a PEM client identity. |
//!
//! # Telemetry & testing
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`http_client_telemetry`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_telemetry.rs) | Wires a custom OpenTelemetry meter provider and shows URI-component classification for safe metrics. |
//! | [`http_client_fake`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/http_client_fake.rs) | Mocks the transport with a [`FakeHandler`](crate::fake::FakeHandler) returning canned responses. |
//!
//! [`fundle`]: https://docs.rs/fundle
