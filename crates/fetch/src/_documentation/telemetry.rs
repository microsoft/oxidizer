// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry emitted by the [`fetch`](crate) crate.
//!
//! All metrics are recorded under the **`fetch`** OpenTelemetry
//! [`Meter`](opentelemetry::metrics::Meter). The meter is obtained from either
//! the global [`MeterProvider`](opentelemetry::metrics::MeterProvider) or from
//! a custom provider supplied via
//! [`HttpClientBuilder::meter_provider`](crate::HttpClientBuilder::meter_provider).
//!
//! # Instrumentation scope
//!
//! Every client carries a single attribute on the **instrumentation scope** of
//! its `fetch` meter, so it applies to *all* metrics the client emits (both
//! request and connection metrics):
//!
//! | Scope attribute | Description | Sample value |
//! |-----------------|-------------|--------------|
//! | `fetch.transport` | Identifies the transport handler the client dispatches through | `"hyper-on-tokio"` |
//!
//! The bundled transports report fixed values — `"hyper-on-tokio"` for the
//! default Tokio + hyper transport and `"fake"` for the transport used by fake
//! HTTP clients. A custom transport reports the `name` passed to
//! [`custom::create_builder`](crate::custom::create_builder).
//!
//! # Metrics
//!
//! | Metric | Instrument | Unit | Emitted when |
//! |--------|-----------|------|--------------|
//! | [`http.client.request.duration`](#httpclientrequestduration) | `Histogram<f64>` | `s` | Every HTTP request completes (success **or** failure) |
//! | [`http.client.connection.setup.duration`](#httpclientconnectionsetupduration) | `Histogram<f64>` | `s` | A TCP/TLS connection attempt finishes (success **or** failure) |
//! | [`http.client.connection.duration`](#httpclientconnectionduration) | `Histogram<f64>` | `s` | A connection is closed (the underlying stream is dropped) |
//!
//! ---
//!
//! ## `http.client.request.duration`
//!
//! Measures the total wall-clock time of an HTTP request from the moment
//! the request enters [`Metrics`](crate::handlers::Metrics)
//! until a response (or error) is returned. Follows the
//! [OpenTelemetry `http.client.request.duration`](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#metric-httpclientrequestduration)
//! semantic convention.
//!
//! **Histogram boundaries (seconds):** `0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0`
//!
//! ### Attributes
//!
//! | Attribute | Required | Description | Sample value |
//! |-----------|----------|-------------|--------------|
//! | `http.request.method` | always | HTTP method of the request | `"GET"` |
//! | `server.address` | always | Hostname (authority) of the target server | `"api.example.com"` |
//! | `server.port` | when derivable | Port number, inferred from scheme when absent (`443` for HTTPS, `80` for HTTP) | `443` |
//! | `url.scheme` | always | URI scheme | `"https"` |
//! | `url.template` | optional | URL template or label when the request was built from a [`templated`](templated_uri::templated) URI | `"/api/v1/crates/{crate_name}"` |
//! | `network.protocol.name` | on success | Network protocol name | `"http"` |
//! | `network.protocol.version` | on success | Negotiated HTTP version of the response | `"1.1"` |
//! | `http.response.status_code` | on success | HTTP status code of the response | `200` |
//! | `error.type` | on failure | A short, metrics-friendly label classifying the error | `"io"` |
//!
//! > **Custom attributes.** Any [`TelemetryAttributes`](crate::telemetry::TelemetryAttributes)
//! > attached to the request or response extensions are merged into the
//! > attribute set. This allows callers to inject domain-specific dimensions.
//!
//! ---
//!
//! ## `http.client.connection.setup.duration`
//!
//! Measures the time it takes to establish a new connection (TCP + TLS
//! handshake). Recorded once per connection attempt — on both success and
//! failure.
//!
//! This metric is only available when the Hyper-based transport is in use
//! (the default for the Tokio runtime).
//!
//! **Histogram boundaries (seconds):** `0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 25.0, 50.0`
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
//! Measures the total lifetime of a connection — from the moment it was
//! successfully established until the underlying stream is dropped (closed).
//! Useful for understanding connection reuse and pool behavior.
//!
//! This metric is only available when the Hyper-based transport is in use.
//!
//! **Histogram boundaries (seconds):** `0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0`
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
//! The `error.type` attribute is a dot-separated label chain built by walking
//! the error's `source()` chain outermost-first, pinpointing *where* the
//! failure occurred:
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
//! | `request_hyper.connect.timed_out` | TCP/TLS handshake timed out |
//! | `request_hyper.connect.connection_refused` | Server refused the connection |
//! | `request_hyper.connect.other` | TLS or unclassified connection error |
//! | `scheme_not_allowed` | HTTP scheme blocked before reaching the network |
//! | `content_encoding_unsupported` | Response used an encoding the client cannot decode |
//! | `abandoned` | Caller dropped the future (e.g. outer timeout) |
