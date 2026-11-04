// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry emitted by the [`fetch`](crate) crate.
//!
//! All metrics use the `fetch` OpenTelemetry [`Meter`](opentelemetry::metrics::Meter). The client
//! gets the meter from the global [`MeterProvider`](opentelemetry::metrics::MeterProvider) or a
//! custom provider set through
//! [`HttpClientBuilder::meter_provider`](crate::HttpClientBuilder::meter_provider).
//!
//! # Instrumentation scope
//!
//! Every client adds attributes to the instrumentation scope of its `fetch` meter. The attributes
//! apply to request and connection metrics:
//!
//! | Scope attribute | Description | Sample value |
//! |-----------------|-------------|--------------|
//! | `fetch.runtime` | Identifies the async runtime the client is built for | `"tokio"` |
//! | `fetch.transport` | Identifies the transport handler the client dispatches through | `"hyper"` |
//! | `http.client.name` | Identifies the client. [`HttpClientBuilder::name`](crate::HttpClientBuilder::name) sets it. The default is `"http_client"`. | `"crates_api_client"` |
//!
//! The default Tokio and hyper transport reports `"tokio"` and `"hyper"`. A fake client reports
//! `"fake"` for both attributes. A custom transport reports the names passed to
//! [`custom::create_builder`](crate::custom::create_builder).
//!
//! # Metrics
//!
//! | Metric | Instrument | Unit | Emitted when |
//! |--------|-----------|------|--------------|
//! | [`http.client.request.duration`](#httpclientrequestduration) | `Histogram<f64>` | `s` | An HTTP request attempt returns or its future is dropped |
//! | [`http.client.request.total_duration`](#httpclientrequesttotal_duration) | `Histogram<f64>` | `s` | A logical request returns or its future is dropped, including all retries and hedged attempts |
//! | [`http.client.connection.setup.duration`](#httpclientconnectionsetupduration) | `Histogram<f64>` | `s` | Name resolution and TCP setup finish, with a TLS handshake for HTTPS |
//! | [`http.client.connection.duration`](#httpclientconnectionduration) | `Histogram<f64>` | `s` | A connection is closed (the underlying stream is dropped) |
//!
//! ---
//!
//! ## `http.client.request.duration`
//!
//! Measures one HTTP request attempt from entry into [`Metrics`](crate::handlers::Metrics) until a
//! response or error returns. It follows the
//! [OpenTelemetry `http.client.request.duration`](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#metric-httpclientrequestduration)
//! semantic convention.
//!
//! The [standard pipeline](crate::pipeline::StandardRequestPipeline) records one measurement for
//! the initial attempt and each retry or hedged attempt. See
//! [`http.client.request.total_duration`](#httpclientrequesttotal_duration) for one measurement of
//! the logical request.
//!
//! If the caller drops an attempt future, the metric uses `error.type=abandoned`.
//!
//! Histogram boundaries in seconds: `0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0`.
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
//! > **Note**: The metric includes [`TelemetryAttributes`](crate::telemetry::TelemetryAttributes)
//! > from request and response extensions.
//!
//! ---
//!
//! ## `http.client.request.total_duration`
//!
//! This proprietary Oxidizer metric is not part of the OpenTelemetry HTTP semantic conventions.
//! Oxidizer defines it in the OpenTelemetry `http.client.*` namespace. It measures the logical
//! request, including every retry and hedged attempt.
//!
//! Timing starts when the request enters the outermost metrics layer. It stops when the first
//! accepted response or terminal error returns. Concurrent hedged attempts do not add duplicate
//! time.
//!
//! A [`Metrics`](crate::handlers::Metrics) handler records this instrument when
//! [`report_total_duration`](crate::handlers::MetricsLayer::report_total_duration) is enabled. In
//! the [standard pipeline](crate::pipeline::StandardRequestPipeline), the outermost `total_metrics`
//! layer wraps the total timeout, recovery, circuit breaker, and per-attempt layers.
//! If the caller drops the logical request future, the metric uses `error.type=abandoned`.
//!
//! Histogram boundaries in seconds: `0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0`.
//!
//! ### Attributes
//!
//! Identical to [`http.client.request.duration`](#httpclientrequestduration).
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
