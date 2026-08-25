// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Drives one HTTP request through the sequential `WinHTTP` lifecycle.
//!
//! This module owns the end-to-end request state machine described in
//! implementation.md section 4.4 and section 6.3: translating request metadata
//! before any handle exists, opening the connect and request handles, applying
//! the native request options, racing header submission against the generic
//! connect timeout (implementation.md section 4.6), uploading the body, and
//! receiving response metadata.
//!
//! The mechanics it builds on live elsewhere: [`crate::operation`] owns the
//! callback-ownership handoff and the per-request operation slot,
//! [`crate::response_headers`] parses the raw header block, and
//! [`crate::error`] binds each failure category to its label and recovery
//! classification.

use std::future::poll_fn;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use bytesbuf::mem::GlobalPool;
use fetch::options::{RequestFilter, TransportOptions};
use fetch::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, HttpResponseBuilder};
use http::uri::Authority;
use http::{HeaderMap, StatusCode, Version};
use http_extensions::HttpBodyOptions;
use http_extensions::timeout::BodyTimeout;
use tick::Clock;
use widestring::U16CString;

use crate::bindings::{
    Bindings as _, BindingsFacade, WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
    WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
    WINHTTP_OPTION_SECURITY_FLAGS,
};
use crate::body::{RequestBodyFraming, WinHttpBodyReader, WinHttpBodyWriter, WinHttpResponseBody, send_body};
use crate::context::{ColdConnectState, CompletionResult, OperationBuffer, OperationKind};
use crate::convert::{
    decompression_mask, disable_feature_mask, dword_bytes, headers_to_utf16, host_to_utf16, method_to_utf16, path_to_utf16,
    protocol_options, request_open_flags,
};
use crate::error::{Result as WinHttpResult, callback_protocol_error, invalid_request, invalid_response, query_error};
use crate::handle::{ConnectHandle, RawHandle, RequestHandle};
use crate::operation::{ContextInstallation, ContextPool, RequestGuard};
use crate::options::ProtocolOptions;
use crate::query::{query_protocol_used, query_raw_headers, query_status_code};
use crate::response_headers::parse_response_headers;
use crate::session::WinHttpSession;
use crate::tls::{WinHttpTlsConfig, security_flags};

#[derive(Debug)]
/// Carries a request error and its log-only connection attribution.
///
/// Cold-connect duration is available only after context installation and only
/// when connection-establishment callbacks identify the request as using a new
/// connection. Keeping this metadata outside [`HttpError`] prevents diagnostic
/// timing from affecting recovery classification or low-cardinality metrics.
pub(crate) struct RequestFailure {
    error: HttpError,
    cold_connect_duration: Option<Duration>,
}

impl RequestFailure {
    fn without_attribution(error: HttpError) -> Self {
        Self {
            error,
            cold_connect_duration: None,
        }
    }

    fn after_context_installation(error: HttpError, state: ColdConnectState, duration: Duration) -> Self {
        Self {
            error,
            cold_connect_duration: match state {
                ColdConnectState::Unobserved => None,
                ColdConnectState::Connecting | ColdConnectState::Connected => Some(duration),
            },
        }
    }

    pub(crate) const fn cold_connect_duration(&self) -> Option<Duration> {
        self.cold_connect_duration
    }

    pub(crate) fn into_error(self) -> HttpError {
        self.error
    }
}

/// Drives one request through the complete sequential WinHTTP lifecycle.
///
/// The driver translates request metadata before crossing the FFI boundary,
/// opens connect and request handles, installs callback ownership, races header
/// submission against the generic connect timeout, uploads the body, receives
/// response headers, and hands the guard to a lazy body reader. Upload and
/// response reception are deliberately sequential; no request has simultaneous
/// send and receive operations.
///
/// The borrowed request body remains with the caller until response creation,
/// which preserves the custom-transport contract for reporting whether a
/// failed request consumed body data.
pub(crate) struct RequestDriver<'body, 'contexts> {
    session: Arc<WinHttpSession>,
    body_builder: HttpBodyBuilder,
    clock: Clock,
    connect_timeout: Duration,
    global_pool: GlobalPool,
    contexts: &'contexts ContextPool,
    request: TranslatedRequest,
    settings: RequestSettings,
    body: &'body mut HttpBody,
    body_framing: RequestBodyFraming,
    body_options: HttpBodyOptions,
}

impl<'body, 'contexts> RequestDriver<'body, 'contexts> {
    #[expect(
        clippy::too_many_arguments,
        reason = "translation requires the transport-owned session, clock, pools, options, and TLS policy"
    )]
    pub(crate) fn new(
        request: &'body mut HttpRequest,
        session: Arc<WinHttpSession>,
        body_builder: HttpBodyBuilder,
        clock: &Clock,
        global_pool: GlobalPool,
        contexts: &'contexts ContextPool,
        options: &TransportOptions,
        tls: &WinHttpTlsConfig,
    ) -> fetch::Result<Self> {
        if matches!(request.version(), Version::HTTP_09 | Version::HTTP_10) {
            return Err(invalid_request(RequestTranslationError::from(UnsendableRequestVersionError::new(
                request.version(),
            ))));
        }

        let mut headers = request.headers().clone();
        let body_framing = RequestBodyFraming::new(&mut headers, request.body().content_length()).map_err(invalid_request)?;
        let translated = TranslatedRequest::new(request, &headers, &options.request_filter)?;
        let protocol = protocol_options(&options.supported_http_versions).map_err(invalid_request)?;
        let settings = RequestSettings {
            protocol,
            security_flags: if translated.secure { security_flags(tls) } else { 0 },
        };
        let body_options = request
            .extensions()
            .get::<BodyTimeout>()
            .map(|timeout| HttpBodyOptions::default().timeout(timeout.duration()))
            .unwrap_or_default();
        let body = request.body_mut();

        Ok(Self {
            session,
            body_builder,
            clock: clock.clone(),
            connect_timeout: options.connect_timeout,
            global_pool,
            contexts,
            request: translated,
            settings,
            body,
            body_framing,
            body_options,
        })
    }

    pub(crate) async fn execute(self, body_polled: &mut bool) -> Result<HttpResponse, RequestFailure> {
        let bindings = self.session.handle().bindings().clone();
        // SAFETY: the Arc-owned session is live and outlives the returned
        // connect handle, which is immediately placed under RAII ownership.
        let connect = unsafe { bindings.connect(self.session.handle().raw(), &self.request.host, self.request.port) }
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;
        let connect = ConnectHandle::new(connect, bindings.clone());
        // SAFETY: connect is live and later moves into RequestContext to
        // outlive the returned request, which immediately acquires one owner.
        let request = unsafe {
            bindings.open_request(
                connect.raw(),
                &self.request.method,
                &self.request.path,
                request_open_flags(self.request.secure, self.body_framing.automatic_chunking()),
            )
        }
        .map_err(crate::error::WinHttpError::into_http_error)
        .map_err(RequestFailure::without_attribution)?;
        let request = RequestHandle::new(request, bindings.clone());

        apply_request_settings(&request, self.settings)
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;

        let mut guard = ContextInstallation::new(request, connect, Arc::clone(&self.session), self.contexts)
            .install()
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;
        let connect_watch = self.clock.stopwatch();

        let send_result = send_request_headers(
            &mut guard,
            &bindings,
            &self.request.headers,
            self.body_framing.total_length(),
            &self.clock,
            self.connect_timeout,
        )
        .await;
        if let Err((error, cold_connect_state)) = send_result {
            return Err(RequestFailure::after_context_installation(
                error,
                cold_connect_state,
                connect_watch.elapsed(),
            ));
        }
        let cold_connect_state = guard.cold_connect_state();
        let connect_duration = connect_watch.elapsed();

        let response_metadata = async {
            {
                let mut writer = WinHttpBodyWriter::new(&mut guard, bindings.clone(), self.global_pool.clone());
                send_body(self.body, &mut writer, body_polled).await?;
                if self.body_framing.automatic_chunking() {
                    writer.end_automatic_chunking().await?;
                }
            }

            {
                let receive = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |request, _context| {
                    // SAFETY: the request body is fully submitted, submit()
                    // armed the headers operation and transferred the live
                    // request handle into its future, and no operation overlaps.
                    unsafe { bindings.receive_response(request) }
                });
                let completion = receive
                    .await
                    .map_err(|_disconnected| callback_protocol_error("the headers completion channel disconnected"))?;
                expect_completion(completion, OperationKind::HeadersAvailable)?;
            }

            let status = query_status_code(&bindings, guard.raw()).map_err(query_error)?;
            let status = u16::try_from(status)
                .ok()
                .and_then(|status| StatusCode::from_u16(status).ok())
                .ok_or_else(|| invalid_response(format!("WinHTTP returned an invalid HTTP status code: {status}")))?;
            let raw_headers = query_raw_headers(&bindings, guard.raw()).map_err(query_error)?;
            let headers = parse_response_headers(&raw_headers).map_err(invalid_response)?;
            let version = query_protocol_used(&bindings, guard.raw()).map_err(query_error)?;

            Ok::<_, HttpError>((status, headers, version))
        }
        .await;

        let (status, headers, version) = match response_metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(RequestFailure::after_context_installation(
                    error,
                    cold_connect_state,
                    connect_duration,
                ));
            }
        };

        let reader = WinHttpBodyReader::new(guard, bindings, self.global_pool);
        let body = self.body_builder.body(WinHttpResponseBody::new(reader), &self.body_options);
        let mut response = HttpResponseBuilder::new(&self.body_builder)
            .status(status)
            .version(version)
            .body(body);
        *response
            .headers_mut()
            .ok_or_else(|| invalid_response("the HTTP response builder rejected response metadata"))
            .map_err(|error| RequestFailure::after_context_installation(error, cold_connect_state, connect_duration))? = headers;
        response
            .build()
            .map_err(invalid_response)
            .map_err(|error| RequestFailure::after_context_installation(error, cold_connect_state, connect_duration))
    }
}

