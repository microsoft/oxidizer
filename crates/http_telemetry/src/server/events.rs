// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server side.

use std::fmt::{self, Write as _};
use std::ops::ControlFlow;

use data_privacy::{Redactor, Sensitive};
use http::HeaderValue;
use observed::processing::FieldVisitorFn;
use observed::{Text, Value, VisitFields, event};
use routerama::RouteHit;

use super::HeaderValues;
use super::route_fields::Route;

/// Produces the event `http.server.request.complete` when the handler returned a response and its body
/// ended.
#[event("http.server.request.complete")]
#[info("HTTP server request completed")]
#[histogram(
    duration_secs,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
    unit = "s"
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
    #[dimension(log = "http.response.body")]
    #[unredacted]
    #[if_none(drop)]
    pub(crate) response_body: Option<Text>,
}

/// Produces the event `http.server.request.error` when the handler or the response body returned an error or
/// panicked.
#[event("http.server.request.error")]
#[error("HTTP server request failed")]
#[histogram(
    duration_secs,
    name = "http.server.request.duration",
    desc = "Duration of HTTP server requests.",
    unit = "s"
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
    #[dimension(log = "http.response.body")]
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
    unit = "s"
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
    #[dimension(log = "http.response.body")]
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

pub(crate) struct RouteParameters<'a>(pub(crate) Option<(&'a Route, RouteHit<'a>)>);

impl VisitFields for RouteParameters<'_> {
    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        if let Some((route, hit)) = &self.0 {
            for (capture, parameter) in hit.captures().iter().zip(&route.parameters) {
                if let Some(parameter) = parameter {
                    let value =
                        |redactor: &dyn Redactor| Value::from_redacted(&Sensitive::new(*capture, parameter.data_class.clone()), redactor);
                    visitor(&parameter.descriptor, &value)?;
                }
            }
        }
        ControlFlow::Continue(())
    }
}

pub(crate) struct Headers<'a>(pub(crate) Option<&'a HeaderValues>);

impl VisitFields for Headers<'_> {
    fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        if let Some(headers) = self.0 {
            // The values of one header are next to each other, so each chunk holds the values of one header.
            for values in headers.values.chunk_by(|left, right| left.0 == right.0) {
                let header_field = &headers.fields[values[0].0];
                let value = |redactor: &dyn Redactor| {
                    Value::from_redacted(&Sensitive::new(JoinedValues(values), header_field.data_class.clone()), redactor)
                };
                visitor(&header_field.descriptor, &value)?;
            }
        }
        ControlFlow::Continue(())
    }
}

struct JoinedValues<'a>(&'a [(usize, HeaderValue)]);

impl fmt::Display for JoinedValues<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for (_, value) in self.0 {
            f.write_str(separator)?;
            for chunk in value.as_bytes().utf8_chunks() {
                f.write_str(chunk.valid())?;
                if !chunk.invalid().is_empty() {
                    f.write_char(char::REPLACEMENT_CHARACTER)?;
                }
            }
            separator = ", ";
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RejectNonEmptyWrites;

    impl fmt::Write for RejectNonEmptyWrites {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            if s.is_empty() { Ok(()) } else { Err(fmt::Error) }
        }
    }

    #[test]
    fn joined_values_return_writer_errors() {
        let written = |values: &[&'static [u8]]| {
            let values: Vec<_> = values.iter().map(|value| (0, HeaderValue::from_bytes(value).unwrap())).collect();
            write!(RejectNonEmptyWrites, "{}", JoinedValues(&values))
        };

        assert_eq!([written(&[b"a"]), written(&[b"", b"a"]), written(&[b"\xff"])], [Err(fmt::Error); 3]);
    }
}
