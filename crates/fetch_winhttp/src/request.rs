// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::future::poll_fn;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::ptr::{NonNull, with_exposed_provenance_mut};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytesbuf::mem::GlobalPool;
use events_once::{Disconnected, RawReceiver};
use fetch::options::{RequestFilter, TransportOptions};
use fetch::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, HttpResponseBuilder, RecoveryInfo};
use http::header::{HeaderName, HeaderValue};
use http::uri::Authority;
use http::{HeaderMap, StatusCode, Version};
use http_extensions::HttpBodyOptions;
use http_extensions::timeout::BodyTimeout;
use plurality::Pool;
use tick::Clock;
use widestring::U16CString;

use crate::bindings::{Bindings as _, Facade};
use crate::body::{RequestBodyPlan, WinHttpBodyReader, WinHttpBodyWriter, WinHttpResponseBody, send_body};
use crate::context::{ColdConnectState, CompletionResult, OperationBuffer, OperationKind, RequestContext};
use crate::error::Result as WinHttpResult;
use crate::error_labels;
use crate::handle::{ConnectHandle, RawHandle, RequestHandle};
use crate::options::{
    ProtocolOptions, QueryError, WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL, WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_OPTION_SECURITY_FLAGS, context_bytes, decompression_mask, disable_feature_mask,
    dword_bytes, headers_to_utf16, host_to_utf16, method_to_utf16, path_to_utf16, protocol_options, query_protocol_used, query_raw_headers,
    query_status_code, request_open_flags, security_flags,
};
use crate::session::WinHttpSession;
use crate::tls::WinHttpTlsConfig;

/// Provides stable callback-context storage for one transport instance.
///
/// Each materialized transport owns a separate pool, so contexts are reused
/// only by requests that share that transport's WinHTTP session. `Pool` is not
/// `Sync`, so the mutex permits allocation and callback-driven return from
/// different threads. The lock is held only while renting or returning an
/// allocation; no WinHTTP call or user code runs while it is held.
pub(crate) type ContextPool = Mutex<Pool<RequestContext>>;

/// Prepares callback ownership for a newly opened request handle.
///
/// The request task initially owns both the RAII request handle and the pooled,
/// pinned context allocation. Successful installation of
/// `WINHTTP_OPTION_CONTEXT_VALUE` transfers context reclamation to the final
/// `HANDLE_CLOSING` callback and returns a [`RequestGuard`] that owns only the
/// request-handle close authority. If installation fails, this type closes the
/// request and reconstructs the pooled box locally.
pub(crate) struct RequestSetup {
    request: RequestHandle,
    context: plurality::Box<RequestContext>,
}

impl RequestSetup {
    pub(crate) fn new(request: RequestHandle, connect: ConnectHandle, session: Arc<WinHttpSession>, contexts: &ContextPool) -> Self {
        let context = contexts
            .lock()
            .expect("the request-context pool lock cannot be poisoned because no user code runs while it is held")
            .alloc_box(RequestContext::new(connect, session));

        Self { request, context }
    }

    pub(crate) fn install(self) -> WinHttpResult<RequestGuard> {
        let Self { request, context } = self;
        let context = plurality::Box::into_raw(context);
        let mut raw_owner = RawContextOwner::new(context);
        let context_value = context.as_ptr().expose_provenance();
        let option_value = context_bytes(context_value);

        if let Err(error) = request
            .bindings()
            .set_option(request.raw(), WINHTTP_OPTION_CONTEXT_VALUE, &option_value)
        {
            drop(request);
            drop(raw_owner);
            return Err(error);
        }

        raw_owner.release();

        Ok(RequestGuard {
            request: Some(request),
            context,
        })
    }
}

impl fmt::Debug for RequestSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestSetup")
            .field("request", &self.request)
            .field("context", &self.context)
            .finish()
    }
}

/// Guards the raw context pointer during the installation handoff.
///
/// Extracting the pooled box is necessary to obtain the stable pointer WinHTTP
/// stores. This guard reconstructs that box on every pre-installation exit; it
/// is explicitly released only after WinHTTP accepts the context value.
struct RawContextOwner {
    context: Option<NonNull<RequestContext>>,
}

impl RawContextOwner {
    const fn new(context: NonNull<RequestContext>) -> Self {
        Self { context: Some(context) }
    }

    fn release(&mut self) {
        self.context = None;
    }
}

impl Drop for RawContextOwner {
    fn drop(&mut self) {
        let Some(context) = self.context.take() else {
            return;
        };

        // SAFETY: this guard uniquely owns the exact pointer returned by
        // plurality::Box::into_raw and reconstructs it only on installation
        // failure, before WinHTTP takes callback ownership.
        drop(unsafe { plurality::Box::<RequestContext>::from_raw(context) });
    }
}

#[derive(Debug)]
/// Owns the close authority for one installed WinHTTP request handle.
///
/// Dropping the guard closes the request exactly once and thereby initiates
/// cancellation of any pending operation. It does not own the context
/// allocation: the final `HANDLE_CLOSING` callback reclaims that allocation and
/// releases the retained connect and session parents.
///
/// Mutable access to this guard is required to submit an operation. The future
/// then owns the request handle itself, leaving this guard unable to submit
/// again even if the future is forgotten. Completion restores the handle only
/// after destroying the receiver endpoint; cancellation destroys the receiver
/// first and then closes the handle.
pub(crate) struct RequestGuard {
    request: Option<RequestHandle>,
    context: NonNull<RequestContext>,
}

