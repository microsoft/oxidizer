// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runnable examples for the [`fetch`](crate) crate.
//!
//! The source for each example is in the
//! [`crates/fetch/examples`](https://github.com/microsoft/oxidizer/tree/main/crates/fetch/examples)
//! folder. Each example is a standalone binary. Use `--all-features` to make every example
//! available.
//!
//! For example:
//!
//! ```sh
//! cargo run -p fetch --all-features --example fetch_tokio
//! ```
//!
//! # Getting started
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`fetch_tokio`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_tokio.rs) | Sends GET requests on Tokio, including one from a spawned task. |
//! | [`fetch_decompression`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_decompression.rs) | Enables gzip response decompression with a fake transport. |
//! | [`fetch_json`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_json.rs) | Deserializes a JSON response into a borrowed struct with [`fetch_json_ref`](crate::HttpRequestBuilder::fetch_json_ref) against a fake handler. |
//! | [`fetch_streaming`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_streaming.rs) | Writes each response [`Stream`](futures::Stream) chunk to a file. |
//!
//! # Configuration and pipelines
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`fetch_minimal_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_minimal_pipeline.rs) | Builds a client with [`minimal_pipeline`](crate::HttpClientBuilder::minimal_pipeline) and no middleware. |
//! | [`fetch_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_advanced.rs) | Sets connection keep-alive, pooling, HTTP/2 options, a custom rustls verifier, and resilience. |
//! | [`fetch_customization`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_customization.rs) | Customizes the standard pipeline's timeout, retry, and intercept layers. |
//! | [`fetch_custom_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_custom_pipeline.rs) | Replaces the standard pipeline with a fully custom layer stack via [`custom_pipeline`](crate::HttpClientBuilder::custom_pipeline). |
//! | [`fetch_custom`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_custom.rs) | Adds an echo transport through [`fetch::custom::create_builder`](crate::custom). |
//! | [`fetch_pooling`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_pooling.rs) | Selects round-robin from several fake transport slots. |
//! | [`fetch_connection_scaling`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_connection_scaling.rs) | Sends concurrent requests that can make the pool add connections. |
//! | [`fetch_api_with_templated_uri`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_api_with_templated_uri.rs) | Uses templated URIs in a typed REST client. |
//! | [`fetch_app`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_app.rs) | Uses [`fundle`] dependency injection, telemetry, and a custom rustls verifier. |
//!
//! # Resilience
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`fetch_resilience`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_resilience.rs) | Changes retry attempts, backoff, and recovery for a fake handler. |
//! | [`fetch_breaker`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_breaker.rs) | Opens a breaker for one origin while another origin continues to work. |
//!
//! # TLS and mutual TLS
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`fetch_native_tls`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_native_tls.rs) | Switches the client to the platform native-TLS backend. |
//! | [`fetch_mtls`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_mtls.rs) | Uses a `PEM` client identity with rustls and follows redirects manually. |
//! | [`fetch_native_tls_mtls`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_native_tls_mtls.rs) | Mutual TLS with the native-TLS backend using a `PEM` client identity. |
//!
//! # Telemetry and testing
//!
//! | Example | Description |
//! |---------|-------------|
//! | [`fetch_telemetry`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_telemetry.rs) | Uses a custom OpenTelemetry meter provider and classified URI components. |
//! | [`fetch_fake`](https://github.com/microsoft/oxidizer/blob/main/crates/fetch/examples/fetch_fake.rs) | Returns canned responses through [`FakeHandler`](crate::fake::FakeHandler). |
//!
//! [`fundle`]: https://docs.rs/fundle
