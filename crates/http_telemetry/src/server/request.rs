// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The values of one server request, and the events that end it.

use std::sync::Arc;

use http::uri::{Authority, PathAndQuery};
use http::{HeaderValue, Method, StatusCode, Version};
use http_extensions::HttpError;
use http_headers::SingleValueField;
use http_headers::headers::{Host, HostView};
use observed::{Sink, Text, emit};
use parking_lot::Mutex;
use routerama::RouteTemplate;

use super::body_buffer::BodyBuffer;
use super::events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError, RouteParameters};
use super::route_fields::RouteFields;
use crate::drop_guard::EmitEvents;
use crate::header_fields::{Headers, RequestAndResponseHeaders};
use crate::shared::{collect_error_labels, http_method_name, method_original, protocol_version};

/// The texts of the statuses 500 to 599, three digits each. `StatusCode::as_str` returns a text that
/// borrows the status, so the `error.type` of a complete event comes from here.
const SERVER_ERROR_STATUSES: &str = concat!(
    "500501502503504505506507508509",
    "510511512513514515516517518519",
    "520521522523524525526527528529",
    "530531532533534535536537538539",
    "540541542543544545546547548549",
    "550551552553554555556557558559",
    "560561562563564565566567568569",
    "570571572573574575576577578579",
    "580581582583584585586587588589",
    "590591592593594595596597598599",
);

pub(crate) enum RequestEnd<'a> {
    /// The handler returned a response with this status, and its body ended.
    Complete(StatusCode),
    /// The handler or the response body returned this error.
    Error(&'a HttpError, Option<StatusCode>),
    /// The handler or the response body panicked.
    Panic(Option<StatusCode>),
    /// The caller dropped the request future before the handler returned, or the response body
    /// before its end.
    Abandoned(Option<StatusCode>),
}

pub(crate) enum UriAuthority {
    /// The authority of the request URI, and the default port of its scheme.
    Uri(Authority, Option<u16>),
    /// The `Host` header of a request whose URI has no authority.
    Host(HeaderValue),
}

pub(crate) struct RouteValues {
    pub(crate) route_fields: Arc<RouteFields>,
    pub(crate) path_and_query: PathAndQuery,
}

/// The values of one server request that its events record.
///
/// When the middleware wraps the response body, the drop guard of the request moves into the wrapper with these
/// values, so they own their data.
pub(crate) struct ServerRequest {
    pub(crate) sink: Sink,
    pub(crate) method: Method,
    pub(crate) version: Version,
    pub(crate) scheme: Option<&'static str>,
    pub(crate) uri_authority: Option<UriAuthority>,
    pub(crate) route_values: Option<RouteValues>,
    pub(crate) headers: Option<Box<RequestAndResponseHeaders>>,
    pub(crate) request_body_buffer: Option<Arc<Mutex<BodyBuffer>>>,
    pub(crate) response_body_buffer: Option<BodyBuffer>,
}

impl ServerRequest {
    pub(crate) fn add_active_request(&self, delta: i64) {
        emit!(
            self.sink,
            ActiveRequests {
                delta,
                method: http_method_name(&self.method),
                scheme: self.scheme,
            }
        );
    }
}

impl EmitEvents for ServerRequest {
    type RequestEnd<'a> = RequestEnd<'a>;

    const ABANDONED: RequestEnd<'static> = RequestEnd::Abandoned(None);