impl RequestGuard {
    pub(crate) fn raw(&self) -> RawHandle {
        self.request
            .as_ref()
            .expect("an unfinished OperationFuture prevents RequestGuard reuse")
            .raw()
    }

    #[cfg(test)]
    pub(crate) fn context_ptr(&self) -> *mut RequestContext {
        self.context.as_ptr()
    }

    pub(crate) fn context_value(&self) -> usize {
        self.context.as_ptr().expose_provenance()
    }

    pub(crate) fn cold_connect_state(&self) -> ColdConnectState {
        let _request = self
            .request
            .as_ref()
            .expect("an unfinished OperationFuture prevents RequestGuard context access");
        // SAFETY: the guard remains alive, so callback ownership keeps the
        // installed context valid while it owns the request handle.
        unsafe { self.context.as_ref() }.cold_connect_state()
    }

    pub(crate) fn submit(
        &mut self,
        kind: OperationKind,
        buffer: OperationBuffer,
        submit: impl FnOnce(RawHandle, usize) -> WinHttpResult<()>,
    ) -> OperationFuture<'_> {
        let request = self
            .request
            .take()
            .expect("cancelling or forgetting an OperationFuture prevents RequestGuard reuse");
        let raw = request.raw();

        // SAFETY: the context was installed from stable pooled storage. The
        // returned future leaves the request-handle slot empty until its
        // receiver endpoint has been destroyed.
        let context = unsafe { self.context.as_ref() };
        // SAFETY: the pooled context has a stable address until
        // HANDLE_CLOSING reclaims it.
        let context = unsafe { Pin::new_unchecked(context) };
        // SAFETY: the context remains pinned while both embedded-event
        // endpoints exist, as described above.
        let receiver = unsafe { context.arm(kind, buffer) };

        let submit_result = submit(raw, self.context_value());

        if let Err(error) = submit_result {
            // SAFETY: the local request handle keeps the context valid. The
            // operation kind atomically wins only if no inline callback
            // already consumed the operation, so synchronous failure cannot
            // double-complete it. No later operation can begin before this
            // method returns because the guard's handle slot remains empty.
            if let Some(active) = unsafe { self.context.as_ref() }.take_kind(kind) {
                active.completion.send(CompletionResult::error(error, active.buffer));
            }
        }

        OperationFuture {
            receiver: ManuallyDrop::new(receiver),
            receiver_live: true,
            request: Some(request),
            context: self.context_value(),
            request_slot: &mut self.request,
        }
    }

    fn close(&mut self) {
        drop(self.request.take());
    }
}

// SAFETY: moving the guard transfers the sole request-handle close authority.
// The context pointer remains valid independently through HANDLE_CLOSING.
unsafe impl Send for RequestGuard {}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Debug)]
/// Owns one request handle while awaiting its callback completion.
///
/// Moving the request handle into this future is the safe-code proof that one
/// request has at most one asynchronous operation outstanding. On completion,
/// the receiver endpoint is destroyed before the handle returns to the guard.
/// On cancellation, the receiver is destroyed before the owned handle closes,
/// allowing `HANDLE_CLOSING` to reclaim the embedded event storage safely.
/// Forgetting the future leaks the handle but leaves the guard unusable.
pub(crate) struct OperationFuture<'guard> {
    receiver: ManuallyDrop<RawReceiver<CompletionResult>>,
    receiver_live: bool,
    request: Option<RequestHandle>,
    context: usize,
    request_slot: &'guard mut Option<RequestHandle>,
}

impl Future for OperationFuture<'_> {
    type Output = std::result::Result<CompletionResult, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: this mutable reference is used only to pin-project the
        // receiver and update unpinned ownership fields; the receiver is not
        // moved.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(this.receiver_live, "OperationFuture cannot be polled after completion");
        // SAFETY: the receiver remains in place for the lifetime of this pinned
        // OperationFuture.
        let result = unsafe { Pin::new_unchecked(&mut *this.receiver) }.poll(cx);
        if result.is_ready() {
            // SAFETY: the receiver is pinned in place and will not be accessed
            // again after receiver_live is cleared.
            unsafe {
                ManuallyDrop::drop(&mut this.receiver);
            }
            this.receiver_live = false;

            let request = this.request.take().expect("a completed OperationFuture retains its request handle");
            debug_assert!(this.request_slot.is_none());
            *this.request_slot = Some(request);
        }

        result
    }
}

impl OperationFuture<'_> {
    fn cold_connect_state(&self) -> ColdConnectState {
        let _request = self
            .request
            .as_ref()
            .expect("cold-connect state is available only while an operation is pending");
        let context = NonNull::new(with_exposed_provenance_mut::<RequestContext>(self.context))
            .expect("OperationFuture retains the non-null installed request context");
        // SAFETY: this future owns the live request handle, so callback
        // ownership keeps the installed context valid.
        unsafe { context.as_ref() }.cold_connect_state()
    }
}

