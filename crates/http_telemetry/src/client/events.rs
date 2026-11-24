// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the client side.

use std::ops::ControlFlow;

use observed::metadata::FieldDescriptor;
use observed::processing::FieldVisitorFn;
use observed::{Text, Value, VisitFields, event};
use templated_uri::PathAndQuery;

use crate::shared::DURATION_BOUNDARIES;

/// Produces the event `http.client.request.complete` when the handler returned a response.
#[event("http.client.request.complete")]
#[debug("HTTP request attempt completed")]
#[histogram(
    duration_secs,
    name = "http.client.request.duration",
    desc = "Duration of HTTP client requests.",
    unit = "s",
    boundaries = DURATION_BOUNDARIES
)]
pub(crate) struct RequestComplete<'a> {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: Text,
    #[dimension(log = "network.protocol.name", metric = "network.protocol.name")]
    #[unredacted]
    pub(crate) protocol_name: &'static str,
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
    #[dimension(log = "url.template", metric = "url.template")]
    #[unredacted]
    pub(crate) url_template: Text,
    #[dimension(log = "url.path.redacted")]
    #[if_none(drop)]
    pub(crate) path: Option<&'a PathAndQuery>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    pub(crate) status_code: u16,
    #[flatten]
    pub(crate) dimensions: Dimensions<'a>,
    #[dimension(log = "http.client.request.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
}

/// Produces the event `http.client.request.error` when the handler returned an error or panicked.
#[event("http.client.request.error")]
#[warning("HTTP request attempt failed")]
#[histogram(
    duration_secs,
    name = "http.client.request.duration",
    desc = "Duration of HTTP client requests.",
    unit = "s",
    boundaries = DURATION_BOUNDARIES
)]
pub(crate) struct RequestError<'a> {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: Text,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.template", metric = "url.template")]
    #[unredacted]
    pub(crate) url_template: Text,
    #[dimension(log = "url.path.redacted")]
    #[if_none(drop)]
    pub(crate) path: Option<&'a PathAndQuery>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: Text,
    #[dimension(log = "exception.message")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) exception_message: Option<Text>,
    #[flatten]
    pub(crate) dimensions: Dimensions<'a>,
    #[dimension(log = "http.client.request.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
}

/// Produces the event `http.client.request.abandoned` when the caller dropped the request future before the handler
/// returned.
#[event("http.client.request.abandoned")]
#[warning("HTTP request attempt abandoned")]
#[histogram(
    duration_secs,
    name = "http.client.request.duration",
    desc = "Duration of HTTP client requests.",
    unit = "s",
    boundaries = DURATION_BOUNDARIES
)]
pub(crate) struct RequestAbandoned<'a> {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: Text,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.template", metric = "url.template")]
    #[unredacted]
    pub(crate) url_template: Text,
    #[dimension(log = "url.path.redacted")]
    #[if_none(drop)]
    pub(crate) path: Option<&'a PathAndQuery>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: &'static str,
    #[flatten]
    pub(crate) dimensions: Dimensions<'a>,
    #[dimension(log = "http.client.request.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
}

/// Produces the event `http.client.request.total.complete` when the handler of a layer that reports the total
/// duration returned a response.
#[event("http.client.request.total.complete")]
#[debug("HTTP request completed")]
#[histogram(
    duration_secs,
    name = "http.client.request.total_duration",
    desc = "Total duration of HTTP client requests, including all retries and hedged attempts.",
    unit = "s",
    boundaries = DURATION_BOUNDARIES
)]
pub(crate) struct RequestTotalComplete<'a> {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: Text,
    #[dimension(log = "network.protocol.name", metric = "network.protocol.name")]
    #[unredacted]
    pub(crate) protocol_name: &'static str,
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
    #[dimension(log = "url.template", metric = "url.template")]
    #[unredacted]
    pub(crate) url_template: Text,
    #[dimension(log = "url.path.redacted")]
    #[if_none(drop)]
    pub(crate) path: Option<&'a PathAndQuery>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    pub(crate) status_code: u16,
    #[flatten]
    pub(crate) dimensions: Dimensions<'a>,
    #[dimension(log = "http.client.request.total_duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
}

/// Produces the event `http.client.request.total.error` when the handler of a layer that reports the total duration
/// returned an error or panicked.
#[event("http.client.request.total.error")]
#[warning("HTTP request failed")]
#[histogram(
    duration_secs,
    name = "http.client.request.total_duration",
    desc = "Total duration of HTTP client requests, including all retries and hedged attempts.",
    unit = "s",
    boundaries = DURATION_BOUNDARIES
)]
pub(crate) struct RequestTotalError<'a> {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: Text,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.template", metric = "url.template")]
    #[unredacted]
    pub(crate) url_template: Text,
    #[dimension(log = "url.path.redacted")]
    #[if_none(drop)]
    pub(crate) path: Option<&'a PathAndQuery>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: Text,
    #[dimension(log = "exception.message")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) exception_message: Option<Text>,
    #[flatten]
    pub(crate) dimensions: Dimensions<'a>,
    #[dimension(log = "http.client.request.total_duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
}

/// Produces the event `http.client.request.total.abandoned` when the caller dropped the request future of a layer that
/// reports the total duration before the handler returned.
#[event("http.client.request.total.abandoned")]
#[warning("HTTP request abandoned")]
#[histogram(
    duration_secs,
    name = "http.client.request.total_duration",
    desc = "Total duration of HTTP client requests, including all retries and hedged attempts.",
    unit = "s",
    boundaries = DURATION_BOUNDARIES
)]
pub(crate) struct RequestTotalAbandoned<'a> {
    #[dimension(log = "http.request.method", metric = "http.request.method")]
    #[unredacted]
    pub(crate) method: &'static str,
    #[dimension(log = "http.request.method_original")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) method_original: Option<Text>,
    #[dimension(log = "url.scheme", metric = "url.scheme")]
    #[unredacted]
    pub(crate) scheme: Text,
    #[dimension(log = "server.address", metric = "server.address")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_address: Option<Text>,
    #[dimension(log = "server.port", metric = "server.port")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) server_port: Option<u16>,
    #[dimension(log = "url.template", metric = "url.template")]
    #[unredacted]
    pub(crate) url_template: Text,
    #[dimension(log = "url.path.redacted")]
    #[if_none(drop)]
    pub(crate) path: Option<&'a PathAndQuery>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: &'static str,
    #[flatten]
    pub(crate) dimensions: Dimensions<'a>,
    #[dimension(log = "http.client.request.total_duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
}

/// The fields that the layer adds to each of its events, each one a log field and a metric dimension.
pub(crate) struct Dimensions<'a>(pub(crate) &'a [(FieldDescriptor, Text)]);

impl VisitFields for Dimensions<'_> {
    const CONTAINS_METRICS: bool = false;

    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        for (descriptor, value) in self.0 {
            visitor(descriptor, &|_| Value::from(value.clone()))?;
        }
        ControlFlow::Continue(())
    }
}