async fn send_request_headers(
    guard: &mut RequestGuard,
    bindings: &BindingsFacade,
    headers: &U16CString,
    total_length: u32,
    clock: &Clock,
    timeout: Duration,
) -> Result<(), (HttpError, ColdConnectState)> {
    let outcome = {
        let send = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |request, context| {
            // SAFETY: submit() armed the send operation and transferred the
            // live request handle into its future, so no operation overlaps.
            // The installed context is the exact dwContext pointer and remains
            // alive through HANDLE_CLOSING; headers remains alive until this
            // completion is observed.
            unsafe { bindings.send_request(request, headers, total_length, context) }
        });
        let mut send = std::pin::pin!(send);
        let mut deadline = std::pin::pin!(clock.delay(timeout));
        let mut deadline_registered = false;

        poll_fn(|cx| {
            if !deadline_registered {
                let _ = deadline.as_mut().poll(cx);
                deadline_registered = true;
            }

            match send.as_mut().poll(cx) {
                Poll::Ready(result) => Poll::Ready(Ok(result)),
                Poll::Pending if deadline.as_mut().poll(cx).is_ready() => Poll::Ready(Err(send.as_ref().get_ref().cold_connect_state())),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    };

    match outcome {
        Ok(completion) => {
            let cold_connect_state = guard.cold_connect_state();
            completion
                .map_err(|_disconnected| callback_protocol_error("the send completion channel disconnected"))
                .and_then(|completion| expect_completion(completion, OperationKind::SendRequest))
                .map_err(|error| (error, cold_connect_state))
        }
        Err(cold_connect_state) => Err((HttpError::timeout(timeout), cold_connect_state)),
    }
}

/// Materializes validated request metadata for the WinHTTP FFI calls.
///
/// Translation happens before opening handles so malformed schemes,
/// authorities, ports, paths, methods, or headers fail without network I/O.
/// The owned NUL-terminated UTF-16 values keep every pointer stable for the
/// duration of its synchronous WinHTTP call while preserving repeated and
/// opaque header values accepted by `http`.
struct TranslatedRequest {
    method: U16CString,
    host: U16CString,
    path: U16CString,
    headers: U16CString,
    port: u16,
    secure: bool,
}

impl TranslatedRequest {
    fn new(request: &HttpRequest, headers: &HeaderMap, filter: &RequestFilter) -> fetch::Result<Self> {
        let uri = request.uri();
        let scheme = uri
            .scheme_str()
            .ok_or_else(|| invalid_request(RequestTranslationError::from(MissingSchemeError::new())))?;
        let secure = match scheme {
            "https" => true,
            "http" if matches!(filter, RequestFilter::HttpAndHttps) => false,
            "http" => {
                return Err(invalid_request(RequestTranslationError::from(PlainHttpDisallowedError::new())));
            }
            scheme => {
                return Err(invalid_request(RequestTranslationError::from(UnsupportedSchemeError::new(
                    scheme.to_owned(),
                ))));
            }
        };
        let authority = uri
            .authority()
            .ok_or_else(|| invalid_request(RequestTranslationError::from(MissingAuthorityError::new())))?;
        if authority.as_str().contains('@') {
            return Err(invalid_request(RequestTranslationError::from(UserInfoInAuthorityError::new())));
        }
        let host = uri
            .host()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| invalid_request(RequestTranslationError::from(MissingHostError::new())))?;
        let port = authority_port(authority, host, if secure { 443 } else { 80 }).map_err(invalid_request)?;
        let path = uri.path_and_query().map_or("/", |path| path.as_str());
        let path = if path.is_empty() { "/" } else { path };
        if !(path.starts_with('/') || path.starts_with('?')) {
            return Err(invalid_request(RequestTranslationError::from(InvalidPathError::new(
                path.to_owned(),
            ))));
        }

        Ok(Self {
            method: method_to_utf16(request.method()).map_err(invalid_request)?,
            host: host_to_utf16(host).map_err(invalid_request)?,
            path: path_to_utf16(path).map_err(invalid_request)?,
            headers: headers_to_utf16(headers).map_err(invalid_request)?,
            port,
            secure,
        })
    }
}

fn authority_port(authority: &Authority, host: &str, default: u16) -> Result<u16, RequestTranslationError> {
    let suffix = authority
        .as_str()
        .strip_prefix(host)
        .ok_or_else(|| InvalidAuthorityError::new(authority.as_str().to_owned()))?;
    if suffix.is_empty() {
        return Ok(default);
    }

    let explicit = suffix
        .strip_prefix(':')
        .ok_or_else(|| InvalidAuthorityError::new(authority.as_str().to_owned()))?;
    if explicit.is_empty() {
        return Err(EmptyPortError::new().into());
    }
    if !explicit.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(NonNumericPortError::new(explicit.to_owned()).into());
    }

    let port = explicit
        .parse::<u16>()
        .map_err(|_out_of_range| OutOfRangePortError::new(explicit.to_owned()))?;
    if port == 0 {
        return Err(ZeroPortError::new().into());
    }

    Ok(port)
}

#[derive(Clone, Copy)]
/// Collects native options applied to every newly opened request handle.
///
/// Protocol requirements and transport-specific TLS relaxations are computed
/// once during translation, then applied before the context is installed or an
/// asynchronous operation can begin.
///
/// This is a plain value bag of native option values and nothing more; it is
/// distinct from [`ContextInstallation`], which performs the one-way
/// callback-ownership handoff at a later stage of the same request.
struct RequestSettings {
    protocol: ProtocolOptions,
    security_flags: u32,
}

fn apply_request_settings(request: &RequestHandle, settings: RequestSettings) -> WinHttpResult<()> {
    let bindings = request.bindings();
    let raw = request.raw();

    if settings.protocol.advanced_mask() != 0 {
        set_dword(
            bindings,
            raw,
            WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
            settings.protocol.advanced_mask(),
        )?;
    }
    if settings.protocol.required() {
        set_dword(bindings, raw, WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, 1)?;
    }
    if settings.security_flags != 0 {
        set_dword(bindings, raw, WINHTTP_OPTION_SECURITY_FLAGS, settings.security_flags)?;
    }
    set_dword(bindings, raw, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER)?;
    set_dword(bindings, raw, WINHTTP_OPTION_DISABLE_FEATURE, disable_feature_mask())?;
    set_dword(bindings, raw, WINHTTP_OPTION_DECOMPRESSION, decompression_mask())?;

    Ok(())
}

fn set_dword(bindings: &BindingsFacade, request: RawHandle, option: u32, value: u32) -> WinHttpResult<()> {
    // SAFETY: apply_request_settings calls this only for a live, exclusively
    // owned request before context installation or asynchronous submission.
    // dword_bytes supplies the exact native representation for every option.
    unsafe { bindings.set_option(request, option, &dword_bytes(value)) }
}

fn expect_completion(completion: CompletionResult, expected: OperationKind) -> fetch::Result<()> {
    match (expected, completion) {
        (OperationKind::SendRequest, CompletionResult::SendRequestComplete)
        | (OperationKind::HeadersAvailable, CompletionResult::HeadersAvailable) => Ok(()),
        (_, CompletionResult::Error { error, .. }) => Err(error.into_http_error()),
        (_, CompletionResult::InvalidStatusInfo { status, len, .. }) => Err(callback_protocol_error(format!(
            "WinHTTP returned invalid status information for callback 0x{status:08x} with {len} bytes"
        ))),
        (_, unexpected) => Err(callback_protocol_error(format!(
            "WinHTTP returned an unexpected completion for {expected:?}: {unexpected:?}"
        ))),
    }
}

/// Identifies request metadata rejected before `WinHTTP` receives the request.
///
/// These failures define the validation boundary between generic `fetch`
/// requests and the URI, version, and endpoint forms accepted by this
/// transport. They are mapped to non-recoverable invalid-request errors
/// (design.md section 7).
///
/// Each condition is a separate `ohno` source type rolled up here through a
/// generated `From` implementation, matching the crate-wide convention for
/// conversion failures (implementation.md section 1.1).
#[ohno::error]
#[from(
    EmptyPortError,
    InvalidAuthorityError,
    InvalidPathError,
    MissingAuthorityError,
    MissingHostError,
    MissingSchemeError,
    NonNumericPortError,
    OutOfRangePortError,
    PlainHttpDisallowedError,
    UnsendableRequestVersionError,
    UnsupportedSchemeError,
    UserInfoInAuthorityError,
    ZeroPortError
)]
#[display("the request cannot be translated into a WinHTTP request")]
struct RequestTranslationError;

/// Reports a request URI authority whose colon is not followed by a port.
#[ohno::error]
#[display("the request URI has an empty explicit port; omit the colon or provide a port from 1 to 65535")]
struct EmptyPortError;

/// Reports a request URI authority that does not decompose into host and port.
#[ohno::error]
#[display("the request URI has a malformed authority: '{authority}'")]
struct InvalidAuthorityError {
    authority: String,
}

/// Reports a request URI path that WinHTTP cannot use as a target.
#[ohno::error]
#[display("the request URI path must start with '/': {path}")]
struct InvalidPathError {
    path: String,
}

/// Reports a request URI with no authority to connect to.
#[ohno::error]
#[display("the request URI has no authority")]
struct MissingAuthorityError;

/// Reports a request URI whose authority carries no host.
#[ohno::error]
#[display("the request URI has no host")]
struct MissingHostError;

/// Reports a request URI with no scheme to select transport security from.
#[ohno::error]
#[display("the request URI has no scheme")]
struct MissingSchemeError;

/// Reports a request URI explicit port that is not decimal digits.
#[ohno::error]
#[display("the request URI explicit port '{port}' is not decimal; provide a port from 1 to 65535")]
struct NonNumericPortError {
    port: String,
}

/// Reports a request URI explicit port above the 16-bit port range.
#[ohno::error]
#[display("the request URI explicit port '{port}' is outside the valid range 1 to 65535")]
struct OutOfRangePortError {
    port: String,
}

/// Reports a plain-HTTP request rejected by the client's request filter.
#[ohno::error]
#[display("plain HTTP requests are disabled for this client")]
struct PlainHttpDisallowedError;

/// Reports a request message whose own HTTP version cannot be sent.
///
/// This names the request message's version field rather than the configured
/// version set, because the two are independently fixable: this one is
/// corrected on the `HttpRequest`, whereas an unusable
/// `TransportOptions::supported_http_versions` entry is reported by
/// `options.rs` and is corrected on the client. An operator has to be able to
/// tell the two apart from the message alone.
#[ohno::error]
#[display("the request message asks for HTTP version {version:?}, which WinHTTP cannot send")]
struct UnsendableRequestVersionError {
    version: Version,
}

/// Reports a request URI scheme this transport does not serve.
#[ohno::error]
#[display("the request URI uses unsupported scheme '{scheme}'")]
struct UnsupportedSchemeError {
    scheme: String,
}

/// Reports request URI user information, which WinHTTP cannot forward.
#[ohno::error]
#[display("the request URI authority contains unsupported user information")]
struct UserInfoInAuthorityError;