impl Drop for OperationFuture<'_> {
    fn drop(&mut self) {
        if self.receiver_live {
            // SAFETY: Drop runs exactly once, and receiver_live proves the
            // manually managed receiver has not already been destroyed.
            unsafe {
                ManuallyDrop::drop(&mut self.receiver);
            }
            self.receiver_live = false;
        }
        // The request field drops after this method. Pending cancellation
        // therefore disconnects the receiver before closing the native handle.
    }
}

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
    body_plan: RequestBodyPlan,
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
            return Err(invalid_request(RequestTranslationError::LegacyVersion(request.version())));
        }

        let mut headers = request.headers().clone();
        let body_plan = RequestBodyPlan::new(&mut headers, request.body().content_length()).map_err(invalid_request)?;
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
            body_plan,
            body_options,
        })
    }

    pub(crate) async fn execute(self, body_polled: &mut bool) -> Result<HttpResponse, RequestFailure> {
        let bindings = self.session.handle().bindings().clone();
        let connect = bindings
            .connect(self.session.handle().raw(), &self.request.host, self.request.port)
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;
        let connect = ConnectHandle::new(connect, bindings.clone());
        let request = bindings
            .open_request(
                connect.raw(),
                &self.request.method,
                &self.request.path,
                request_open_flags(self.request.secure, self.body_plan.automatic_chunking()),
            )
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;
        let request = RequestHandle::new(request, bindings.clone());

        apply_request_settings(&request, self.settings)
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;

        let mut guard = RequestSetup::new(request, connect, Arc::clone(&self.session), self.contexts)
            .install()
            .map_err(crate::error::WinHttpError::into_http_error)
            .map_err(RequestFailure::without_attribution)?;
        let connect_watch = self.clock.stopwatch();

        let send_result = send_request_headers(
            &mut guard,
            &bindings,
            &self.request.headers,
            self.body_plan.total_length(),
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
                if self.body_plan.automatic_chunking() {
                    writer.end_automatic_chunking().await?;
                }
            }

            {
                let receive = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |request, _context| {
                    bindings.receive_response(request)
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
    bindings: &Facade,
    headers: &U16CString,
    total_length: u32,
    clock: &Clock,
    timeout: Duration,
) -> Result<(), (HttpError, ColdConnectState)> {
    let outcome = {
        let send = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |request, context| {
            // SAFETY: the installed context is the exact pointer passed as
            // dwContext, and the UTF-16 header buffer remains alive until
            // the completion is awaited below.
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
            .ok_or_else(|| invalid_request(RequestTranslationError::MissingScheme))?;
        let secure = match scheme {
            "https" => true,
            "http" if matches!(filter, RequestFilter::HttpAndHttps) => false,
            "http" => return Err(invalid_request(RequestTranslationError::HttpDisallowed)),
            scheme => return Err(invalid_request(RequestTranslationError::UnsupportedScheme(scheme.to_owned()))),
        };
        let authority = uri
            .authority()
            .ok_or_else(|| invalid_request(RequestTranslationError::MissingAuthority))?;
        if authority.as_str().contains('@') {
            return Err(invalid_request(RequestTranslationError::UserInfo));
        }
        let host = uri
            .host()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| invalid_request(RequestTranslationError::MissingHost))?;
        let port = authority_port(authority, host, if secure { 443 } else { 80 }).map_err(invalid_request)?;
        let path = uri.path_and_query().map_or("/", |path| path.as_str());
        let path = if path.is_empty() { "/" } else { path };
        if !(path.starts_with('/') || path.starts_with('?')) {
            return Err(invalid_request(RequestTranslationError::InvalidPath(path.to_owned())));
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
        .ok_or_else(|| RequestTranslationError::InvalidAuthority(authority.as_str().to_owned()))?;
    if suffix.is_empty() {
        return Ok(default);
    }

    let explicit = suffix
        .strip_prefix(':')
        .ok_or_else(|| RequestTranslationError::InvalidAuthority(authority.as_str().to_owned()))?;
    if explicit.is_empty() {
        return Err(RequestTranslationError::EmptyPort);
    }
    if !explicit.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RequestTranslationError::NonNumericPort(explicit.to_owned()));
    }

    let port = explicit
        .parse::<u16>()
        .map_err(|_out_of_range| RequestTranslationError::OutOfRangePort(explicit.to_owned()))?;
    if port == 0 {
        return Err(RequestTranslationError::ZeroPort);
    }

    Ok(port)
}

#[derive(Clone, Copy)]
/// Collects native options applied to every newly opened request handle.
///
/// Protocol requirements and transport-specific TLS relaxations are computed
/// once during translation, then applied before the context is installed or an
/// asynchronous operation can begin.
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

fn set_dword(bindings: &Facade, request: RawHandle, option: u32, value: u32) -> WinHttpResult<()> {
    bindings.set_option(request, option, &dword_bytes(value))
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

fn query_error(error: QueryError) -> HttpError {
    match error {
        QueryError::WinHttp(error) => error.into_http_error(),
        QueryError::Conversion(error) => invalid_response(error),
    }
}

fn invalid_request(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), error_labels::INVALID_REQUEST)
}

fn invalid_response(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), error_labels::REQUEST_WINHTTP)
}

fn callback_protocol_error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    invalid_response(error)
}

#[derive(Debug)]
/// Identifies request metadata rejected before WinHTTP receives the request.
///
/// These failures define the validation boundary between generic `fetch`
/// requests and the URI, version, and endpoint forms accepted by this
/// transport. They are mapped to non-recoverable invalid-request errors.
enum RequestTranslationError {
    EmptyPort,
    HttpDisallowed,
    InvalidAuthority(String),
    InvalidPath(String),
    LegacyVersion(Version),
    MissingAuthority,
    MissingHost,
    MissingScheme,
    NonNumericPort(String),
    OutOfRangePort(String),
    UnsupportedScheme(String),
    UserInfo,
    ZeroPort,
}