    #[expect(
        clippy::too_many_lines,
        reason = "a request can end in four ways, and the code for each way lists all fields of its event"
    )]
    fn emit_events(&self, end: RequestEnd<'_>, duration_secs: f64) {
        let method = http_method_name(&self.method);
        let protocol_version = protocol_version(self.version);
        let (server_address, server_port) = match &self.uri_authority {
            Some(UriAuthority::Uri(authority, default_port)) => (Some(authority.host()), authority.port_u16().or(*default_port)),
            Some(UriAuthority::Host(value)) => {
                let host = <Host as SingleValueField>::decode_view(value.into()).ok();
                (
                    host.as_ref().map(HostView::host),
                    host.and_then(|host| host.network_port().ok().flatten()),
                )
            }
            None => (None, None),
        };
        let server_address = || {
            server_address
                .filter(|host| !host.is_empty())
                .map(|host| Text::from(Arc::<str>::from(host)))
        };
        let headers = self.headers.as_deref();
        let match_route = || {
            let route = self.route_values.as_ref().and_then(
                |RouteValues {
                     route_fields,
                     path_and_query,
                 }| {
                    let route_hit = route_fields
                        .route_resolver
                        .match_route(self.method.as_str(), path_and_query.path())?;
                    Some((&*route_fields.parameters, route_hit))
                },
            );
            (
                route.as_ref().map(|(_, route_hit)| match route_hit.template() {
                    RouteTemplate::Static(template) => Text::from(*template),
                    RouteTemplate::Shared(template) => Text::from(Arc::clone(template)),
                }),
                RouteParameters(route),
            )
        };
        match end {
            RequestEnd::Complete(status_code) => {
                let error_type = status_code.as_u16().checked_sub(500).and_then(|index| {
                    let start = usize::from(index) * 3;
                    SERVER_ERROR_STATUSES.get(start..start + 3)
                });
                emit!(self.sink, {
                    let (route_template, route_parameters) = match_route();
                    RequestComplete {
                        method,
                        method_original: method_original(&self.method),
                        scheme: self.scheme,
                        protocol_version,
                        server_address: server_address(),
                        server_port,
                        route_template,
                        route_parameters,
                        status_code: status_code.as_u16(),
                        error_type,
                        duration_secs,
                        request_headers: Headers(headers.and_then(|headers| headers.request.as_ref())),
                        response_headers: Headers(headers.and_then(|headers| headers.response.as_ref())),
                        request_body: self.request_body_buffer.as_ref().map(|buffer| buffer.lock().text()),
                        response_body: self.response_body_buffer.as_ref().map(BodyBuffer::text),
                    }
                });
            }
            RequestEnd::Error(error, status_code) => emit!(self.sink, {
                let (route_template, route_parameters) = match_route();
                RequestError {
                    method,
                    method_original: method_original(&self.method),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    route_template,
                    route_parameters,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: Text::from(collect_error_labels(error).into_cow()),
                    duration_secs,
                    request_headers: Headers(headers.and_then(|headers| headers.request.as_ref())),
                    response_headers: Headers(headers.and_then(|headers| headers.response.as_ref())),
                    request_body: self.request_body_buffer.as_ref().map(|buffer| buffer.lock().text()),
                    response_body: self.response_body_buffer.as_ref().map(BodyBuffer::text),
                }
            }),
            RequestEnd::Panic(status_code) => emit!(self.sink, {
                let (route_template, route_parameters) = match_route();
                RequestError {
                    method,
                    method_original: method_original(&self.method),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    route_template,
                    route_parameters,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: Text::from("panic"),
                    duration_secs,
                    request_headers: Headers(headers.and_then(|headers| headers.request.as_ref())),
                    response_headers: Headers(headers.and_then(|headers| headers.response.as_ref())),
                    request_body: self.request_body_buffer.as_ref().map(|buffer| buffer.lock().text()),
                    response_body: self.response_body_buffer.as_ref().map(BodyBuffer::text),
                }
            }),
            RequestEnd::Abandoned(status_code) => emit!(self.sink, {
                let (route_template, route_parameters) = match_route();
                RequestAbandoned {
                    method,
                    method_original: method_original(&self.method),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    route_template,
                    route_parameters,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: "abandoned",
                    duration_secs,
                    request_headers: Headers(headers.and_then(|headers| headers.request.as_ref())),
                    response_headers: Headers(headers.and_then(|headers| headers.response.as_ref())),
                    request_body: self.request_body_buffer.as_ref().map(|buffer| buffer.lock().text()),
                    response_body: self.response_body_buffer.as_ref().map(BodyBuffer::text),
                }
            }),
        }
        self.add_active_request(-1);
    }
}
