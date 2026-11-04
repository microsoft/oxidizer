// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server side.

use observed::{Text, event};

/// Produces the event `http.server.request.complete` when the handler returned a response and its body
/// ended.
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
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) scheme: Option<&'static str>,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    pub(crate) status_code: u16,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) error_type: Option<&'static str>,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_ms: u64,
}

/// Produces the event `http.server.request.error` when the handler or the response body returned an error or
/// panicked.
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
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) scheme: Option<&'static str>,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) status_code: Option<u16>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: Text,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_ms: u64,
}

/// Produces the event `http.server.request.abandoned` when the caller dropped the request future before the
/// handler returned, or the response body before its end.
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
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) scheme: Option<&'static str>,
    #[dimension(log = "network.protocol.version", metric = "network.protocol.version")]
    #[unredacted]
    pub(crate) protocol_version: &'static str,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) status_code: Option<u16>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: &'static str,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_ms: u64,
}

/// Produces the event `http.server.active_requests`, with `delta` `1` when a request starts and `-1`
/// when it ends.
#[event("http.server.active_requests")]
#[debug("Active HTTP server requests changed")]
#[updown_counter(delta, desc = "Number of active HTTP server requests.", unit = "{request}")]
pub(crate) struct ActiveRequests {
    #[dimension(log = "http.server.active_requests")]
    #[unredacted]
    pub(crate) delta: i64,
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) scheme: Option<&'static str>,
}