impl fmt::Display for RequestTranslationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPort => f.write_str("the request URI has an empty explicit port; omit the colon or provide a port from 1 to 65535"),
            Self::HttpDisallowed => f.write_str("plain HTTP requests are disabled for this client"),
            Self::InvalidAuthority(authority) => write!(f, "the request URI has a malformed authority: '{authority}'"),
            Self::InvalidPath(path) => write!(f, "the request URI path must start with '/': {path}"),
            Self::LegacyVersion(version) => write!(f, "WinHTTP does not support requested HTTP version {version:?}"),
            Self::MissingAuthority => f.write_str("the request URI has no authority"),
            Self::MissingHost => f.write_str("the request URI has no host"),
            Self::MissingScheme => f.write_str("the request URI has no scheme"),
            Self::NonNumericPort(port) => {
                write!(
                    f,
                    "the request URI explicit port '{port}' is not decimal; provide a port from 1 to 65535"
                )
            }
            Self::OutOfRangePort(port) => {
                write!(f, "the request URI explicit port '{port}' is outside the valid range 1 to 65535")
            }
            Self::UnsupportedScheme(scheme) => write!(f, "the request URI uses unsupported scheme '{scheme}'"),
            Self::UserInfo => f.write_str("the request URI authority contains unsupported user information"),
            Self::ZeroPort => f.write_str("the request URI explicit port is zero; provide a port from 1 to 65535"),
        }
    }
}

impl std::error::Error for RequestTranslationError {}

#[derive(Debug)]
/// Identifies malformed response metadata returned by WinHTTP.
///
/// Header parsing preserves repeated values but requires a valid HTTP status
/// line, CRLF framing, ASCII field names, and `http`-compatible values. These
/// errors indicate an invalid transport response rather than an HTTP status
/// failure.
pub(crate) enum ResponseHeadersError {
    InvalidHeaderName(String),
    InvalidHeaderValue(String),
    InvalidStatusLine,
    MissingHeaderTerminator,
    MissingNameValueSeparator,
    NonAsciiHeaderName(u8),
    TrailingData,
}

impl fmt::Display for ResponseHeadersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeaderName(error) => write!(f, "WinHTTP returned an invalid response header name: {error}"),
            Self::InvalidHeaderValue(error) => write!(f, "WinHTTP returned an invalid response header value: {error}"),
            Self::InvalidStatusLine => f.write_str("WinHTTP returned a malformed response status line"),
            Self::MissingHeaderTerminator => f.write_str("WinHTTP returned a response header block without a terminating empty line"),
            Self::MissingNameValueSeparator => f.write_str("WinHTTP returned a response header without a ':' separator"),
            Self::NonAsciiHeaderName(byte) => write!(f, "WinHTTP returned a non-ASCII response header name byte: 0x{byte:02x}"),
            Self::TrailingData => f.write_str("WinHTTP returned data after the response header terminator"),
        }
    }
}

impl std::error::Error for ResponseHeadersError {}

fn parse_response_headers(raw: &[u8]) -> Result<HeaderMap, ResponseHeadersError> {
    let mut cursor = 0;
    let status_line = take_crlf_line(raw, &mut cursor).ok_or(ResponseHeadersError::InvalidStatusLine)?;
    if !status_line.starts_with(b"HTTP/") {
        return Err(ResponseHeadersError::InvalidStatusLine);
    }

    parse_header_fields(raw, cursor)
}

pub(crate) fn parse_response_trailers(raw: &[u8]) -> Result<HeaderMap, ResponseHeadersError> {
    parse_header_fields(raw, 0)
}

fn parse_header_fields(raw: &[u8], mut cursor: usize) -> Result<HeaderMap, ResponseHeadersError> {
    let mut headers = HeaderMap::new();

    loop {
        let line = take_crlf_line(raw, &mut cursor).ok_or(ResponseHeadersError::MissingHeaderTerminator)?;
        if line.is_empty() {
            if cursor != raw.len() {
                return Err(ResponseHeadersError::TrailingData);
            }
            return Ok(headers);
        }

        let separator = line
            .iter()
            .position(|byte| *byte == b':')
            .filter(|separator| *separator != 0)
            .ok_or(ResponseHeadersError::MissingNameValueSeparator)?;
        let name = header_name(&line[..separator])?;
        let value = header_value(&line[separator + 1..])?;
        headers.append(name, value);
    }
}

fn take_crlf_line<'a>(raw: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let remaining = raw.get(*cursor..)?;
    let end = remaining.windows(2).position(|pair| pair == b"\r\n")?;
    let start = *cursor;
    *cursor += end + 2;

    raw.get(start..start + end)
}

fn header_name(bytes: &[u8]) -> Result<HeaderName, ResponseHeadersError> {
    if let Some(byte) = bytes.iter().copied().find(|byte| !byte.is_ascii()) {
        return Err(ResponseHeadersError::NonAsciiHeaderName(byte));
    }

    HeaderName::from_bytes(bytes).map_err(|error| ResponseHeadersError::InvalidHeaderName(error.to_string()))
}

fn header_value(bytes: &[u8]) -> Result<HeaderValue, ResponseHeadersError> {
    let bytes = trim_optional_whitespace(bytes);

    HeaderValue::from_bytes(bytes).map_err(|error| ResponseHeadersError::InvalidHeaderValue(error.to_string()))
}

