// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types for the request telemetry and the connection diagnostics of `fetch`.
//!
//! `fetch` emits its request telemetry as [`http_telemetry`] client events through the sink of its
//! deps. [`Telemetry`][crate::telemetry::Telemetry], [`TelemetryLayer`][crate::telemetry::TelemetryLayer]
//! and [`Client`][crate::telemetry::Client] build the layer of those events. A custom
//! pipeline gets the layer of its client from
//! [`PipelineContext::client_telemetry`](crate::pipeline::PipelineContext::client_telemetry).
//! [`ConnectionInfo`][crate::telemetry::ConnectionInfo] describes the connection that served a
//! response.
//!
//! For the full list of emitted metrics and events, see the
//! [telemetry reference](crate::documentation::telemetry).

/// Diagnostic information about the connection that served an HTTP response.
///
/// Re-exported from [`fetch_options`]. The bundled Hyper transport adds it as a response extension.
/// A custom transport must add it if it supports this diagnostic.
/// [`FakeHandler`](crate::fake::FakeHandler) does not add it. Read it through
/// `response.extensions().get::<ConnectionInfo>()`.
pub use fetch_options::ConnectionInfo;
pub use http_telemetry::{Client, Telemetry, TelemetryLayer};

/// Key of the runtime of a client in each request event.
pub(crate) const FETCH_RUNTIME_ATTRIBUTE: &str = "fetch.runtime";

/// Key of the transport of a client in each request event.
pub(crate) const FETCH_TRANSPORT_ATTRIBUTE: &str = "fetch.transport";

/// Key of the name of a client in each request event.
pub(crate) const HTTP_CLIENT_NAME_ATTRIBUTE: &str = "http.client.name";
