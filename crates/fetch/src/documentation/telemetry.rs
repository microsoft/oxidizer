// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry emitted by the [`fetch`](crate) crate.
//!
//! `fetch` emits its [request telemetry](#request-telemetry) as `observed` events through the sink
//! of its deps. Its [connection metrics](#connection-metrics) use the `fetch` OpenTelemetry
//! [`Meter`](opentelemetry::metrics::Meter). The client gets the meter from the global
//! [`MeterProvider`](opentelemetry::metrics::MeterProvider) or a custom provider set through
//! [`HttpClientBuilder::meter_provider`](crate::HttpClientBuilder::meter_provider).
//!
//! # Client attributes
//!
//! Every client has three attributes: its runtime, its transport and its name. The instrumentation
//! scope of its `fetch` meter has them, so the connection metrics and the instruments of
//! [`CustomContext::meter`](crate::custom::CustomContext::meter) carry them. Every request event
//! has them as fields:
//!
//! | Key | Description | Sample value |
//! |-----|-------------|--------------|
//! | `fetch.runtime` | Identifies the async runtime the client is built for | `"tokio"` |
//! | `fetch.transport` | Identifies the transport handler the client dispatches through | `"hyper"` |
//! | `http.client.name` | Identifies the client. [`HttpClientBuilder::name`](crate::HttpClientBuilder::name) sets it. The default is `"http_client"`. | `"crates_api_client"` |
//!
//! The default Tokio and hyper transport reports `"tokio"` and `"hyper"`. A fake client reports
//! `"fake"` for both values. A custom transport reports the names passed to
//! [`custom::create_builder`](crate::custom::create_builder).
//!
//! # Request telemetry
//!
//! `fetch` emits its request telemetry as the client events of
//! [`http_telemetry`](https://docs.rs/http_telemetry/latest/http_telemetry/), through the sink of its
//! deps. The [standard pipeline](crate::pipeline::StandardRequestPipeline) emits one attempt event
//! for each attempt, which records `http.client.request.duration`, and one total event for each
//! request, which records `http.client.request.total_duration`. Request durations are in seconds,
//! like the connection durations.
//!
//! A custom pipeline emits request events only if it stacks the layer of
//! [`PipelineContext::client_telemetry`](crate::pipeline::PipelineContext::client_telemetry). A
//! minimal pipeline emits no request events.
//!
//! # Connection metrics
//!
//! | Metric | Instrument | Unit | Emitted when |
//! |--------|-----------|------|--------------|
//! | [`http.client.connection.setup.duration`](#httpclientconnectionsetupduration) | `Histogram<f64>` | `s` | Name resolution and TCP setup finish, with a TLS handshake for HTTPS |
//! | [`http.client.connection.duration`](#httpclientconnectionduration) | `Histogram<f64>` | `s` | A connection is closed (the underlying stream is dropped) |
//!
//! ---
//!
//! ## `http.client.connection.setup.duration`
//!
//! Measures name resolution and TCP connection setup. For HTTPS, it also measures the TLS
//! handshake. It records success and failure.
//!
//! This metric is only available when the Hyper-based transport is in use
//! (the default for the Tokio runtime).
//!
//! Histogram boundaries in seconds: `0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 25.0, 50.0`.
//!
//! ### Attributes
//!
//! | Attribute | Required | Description | Sample value |
//! |-----------|----------|-------------|--------------|
//! | `server.address` | always | Hostname of the target server | `"api.example.com"` |
//! | `server.port` | always | Port number of the target server | `443` |
//! | `url.scheme` | always | URI scheme | `"https"` |
//! | `network.protocol.version` | on success | Negotiated protocol version (`"2"` for HTTP/2, `"1"` for HTTP/1) | `"2"` |
//! | `error.type` | on failure | Label classifying the failure | `"timeout"` |
//!
//! ---
//!
//! ## `http.client.connection.duration`
//!
//! Measures a connection from successful establishment until the underlying stream is dropped.
//! Use it to inspect connection reuse and pool behavior.
//!
//! This metric is only available when the Hyper-based transport is in use.
//!
//! Histogram boundaries in seconds: `0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0`.
//!
//! ### Attributes
//!
//! | Attribute | Required | Description | Sample value |
//! |-----------|----------|-------------|--------------|
//! | `server.address` | always | Hostname of the target server | `"api.example.com"` |
//! | `server.port` | always | Port number of the target server | `443` |
//! | `url.scheme` | always | URI scheme | `"https"` |
//! | `network.protocol.version` | always | Negotiated protocol version (`"2"` for HTTP/2, `"1"` for HTTP/1) | `"2"` |
//!
//! ---
//!
//! # Error labels
//!
//! The `error.type` attribute is a dot-separated label chain. It reads the error `source()` chain
//! from the outer error to the inner error:
//!
//! ```text
//! "request_hyper.connect.timed_out"
//!  ^^^^^^^^^^^^^^ ^^^^^^^ ^^^^^^^^^
//!  transport       phase   IO error kind
//! ```
//!
//! ## Examples
//!
//! | `error.type` | What happened |
//! |--------------|---------------|
//! | `request_hyper.connect.timed_out` | Name resolution, TCP connection, or TLS setup timed out |
//! | `request_hyper.connect.connection_refused` | Server refused the connection |
//! | `request_hyper.connect.other` | TLS or unclassified connection error |
//! | `scheme_not_allowed` | URI scheme rejected by the client policy or transport before network I/O |
//! | `compression_invalid` | Response body was malformed for its declared compression format |
//! | `compression_limit_exceeded` | A configured limit stopped further decompression |
//! | `compression_unsupported` | Response used a compression format that is not enabled |
//! | `abandoned` | Caller dropped the future, for example because of an outer timeout |
//! | `panic` | The inner handler panicked |