fn trim_optional_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|byte| matches!(*byte, b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(|byte| matches!(*byte, b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }

    bytes
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe};
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
        WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED,
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
    };

    use super::{
        ContextPool, OperationFuture, RawContextOwner, RequestDriver, RequestFailure, RequestGuard, RequestSettings, RequestSetup,
        RequestTranslationError, ResponseHeadersError, TranslatedRequest, send_request_headers,
    };
    use crate::WinHttpTlsConfig;
    use crate::bindings::{Facade, MockBindings};
    use crate::callback::dispatch_completion;
    use crate::context::{ColdConnectState, CompletionResult, OperationBuffer, OperationKind};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RawHandle, RequestHandle, SessionHandle};
    use crate::options::{
        WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH, WINHTTP_OPTION_CONTEXT_VALUE,
        WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
        WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_VERSION,
    };
    use crate::session::WinHttpSession;

    assert_impl_all!(ContextPool: Send, Sync, std::fmt::Debug, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestSetup: Send, std::fmt::Debug, UnwindSafe);
    assert_impl_all!(RequestGuard: Send, std::fmt::Debug);
    // The setup owns the context, whose actual UnsafeCell state prevents safe shared observation.
    assert_not_impl_any!(RequestSetup: RefUnwindSafe);
    // These pointer owners refer to the callback context's actual UnsafeCell state.
    assert_not_impl_any!(RawContextOwner: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RequestGuard: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RequestGuard: Sync);
    assert_impl_all!(OperationFuture<'static>: Send, std::fmt::Debug);
    // The future mutably borrows the request-handle slot.
    assert_not_impl_any!(OperationFuture<'static>: UnwindSafe);
    // Shared observation after an unwind cannot mutate the borrowed slot or receiver.
    assert_impl_all!(OperationFuture<'static>: RefUnwindSafe);
    assert_not_impl_any!(OperationFuture<'static>: Sync);
    // HttpError contains user-erased error state without unwind-safety bounds.
    assert_not_impl_any!(RequestFailure: UnwindSafe, RefUnwindSafe);
    // The driver holds a mutable body borrow whose erased implementation may expose partial mutation.
    assert_not_impl_any!(RequestDriver<'static, 'static>: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(TranslatedRequest: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestSettings: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestTranslationError: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ResponseHeadersError: UnwindSafe, RefUnwindSafe);

    const SESSION: usize = 1;
    const CONNECT: usize = 2;
    const REQUEST: usize = 3;

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
        let expected_headers = crate::options::headers_to_utf16(request.headers()).unwrap().as_slice().to_vec();
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
        assert!(response.body().is_empty());
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
        assert!(response.body().is_empty());
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
            .connection_idle_timeout(Duration::from_secs(1))
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
            assert!(error.to_string().contains("does not support requested HTTP version"));
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
        assert_eq!(result.unwrap_err().label(), "invalid_request");
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

        let context = std::ptr::with_exposed_provenance_mut(record.installed_context.load(Ordering::SeqCst));
        let mut written = 0_u32;
        record.end_write_completions.fetch_add(1, Ordering::SeqCst);
        complete(
            context,
            WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
            (&raw mut written).cast(),
            status_info_len::<u32>(),
        );

        let Poll::Ready(response) = future.as_mut().poll(&mut cx) else {
            panic!("response becomes ready after the terminal write completes");
        };
        let response = response.unwrap();
        assert_eq!(record.receive_calls.load(Ordering::SeqCst), 1);

        drop(future);
        drop(response);
        drop((request, options, tls));
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        closing(context);
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

        let context = std::ptr::with_exposed_provenance_mut(record.installed_context.load(Ordering::SeqCst));
        closing(context);

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
            cold_connect_state: ColdConnectState::Connecting,
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
        let context = std::ptr::with_exposed_provenance_mut(record.installed_context.load(Ordering::SeqCst));
        closing(context);

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
        let context = std::ptr::with_exposed_provenance_mut(record.installed_context.load(Ordering::SeqCst));
        closing(context);

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn request_body_timeout_reaches_the_response_body_and_closes_a_pending_read() {
        let mut request = request(Method::GET, "https://example.com/");
        request.extensions_mut().insert(BodyTimeout::new(Duration::from_secs(1)));
        let config = LifecycleConfig {
            defer_data_available: true,
            ..LifecycleConfig::default()
        };
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
        let response = futures::executor::block_on(driver.execute(&mut body_polled)).unwrap();

        assert_eq!(record.request_closes.load(Ordering::SeqCst), 0);
        let error = futures::executor::block_on(response.into_body().into_bytes()).unwrap_err();
        assert_eq!(error.label(), "body_timeout");
        assert_eq!(record.data_available_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);

        let context = std::ptr::with_exposed_provenance_mut(record.installed_context.load(Ordering::SeqCst));
        closing(context);
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

        for (raw_headers, message) in [
            (b"HTTP/1.1 200 OK\r\nmissing-colon\r\n\r\n".to_vec(), "without a ':' separator"),
            (
                [b"HTTP/1.1 200 OK\r\n".as_slice(), &[0x80], b": value\r\n\r\n"].concat(),
                "non-ASCII response header name",
            ),
            (
                b"HTTP/1.1 200 OK\r\nx-invalid: contains\nnewline\r\n\r\n".to_vec(),
                "invalid response header value",
            ),
        ] {
            let malformed = LifecycleConfig {
                raw_headers,
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
            assert!(error.to_string().contains(message), "{error}");
            assert_lifecycle_closed(&record);
        }

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
        cold_connect_state: ColdConnectState,
        completed_writes: Arc<AtomicUsize>,
        defer_write_completion: bool,
        max_write_completion: Option<u32>,
        defer_data_available: bool,
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
                cold_connect_state: ColdConnectState::Unobserved,
                completed_writes: Arc::new(AtomicUsize::new(0)),
                defer_write_completion: false,
                max_write_completion: None,
                defer_data_available: false,
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
            Ok(driver) => futures::executor::block_on(driver.execute(&mut body_polled)).map_err(RequestFailure::into_error),
            Err(error) => Err(error),
        };
        let result = result.map(|response| {
            let (parts, body) = response.into_parts();
            drop(body);
            fetch::HttpResponse::from_parts(parts, body_builder.empty())
        });
        drop((request, options, tls));

        let context = record.installed_context.load(Ordering::SeqCst);
        if context != 0 {
            assert_eq!(
                record.request_closes.load(Ordering::SeqCst),
                1,
                "the request guard closes before HANDLE_CLOSING"
            );
            closing(std::ptr::with_exposed_provenance_mut(context));
        }

        drop(session);
        assert_eq!(contexts.lock().unwrap().len(), 0);

        (result, record)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the lifecycle mock keeps one complete WinHTTP script visible in one place"
    )]
    fn lifecycle_bindings(config: Arc<LifecycleConfig>, record: Arc<LifecycleRecord>) -> Facade {
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
                let context = usize::from_ne_bytes(value.try_into().unwrap());
                option_record.installed_context.store(context, Ordering::SeqCst);
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

            match send_config.cold_connect_state {
                ColdConnectState::Unobserved => {}
                ColdConnectState::Connecting => complete(
                    std::ptr::with_exposed_provenance_mut(context),
                    WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
                    std::ptr::null_mut(),
                    0,
                ),
                ColdConnectState::Connected => {
                    complete(
                        std::ptr::with_exposed_provenance_mut(context),
                        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
                        std::ptr::null_mut(),
                        0,
                    );
                    complete(
                        std::ptr::with_exposed_provenance_mut(context),
                        WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER,
                        std::ptr::null_mut(),
                        0,
                    );
                }
            }

            if send_config.failure == Some(LifecycleFailure::SendCallback) {
                complete_request_error(context, 12030);
            } else if send_config.defer_send_completion {
                return Ok(());
            } else if send_config.complete_send_on_foreign_thread {
                thread::spawn(move || {
                    complete(
                        std::ptr::with_exposed_provenance_mut(context),
                        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                        std::ptr::null_mut(),
                        0,
                    );
                })
                .join()
                .unwrap();
            } else {
                complete(
                    std::ptr::with_exposed_provenance_mut(context),
                    WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                    std::ptr::null_mut(),
                    0,
                );
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

            let context = write_record.installed_context.load(Ordering::SeqCst);
            if write_config.failure
                == Some(if ending_chunking {
                    LifecycleFailure::EndWriteCallback
                } else {
                    LifecycleFailure::WriteCallback
                })
            {
                complete_request_error(context, 12030);
            } else {
                let mut written = write_config.max_write_completion.map_or(len, |maximum| maximum.min(len));
                if ending_chunking {
                    write_record.end_write_completions.fetch_add(1, Ordering::SeqCst);
                } else {
                    write_config.completed_writes.fetch_add(1, Ordering::SeqCst);
                }
                complete(
                    std::ptr::with_exposed_provenance_mut(context),
                    WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                    (&raw mut written).cast(),
                    status_info_len::<u32>(),
                );
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

            let context = receive_record.installed_context.load(Ordering::SeqCst);
            if receive_config.failure == Some(LifecycleFailure::ReceiveCallback) {
                complete_request_error(context, 12175);
            } else {
                complete(
                    std::ptr::with_exposed_provenance_mut(context),
                    WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
                    std::ptr::null_mut(),
                    0,
                );
            }
            Ok(())
        });

        let data_available_config = Arc::clone(&config);
        let data_available_record = Arc::clone(&record);
        bindings.expect_query_data_available().returning(move |_| {
            data_available_record.data_available_calls.fetch_add(1, Ordering::SeqCst);
            if data_available_config.defer_data_available {
                return Ok(());
            }

            let mut available = 0_u32;
            let context = data_available_record.installed_context.load(Ordering::SeqCst);
            complete(
                std::ptr::with_exposed_provenance_mut(context),
                WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                (&raw mut available).cast(),
                status_info_len::<u32>(),
            );
            Ok(())
        });

        let read_record = Arc::clone(&record);
        bindings.expect_read_data().returning(move |_, _, _| {
            let context = read_record.installed_context.load(Ordering::SeqCst);
            complete(
                std::ptr::with_exposed_provenance_mut(context),
                WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
                std::ptr::null_mut(),
                0,
            );
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

        Facade::mock(Arc::new(bindings))
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

    fn complete_request_error(context: usize, code: u32) {
        let mut result = WINHTTP_ASYNC_RESULT {
            dwResult: 0,
            dwError: code,
        };
        complete(
            std::ptr::with_exposed_provenance_mut(context),
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            (&raw mut result).cast(),
            status_info_len::<WINHTTP_ASYNC_RESULT>(),
        );
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
    fn context_option_failure_returns_context_and_closes_each_handle_once() {
        let (facade, closes) = bindings(true);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let setup = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        );

        assert_eq!(contexts.lock().unwrap().len(), 1);
        let error = setup.install().unwrap_err();

        assert_eq!(error.code(), 12019);
        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 0);

        drop(session);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn installed_context_is_reclaimed_only_by_handle_closing() {
        let (facade, closes) = bindings(false);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .unwrap();
        let context = guard.context_ptr();

        drop(session);
        drop(guard);

        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
        assert_eq!(closes.session.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        closing(context);

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn inline_completion_before_submit_returns_is_observed() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, context_value| {
            assert_eq!(context_value, context.expose_provenance());
            assert_eq!(context_value, closes.context.load(Ordering::SeqCst));
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
            Ok(())
        });

        assert!(matches!(
            futures::executor::block_on(future).unwrap(),
            CompletionResult::SendRequestComplete
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn foreign_thread_completion_wakes_send_future() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, context_value| {
            thread::spawn(move || {
                let context = std::ptr::with_exposed_provenance_mut(context_value);
                complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
            });
            Ok(())
        });

        assert!(matches!(
            futures::executor::block_on(future).unwrap(),
            CompletionResult::HeadersAvailable
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn synchronous_submit_failure_completes_once() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
            Err(WinHttpError::new(12029, WinHttpOperation::SendRequest))
        });

        let CompletionResult::Error { error, _buffer: buffer } = futures::executor::block_on(future).unwrap() else {
            panic!("synchronous failure must produce an error completion");
        };
        assert_eq!(error.code(), 12029);
        assert!(buffer.is_none());
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn sequential_success_statuses_decode_and_return_buffers() {
        let (mut guard, context, contexts, session, closes) = installed();

        let send = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
            Ok(())
        });
        assert!(matches!(
            futures::executor::block_on(send).unwrap(),
            CompletionResult::SendRequestComplete
        ));

        let headers = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, _| {
            complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
            Ok(())
        });
        assert!(matches!(
            futures::executor::block_on(headers).unwrap(),
            CompletionResult::HeadersAvailable
        ));

        let mut available = 17_u32;
        let data = guard.submit(OperationKind::DataAvailable, OperationBuffer::none(), |_, _| {
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                (&raw mut available).cast(),
                status_info_len::<u32>(),
            );
            Ok(())
        });
        assert!(matches!(
            futures::executor::block_on(data).unwrap(),
            CompletionResult::DataAvailable(17)
        ));

        let mut read_memory = [0_u8; 8];
        let read_address = read_memory.as_mut_ptr().addr();
        let read = guard.submit(
            OperationKind::Read,
            OperationBuffer::read(GlobalPool::new().reserve(8), read_address, 8),
            |_, _| {
                complete(context, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, read_memory.as_mut_ptr().cast(), 5);
                Ok(())
            },
        );
        assert!(matches!(
            futures::executor::block_on(read).unwrap(),
            CompletionResult::ReadComplete { len: 5, .. }
        ));

        let mut written = 4_u32;
        let write = guard.submit(
            OperationKind::Write,
            OperationBuffer::write(BytesView::copied_from_slice(b"data", &GlobalPool::new()), 4),
            |_, _| {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                    (&raw mut written).cast(),
                    status_info_len::<u32>(),
                );
                Ok(())
            },
        );
        assert!(matches!(
            futures::executor::block_on(write).unwrap(),
            CompletionResult::WriteComplete { len: 4, .. }
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn cancellation_retains_read_and_write_operations_until_handle_closing() {
        for buffer in [
            OperationBuffer::read(GlobalPool::new().reserve(8), NonNull::<u8>::dangling().as_ptr().addr(), 8),
            OperationBuffer::write(BytesView::copied_from_slice(b"outstanding", &GlobalPool::new()), 11),
        ] {
            let (mut guard, context, contexts, session, closes) = installed();
            let kind = match buffer {
                OperationBuffer::Read { .. } => OperationKind::Read,
                OperationBuffer::Write { .. } => OperationKind::Write,
                OperationBuffer::None => unreachable!("test buffers are read or write"),
            };
            let future = guard.submit(kind, buffer, |_, _| Ok(()));

            assert_eq!(contexts.lock().unwrap().len(), 1);
            drop(future);
            assert!(guard.request.is_none());
            assert_eq!(closes.request.load(Ordering::SeqCst), 1);
            let rejected = std::panic::catch_unwind(AssertUnwindSafe(|| {
                drop(guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(())));
            }));
            assert!(rejected.is_err());
            drop(session);
            drop(guard);

            assert_eq!(closes.request.load(Ordering::SeqCst), 1);
            assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
            assert_eq!(closes.session.load(Ordering::SeqCst), 0);
            assert_eq!(contexts.lock().unwrap().len(), 1);

            closing(context);

            assert_eq!(contexts.lock().unwrap().len(), 0);
            assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
            assert_eq!(closes.session.load(Ordering::SeqCst), 1);
        }
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
                let context = std::ptr::with_exposed_provenance_mut(request_counts.context.load(Ordering::SeqCst));
                closing(context);
                Ok(())
            });
        let request_facade = Facade::mock(Arc::new(request_bindings));

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
        let parent_facade = Facade::mock(Arc::new(parent_bindings));

        let contexts = ContextPool::new(Pool::new());
        let session = session(parent_facade.clone());
        let mut guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), request_facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), parent_facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .unwrap();
        let context = guard.context_ptr();
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
        complete(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
        control.advance(Duration::from_secs(1));
        let Poll::Ready(Err((error, state))) = future.as_mut().poll(&mut cx) else {
            panic!("the connect timeout must win the pending send");
        };
        assert_eq!(state, ColdConnectState::Connecting);
        assert_eq!(error.label(), "response_timeout");

        drop(future);
        assert!(guard.request.is_none());
        drop(session);
        drop(guard);

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn request_error_classification_uses_error_code_in_both_secure_status_orders() {
        for secure_first in [true, false] {
            let (mut guard, context, contexts, session, closes) = installed();
            let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()));
            let mut secure_flags = 0x20_u32;
            let mut async_result = WINHTTP_ASYNC_RESULT {
                dwResult: 7,
                dwError: 12175,
            };

            if secure_first {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
                    (&raw mut secure_flags).cast(),
                    status_info_len::<u32>(),
                );
            }
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                (&raw mut async_result).cast(),
                status_info_len::<WINHTTP_ASYNC_RESULT>(),
            );

            let CompletionResult::Error { error, _buffer: buffer } = futures::executor::block_on(future).unwrap() else {
                panic!("request error must produce an error completion");
            };
            assert_eq!(error.code(), 12175);
            assert!(buffer.is_none());
            assert_eq!(error.secure_failure_flags(), secure_first.then_some(0x20));

            if !secure_first {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
                    (&raw mut secure_flags).cast(),
                    status_info_len::<u32>(),
                );
                // SAFETY: the guard is still alive, so callback ownership keeps
                // the installed context valid.
                assert_eq!(unsafe { &*context }.secure_failure_flags(), Some(0x20));
            }

            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn duplicate_and_late_completions_cannot_send_twice() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()));

        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        assert!(matches!(
            futures::executor::block_on(future).unwrap(),
            CompletionResult::SendRequestComplete
        ));
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);

        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn malformed_status_info_is_not_dereferenced() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()));
        let mut bytes = [0_u8; size_of::<WINHTTP_ASYNC_RESULT>() + align_of::<WINHTTP_ASYNC_RESULT>()];
        let offset = (0..align_of::<WINHTTP_ASYNC_RESULT>())
            .find(|offset| !(bytes.as_ptr().addr() + offset).is_multiple_of(align_of::<WINHTTP_ASYNC_RESULT>()))
            .unwrap();
        // SAFETY: offset is less than the type alignment, and the byte array has
        // that much padding beyond the status structure's required length.
        let unaligned = unsafe { bytes.as_mut_ptr().add(offset) }.cast();

        complete(
            context,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            unaligned,
            status_info_len::<WINHTTP_ASYNC_RESULT>(),
        );

        assert!(matches!(
            futures::executor::block_on(future).unwrap(),
            CompletionResult::InvalidStatusInfo {
                status: WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                ..
            }
        ));

        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn connect_attribution_is_bounded_and_handle_created_is_inert() {
        let (guard, context, contexts, session, closes) = installed();

        complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, std::ptr::null_mut(), 0);
        // SAFETY: the guard is alive and therefore the context is valid.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Unobserved);

        complete(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
        // SAFETY: the guard is alive and therefore the context is valid.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Connecting);

        complete(context, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, std::ptr::null_mut(), 0);
        // SAFETY: the guard is alive and therefore the context is valid.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Connected);

        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn installed_context_outlives_its_pool_handle() {
        let (facade, closes) = bindings(false);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .unwrap();
        let context = guard.context_ptr();

        drop(contexts);

        drop(session);
        drop(guard);
        closing(context);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    fn installed() -> (
        RequestGuard,
        *mut crate::context::RequestContext,
        ContextPool,
        Arc<WinHttpSession>,
        Arc<CloseCounts>,
    ) {
        let (facade, closes) = bindings(false);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .unwrap();
        let context = guard.context_ptr();

        (guard, context, contexts, session, closes)
    }

    fn finish(
        guard: RequestGuard,
        context: *mut crate::context::RequestContext,
        contexts: &ContextPool,
        session: Arc<WinHttpSession>,
        closes: &CloseCounts,
    ) {
        drop(session);
        drop(guard);
        closing(context);

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    fn complete(context: *mut crate::context::RequestContext, status: u32, info: *mut c_void, len: u32) {
        // SAFETY: every test passes a live installed context and preserves each
        // status-info object for the duration of the synchronous dispatch.
        unsafe {
            dispatch_completion(context, status, info, len);
        }
    }

    fn status_info_len<T>() -> u32 {
        u32::try_from(size_of::<T>()).unwrap()
    }

    fn closing(context: *mut crate::context::RequestContext) {
        complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
    }

    fn session(facade: Facade) -> Arc<WinHttpSession> {
        Arc::new(WinHttpSession::from_handle(SessionHandle::new(raw_handle(SESSION), facade)))
    }

    fn bindings(fail_context_option: bool) -> (Facade, Arc<CloseCounts>) {
        let closes = Arc::new(CloseCounts::default());
        let mut bindings = MockBindings::new();
        let context_counts = Arc::clone(&closes);
        bindings
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
                if fail_context_option {
                    Err(WinHttpError::new(12019, WinHttpOperation::SetOption))
                } else {
                    Ok(())
                }
            });
        let close_counts = Arc::clone(&closes);
        bindings.expect_close_handle().times(3).returning(move |handle| {
            match handle.as_ptr().addr() {
                SESSION => &close_counts.session,
                CONNECT => &close_counts.connect,
                REQUEST => &close_counts.request,
                _ => panic!("unexpected test handle"),
            }
            .fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        (Facade::mock(Arc::new(bindings)), closes)
    }

    #[derive(Default)]
    struct CloseCounts {
        context: AtomicUsize,
        session: AtomicUsize,
        connect: AtomicUsize,
        request: AtomicUsize,
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).unwrap()
    }
}