/// Reports a request URI explicit port of zero, which cannot be connected to.
#[ohno::error]
#[display("the request URI explicit port is zero; provide a port from 1 to 65535")]
struct ZeroPortError;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::pin::Pin;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;
    use std::{slice, thread};

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use fetch::options::{ConnectionLifetime, ConnectionPoolOptions, Http2Options, RequestFilter, TransportOptions};
    use fetch::{HttpBodyBuilder, HttpError, HttpRequest};
    use http::header::HeaderValue;
    use http::{HeaderMap, Method, Version};
    use http_body::{Frame, SizeHint};
    use http_extensions::HttpBodyOptions;
    use http_extensions::timeout::BodyTimeout;
    use ohno::Labeled as _;
    use plurality::Pool;
    use recoverable::{Recovery as _, RecoveryInfo};
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use tick::{Clock, ClockControl};
    use widestring::U16CString;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
    };

    use super::{
        RequestDriver, RequestFailure, RequestSettings, RequestTranslationError, TranslatedRequest, UnsendableRequestVersionError,
        expect_completion, send_request_headers,
    };
    use crate::WinHttpTlsConfig;
    use crate::bindings::{
        BindingsFacade, MockBindings, WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH,
        WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
        WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_VERSION,
    };
    use crate::context::{ColdConnectState, CompletionResult, OperationBuffer, OperationKind};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RequestHandle};
    use crate::mocks::{
        CONNECT, CloseCounts, REQUEST, SESSION, closing, complete, complete_request_error, context_pointer, drive, installed_context,
        installed_context_value, raw_handle, session, status_info_len,
    };
    use crate::operation::{ContextInstallation, ContextPool};

    // HttpError contains user-erased error state without unwind-safety bounds.
    assert_not_impl_any!(RequestFailure: UnwindSafe, RefUnwindSafe);
    // The driver holds a mutable body borrow whose erased implementation may expose partial mutation.
    assert_not_impl_any!(RequestDriver<'static, 'static>: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(TranslatedRequest: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestSettings: UnwindSafe, RefUnwindSafe);
    // Every `ohno` error owns a boxed source without unwind-safety bounds.
    assert_not_impl_any!(RequestTranslationError: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(UnsendableRequestVersionError: UnwindSafe, RefUnwindSafe);

    #[derive(Debug)]
    struct ScriptedBody {
        frames: VecDeque<(usize, fetch::Result<Frame<BytesView>>)>,
        completed_writes: Arc<AtomicUsize>,
        expected_writes_at_end: usize,
        content_length: Option<u64>,
    }

    impl ScriptedBody {
        fn new(
            frames: impl IntoIterator<Item = (usize, fetch::Result<Frame<BytesView>>)>,
            completed_writes: Arc<AtomicUsize>,
            expected_writes_at_end: usize,
            content_length: Option<u64>,
        ) -> Self {
            Self {
                frames: frames.into_iter().collect(),
                completed_writes,
                expected_writes_at_end,
                content_length,
            }
        }
    }

    impl http_body::Body for ScriptedBody {
        type Data = BytesView;
        type Error = HttpError;

        fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let expected_writes = self
                .frames
                .front()
                .map_or(self.expected_writes_at_end, |(expected_writes, _frame)| *expected_writes);
            assert_eq!(
                self.completed_writes.load(Ordering::SeqCst),
                expected_writes,
                "the next request-body frame must not be polled before the previous frame is fully written"
            );

            Poll::Ready(self.frames.pop_front().map(|(_expected_writes, frame)| frame))
        }

        fn is_end_stream(&self) -> bool {
            self.frames.is_empty()
        }

        fn size_hint(&self) -> SizeHint {
            self.content_length.map_or_else(SizeHint::default, SizeHint::with_exact)
        }
    }

    #[test]
    fn get_https_headers_only_lifecycle_translates_and_builds_response() {
        let mut request = request(Method::GET, "https://example.com/resource?q=1");
        request.headers_mut().append("x-request", HeaderValue::from_static("first"));
        request.headers_mut().append("x-request", HeaderValue::from_static("second"));
        let expected_headers = crate::convert::headers_to_utf16(request.headers()).unwrap().as_slice().to_vec();
        let config = LifecycleConfig {
            status: 404,
            raw_headers: raw_headers(&[
                ("content-type", b"text/plain"),
                ("set-cookie", b"first=1"),
                ("set-cookie", b"second=2"),
            ]),
            protocol: 1,
            ..LifecycleConfig::default()
        };

        let (response, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);
        let response = response.unwrap();

        assert_eq!(response.status(), 404);
        assert_eq!(response.version(), Version::HTTP_2);
        assert_eq!(
            record.sent_total_length.load(Ordering::SeqCst),
            0,
            "a request without body content advertises no content length"
        );
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 0, "no request body is written");
        assert_eq!(
            record.data_available_calls.load(Ordering::SeqCst),
            0,
            "response body reads begin only when the caller polls the body"
        );
        assert!(response.extensions().is_empty(), "ConnectionInfo is not attached");
        assert!(response.headers().get("content-length").is_none());
        assert_eq!(
            response
                .headers()
                .get_all("set-cookie")
                .iter()
                .map(HeaderValue::as_bytes)
                .collect::<Vec<_>>(),
            [b"first=1".as_slice(), b"second=2".as_slice()]
        );
        assert_eq!(*record.connect.lock().unwrap(), Some(("example.com".to_owned(), 443)));
        assert_eq!(
            *record.open_request.lock().unwrap(),
            Some(("GET".to_owned(), "/resource?q=1".to_owned(), WINHTTP_FLAG_SECURE.0))
        );
        assert_eq!(*record.sent_headers.lock().unwrap(), expected_headers);
        assert!(record.sent_headers.lock().unwrap().ends_with(&[u16::from(b'\r'), u16::from(b'\n')]));
        assert_eq!(
            record.send_context.load(Ordering::SeqCst),
            record.installed_context.load(Ordering::SeqCst)
        );
        assert_ne!(record.send_context.load(Ordering::SeqCst), 0);
        assert_eq!(
            dword_options(&record),
            [
                (WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, 1),
                (WINHTTP_OPTION_REDIRECT_POLICY, 0),
                (WINHTTP_OPTION_DISABLE_FEATURE, 5),
                (WINHTTP_OPTION_DECOMPRESSION, 3),
            ]
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn head_http_uses_explicit_port_root_path_and_legacy_version_query() {
        let mut options = TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options.supported_http_versions = vec![Version::HTTP_11];
        let config = LifecycleConfig {
            status: 204,
            protocol: 0,
            legacy_version: "HTTP/1.1".to_owned(),
            raw_headers: raw_headers(&[("x-head", b"yes")]),
            ..LifecycleConfig::default()
        };

        let (response, record) = run_lifecycle(
            request(Method::HEAD, "http://example.com:8080"),
            options,
            WinHttpTlsConfig::default(),
            config,
        );
        let response = response.unwrap();

        assert_eq!(response.status(), 204);
        assert_eq!(response.version(), Version::HTTP_11);
        assert_eq!(
            record.data_available_calls.load(Ordering::SeqCst),
            0,
            "response body reads begin only when the caller polls the body"
        );
        assert_eq!(*record.connect.lock().unwrap(), Some(("example.com".to_owned(), 8080)));
        assert_eq!(*record.open_request.lock().unwrap(), Some(("HEAD".to_owned(), "/".to_owned(), 0)));
        assert_eq!(
            dword_options(&record),
            [
                (WINHTTP_OPTION_REDIRECT_POLICY, 0),
                (WINHTTP_OPTION_DISABLE_FEATURE, 5),
                (WINHTTP_OPTION_DECOMPRESSION, 3),
            ]
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn absolute_uri_defaults_ports_and_preserves_query_with_an_empty_path() {
        let mut http_options = TransportOptions::default();
        http_options.request_filter = RequestFilter::HttpAndHttps;
        let (plain_response, plain_record) = run_lifecycle(
            request(Method::GET, "http://example.com?mode=http"),
            http_options,
            WinHttpTlsConfig::default(),
            LifecycleConfig::default(),
        );
        plain_response.unwrap();
        assert_eq!(*plain_record.connect.lock().unwrap(), Some(("example.com".to_owned(), 80)));
        assert_eq!(
            *plain_record.open_request.lock().unwrap(),
            Some(("GET".to_owned(), "/?mode=http".to_owned(), 0))
        );
        assert_lifecycle_closed(&plain_record);

        let (secure_response, secure_record) = run_lifecycle(
            request(Method::GET, "https://example.com"),
            TransportOptions::default(),
            WinHttpTlsConfig::default(),
            LifecycleConfig::default(),
        );
        secure_response.unwrap();
        assert_eq!(*secure_record.connect.lock().unwrap(), Some(("example.com".to_owned(), 443)));
        assert_eq!(
            *secure_record.open_request.lock().unwrap(),
            Some(("GET".to_owned(), "/".to_owned(), WINHTTP_FLAG_SECURE.0))
        );
        assert_lifecycle_closed(&secure_record);
    }

    #[test]
    fn explicit_and_default_ports_support_bracketed_ipv6_authorities() {
        for (uri, expected_host, expected_port) in [
            ("https://example.com:8443/", "example.com", 8443),
            ("https://[::1]:8443/", "[::1]", 8443),
            ("https://[::1]/", "[::1]", 443),
        ] {
            let (response, record) = run_lifecycle(
                request(Method::GET, uri),
                TransportOptions::default(),
                WinHttpTlsConfig::default(),
                LifecycleConfig::default(),
            );

            response.unwrap();
            assert_eq!(*record.connect.lock().unwrap(), Some((expected_host.to_owned(), expected_port)));
            assert_lifecycle_closed(&record);
        }
    }

    #[test]
    fn invalid_explicit_ports_fail_before_native_io() {
        for (uri, message) in [
            ("https://example.com:/", "empty explicit port"),
            ("https://example.com:12x/", "is not decimal"),
            ("https://example.com:65536/", "outside the valid range"),
            ("https://example.com:0/", "explicit port is zero"),
            ("https://[::1]:/", "empty explicit port"),
            ("https://[::1]:65536/", "outside the valid range"),
        ] {
            let (result, record) = run_lifecycle(
                request(Method::GET, uri),
                TransportOptions::default(),
                WinHttpTlsConfig::default(),
                LifecycleConfig::default(),
            );

            let error = result.unwrap_err();
            assert_eq!(error.label(), "invalid_request");
            assert_eq!(error.recovery(), RecoveryInfo::never());
            assert!(error.to_string().contains(message), "{uri}: {error}");
            assert_eq!(record.connect_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn authority_user_information_is_rejected_before_native_io() {
        for uri in [
            "https://user@example.com/",
            "https://user:pass@example.com/",
            "https://user:pass@example.com:8443/",
        ] {
            let (result, record) = run_lifecycle(
                request(Method::GET, uri),
                TransportOptions::default(),
                WinHttpTlsConfig::default(),
                LifecycleConfig::default(),
            );

            let error = result.unwrap_err();
            assert_eq!(error.label(), "invalid_request");
            assert_eq!(error.recovery(), RecoveryInfo::never());
            assert!(
                error
                    .to_string()
                    .contains("the request URI authority contains unsupported user information"),
                "{uri}: {error}"
            );
            assert_eq!(record.connect_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn advanced_protocol_combinations_apply_required_semantics_without_downgrade() {
        for (versions, expected_mask, negotiated) in [
            (vec![Version::HTTP_2], 1, Version::HTTP_2),
            (vec![Version::HTTP_3], 2, Version::HTTP_3),
            (vec![Version::HTTP_2, Version::HTTP_3], 3, Version::HTTP_3),
        ] {
            let mut options = TransportOptions::default();
            options.supported_http_versions = versions;
            let config = LifecycleConfig {
                protocol: if negotiated == Version::HTTP_2 { 1 } else { 2 },
                ..LifecycleConfig::default()
            };

            let (response, record) = run_lifecycle(
                request(Method::GET, "https://example.com/"),
                options,
                WinHttpTlsConfig::default(),
                config,
            );

            assert_eq!(response.unwrap().version(), negotiated);
            assert_eq!(
                &dword_options(&record)[..2],
                [
                    (WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, expected_mask),
                    (WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, 1),
                ]
            );
            assert_lifecycle_closed(&record);
        }
    }

    #[test]
    fn tls_relaxations_are_independent_request_masks() {
        for (tls, expected) in [
            (WinHttpTlsConfig::default(), None),
            (WinHttpTlsConfig::builder().accept_invalid_certs(true).build(), Some(0x2300)),
            (WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build(), Some(0x1000)),
            (
                WinHttpTlsConfig::builder()
                    .accept_invalid_certs(true)
                    .accept_invalid_hostnames(true)
                    .build(),
                Some(0x3300),
            ),
        ] {
            let (response, record) = run_lifecycle(
                request(Method::GET, "https://example.com/"),
                TransportOptions::default(),
                tls,
                LifecycleConfig::default(),
            );

            response.unwrap();
            let actual = dword_options(&record)
                .into_iter()
                .find_map(|(option, value)| (option == WINHTTP_OPTION_SECURITY_FLAGS).then_some(value));
            assert_eq!(actual, expected);
            assert_lifecycle_closed(&record);
        }

        let mut options = TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        let tls = WinHttpTlsConfig::builder()
            .accept_invalid_certs(true)
            .accept_invalid_hostnames(true)
            .build();
        let (response, record) = run_lifecycle(
            request(Method::GET, "http://example.com/"),
            options,
            tls,
            LifecycleConfig::default(),
        );

        response.unwrap();
        assert!(
            dword_options(&record)
                .into_iter()
                .all(|(option, _)| option != WINHTTP_OPTION_SECURITY_FLAGS)
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn redirect_cookie_auth_and_decompression_options_are_required() {
        let (response, record) = run_lifecycle(
            request(Method::GET, "https://example.com/"),
            TransportOptions::default(),
            WinHttpTlsConfig::default(),
            LifecycleConfig::default(),
        );

        response.unwrap();
        assert_eq!(
            dword_options(&record),
            [
                (WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, 1),
                (WINHTTP_OPTION_REDIRECT_POLICY, 0),
                (WINHTTP_OPTION_DISABLE_FEATURE, 5),
                (WINHTTP_OPTION_DECOMPRESSION, 3),
            ]
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn unsupported_generic_options_do_not_add_native_request_configuration() {
        let mut options = TransportOptions::default();
        options.connection_pool = ConnectionPoolOptions::default()
            .max_connections(1)
            .connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(2)));
        options.http_2 = Http2Options::default().initial_max_send_streams(1).adaptive_window(true);
        options.extra.insert(42_u32);

        let (response, record) = run_lifecycle(
            request(Method::GET, "https://example.com/"),
            options,
            WinHttpTlsConfig::default(),
            LifecycleConfig::default(),
        );

        response.unwrap();
        assert_eq!(
            dword_options(&record),
            [
                (WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, 1),
                (WINHTTP_OPTION_REDIRECT_POLICY, 0),
                (WINHTTP_OPTION_DISABLE_FEATURE, 5),
                (WINHTTP_OPTION_DECOMPRESSION, 3),
            ]
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn malformed_or_unsupported_requests_fail_before_native_io() {
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_frozen());
        let asterisk_form = http::Request::builder()
            .method(Method::OPTIONS)
            .uri(
                http::Uri::builder()
                    .scheme("https")
                    .authority("example.com")
                    .path_and_query("*")
                    .build()
                    .unwrap(),
            )
            .body(body_builder.empty())
            .unwrap();
        let cases = [
            (
                request(Method::GET, "/relative"),
                TransportOptions::default(),
                "request URI has no scheme",
            ),
            (
                request(Method::GET, "ftp://example.com/"),
                TransportOptions::default(),
                "unsupported scheme",
            ),
            (
                request(Method::GET, "http://example.com/"),
                TransportOptions::default(),
                "plain HTTP requests are disabled",
            ),
            (asterisk_form, TransportOptions::default(), "path must start with '/'"),
        ];

        for (request, options, message) in cases {
            let (result, record) = run_lifecycle(request, options, WinHttpTlsConfig::default(), LifecycleConfig::default());
            let error = result.unwrap_err();
            assert_eq!(error.label(), "invalid_request");
            assert_eq!(error.recovery(), RecoveryInfo::never());
            assert!(error.to_string().contains(message));
            assert_eq!(record.connect_calls.load(Ordering::SeqCst), 0);
            assert_eq!(record.request_closes.load(Ordering::SeqCst), 0);
            assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
        }

        Method::from_bytes(b"GET\r\nInjected").unwrap_err();
        http::HeaderName::from_bytes(b"bad header").unwrap_err();
        HeaderValue::from_bytes(b"value\r\nInjected: true").unwrap_err();
    }

    #[test]
    fn legacy_requested_versions_are_invalid_requests() {
        // The configured version set and the request message's own version are
        // two independently fixable conditions, so each names its own source.
        for version in [Version::HTTP_09, Version::HTTP_10] {
            let mut options = TransportOptions::default();
            options.supported_http_versions = vec![version];
            let (result, record) = run_lifecycle(
                request(Method::GET, "https://example.com/"),
                options,
                WinHttpTlsConfig::default(),
                LifecycleConfig::default(),
            );

            let error = result.unwrap_err();
            assert_eq!(error.label(), "invalid_request");
            assert!(
                error.to_string().contains("WinHTTP does not support requested HTTP version"),
                "{error}"
            );
            assert_eq!(record.connect_calls.load(Ordering::SeqCst), 0);
        }

        let mut legacy_request = request(Method::GET, "https://example.com/");
        *legacy_request.version_mut() = Version::HTTP_10;
        let (result, record) = run_lifecycle(
            legacy_request,
            TransportOptions::default(),
            WinHttpTlsConfig::default(),
            LifecycleConfig::default(),
        );
        let error = result.unwrap_err();
        assert_eq!(error.label(), "invalid_request");
        assert!(
            error
                .to_string()
                .contains("the request message asks for HTTP version HTTP/1.0, which WinHTTP cannot send"),
            "{error}"
        );
        assert_eq!(record.connect_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn request_upload_writes_every_frame_and_contiguous_span_sequentially() {
        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
        let first = BytesView::copied_from_slice(b"ab", &memory);
        let second = BytesView::copied_from_slice(b"cd", &memory);
        let third = BytesView::copied_from_slice(b"ef", &memory);
        let expected_addresses = [
            first.first_slice().as_ptr().addr(),
            second.first_slice().as_ptr().addr(),
            third.first_slice().as_ptr().addr(),
        ];
        let segmented = BytesView::from_views([first, second]);
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new(
                [
                    (0, Ok(Frame::data(BytesView::new()))),
                    (0, Ok(Frame::data(segmented))),
                    (2, Ok(Frame::data(third))),
                ],
                Arc::clone(&completed_writes),
                3,
                Some(6),
            ),
            &HttpBodyOptions::default(),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            ..LifecycleConfig::default()
        };

        let (response, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);

        response.unwrap();
        assert_eq!(record.sent_total_length.load(Ordering::SeqCst), 6);
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *record.writes.lock().unwrap(),
            [
                RecordedWrite {
                    address: expected_addresses[0],
                    bytes: b"ab".to_vec(),
                },
                RecordedWrite {
                    address: expected_addresses[1],
                    bytes: b"cd".to_vec(),
                },
                RecordedWrite {
                    address: expected_addresses[2],
                    bytes: b"ef".to_vec(),
                },
            ]
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn partial_write_completions_advance_within_the_active_span() {
        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
        let data = BytesView::copied_from_slice(b"abcd", &memory);
        let address = data.first_slice().as_ptr().addr();
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new([(0, Ok(Frame::data(data)))], Arc::clone(&completed_writes), 2, Some(4)),
            &HttpBodyOptions::default(),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            max_write_completion: Some(2),
            ..LifecycleConfig::default()
        };

        let (response, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);

        response.unwrap();
        assert_eq!(
            *record.writes.lock().unwrap(),
            [
                RecordedWrite {
                    address,
                    bytes: b"abcd".to_vec(),
                },
                RecordedWrite {
                    address: address + 2,
                    bytes: b"cd".to_vec(),
                },
            ]
        );
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn unknown_length_uploads_use_automatic_chunking_for_every_supported_protocol() {
        for (versions, protocol) in [(vec![Version::HTTP_11], 0), (vec![Version::HTTP_2], 1), (vec![Version::HTTP_3], 2)] {
            let memory = GlobalPool::new();
            let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
            let completed_writes = Arc::new(AtomicUsize::new(0));
            let body = body_builder.body(
                ScriptedBody::new(
                    [(0, Ok(Frame::data(BytesView::copied_from_slice(b"x", &memory))))],
                    Arc::clone(&completed_writes),
                    1,
                    None,
                ),
                &HttpBodyOptions::default(),
            );
            let request = http::Request::builder()
                .method(Method::POST)
                .uri("https://example.com/")
                .body(body)
                .unwrap();
            let mut options = TransportOptions::default();
            options.supported_http_versions = versions;
            let config = LifecycleConfig {
                protocol,
                completed_writes,
                ..LifecycleConfig::default()
            };

            let (response, record) = run_lifecycle(request, options, WinHttpTlsConfig::default(), config);

            response.unwrap();
            assert_eq!(
                record.open_request.lock().unwrap().as_ref().unwrap().2,
                WINHTTP_FLAG_SECURE.0 | WINHTTP_FLAG_AUTOMATIC_CHUNKING
            );
            assert_eq!(
                record.sent_total_length.load(Ordering::SeqCst),
                WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH as usize
            );
            assert_eq!(record.write_calls.load(Ordering::SeqCst), 1);
            assert_eq!(record.end_write_calls.load(Ordering::SeqCst), 1);
            assert_eq!(record.end_write_completions.load(Ordering::SeqCst), 1);
            assert_eq!(record.receive_calls.load(Ordering::SeqCst), 1);
            assert_lifecycle_closed(&record);
        }
    }

    #[test]
    fn unknown_length_upload_waits_for_terminal_write_completion_before_receiving_response() {
        let memory = GlobalPool::new();
        let clock = Clock::new_frozen();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &clock);
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new([], Arc::clone(&completed_writes), 0, None),
            &HttpBodyOptions::default(),
        );
        let mut request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            defer_write_completion: true,
            ..LifecycleConfig::default()
        };
        let record = Arc::new(LifecycleRecord::default());
        let facade = lifecycle_bindings(Arc::new(config), Arc::clone(&record));
        let session = session(facade);
        let contexts = ContextPool::new(Pool::new());
        let options = TransportOptions::default();
        let tls = WinHttpTlsConfig::default();
        let driver = RequestDriver::new(
            &mut request,
            Arc::clone(&session),
            body_builder,
            &clock,
            memory,
            &contexts,
            &options,
            &tls,
        )
        .unwrap();
        let mut body_polled = false;
        let mut future = Box::pin(driver.execute(&mut body_polled));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 0);
        assert_eq!(record.end_write_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.end_write_completions.load(Ordering::SeqCst), 0);
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);

        let context = recorded_context(&record);
        let mut written = 0_u32;
        record.end_write_completions.fetch_add(1, Ordering::SeqCst);
        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload readable and unmodified for the call, no overlapping
        // notification, no outstanding exclusive borrow, and no use of the
        // context after the reclaiming notification. The lifecycle mock
        // registered this context at installation and this test reclaims it
        // only below; the payload is the initialized local `written`, which
        // outlives the call and nothing else can reach; the test drives every
        // notification from its own thread, and the deferred script raises
        // none; and the pipeline borrows the context only sharedly.
        unsafe {
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                (&raw mut written).cast(),
                status_info_len::<u32>(),
            );
        }

        let Poll::Ready(response) = future.as_mut().poll(&mut cx) else {
            panic!("response becomes ready after the terminal write completes");
        };
        let response = response.unwrap();
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 1);

        drop(future);
        drop(response);
        drop((request, options, tls));
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // context was registered at installation and nothing has reclaimed it;
        // the response owning the guard is dropped above, so no notification is
        // in flight and none can follow; and nothing borrowed the context
        // exclusively. The pointer is not used again.
        unsafe {
            closing(context);
        }
        drop(session);

        assert!(body_polled);
        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn large_known_length_uses_explicit_content_length_without_allocating_the_body() {
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_frozen());
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let length = u64::from(u32::MAX) + 1;
        let body = body_builder.body(
            ScriptedBody::new([], Arc::clone(&completed_writes), 0, Some(length)),
            &HttpBodyOptions::default(),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            ..LifecycleConfig::default()
        };

        let (response, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);

        response.unwrap();
        assert_eq!(
            record.sent_total_length.load(Ordering::SeqCst),
            WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH as usize
        );
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 0);
        let sent_headers = String::from_utf16(&record.sent_headers.lock().unwrap()).unwrap();
        assert!(sent_headers.contains("content-length: 4294967296\r\n"));
        assert_eq!(record.open_request.lock().unwrap().as_ref().unwrap().2, WINHTTP_FLAG_SECURE.0);
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn request_body_errors_and_trailers_stop_before_response_reception() {
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_frozen());
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new(
                [(0, Err(HttpError::validation("request body stream failed")))],
                Arc::clone(&completed_writes),
                0,
                None,
            ),
            &HttpBodyOptions::default(),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            ..LifecycleConfig::default()
        };

        let (result, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);
        let error = result.unwrap_err();
        assert_eq!(error.label(), "validation");
        assert!(error.to_string().contains("request body stream failed"));
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 0);
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);
        assert_lifecycle_closed(&record);

        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new([(0, Ok(Frame::trailers(HeaderMap::new())))], Arc::clone(&completed_writes), 0, None),
            &HttpBodyOptions::default(),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            ..LifecycleConfig::default()
        };

        let (result, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);
        let error = result.unwrap_err();
        assert_eq!(error.label(), "invalid_request");
        assert!(error.to_string().contains("cannot submit request trailer frames"));
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 0);
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn final_unknown_length_write_failures_stop_before_response_reception() {
        for failure in [LifecycleFailure::EndWriteSync, LifecycleFailure::EndWriteCallback] {
            let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_frozen());
            let completed_writes = Arc::new(AtomicUsize::new(0));
            let body = body_builder.body(
                ScriptedBody::new([], Arc::clone(&completed_writes), 0, None),
                &HttpBodyOptions::default(),
            );
            let request = http::Request::builder()
                .method(Method::POST)
                .uri("https://example.com/")
                .body(body)
                .unwrap();
            let config = LifecycleConfig {
                failure: Some(failure),
                completed_writes,
                ..LifecycleConfig::default()
            };

            let (result, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);
            let error = result.unwrap_err();
            assert_eq!(error.label(), "request_winhttp");
            assert_eq!(record.write_calls.load(Ordering::SeqCst), 0);
            assert_eq!(record.end_write_calls.load(Ordering::SeqCst), 1);
            assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);
            assert_lifecycle_closed(&record);
        }
    }

    #[test]
    fn synchronous_callback_and_zero_length_write_failures_are_reported() {
        for failure in [LifecycleFailure::WriteSync, LifecycleFailure::WriteCallback] {
            let memory = GlobalPool::new();
            let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
            let completed_writes = Arc::new(AtomicUsize::new(0));
            let body = body_builder.body(
                ScriptedBody::new(
                    [(0, Ok(Frame::data(BytesView::copied_from_slice(b"data", &memory))))],
                    Arc::clone(&completed_writes),
                    0,
                    Some(4),
                ),
                &HttpBodyOptions::default(),
            );
            let request = http::Request::builder()
                .method(Method::POST)
                .uri("https://example.com/")
                .body(body)
                .unwrap();
            let config = LifecycleConfig {
                failure: Some(failure),
                completed_writes,
                ..LifecycleConfig::default()
            };

            let (result, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);
            let error = result.unwrap_err();
            assert_eq!(error.label(), "request_winhttp");
            assert_eq!(record.write_calls.load(Ordering::SeqCst), 1);
            assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);
            assert_lifecycle_closed(&record);
        }

        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new(
                [(0, Ok(Frame::data(BytesView::copied_from_slice(b"data", &memory))))],
                Arc::clone(&completed_writes),
                0,
                Some(4),
            ),
            &HttpBodyOptions::default(),
        );
        let request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            max_write_completion: Some(0),
            ..LifecycleConfig::default()
        };

        let (result, record) = run_lifecycle(request, TransportOptions::default(), WinHttpTlsConfig::default(), config);
        let error = result.unwrap_err();
        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("without writing any bytes"));
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);
        assert_lifecycle_closed(&record);
    }

    #[test]
    fn cancelling_an_active_upload_retains_the_buffer_and_parents_until_handle_closing() {
        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new(
                [(0, Ok(Frame::data(BytesView::copied_from_slice(b"outstanding", &memory))))],
                Arc::clone(&completed_writes),
                0,
                Some(11),
            ),
            &HttpBodyOptions::default(),
        );
        let mut request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            defer_write_completion: true,
            ..LifecycleConfig::default()
        };
        let record = Arc::new(LifecycleRecord::default());
        let facade = lifecycle_bindings(Arc::new(config), Arc::clone(&record));
        let session = session(facade);
        let contexts = ContextPool::new(Pool::new());
        let response_body_builder = HttpBodyBuilder::new(memory.clone(), &Clock::new_frozen());
        let options = TransportOptions::default();
        let tls = WinHttpTlsConfig::default();
        let driver = RequestDriver::new(
            &mut request,
            Arc::clone(&session),
            response_body_builder,
            &Clock::new_frozen(),
            memory,
            &contexts,
            &options,
            &tls,
        )
        .unwrap();
        let mut body_polled = false;
        let mut future = Box::pin(driver.execute(&mut body_polled));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        drop(future);
        drop(session);

        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // lifecycle mock registered this context at installation and nothing
        // has reclaimed it; the future owning the guard is dropped above, so no
        // notification is in flight and the deferred script raises none; the
        // pipeline borrows the context only sharedly; and the pointer is not
        // used again.
        unsafe {
            closing(recorded_context(&record));
        }

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn connect_timeout_closes_the_request_and_retains_parents_until_handle_closing() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let mut request = request(Method::GET, "https://example.com/");
        let config = LifecycleConfig {
            defer_send_completion: true,
            notify_connecting: true,
            ..LifecycleConfig::default()
        };
        let record = Arc::new(LifecycleRecord::default());
        let facade = lifecycle_bindings(Arc::new(config), Arc::clone(&record));
        let session = session(facade);
        let contexts = ContextPool::new(Pool::new());
        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &clock);
        let mut options = TransportOptions::default();
        options.connect_timeout = Duration::from_secs(1);
        let tls = WinHttpTlsConfig::default();
        let driver = RequestDriver::new(
            &mut request,
            Arc::clone(&session),
            body_builder,
            &clock,
            memory,
            &contexts,
            &options,
            &tls,
        )
        .unwrap();
        let mut body_polled = false;
        let mut future = Box::pin(driver.execute(&mut body_polled));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(record.send_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        control.advance(Duration::from_secs(1));
        let Poll::Ready(Err(failure)) = future.as_mut().poll(&mut cx) else {
            panic!("the total connect deadline must fail the pending send");
        };
        drop(future);
        assert_eq!(failure.cold_connect_duration(), Some(Duration::from_secs(1)));
        let error = failure.into_error();
        assert_eq!(error.label(), "response_timeout");
        assert_eq!(error.recovery(), RecoveryInfo::retry());
        assert!(!body_polled);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        drop(session);
        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // lifecycle mock registered this context at installation and nothing
        // has reclaimed it; the future owning the guard is dropped above, so no
        // notification is in flight and the deferred script raises none; the
        // pipeline borrows the context only sharedly; and the pointer is not
        // used again.
        unsafe {
            closing(recorded_context(&record));
        }

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn completed_send_cancels_connect_timeout_before_upload() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &clock);
        let completed_writes = Arc::new(AtomicUsize::new(0));
        let body = body_builder.body(
            ScriptedBody::new(
                [(0, Ok(Frame::data(BytesView::copied_from_slice(b"pending upload", &memory))))],
                Arc::clone(&completed_writes),
                0,
                Some(14),
            ),
            &HttpBodyOptions::default(),
        );
        let mut request = http::Request::builder()
            .method(Method::POST)
            .uri("https://example.com/")
            .body(body)
            .unwrap();
        let config = LifecycleConfig {
            completed_writes,
            defer_write_completion: true,
            ..LifecycleConfig::default()
        };
        let record = Arc::new(LifecycleRecord::default());
        let facade = lifecycle_bindings(Arc::new(config), Arc::clone(&record));
        let session = session(facade);
        let contexts = ContextPool::new(Pool::new());
        let mut options = TransportOptions::default();
        options.connect_timeout = Duration::from_secs(1);
        let tls = WinHttpTlsConfig::default();
        let driver = RequestDriver::new(
            &mut request,
            Arc::clone(&session),
            body_builder,
            &clock,
            memory,
            &contexts,
            &options,
            &tls,
        )
        .unwrap();
        let mut body_polled = false;
        let mut future = Box::pin(driver.execute(&mut body_polled));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(record.write_calls.load(Ordering::SeqCst), 1);

        control.advance(Duration::from_secs(1));
        assert!(
            future.as_mut().poll(&mut cx).is_pending(),
            "upload remains outside the completed connect deadline"
        );
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 0);

        drop(future);
        assert!(body_polled);
        drop(session);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // lifecycle mock registered this context at installation and nothing
        // has reclaimed it; the future owning the guard is dropped above, so no
        // notification is in flight and the deferred script raises none; the
        // pipeline borrows the context only sharedly; and the pointer is not
        // used again.
        unsafe {
            closing(recorded_context(&record));
        }

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn request_body_timeout_reaches_the_response_body_and_closes_a_pending_read() {
        let mut request = request(Method::GET, "https://example.com/");
        request.extensions_mut().insert(BodyTimeout::new(Duration::from_secs(1)));
        let config = LifecycleConfig::default();
        let record = Arc::new(LifecycleRecord::default());
        let facade = lifecycle_bindings(Arc::new(config), Arc::clone(&record));
        let session = session(facade);
        let contexts = ContextPool::new(Pool::new());
        let memory = GlobalPool::new();
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &clock);
        let options = TransportOptions::default();
        let tls = WinHttpTlsConfig::default();
        let driver = RequestDriver::new(
            &mut request,
            Arc::clone(&session),
            body_builder,
            &clock,
            memory,
            &contexts,
            &options,
            &tls,
        )
        .unwrap();
        let mut body_polled = false;
        let response = drive(driver.execute(&mut body_polled)).unwrap();

        assert_eq!(record.request_closes.load(Ordering::SeqCst), 0);
        let error = drive(response.into_body().into_bytes()).unwrap_err();
        assert_eq!(error.label(), "body_timeout");
        assert_eq!(record.data_available_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);

        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // lifecycle mock registered this context at installation and nothing
        // has reclaimed it; the body owning the guard is consumed above, so no
        // notification is in flight and the deferred script raises none; the
        // pipeline borrows the context only sharedly; and the pointer is not
        // used again.
        unsafe {
            closing(recorded_context(&record));
        }
        drop(session);

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn every_required_request_option_failure_aborts_before_send() {
        let mut options = TransportOptions::default();
        options.supported_http_versions = vec![Version::HTTP_2, Version::HTTP_3];
        let tls = WinHttpTlsConfig::builder()
            .accept_invalid_certs(true)
            .accept_invalid_hostnames(true)
            .build();

        for failed_index in 0..7 {
            let config = LifecycleConfig {
                failure: Some(LifecycleFailure::Option(failed_index)),
                ..LifecycleConfig::default()
            };
            let (result, record) = run_lifecycle(request(Method::GET, "https://example.com/"), options.clone(), tls.clone(), config);

            assert!(result.is_err(), "option {failed_index} must fail the request");
            assert_eq!(record.send_calls.load(Ordering::SeqCst), 0);
            assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
            assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
            assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn synchronous_and_callback_errors_fail_the_matching_stage() {
        for (failure, expected_label, expected_recovery) in [
            (LifecycleFailure::Connect, "connect", RecoveryInfo::retry()),
            (LifecycleFailure::OpenRequest, "request_winhttp", RecoveryInfo::unknown()),
            (LifecycleFailure::SendSync, "connect", RecoveryInfo::retry()),
            (LifecycleFailure::SendCallback, "request_winhttp", RecoveryInfo::retry()),
            (LifecycleFailure::ReceiveSync, "timeout", RecoveryInfo::retry()),
            (LifecycleFailure::ReceiveCallback, "tls", RecoveryInfo::never()),
            (LifecycleFailure::QueryStatus, "timeout", RecoveryInfo::retry()),
            (LifecycleFailure::QueryHeaders, "request_winhttp", RecoveryInfo::never()),
            (LifecycleFailure::QueryProtocol, "tls", RecoveryInfo::never()),
        ] {
            let config = LifecycleConfig {
                failure: Some(failure),
                ..LifecycleConfig::default()
            };
            let (result, record) = run_lifecycle(
                request(Method::GET, "https://example.com/"),
                TransportOptions::default(),
                WinHttpTlsConfig::default(),
                config,
            );

            let error = result.unwrap_err();
            assert_eq!(error.label(), expected_label, "{failure:?}");
            assert_eq!(error.recovery(), expected_recovery, "{failure:?}");
            assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
            if failure != LifecycleFailure::Connect {
                assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[test]
    fn a_completion_that_does_not_answer_the_awaited_stage_is_a_protocol_error() {
        // Each request stage awaits exactly one completion. Anything else the
        // callback can deliver - a completion belonging to a different stage, or
        // one whose status information WinHTTP filled in inconsistently - leaves
        // the stage without the answer it needs and is reported as a violation
        // of the callback contract rather than silently accepted.
        let unexpected = expect_completion(CompletionResult::DataAvailable(4), OperationKind::SendRequest).unwrap_err();
        assert_eq!(unexpected.label(), "request_winhttp");
        assert!(unexpected.to_string().contains("unexpected completion for SendRequest"));

        let malformed = expect_completion(
            CompletionResult::invalid_status_info(0x0002_0000, 3, OperationBuffer::none()),
            OperationKind::HeadersAvailable,
        )
        .unwrap_err();
        assert_eq!(malformed.label(), "request_winhttp");
        assert!(malformed.to_string().contains("callback 0x00020000 with 3 bytes"));
    }

    #[test]
    fn response_header_validation_and_foreign_thread_completion_are_deterministic() {
        let config = LifecycleConfig {
            complete_send_on_foreign_thread: true,
            status: 500,
            protocol: 2,
            raw_headers: raw_headers(&[("content-length", b"123"), ("x-duplicate", b"one"), ("x-duplicate", &[0x80, 0xff])]),
            ..LifecycleConfig::default()
        };
        let (response, record) = run_lifecycle(
            request(Method::GET, "https://example.com/"),
            TransportOptions::default(),
            WinHttpTlsConfig::default(),
            config,
        );
        let response = response.unwrap();

        assert_eq!(response.status(), 500);
        assert_eq!(response.version(), Version::HTTP_3);
        assert_eq!(response.headers().get("content-length"), Some(&HeaderValue::from_static("123")));
        assert_eq!(
            response
                .headers()
                .get_all("x-duplicate")
                .iter()
                .map(HeaderValue::as_bytes)
                .collect::<Vec<_>>(),
            [b"one".as_slice(), &[0x80, 0xff]]
        );
        assert_lifecycle_closed(&record);

        // The parser's own rules are covered directly in `response_headers`,
        // which needs no handle, no callback, and no lifecycle. What only a
        // lifecycle can show is that a parser rejection reaches the caller as a
        // non-recoverable `request_winhttp` failure with every handle closed,
        // so exactly one malformed block is driven end to end here.
        let malformed = LifecycleConfig {
            raw_headers: b"HTTP/1.1 200 OK\r\nmissing-colon\r\n\r\n".to_vec(),
            ..LifecycleConfig::default()
        };
        let (result, record) = run_lifecycle(
            request(Method::GET, "https://example.com/"),
            TransportOptions::default(),
            WinHttpTlsConfig::default(),
            malformed,
        );
        let error = result.unwrap_err();
        assert_eq!(error.label(), "request_winhttp");
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert!(error.to_string().contains("without a ':' separator"), "{error}");
        assert_lifecycle_closed(&record);

        let malformed_protocol = LifecycleConfig {
            protocol: 3,
            ..LifecycleConfig::default()
        };
        let (result, record) = run_lifecycle(
            request(Method::GET, "https://example.com/"),
            TransportOptions::default(),
            WinHttpTlsConfig::default(),
            malformed_protocol,
        );
        let error = result.unwrap_err();
        assert_eq!(error.label(), "request_winhttp");
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_lifecycle_closed(&record);
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum LifecycleFailure {
        Connect,
        OpenRequest,
        Option(usize),
        SendSync,
        SendCallback,
        WriteSync,
        WriteCallback,
        EndWriteSync,
        EndWriteCallback,
        ReceiveSync,
        ReceiveCallback,
        QueryStatus,
        QueryHeaders,
        QueryProtocol,
    }

    #[derive(Clone)]
    #[expect(
        clippy::struct_excessive_bools,
        reason = "the lifecycle mock independently scripts completion, upload, and read behavior"
    )]
    struct LifecycleConfig {
        status: u32,
        raw_headers: Vec<u8>,
        protocol: u32,
        legacy_version: String,
        failure: Option<LifecycleFailure>,
        complete_send_on_foreign_thread: bool,
        defer_send_completion: bool,
        notify_connecting: bool,
        completed_writes: Arc<AtomicUsize>,
        defer_write_completion: bool,
        max_write_completion: Option<u32>,
    }

    impl Default for LifecycleConfig {
        fn default() -> Self {
            Self {
                status: 200,
                raw_headers: raw_headers(&[("content-length", b"0")]),
                protocol: 1,
                legacy_version: "HTTP/1.1".to_owned(),
                failure: None,
                complete_send_on_foreign_thread: false,
                defer_send_completion: false,
                notify_connecting: false,
                completed_writes: Arc::new(AtomicUsize::new(0)),
                defer_write_completion: false,
                max_write_completion: None,
            }
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedWrite {
        address: usize,
        bytes: Vec<u8>,
    }

    #[derive(Default)]
    struct LifecycleRecord {
        connect: Mutex<Option<(String, u16)>>,
        open_request: Mutex<Option<(String, String, u32)>>,
        options: Mutex<Vec<(u32, Vec<u8>)>>,
        sent_headers: Mutex<Vec<u16>>,
        sent_total_length: AtomicUsize,
        writes: Mutex<Vec<RecordedWrite>>,
        connect_calls: AtomicUsize,
        send_calls: AtomicUsize,
        write_calls: AtomicUsize,
        end_write_calls: AtomicUsize,
        end_write_completions: AtomicUsize,
        receive_calls: AtomicUsize,
        data_available_calls: AtomicUsize,
        installed_context: AtomicUsize,
        send_context: AtomicUsize,
        session_closes: AtomicUsize,
        connect_closes: AtomicUsize,
        request_closes: AtomicUsize,
    }

    fn run_lifecycle(
        mut request: HttpRequest,
        options: TransportOptions,
        tls: WinHttpTlsConfig,
        config: LifecycleConfig,
    ) -> (fetch::Result<fetch::HttpResponse>, Arc<LifecycleRecord>) {
        let record = Arc::new(LifecycleRecord::default());
        let facade = lifecycle_bindings(Arc::new(config), Arc::clone(&record));
        let session = session(facade);
        let contexts = ContextPool::new(Pool::new());
        let global_pool = GlobalPool::new();
        let clock = Clock::new_frozen();
        let body_builder = HttpBodyBuilder::new(global_pool.clone(), &clock);
        let mut body_polled = false;
        let result = match RequestDriver::new(
            &mut request,
            Arc::clone(&session),
            body_builder.clone(),
            &clock,
            global_pool,
            &contexts,
            &options,
            &tls,
        ) {
            Ok(driver) => drive(driver.execute(&mut body_polled)).map_err(RequestFailure::into_error),
            Err(error) => Err(error),
        };
        // The response body owns the request guard, so it must be released here
        // for the HANDLE_CLOSING dispatch below to complete the ownership
        // protocol. The returned response therefore carries a stand-in body:
        // it reflects nothing the driver produced, and response-body behavior
        // is asserted through the record or by tests that drive the body
        // themselves.
        let result = result.map(|response| {
            let (parts, body) = response.into_parts();
            drop(body);
            fetch::HttpResponse::from_parts(parts, body_builder.empty())
        });
        drop((request, options, tls));

        if record.installed_context.load(Ordering::SeqCst) != 0 {
            assert_eq!(
                record.request_closes.load(Ordering::SeqCst),
                1,
                "the request guard closes before HANDLE_CLOSING"
            );
            // SAFETY: closing requires an installed, not-yet-reclaimed context,
            // no overlapping notification, no outstanding exclusive borrow, and
            // no dereference of the pointer or of a guard holding it
            // afterwards. The lifecycle mock registered this context at
            // installation and delivered no reclaiming notification; the
            // pipeline has run to completion and its response body, which owns
            // the guard, is dropped above, so nothing is in flight and nothing
            // can follow; and the pipeline borrows the context only sharedly.
            unsafe {
                closing(recorded_context(&record));
            }
        }

        drop(session);
        assert_eq!(contexts.lock().unwrap().len(), 0);

        (result, record)
    }

    /// Builds a mock `WinHTTP` that plays `config` for one request lifecycle.
    ///
    /// The script this installs is what discharges the obligations of
    /// [`complete`] for every notification it delivers. Each notification is
    /// raised from inside a binding the request pipeline called, so it runs on
    /// the submitting thread - or, where the script models a foreign completion
    /// thread, on a thread joined before the binding returns - and can overlap
    /// no other notification for the context; the context is the one registered
    /// here at installation, and only [`run_lifecycle`] reclaims it, after the
    /// pipeline and the guard it owns are dropped; and nothing in the pipeline
    /// borrows the context exclusively.
    #[expect(
        clippy::too_many_lines,
        reason = "the lifecycle mock keeps one complete WinHTTP script visible in one place"
    )]
    fn lifecycle_bindings(config: Arc<LifecycleConfig>, record: Arc<LifecycleRecord>) -> BindingsFacade {
        let mut bindings = MockBindings::new();

        let connect_config = Arc::clone(&config);
        let connect_record = Arc::clone(&record);
        bindings.expect_connect().returning(move |_, host, port| {
            connect_record.connect_calls.fetch_add(1, Ordering::SeqCst);
            *connect_record.connect.lock().unwrap() = Some((host.to_string_lossy(), port));
            if connect_config.failure == Some(LifecycleFailure::Connect) {
                Err(WinHttpError::new(12029, WinHttpOperation::Connect))
            } else {
                Ok(raw_handle(CONNECT))
            }
        });

        let open_config = Arc::clone(&config);
        let open_record = Arc::clone(&record);
        bindings.expect_open_request().returning(move |_, method, path, flags| {
            *open_record.open_request.lock().unwrap() = Some((method.to_string_lossy(), path.to_string_lossy(), flags));
            if open_config.failure == Some(LifecycleFailure::OpenRequest) {
                Err(WinHttpError::new(12005, WinHttpOperation::OpenRequest))
            } else {
                Ok(raw_handle(REQUEST))
            }
        });

        let option_index = Arc::new(AtomicUsize::new(0));
        let option_config = Arc::clone(&config);
        let option_record = Arc::clone(&record);
        bindings.expect_set_option().returning(move |_, option, value| {
            let index = option_index.fetch_add(1, Ordering::SeqCst);
            option_record.options.lock().unwrap().push((option, value.to_vec()));
            if option_config.failure == Some(LifecycleFailure::Option(index)) {
                return Err(WinHttpError::new(
                    13_000 + u32::try_from(index).unwrap(),
                    WinHttpOperation::SetOption,
                ));
            }
            if option == WINHTTP_OPTION_CONTEXT_VALUE {
                // The context is registered where its installation is observed.
                // Registering it at delivery instead would admit every address
                // the harness could name and cost the record its purpose.
                let context = installed_context_value(usize::from_ne_bytes(value.try_into().unwrap()));
                option_record.installed_context.store(context.addr(), Ordering::SeqCst);
            }
            Ok(())
        });

        let send_config = Arc::clone(&config);
        let send_record = Arc::clone(&record);
        bindings.expect_send_request().returning(move |_, headers, total_len, context| {
            send_record.send_calls.fetch_add(1, Ordering::SeqCst);
            send_record.send_context.store(context, Ordering::SeqCst);
            send_record.sent_total_length.store(total_len as usize, Ordering::SeqCst);
            *send_record.sent_headers.lock().unwrap() = headers.as_slice().to_vec();

            if send_config.failure == Some(LifecycleFailure::SendSync) {
                return Err(WinHttpError::new(12029, WinHttpOperation::SendRequest));
            }

            if send_config.notify_connecting {
                // SAFETY: complete requires an installed, not-yet-reclaimed
                // context, a payload matching the notification, no overlapping
                // notification, no outstanding exclusive borrow, and no use of
                // the context after the reclaiming notification. The script
                // (`lifecycle_bindings`) establishes all of them; a diagnostic
                // connect status carries no payload, which a null pointer of
                // zero length states.
                unsafe {
                    complete(
                        recorded_context(&send_record),
                        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
                        std::ptr::null_mut(),
                        0,
                    );
                }
            }

            if send_config.failure == Some(LifecycleFailure::SendCallback) {
                // SAFETY: complete_request_error requires an installed,
                // not-yet-reclaimed context, no overlapping notification, no
                // outstanding exclusive borrow, and no use of the context after
                // the reclaiming notification, and it supplies the payload
                // itself. The script (`lifecycle_bindings`) establishes all of
                // them.
                unsafe {
                    complete_request_error(recorded_context(&send_record), 12030);
                }
            } else if send_config.defer_send_completion {
                return Ok(());
            } else if send_config.complete_send_on_foreign_thread {
                thread::spawn(move || {
                    // SAFETY: complete requires an installed, not-yet-reclaimed
                    // context, a payload matching the notification, no
                    // overlapping notification, no outstanding exclusive
                    // borrow, and no use of the context after the reclaiming
                    // notification. The script (`lifecycle_bindings`)
                    // establishes all of them: this thread is joined below
                    // before the binding returns, so the notification it
                    // delivers still overlaps no other one. A send completion
                    // carries no payload.
                    unsafe {
                        complete(
                            context_pointer(context),
                            WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                            std::ptr::null_mut(),
                            0,
                        );
                    }
                })
                .join()
                .unwrap();
            } else {
                // SAFETY: as for the foreign-thread delivery above, except that
                // this one runs on the submitting thread itself.
                unsafe {
                    complete(
                        recorded_context(&send_record),
                        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                        std::ptr::null_mut(),
                        0,
                    );
                }
            }

            Ok(())
        });

        let write_config = Arc::clone(&config);
        let write_record = Arc::clone(&record);
        bindings.expect_write_data().returning(move |_, buffer, len| {
            let ending_chunking = buffer.is_none();
            if let Some(buffer) = buffer {
                write_record.write_calls.fetch_add(1, Ordering::SeqCst);
                // SAFETY: the active OperationBuffer retains the exact
                // contiguous span passed to the mock for at least the
                // duration of this call.
                let bytes = unsafe { slice::from_raw_parts(buffer.as_ptr(), len as usize) }.to_vec();
                write_record.writes.lock().unwrap().push(RecordedWrite {
                    address: buffer.as_ptr().addr(),
                    bytes,
                });
            } else {
                assert_eq!(len, 0, "only the final zero-length write has no buffer");
                write_record.end_write_calls.fetch_add(1, Ordering::SeqCst);
            }

            if write_config.failure
                == Some(if ending_chunking {
                    LifecycleFailure::EndWriteSync
                } else {
                    LifecycleFailure::WriteSync
                })
            {
                return Err(WinHttpError::new(12030, WinHttpOperation::WriteData));
            }
            if write_config.defer_write_completion {
                return Ok(());
            }

            let context = recorded_context(&write_record);
            if write_config.failure
                == Some(if ending_chunking {
                    LifecycleFailure::EndWriteCallback
                } else {
                    LifecycleFailure::WriteCallback
                })
            {
                // SAFETY: complete_request_error requires an installed,
                // not-yet-reclaimed context, no overlapping notification, no
                // outstanding exclusive borrow, and no use of the context after
                // the reclaiming notification, and it supplies the payload
                // itself. The script (`lifecycle_bindings`) establishes all of
                // them.
                unsafe {
                    complete_request_error(context, 12030);
                }
            } else {
                let mut written = write_config.max_write_completion.map_or(len, |maximum| maximum.min(len));
                if ending_chunking {
                    write_record.end_write_completions.fetch_add(1, Ordering::SeqCst);
                } else {
                    write_config.completed_writes.fetch_add(1, Ordering::SeqCst);
                }
                // SAFETY: complete requires an installed, not-yet-reclaimed
                // context, a payload readable and unmodified for the call, no
                // overlapping notification, no outstanding exclusive borrow,
                // and no use of the context after the reclaiming notification.
                // The script (`lifecycle_bindings`) establishes all of them;
                // the payload is the initialized local `written`, which
                // outlives the call and nothing else can reach.
                unsafe {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                        (&raw mut written).cast(),
                        status_info_len::<u32>(),
                    );
                }
            }

            Ok(())
        });

        let receive_config = Arc::clone(&config);
        let receive_record = Arc::clone(&record);
        bindings.expect_receive_response().returning(move |_| {
            receive_record.receive_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                receive_record.write_calls.load(Ordering::SeqCst),
                receive_config.completed_writes.load(Ordering::SeqCst),
                "response reception must not begin before the final write completes"
            );
            assert_eq!(
                receive_record.end_write_calls.load(Ordering::SeqCst),
                receive_record.end_write_completions.load(Ordering::SeqCst),
                "response reception must not begin before automatic chunking ends"
            );
            if receive_config.failure == Some(LifecycleFailure::ReceiveSync) {
                return Err(WinHttpError::new(12002, WinHttpOperation::ReceiveResponse));
            }

            let context = recorded_context(&receive_record);
            if receive_config.failure == Some(LifecycleFailure::ReceiveCallback) {
                // SAFETY: complete_request_error requires an installed,
                // not-yet-reclaimed context, no overlapping notification, no
                // outstanding exclusive borrow, and no use of the context after
                // the reclaiming notification, and it supplies the payload
                // itself. The script (`lifecycle_bindings`) establishes all of
                // them.
                unsafe {
                    complete_request_error(context, 12175);
                }
            } else {
                // SAFETY: complete requires an installed, not-yet-reclaimed
                // context, a payload matching the notification, no overlapping
                // notification, no outstanding exclusive borrow, and no use of
                // the context after the reclaiming notification. The script
                // (`lifecycle_bindings`) establishes all of them, and a
                // headers-available completion carries no payload.
                unsafe {
                    complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
                }
            }
            Ok(())
        });

        let data_available_record = Arc::clone(&record);
        // The lifecycle script drives requests only as far as the response
        // headers, so it records the availability query the body reader issues
        // and leaves it outstanding; body reads themselves belong to the
        // reader's own tests.
        bindings.expect_query_data_available().returning(move |_| {
            data_available_record.data_available_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let header_config = Arc::clone(&config);
        bindings
            .expect_query_headers()
            .returning(move |_, info_level, buffer, byte_len| match info_level {
                level if level == (WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER) => {
                    if header_config.failure == Some(LifecycleFailure::QueryStatus) {
                        return Err(WinHttpError::new(12002, WinHttpOperation::QueryHeaders));
                    }
                    let output = buffer.unwrap().cast::<u32>();
                    // SAFETY: the lifecycle supplies a writable DWORD buffer.
                    unsafe { output.as_ptr().write(header_config.status) };
                    *byte_len = 4;
                    Ok(())
                }
                level if level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING) => {
                    if header_config.failure == Some(LifecycleFailure::QueryHeaders) {
                        return Err(WinHttpError::new(12152, WinHttpOperation::QueryHeaders));
                    }
                    write_byte_query(&header_config.raw_headers, buffer, byte_len)
                }
                level if level == (WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING | 0x0200_0000) => {
                    Err(WinHttpError::new(12150, WinHttpOperation::QueryHeaders))
                }
                WINHTTP_QUERY_VERSION => {
                    let units = header_config.legacy_version.encode_utf16().collect::<Vec<_>>();
                    write_utf16_query(&units, buffer, byte_len)
                }
                _ => panic!("unexpected header query level {info_level}"),
            });

        let protocol_config = Arc::clone(&config);
        bindings.expect_query_option().returning(move |_, _, buffer, byte_len| {
            if protocol_config.failure == Some(LifecycleFailure::QueryProtocol) {
                return Err(WinHttpError::new(12175, WinHttpOperation::QueryOption));
            }
            let output = buffer.unwrap().cast::<u32>();
            // SAFETY: the lifecycle supplies a writable DWORD buffer.
            unsafe { output.as_ptr().write(protocol_config.protocol) };
            *byte_len = 4;
            Ok(())
        });

        let close_record = Arc::clone(&record);
        bindings.expect_close_handle().returning(move |handle| {
            match handle.as_ptr().addr() {
                SESSION => &close_record.session_closes,
                CONNECT => &close_record.connect_closes,
                REQUEST => &close_record.request_closes,
                _ => panic!("unexpected lifecycle handle"),
            }
            .fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        drop((config, record));

        BindingsFacade::mock(Arc::new(bindings))
    }

    fn write_utf16_query(units: &[u16], buffer: Option<NonNull<u8>>, byte_len: &mut u32) -> crate::error::Result<()> {
        let required_units = units.len().checked_add(1).unwrap();
        let required_bytes = required_units.checked_mul(2).unwrap();
        let required_bytes = u32::try_from(required_bytes).unwrap();

        let Some(buffer) = buffer else {
            *byte_len = required_bytes;
            return Err(WinHttpError::new(122, WinHttpOperation::QueryHeaders));
        };

        assert!(*byte_len >= required_bytes);
        let output = buffer.cast::<u16>();
        // SAFETY: the sizing query reserved required_units writable UTF-16
        // units; this copies the content and writes the trailing NUL.
        // SAFETY: the destination has capacity for every source unit.
        unsafe { output.as_ptr().copy_from_nonoverlapping(units.as_ptr(), units.len()) };
        // SAFETY: required_units includes one element after the copied content.
        let terminator = unsafe { output.as_ptr().add(units.len()) };
        // SAFETY: terminator points to the final writable UTF-16 unit.
        unsafe { terminator.write(0) };
        *byte_len = u32::try_from(units.len() * 2).unwrap();
        Ok(())
    }

    fn write_byte_query(bytes: &[u8], buffer: Option<NonNull<u8>>, byte_len: &mut u32) -> crate::error::Result<()> {
        let required_bytes = bytes.len().checked_add(1).unwrap();
        let required_bytes = u32::try_from(required_bytes).unwrap();

        let Some(output) = buffer else {
            *byte_len = required_bytes;
            return Err(WinHttpError::new(122, WinHttpOperation::QueryHeaders));
        };

        assert!(*byte_len >= required_bytes);
        // SAFETY: the sizing query reserved required_bytes writable bytes;
        // this copies the content and writes the trailing NUL.
        unsafe { output.as_ptr().copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        // SAFETY: required_bytes includes one byte after the copied content.
        let terminator = unsafe { output.as_ptr().add(bytes.len()) };
        // SAFETY: terminator points to the final writable byte.
        unsafe { terminator.write(0) };
        *byte_len = u32::try_from(bytes.len()).unwrap();
        Ok(())
    }

    fn dword_options(record: &LifecycleRecord) -> Vec<(u32, u32)> {
        record
            .options
            .lock()
            .unwrap()
            .iter()
            .filter(|(option, value)| *option != WINHTTP_OPTION_CONTEXT_VALUE && value.len() == size_of::<u32>())
            .map(|(option, value)| (*option, u32::from_ne_bytes(value.as_slice().try_into().unwrap())))
            .collect()
    }

    fn assert_lifecycle_closed(record: &LifecycleRecord) {
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    fn request(method: Method, uri: &str) -> HttpRequest {
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_frozen());
        http::Request::builder().method(method).uri(uri).body(body_builder.empty()).unwrap()
    }

    fn raw_headers(headers: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = b"HTTP/1.1 200 OK\r\n".to_vec();
        for (name, value) in headers {
            bytes.extend(name.bytes());
            bytes.extend(b": ");
            bytes.extend_from_slice(value);
            bytes.extend(b"\r\n");
        }
        bytes.extend(b"\r\n");
        bytes
    }

    #[test]
    fn connect_timeout_snapshots_state_before_inline_handle_closing() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let closes = Arc::new(CloseCounts::default());
        let mut request_bindings = MockBindings::new();
        let context_counts = Arc::clone(&closes);
        request_bindings
            .expect_set_option()
            .withf(|handle, option, value| {
                *handle == raw_handle(REQUEST)
                    && *option == WINHTTP_OPTION_CONTEXT_VALUE
                    && value.len() == size_of::<usize>()
                    && usize::from_ne_bytes(value.try_into().unwrap()) != 0
            })
            .once()
            .returning(move |_, _, value| {
                context_counts
                    .context
                    .store(usize::from_ne_bytes(value.try_into().unwrap()), Ordering::SeqCst);
                Ok(())
            });
        request_bindings.expect_send_request().once().returning(|_, _, _, _| Ok(()));
        let request_counts = Arc::clone(&closes);
        request_bindings
            .expect_close_handle()
            .withf(|handle| handle.as_ptr().addr() == REQUEST)
            .once()
            .returning(move |_| {
                request_counts.request.fetch_add(1, Ordering::SeqCst);
                // SAFETY: closing requires an installed, not-yet-reclaimed
                // context, no overlapping notification, no outstanding
                // exclusive borrow, and no dereference of the pointer or of a
                // guard holding it afterwards. The context was registered when
                // the test read it from the guard, and no earlier notification
                // reclaimed it; the future that owned the pending operation is
                // dropped before this close, so no notification is in flight
                // and the mock raises none after this one; the test borrows the
                // context only sharedly; and the only step left is the
                // remainder of `RequestGuard::drop`, which releases the request
                // handle without reaching the context.
                unsafe {
                    closing(context_pointer(request_counts.context.load(Ordering::SeqCst)));
                }
                Ok(())
            });
        let request_facade = BindingsFacade::mock(Arc::new(request_bindings));

        let mut parent_bindings = MockBindings::new();
        let parent_counts = Arc::clone(&closes);
        parent_bindings.expect_close_handle().times(2).returning(move |handle| {
            match handle.as_ptr().addr() {
                SESSION => &parent_counts.session,
                CONNECT => &parent_counts.connect,
                _ => panic!("unexpected parent test handle"),
            }
            .fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let parent_facade = BindingsFacade::mock(Arc::new(parent_bindings));

        let contexts = ContextPool::new(Pool::new());
        let session = session(parent_facade.clone());
        let mut guard = ContextInstallation::new(
            RequestHandle::new(raw_handle(REQUEST), request_facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), parent_facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .unwrap();
        let context = installed_context(&guard);
        let headers = U16CString::from_str("").unwrap();
        let mut future = Box::pin(send_request_headers(
            &mut guard,
            &request_facade,
            &headers,
            0,
            &clock,
            Duration::from_secs(1),
        ));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload matching the notification, no overlapping notification, no
        // outstanding exclusive borrow, and no use of the context after the
        // reclaiming notification. `installed_context` returned the recorded
        // pointer for the live guard and nothing has reclaimed it; the test
        // drives every notification from its own thread and the mock raises
        // none of its own until the close below; the send-request future
        // borrows the context only sharedly; and a connect-progress
        // notification carries no payload.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
        }
        control.advance(Duration::from_secs(1));
        let Poll::Ready(Err((error, state))) = future.as_mut().poll(&mut cx) else {
            panic!("the connect timeout must win the pending send");
        };
        assert_eq!(state, ColdConnectState::Connecting);
        assert_eq!(error.label(), "response_timeout");

        drop(future);
        assert!(guard.request_handle_taken());
        drop(session);
        drop(guard);

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    /// Reads the context pointer the lifecycle mock recorded at installation.
    fn recorded_context(record: &LifecycleRecord) -> *mut crate::context::RequestContext {
        context_pointer(record.installed_context.load(Ordering::SeqCst))
    }
}
