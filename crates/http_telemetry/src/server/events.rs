// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server side.

use observed::{Text, event};

/// Produces the event `http.server.request.complete` when the handler returned a response.
#[event("http.server.request.complete")]
#[info("HTTP server request completed")]
#[histogram(
    duration_ms,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
    unit = "ms"
)]
pub(crate) struct RequestComplete {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    pub(crate) status_code: i64,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) error_type: Option<&'static str>,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_ms: u64,
}

/// Produces the event `http.server.request.error` when the handler returned an error or panicked.
#[event("http.server.request.error")]
#[error("HTTP server request failed")]
#[histogram(
    duration_ms,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
    unit = "ms"
)]
pub(crate) struct RequestError {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: Text,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_ms: u64,
}

/// Produces the event `http.server.request.abandoned` when the caller dropped the request future before the
/// handler returned.
#[event("http.server.request.abandoned")]
#[warning("HTTP server request abandoned")]
#[histogram(
    duration_ms,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
    unit = "ms"
)]
pub(crate) struct RequestAbandoned {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: &'static str,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_ms: u64,
}
