// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server side.

use std::ops::ControlFlow;

use data_privacy::{Redactor, Sensitive};
use observed::processing::FieldVisitorFn;
use observed::{Text, Value, VisitFields, event};
use routerama::RouteHit;

use super::route_fields::RouteParameter;
use crate::header_fields::Headers;
use crate::shared::DURATION_BOUNDARIES;

/// Produces the event `http.server.request.complete` when the handler returned a response and its body
/// ended.
#[event("http.server.request.complete")]
#[info("HTTP server request completed")]
#[histogram(
    duration_secs,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
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
    #[dimension(log = "http.route", metric = "http.route")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) route_template: Option<Text>,
    #[flatten]
    pub(crate) route_parameters: RouteParameters<'a>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    pub(crate) status_code: u16,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) error_type: Option<&'static str>,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
    #[flatten]
    pub(crate) request_headers: Headers<'a>,
    #[flatten]
    pub(crate) response_headers: Headers<'a>,
    #[dimension(log = "http.request.body.content")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) request_body: Option<Text>,
    #[dimension(log = "http.response.body.content")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) response_body: Option<Text>,
}

/// Produces the event `http.server.request.error` when the handler or the response body returned an error, or when
/// the handler, the request body during its read or the response body panicked.
#[event("http.server.request.error")]
#[error("HTTP server request failed")]
#[histogram(
    duration_secs,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
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
    #[dimension(log = "http.route", metric = "http.route")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) route_template: Option<Text>,
    #[flatten]
    pub(crate) route_parameters: RouteParameters<'a>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) status_code: Option<u16>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: Text,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
    #[flatten]
    pub(crate) request_headers: Headers<'a>,
    #[flatten]
    pub(crate) response_headers: Headers<'a>,
    #[dimension(log = "http.request.body.content")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) request_body: Option<Text>,
    #[dimension(log = "http.response.body.content")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) response_body: Option<Text>,
}

/// Produces the event `http.server.request.abandoned` when the caller dropped the request future before the
/// handler returned, or the response body before its end.
#[event("http.server.request.abandoned")]
#[warning("HTTP server request abandoned")]
#[histogram(
    duration_secs,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
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
    #[dimension(log = "http.route", metric = "http.route")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) route_template: Option<Text>,
    #[flatten]
    pub(crate) route_parameters: RouteParameters<'a>,
    #[dimension(log = "http.response.status_code", metric = "http.response.status_code")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) status_code: Option<u16>,
    #[dimension(log = "error.type", metric = "error.type")]
    #[unredacted]
    pub(crate) error_type: &'static str,
    #[dimension(log = "http.server.request.duration")]
    #[unredacted]
    pub(crate) duration_secs: f64,
    #[flatten]
    pub(crate) request_headers: Headers<'a>,
    #[flatten]
    pub(crate) response_headers: Headers<'a>,
    #[dimension(log = "http.request.body.content")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) request_body: Option<Text>,
    #[dimension(log = "http.response.body.content")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) response_body: Option<Text>,
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

pub(crate) struct RouteParameters<'a>(pub(crate) Option<(&'a [RouteParameter], RouteHit<'a, 'a>)>);

impl VisitFields for RouteParameters<'_> {
    const CONTAINS_METRICS: bool = false;

    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        if let Some((parameters, hit)) = &self.0 {
            for parameter in *parameters {
                if let Some(unredacted_value) = hit.parameter(&parameter.name) {
                    let redacted_value = |redactor: &dyn Redactor| {
                        Value::from_redacted(&Sensitive::new(unredacted_value, parameter.data_class.clone()), redactor)
                    };
                    visitor(&parameter.descriptor, &redacted_value)?;
                }
            }
        }
        ControlFlow::Continue(())
    }
}
