// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The drop guard that emits the event of one request.

use std::net::Ipv6Addr;
use std::sync::Arc;

use http::uri::Authority;
use http::{HeaderValue, Method, StatusCode, Version};
use http_extensions::HttpError;
use observed::context::Transfer;
use observed::{Sink, Text, emit};
use ohno::{ErrorLabel, Labeled};
use tick::Stopwatch;

use super::OTHER;
use super::events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};

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

/// The authority of the target URI
pub(crate) enum UriAuthority {
    /// The authority of the request URI, and the default port of its scheme.
    Uri(Authority, Option<u16>),
    /// The `Host` header of a request whose URI has no authority.
    Host(HeaderValue),
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
            Some(UriAuthority::Host(value)) => decode_host(value).map_or((None, None), |(host, port)| (Some(host), port)),
            None => (None, None),
        };
        // `emit!` builds an event only for a sink that is interested in it, so the method and the host are copied
        // only then.
        let method_original = || (method == OTHER).then(|| Text::from(Arc::<str>::from(self.method.as_str())));
        let server_address = || {
            server_address
                .filter(|host| !host.is_empty())
                .map(|host| Text::from(Arc::<str>::from(host)))
        };
        match end {
            RequestEnd::Complete(status_code) => {
                let error_type = status_code.as_u16().checked_sub(500).and_then(|index| {
                    let start = usize::from(index) * 3;
                    SERVER_ERROR_STATUSES.get(start..start + 3)
                });
                emit!(
                    self.sink,
                    RequestComplete {
                        method,
                        method_original: method_original(),
                        scheme: self.scheme,
                        protocol_version,
                        server_address: server_address(),
                        server_port,
                        status_code: status_code.as_u16(),
                        error_type,
                        duration_secs,
                    }
                );
            }
            RequestEnd::Error(error, status_code) => emit!(
                self.sink,
                RequestError {
                    method,
                    method_original: method_original(),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: Text::from(ErrorLabel::from_error_chain(error, resolve_error_label).into_cow()),
                    duration_secs,
                }
            ),
            RequestEnd::Panic(status_code) => emit!(
                self.sink,
                RequestError {
                    method,
                    method_original: method_original(),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: Text::from("panic"),
                    duration_secs,
                }
            ),
            RequestEnd::Abandoned(status_code) => emit!(
                self.sink,
                RequestAbandoned {
                    method,
                    method_original: method_original(),
                    scheme: self.scheme,
                    protocol_version,
                    server_address: server_address(),
                    server_port,
                    status_code: status_code.map(|status_code| status_code.as_u16()),
                    error_type: "abandoned",
                    duration_secs,
                }
            ),
        }
        self.add_active_request(-1);
    }
}

/// Reads the strict Host grammar without normalizing the address (RFC 9110, section 7.2).
fn decode_host(value: &HeaderValue) -> Option<(&str, Option<u16>)> {
    let value = value.to_str().ok()?;
    let (host, port) = if let Some(literal) = value.strip_prefix('[') {
        let (literal, suffix) = literal.split_once(']')?;
        if !valid_ip_literal(literal) {
            return None;
        }
        let host = &value[..literal.len() + 2];
        let port = if suffix.is_empty() { None } else { Some(suffix.strip_prefix(':')?) };
        (host, port)
    } else {
        let (host, port) = value.split_once(':').map_or((value, None), |(host, port)| (host, Some(port)));
        if (host.is_empty() && port.is_some()) || !valid_registered_name(host.as_bytes()) {
            return None;
        }
        (host, port)
    };
    if let Some(port) = port
        && !port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    // An empty or overflowing decimal port leaves the address usable.
    Some((host, port.and_then(|port| port.parse().ok())))
}

fn valid_registered_name(mut bytes: &[u8]) -> bool {
    while let Some((&byte, rest)) = bytes.split_first() {
        if host_name_byte(byte) {
            bytes = rest;
        } else if byte == b'%' && rest.first().is_some_and(u8::is_ascii_hexdigit) && rest.get(1).is_some_and(u8::is_ascii_hexdigit) {
            bytes = &rest[2..];
        } else {
            return false;
        }
    }
    true
}

fn valid_ip_literal(literal: &str) -> bool {
    let Some(versioned) = literal.strip_prefix('v').or_else(|| literal.strip_prefix('V')) else {
        return literal.parse::<Ipv6Addr>().is_ok();
    };
    let Some((version, address)) = versioned.split_once('.') else {
        return false;
    };
    !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !address.is_empty()
        && address.bytes().all(|byte| host_name_byte(byte) || byte == b':')
}

/// Unreserved and sub-delimiter bytes (RFC 3986, sections 2.2 and 2.3).
const fn host_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
        )
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
