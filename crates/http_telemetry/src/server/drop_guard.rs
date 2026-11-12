// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The drop guard that emits the event of one request.

use std::sync::Arc;

use http::uri::{Authority, PathAndQuery};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Version};
use http_extensions::HttpError;
use http_headers::SingleValueField;
use http_headers::headers::{Host, HostView};
use observed::context::Transfer;
use observed::{Sink, Text, emit};
use ohno::{ErrorLabel, Labeled};
use smallvec::SmallVec;
use tick::Stopwatch;

use super::events::{ActiveRequests, Headers, RequestAbandoned, RequestComplete, RequestError, RouteParameters};
use super::{HeaderField, OTHER, RouteFields};

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

#[derive(Clone, Copy)]
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

/// The values of the configured headers of the request and of the response.
pub(crate) struct RequestAndResponseHeaders {
    pub(crate) request: Option<HeaderValues>,
    pub(crate) response: Option<HeaderValues>,
}

pub(crate) struct HeaderValues {
    pub(crate) fields: Arc<[HeaderField]>,
    pub(crate) values: SmallVec<[(usize, HeaderValue); 2]>,
}

impl HeaderValues {
    pub(crate) fn new(fields: Arc<[HeaderField]>, headers: &HeaderMap) -> Self {
        let values = fields
            .iter()
            .enumerate()
            .flat_map(|(index, field)| headers.get_all(&field.header_name).iter().map(move |value| (index, value.clone())))
            .collect();
        Self { fields, values }
    }
}

/// Drop guard that emits the event of one request, also when the caller drops the request future.
///
/// The service creates the guard in `execute`, so the duration starts at the call. When the
/// middleware wraps the response body, the guard moves into the wrapper. [`emit`] emits the event
/// of the request, once. If the caller drops the request future before that, the guard emits
/// `http.server.request.abandoned`.
///
/// [`emit`]: Self::emit
pub(crate) struct TelemetryDropGuard {
    pub(crate) sink: Sink,
    pub(crate) stopwatch: Stopwatch,
    pub(crate) enrichment: Transfer,
    pub(crate) method: Method,
    pub(crate) version: Version,
    pub(crate) scheme: Option<&'static str>,
    pub(crate) uri_authority: Option<UriAuthority>,
    pub(crate) route_values: Option<RouteValues>,
    pub(crate) headers: Option<Box<RequestAndResponseHeaders>>,
    pub(crate) finished: bool,
}

impl TelemetryDropGuard {
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

    #[expect(
        clippy::too_many_lines,
        reason = "a request can end in four ways, and the code for each way lists all fields of its event"
    )]
    pub(crate) fn emit(&mut self, end: RequestEnd<'_>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let duration_secs = self.stopwatch.elapsed().as_secs_f64();
        let _enrichment = self.enrichment.apply_current_thread();
        let method = http_method_name(&self.method);
        let protocol_version = match self.version {
            Version::HTTP_10 => "1.0",
            Version::HTTP_11 => "1.1",
            Version::HTTP_2 => "2",
            Version::HTTP_3 => "3",
            _ => OTHER,
        };
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
        let method_original = || (method == OTHER).then(|| Text::from(Arc::<str>::from(self.method.as_str())));
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
                    let route_hit = route_fields.route_table.match_route(self.method.as_str(), path_and_query.path())?;
                    Some((route_fields.routes.get(route_hit.route())?.as_ref()?, route_hit))
                },
            );
            (route.as_ref().map(|(route, _)| route.template.clone()), RouteParameters(route))
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
                        method_original: method_original(),
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
                    }
                });
            }
            RequestEnd::Error(error, status_code) => emit!(self.sink, {
                let (route_template, route_parameters) = match_route();
                RequestError {
                    method,
                    method_original: method_original(),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    route_template,
                    route_parameters,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: Text::from(ErrorLabel::from_error_chain(error, resolve_error_label).into_cow()),
                    duration_secs,
                    request_headers: Headers(headers.and_then(|headers| headers.request.as_ref())),
                    response_headers: Headers(headers.and_then(|headers| headers.response.as_ref())),
                }
            }),
            RequestEnd::Panic(status_code) => emit!(self.sink, {
                let (route_template, route_parameters) = match_route();
                RequestError {
                    method,
                    method_original: method_original(),
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
                }
            }),
            RequestEnd::Abandoned(status_code) => emit!(self.sink, {
                let (route_template, route_parameters) = match_route();
                RequestAbandoned {
                    method,
                    method_original: method_original(),
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
                }
            }),
        }
        self.add_active_request(-1);
    }
}

/// The label of one error of a source chain: the label of an [`HttpError`], the kind of an I/O
/// error, or none for any other error.
fn resolve_error_label(error: &(dyn std::error::Error + 'static)) -> Option<ErrorLabel> {
    if let Some(error) = error.downcast_ref::<HttpError>() {
        return Some(error.label().clone());
    }
    error.downcast_ref::<std::io::Error>().map(resolve_io_error_label)
}

/// The kind of the I/O error that `error` wraps, if it wraps one, else the kind of `error`.
fn resolve_io_error_label(error: &std::io::Error) -> ErrorLabel {
    match error.get_ref().and_then(|inner| inner.downcast_ref::<std::io::Error>()) {
        Some(inner) => resolve_io_error_label(inner),
        None => error.kind().into(),
    }
}

/// The value of `http.request.method` for `method`.
const fn http_method_name(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => OTHER,
    }
}

impl Drop for TelemetryDropGuard {
    fn drop(&mut self) {
        // Emit nothing while the thread panics, because a panic in a processor now aborts the process.
        if self.finished || std::thread::panicking() {
            return;
        }

        self.emit(RequestEnd::Abandoned(None));
    }
}
