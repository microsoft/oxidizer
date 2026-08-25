// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

use fetch::options::{ConnectionKeepAlive, TransportOptions};
use widestring::U16CString;
use windows::Win32::Networking::WinHttp::{
    WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE, WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE, WINHTTP_CALLBACK_FLAG_READ_COMPLETE,
    WINHTTP_CALLBACK_FLAG_REQUEST_ERROR, WINHTTP_CALLBACK_FLAG_SECURE_FAILURE, WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE,
    WINHTTP_CALLBACK_FLAG_WRITE_COMPLETE, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
    WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS,
    WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
};

use crate::bindings::{
    Bindings as _, BindingsFacade, WINHTTP_FLAG_ASYNC, WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT, WINHTTP_OPTION_HTTP2_KEEPALIVE,
    WINHTTP_OPTION_HTTP3_KEEPALIVE,
};
use crate::callback::status_callback;
use crate::convert::{UNLIMITED_TIMEOUT, connection_idle_timeout_millis, dword_bytes, http2_keep_alive_millis, http3_keep_alive_millis};
use crate::error::WinHttpError;
use crate::handle::SessionHandle;

const USER_AGENT: &str = "fetch_winhttp";
const TRUE_BYTES: [u8; size_of::<i32>()] = 1_i32.to_ne_bytes();

/// Notifications the session subscribes to.
///
/// The completion set is narrower than the native
/// `WINHTTP_CALLBACK_FLAG_ALL_COMPLETIONS`, which also covers the
/// proxy-resolution completions. The transport resolves proxies through
/// automatic detection configured on the session rather than through
/// `WinHttpGetProxyForUrlEx` or `WinHttpGetProxySettingsEx`, so those
/// notifications can never arrive and the callback has no handling for them.
/// Subscribing to them would describe a protocol the transport does not
/// implement.
///
/// Every operand below is a distinct bit, so `|` and `^` compute the same
/// value here and a mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
pub(crate) fn session_notification_flags() -> u32 {
    let dispatched_completions = WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
        | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
        | WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE
        | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
        | WINHTTP_CALLBACK_FLAG_WRITE_COMPLETE
        | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR;
    let handles = WINHTTP_CALLBACK_STATUS_HANDLE_CREATED | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;
    let connect_to_server = WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER | WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER;

    dispatched_completions | WINHTTP_CALLBACK_FLAG_SECURE_FAILURE | handles | connect_to_server
}

/// Count of `set_option` calls one session performs with keep-alive disabled.
///
/// Tests elsewhere in the crate assert only that a session was constructed, not
/// what it configured, so they count calls against this instead of repeating a
/// bare number. Adding or removing a session option therefore stays a one-line
/// co-edit here rather than a hunt through unrelated modules.
#[cfg(test)]
pub(crate) const SESSION_OPTIONS_WITHOUT_KEEP_ALIVE: usize = 3;

#[derive(Debug)]
/// Defines the OS connection-pool boundary for one transport instance.
///
/// The custom transport factory creates one session for each materialized core
/// and pool slot. The session configures automatic proxy discovery, native
/// timeout policy, keep-alive behavior, and the callback inherited by all child
/// request handles. Disabling WinHTTP global pooling keeps independently built
/// clients from sharing connections through process-wide OS state.
///
/// The owner is shared by requests through `Arc` and retained in each installed
/// request context until final `HANDLE_CLOSING`, ensuring the session outlives
/// every callback that depends on it.
pub(crate) struct WinHttpSession {
    handle: SessionHandle,
}

impl WinHttpSession {
    pub(crate) fn new(bindings: BindingsFacade, transport_options: &TransportOptions) -> Result<Self, SessionInitializationFailure> {
        let user_agent = U16CString::from_str(USER_AGENT).expect("the static WinHTTP user agent contains no NUL characters");
        // SAFETY: open requires the trait-level invariants, WINHTTP_FLAG_ASYNC
        // among the flags, and a returned handle that immediately acquires one
        // exactly-once owner. The flag is passed literally; a successful handle
        // is moved into SessionHandle on the next statement, which is the sole
        // closer; and the trait-level invariants are vacuous here because no
        // handle, context, operation, or lent buffer exists yet.
        let raw = unsafe { bindings.open(&user_agent, WINHTTP_FLAG_ASYNC) }
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::Open, error))?;
        let handle = SessionHandle::new(raw, bindings);

        // SAFETY: set_timeouts requires a live session that cannot close during
        // the call, plus the trait-level invariants. The local SessionHandle is
        // the only owner of a just-opened session, so no other thread can close
        // it; the values are the native millisecond representation the option
        // takes. The session was opened with WINHTTP_FLAG_ASYNC and still has
        // no child handle, context, operation, or lent buffer, so the
        // trait-level invariants hold.
        unsafe {
            handle.bindings().set_timeouts(
                handle.raw(),
                UNLIMITED_TIMEOUT,
                UNLIMITED_TIMEOUT,
                UNLIMITED_TIMEOUT,
                UNLIMITED_TIMEOUT,
            )
        }
        .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::SetTimeouts, error))?;
        // SAFETY: set_option requires a live handle, the native representation
        // for the option, a valid lifecycle stage, and the trait-level
        // invariants. The local SessionHandle solely owns a live session,
        // TRUE_BYTES is the native BOOL this option takes, and a session accepts
        // it before it has children, which is this stage. The trait-level
        // invariants hold because the session was opened with
        // WINHTTP_FLAG_ASYNC and has no child handle, context, operation, or
        // lent buffer yet.
        unsafe {
            handle
                .bindings()
                .set_option(handle.raw(), WINHTTP_OPTION_DISABLE_GLOBAL_POOLING, &TRUE_BYTES)
        }
        .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::DisableGlobalPooling, error))?;
        // SAFETY: as for the option above, with dword_bytes supplying the
        // native DWORD representation this option takes.
        unsafe {
            handle.bindings().set_option(
                handle.raw(),
                WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT,
                &dword_bytes(connection_idle_timeout_millis(
                    &transport_options.connection_pool.connection_idle_timeout,
                )),
            )
        }
        .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::ConnectionIdleTimeout, error))?;
        // This option promises WinHTTP that the status callback returns
        // promptly and never waits for a subsequent WinHTTP call, which lets
        // WinHTTP dispatch notifications on threads it cannot afford to stall.
        // The callback honors that: it only moves an already-prepared
        // completion payload across a channel, and the one WinHTTP call it can
        // reach, closing the parent handles a reclaimed context released, only
        // starts their teardown rather than awaiting an operation.
        // Ref: callback.rs, close_context.
        //
        // SAFETY: as for the option above, with TRUE_BYTES supplying the native
        // BOOL representation this option takes.
        unsafe {
            handle
                .bindings()
                .set_option(handle.raw(), WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS, &TRUE_BYTES)
        }
        .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::AssuredNonBlockingCallbacks, error))?;
        if let Some(interval) = Self::keep_alive_interval(&transport_options.connection_keep_alive) {
            // SAFETY: as for the options above, with dword_bytes supplying the
            // native DWORD representation this option takes.
            unsafe {
                handle.bindings().set_option(
                    handle.raw(),
                    WINHTTP_OPTION_HTTP2_KEEPALIVE,
                    &dword_bytes(http2_keep_alive_millis(interval)),
                )
            }
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::Http2KeepAlive, error))?;
            // SAFETY: as for the options above, with dword_bytes supplying the
            // native DWORD representation this option takes.
            unsafe {
                handle.bindings().set_option(
                    handle.raw(),
                    WINHTTP_OPTION_HTTP3_KEEPALIVE,
                    &dword_bytes(http3_keep_alive_millis(interval)),
                )
            }
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::Http3KeepAlive, error))?;
        }
        // SAFETY: set_status_callback requires a live session, a callback that
        // is present and stays valid for the session lifetime, notification
        // flags enabling every status the callback protocol requires, and
        // registration before every child request that relies on that protocol,
        // plus the trait-level invariants. The local SessionHandle solely owns
        // a live session; status_callback is a static function item, so it
        // outlives the session; notification_mask_enables_every_dispatched_status
        // proves session_notification_flags() covers every consumed status; and
        // the session is returned to the caller only after this call, so no
        // child request can exist yet. That same absence of children,
        // contexts, operations, and lent buffers discharges the trait-level
        // invariants for a session opened with WINHTTP_FLAG_ASYNC.
        unsafe {
            handle
                .bindings()
                .set_status_callback(handle.raw(), Some(status_callback), session_notification_flags())
        }
        .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::SetStatusCallback, error))?;

        Ok(Self { handle })
    }

    fn keep_alive_interval(keep_alive: &ConnectionKeepAlive) -> Option<std::time::Duration> {
        match keep_alive {
            ConnectionKeepAlive::Disabled => None,
            ConnectionKeepAlive::ActiveConnections { interval, .. } | ConnectionKeepAlive::ActiveAndIdleConnections { interval, .. } => {
                Some(*interval)
            }
        }
    }

    pub(crate) const fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    #[cfg(test)]
    pub(crate) const fn from_handle(handle: SessionHandle) -> Self {
        Self { handle }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Preserves why a transport instance could not initialize its session.
///
/// The custom transport factory is infallible, so this value is retained in a
/// permanently failed transport state. Each request then receives a fresh
/// `HttpError` derived from the original Win32 failure without opening connect
/// or request handles.
pub(crate) struct SessionInitializationFailure {
    operation: SessionInitializationOperation,
    error: WinHttpError,
}

impl SessionInitializationFailure {
    pub(crate) const fn new(operation: SessionInitializationOperation, error: WinHttpError) -> Self {
        Self { operation, error }
    }

    pub(crate) const fn code(&self) -> u32 {
        self.error.code()
    }

    pub(crate) const fn operation(&self) -> SessionInitializationOperation {
        self.operation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Identifies the required session setup step that failed.
///
/// Every listed step is part of the supported-platform contract; there is no
/// capability fallback. Recording the step gives the preserved Win32 error
/// enough context for diagnostics when the failed transport handles requests.
pub(crate) enum SessionInitializationOperation {
    Open,
    SetTimeouts,
    DisableGlobalPooling,
    ConnectionIdleTimeout,
    AssuredNonBlockingCallbacks,
    Http2KeepAlive,
    Http3KeepAlive,
    SetStatusCallback,
}

impl fmt::Display for SessionInitializationOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "opening the WinHTTP session",
            Self::SetTimeouts => "configuring WinHTTP session timeouts",
            Self::DisableGlobalPooling => "disabling WinHTTP global connection pooling",
            Self::ConnectionIdleTimeout => "configuring the WinHTTP connection idle timeout",
            Self::AssuredNonBlockingCallbacks => "enabling assured non-blocking WinHTTP callbacks",
            Self::Http2KeepAlive => "configuring WinHTTP HTTP/2 keep-alive",
            Self::Http3KeepAlive => "configuring WinHTTP HTTP/3 keep-alive",
            Self::SetStatusCallback => "registering the WinHTTP status callback",
        })
    }
}

impl fmt::Display for SessionInitializationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed with Win32 error {}", self.operation, self.error.code())
    }
}

impl Error for SessionInitializationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error as _;
    use std::ffi::c_void;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;
    use std::time::Duration;

    use fetch::options::{ConnectionIdleTimeout, ConnectionKeepAlive, TransportOptions};
    use mockall::Sequence;
    use static_assertions::assert_impl_all;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE, WINHTTP_CALLBACK_FLAG_GETPROXYSETTINGS_COMPLETE,
        WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
        WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
        WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS, WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
    };

    use super::{
        SessionInitializationFailure, SessionInitializationOperation, TRUE_BYTES, UNLIMITED_TIMEOUT, USER_AGENT, WinHttpSession,
        session_notification_flags,
    };
    use crate::bindings::{
        BindingsFacade, MockBindings, StatusCallback, WINHTTP_FLAG_ASYNC, WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT,
        WINHTTP_OPTION_HTTP2_KEEPALIVE, WINHTTP_OPTION_HTTP3_KEEPALIVE,
    };
    use crate::callback::status_callback;
    use crate::context::OperationKind;
    use crate::convert::{connection_idle_timeout_millis, dword_bytes};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::RawHandle;

    assert_impl_all!(WinHttpSession: Send, Sync, std::fmt::Debug, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        SessionInitializationFailure: Send, Sync, Clone, std::fmt::Debug, std::error::Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(SessionInitializationOperation: UnwindSafe, RefUnwindSafe);

    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
    /// One native call in the session setup sequence, in the order performed.
    ///
    /// The ordering is meaningful: [`SetupScript::reaches`] uses it to decide
    /// which calls a given failure point stops short of, so variants must stay
    /// declared in setup order.
    enum FailurePoint {
        Open,
        SetTimeouts,
        DisableGlobalPooling,
        ConnectionIdleTimeout,
        AssuredNonBlockingCallbacks,
        Http2KeepAlive,
        Http3KeepAlive,
        SetStatusCallback,
    }

    #[derive(Clone, Copy)]
    /// Describes the native call script one session construction should produce.
    struct SetupScript {
        failure: Option<FailurePoint>,
        idle_timeout_millis: u32,
        keep_alive: bool,
    }

    impl SetupScript {
        fn new() -> Self {
            Self {
                failure: None,
                idle_timeout_millis: default_idle_timeout_millis(),
                keep_alive: true,
            }
        }

        /// Whether construction reaches `step`.
        ///
        /// Setup is a straight-line sequence, so every step up to and including
        /// the failing one runs and every later step does not.
        fn reaches(self, step: FailurePoint) -> bool {
            self.failure.is_none_or(|point| point >= step)
        }

        fn result(self, step: FailurePoint) -> crate::error::Result<()> {
            if self.failure == Some(step) {
                Err(WinHttpError::new(error_code(step), operation(step)))
            } else {
                Ok(())
            }
        }
    }

    /// Millisecond encoding of `ConnectionIdleTimeout::default()`.
    ///
    /// Derived rather than written out so that a change to `fetch`'s default
    /// surfaces as a conversion result here instead of as an opaque mock
    /// argument mismatch.
    fn default_idle_timeout_millis() -> u32 {
        connection_idle_timeout_millis(&ConnectionIdleTimeout::default())
    }

    #[test]
    fn session_setup_succeeds_in_exact_order_with_expected_values() {
        let bindings = configured_bindings(SetupScript::new());

        let session = WinHttpSession::new(BindingsFacade::mock(Arc::new(bindings)), &active_transport_options()).unwrap();

        assert_eq!(session.handle().raw(), raw_handle());
        drop(session);
    }

    #[test]
    fn disabled_keep_alive_leaves_session_probe_options_unset() {
        let bindings = configured_bindings(SetupScript {
            keep_alive: false,
            ..SetupScript::new()
        });

        let session = WinHttpSession::new(BindingsFacade::mock(Arc::new(bindings)), &TransportOptions::default()).unwrap();

        drop(session);
    }

    #[test]
    fn unlimited_idle_timeout_requests_the_largest_representable_window() {
        let bindings = configured_bindings(SetupScript {
            idle_timeout_millis: u32::MAX,
            ..SetupScript::new()
        });
        let mut options = active_transport_options();
        options.connection_pool.connection_idle_timeout = ConnectionIdleTimeout::Unlimited;

        let session = WinHttpSession::new(BindingsFacade::mock(Arc::new(bindings)), &options).unwrap();

        drop(session);
    }

    #[test]
    fn idle_timeout_below_the_native_minimum_is_raised_to_it() {
        let bindings = configured_bindings(SetupScript {
            idle_timeout_millis: 5_000,
            ..SetupScript::new()
        });
        let mut options = active_transport_options();
        options.connection_pool.connection_idle_timeout = ConnectionIdleTimeout::Limited(Duration::from_millis(1));

        let session = WinHttpSession::new(BindingsFacade::mock(Arc::new(bindings)), &options).unwrap();

        drop(session);
    }

    /// Diagnostic, error, and handle statuses `callback::dispatch_completion`
    /// acts on outside the awaited-operation completions.
    ///
    /// Hand-maintained: the dispatch `match` in `callback.rs` names this
    /// constant as a required co-edit, because a status dispatched there but
    /// missing here would be silently uncovered by this test.
    const DISPATCHED_STATUSES: [(&str, u32); 6] = [
        ("REQUEST_ERROR", WINHTTP_CALLBACK_STATUS_REQUEST_ERROR),
        ("SECURE_FAILURE", WINHTTP_CALLBACK_STATUS_SECURE_FAILURE),
        ("CONNECTING_TO_SERVER", WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER),
        ("CONNECTED_TO_SERVER", WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER),
        ("HANDLE_CREATED", WINHTTP_CALLBACK_STATUS_HANDLE_CREATED),
        ("HANDLE_CLOSING", WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING),
    ];

    /// Asserts that `session_notification_flags()` enables every notification the
    /// callback protocol consumes.
    ///
    /// Awaited operation completions come from `OperationKind::ALL`, so the
    /// enumeration itself supplies that half of the coverage; the remaining
    /// statuses come from `DISPATCHED_STATUSES`.
    #[test]
    fn notification_mask_enables_every_dispatched_status() {
        for kind in OperationKind::ALL {
            assert_status_enabled(&format!("{kind:?}"), awaited_status(kind));
        }

        for (name, status) in DISPATCHED_STATUSES {
            assert_status_enabled(name, status);
        }
    }

    /// Asserts that `session_notification_flags()` enables no notification the
    /// callback protocol cannot receive.
    ///
    /// The proxy-resolution completions are the ones a reader is most likely to
    /// add back, because the native `WINHTTP_CALLBACK_FLAG_ALL_COMPLETIONS`
    /// includes them. The transport never calls `WinHttpGetProxyForUrlEx` or
    /// `WinHttpGetProxySettingsEx`, so subscribing would announce handling that
    /// the callback does not implement.
    #[test]
    fn notification_mask_excludes_proxy_completions_the_transport_cannot_receive() {
        for (name, flag) in [
            ("GETPROXYFORURL_COMPLETE", WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE),
            ("GETPROXYSETTINGS_COMPLETE", WINHTTP_CALLBACK_FLAG_GETPROXYSETTINGS_COMPLETE),
        ] {
            assert_eq!(
                session_notification_flags() & flag,
                0,
                "session_notification_flags() enables {name}, but the transport never calls the API that raises it"
            );
        }
    }

    /// Asserts that the registered mask lets WinHTTP deliver `status`.
    ///
    /// Every `WINHTTP_CALLBACK_STATUS_*` value is a distinct single bit and each
    /// `WINHTTP_CALLBACK_FLAG_*` notification flag is the union of the status
    /// bits it enables, so a status value doubles as its own mask bit.
    /// Ref: <https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpsetstatuscallback>
    fn assert_status_enabled(name: &str, status: u32) {
        assert_eq!(
            session_notification_flags() & status,
            status,
            "session_notification_flags() does not enable {name}, which the callback protocol consumes. \
             WinHTTP would never deliver that notification, and because nothing else wakes an operation \
             future and every native timeout is unlimited, the affected request would wait forever \
             instead of failing."
        );
    }

    /// Reports the callback status that completes `kind`.
    ///
    /// The match is exhaustive on purpose: a new `OperationKind` variant stops
    /// this test compiling until its completion status is stated here, and
    /// `OperationKind::ALL` then feeds it into the mask assertion.
    fn awaited_status(kind: OperationKind) -> u32 {
        match kind {
            OperationKind::SendRequest
            | OperationKind::HeadersAvailable
            | OperationKind::DataAvailable
            | OperationKind::Read
            | OperationKind::Write => kind.callback_status(),
        }
    }

    #[test]
    fn open_failure_does_not_close_a_handle() {
        assert_failure(FailurePoint::Open);
    }

    #[test]
    fn timeout_failure_closes_the_session_once() {
        assert_failure(FailurePoint::SetTimeouts);
    }

    #[test]
    fn global_pooling_failure_closes_the_session_once() {
        assert_failure(FailurePoint::DisableGlobalPooling);
    }

    #[test]
    fn connection_idle_timeout_failure_closes_the_session_once() {
        assert_failure(FailurePoint::ConnectionIdleTimeout);
    }

    #[test]
    fn callback_assurance_failure_closes_the_session_once() {
        assert_failure(FailurePoint::AssuredNonBlockingCallbacks);
    }

    #[test]
    fn http2_keep_alive_failure_closes_the_session_once() {
        assert_failure(FailurePoint::Http2KeepAlive);
    }

    #[test]
    fn http3_keep_alive_failure_closes_the_session_once() {
        assert_failure(FailurePoint::Http3KeepAlive);
    }

    #[test]
    fn callback_registration_failure_closes_the_session_once() {
        assert_failure(FailurePoint::SetStatusCallback);
    }

    fn assert_failure(point: FailurePoint) {
        let bindings = configured_bindings(SetupScript {
            failure: Some(point),
            ..SetupScript::new()
        });
        let error = WinHttpSession::new(BindingsFacade::mock(Arc::new(bindings)), &active_transport_options()).unwrap_err();

        assert_eq!(error.code(), error_code(point));
        assert_eq!(error.operation(), initialization_operation(point));
    }

    fn configured_bindings(script: SetupScript) -> MockBindings {
        let mut bindings = MockBindings::new();
        let mut sequence = Sequence::new();
        let raw = raw_handle();

        bindings
            .expect_open()
            .withf(|user_agent, flags| user_agent.to_string_lossy() == USER_AGENT && *flags == WINHTTP_FLAG_ASYNC)
            .once()
            .in_sequence(&mut sequence)
            .return_once(move |_, _| script.result(FailurePoint::Open).map(|()| raw));

        if script.reaches(FailurePoint::SetTimeouts) {
            bindings
                .expect_set_timeouts()
                .withf(move |handle, resolve, connect, send, receive| {
                    *handle == raw
                        && *resolve == UNLIMITED_TIMEOUT
                        && *connect == UNLIMITED_TIMEOUT
                        && *send == UNLIMITED_TIMEOUT
                        && *receive == UNLIMITED_TIMEOUT
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _, _, _| script.result(FailurePoint::SetTimeouts));
        }

        expect_option(
            &mut bindings,
            &mut sequence,
            script,
            FailurePoint::DisableGlobalPooling,
            WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
            TRUE_BYTES.to_vec(),
        );
        expect_option(
            &mut bindings,
            &mut sequence,
            script,
            FailurePoint::ConnectionIdleTimeout,
            WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT,
            dword_bytes(script.idle_timeout_millis).to_vec(),
        );
        expect_option(
            &mut bindings,
            &mut sequence,
            script,
            FailurePoint::AssuredNonBlockingCallbacks,
            WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS,
            TRUE_BYTES.to_vec(),
        );

        if script.keep_alive {
            expect_option(
                &mut bindings,
                &mut sequence,
                script,
                FailurePoint::Http2KeepAlive,
                WINHTTP_OPTION_HTTP2_KEEPALIVE,
                dword_bytes(5_000).to_vec(),
            );
            expect_option(
                &mut bindings,
                &mut sequence,
                script,
                FailurePoint::Http3KeepAlive,
                WINHTTP_OPTION_HTTP3_KEEPALIVE,
                dword_bytes(1).to_vec(),
            );
        }

        if script.reaches(FailurePoint::SetStatusCallback) {
            let expected_callback: StatusCallback = Some(status_callback);
            bindings
                .expect_set_status_callback()
                .withf(move |handle, callback, flags| {
                    *handle == raw && status_callback_matches(*callback, expected_callback) && *flags == session_notification_flags()
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _| script.result(FailurePoint::SetStatusCallback));
        }

        if script.failure != Some(FailurePoint::Open) {
            bindings
                .expect_close_handle()
                .withf(move |handle| *handle == raw)
                .once()
                .in_sequence(&mut sequence)
                .returning(|_| Ok(()));
        }

        bindings
    }

    fn expect_option(
        bindings: &mut MockBindings,
        sequence: &mut Sequence,
        script: SetupScript,
        step: FailurePoint,
        option: u32,
        expected: Vec<u8>,
    ) {
        if !script.reaches(step) {
            return;
        }

        let raw = raw_handle();
        bindings
            .expect_set_option()
            .withf(move |handle, actual_option, value| *handle == raw && *actual_option == option && value == expected)
            .once()
            .in_sequence(sequence)
            .return_once(move |_, _, _| script.result(step));
    }

    fn status_callback_matches(actual: StatusCallback, expected: StatusCallback) -> bool {
        let (Some(actual), Some(expected)) = (actual, expected) else {
            return false;
        };

        #[cfg(miri)]
        {
            // Miri may materialize distinct shims for the same function item.
            let _ = (actual, expected);
            true
        }

        #[cfg(not(miri))]
        {
            std::ptr::fn_addr_eq(actual, expected)
        }
    }

    const fn error_code(point: FailurePoint) -> u32 {
        match point {
            FailurePoint::Open => 12_000,
            FailurePoint::SetTimeouts => 12_001,
            FailurePoint::DisableGlobalPooling => 12_002,
            FailurePoint::ConnectionIdleTimeout => 12_003,
            FailurePoint::AssuredNonBlockingCallbacks => 12_004,
            FailurePoint::Http2KeepAlive => 12_005,
            FailurePoint::Http3KeepAlive => 12_006,
            FailurePoint::SetStatusCallback => 12_007,
        }
    }

    const fn operation(point: FailurePoint) -> WinHttpOperation {
        match point {
            FailurePoint::Open => WinHttpOperation::Open,
            FailurePoint::SetTimeouts => WinHttpOperation::SetTimeouts,
            FailurePoint::DisableGlobalPooling
            | FailurePoint::ConnectionIdleTimeout
            | FailurePoint::AssuredNonBlockingCallbacks
            | FailurePoint::Http2KeepAlive
            | FailurePoint::Http3KeepAlive => WinHttpOperation::SetOption,
            FailurePoint::SetStatusCallback => WinHttpOperation::SetStatusCallback,
        }
    }

    const fn initialization_operation(point: FailurePoint) -> SessionInitializationOperation {
        match point {
            FailurePoint::Open => SessionInitializationOperation::Open,
            FailurePoint::SetTimeouts => SessionInitializationOperation::SetTimeouts,
            FailurePoint::DisableGlobalPooling => SessionInitializationOperation::DisableGlobalPooling,
            FailurePoint::ConnectionIdleTimeout => SessionInitializationOperation::ConnectionIdleTimeout,
            FailurePoint::AssuredNonBlockingCallbacks => SessionInitializationOperation::AssuredNonBlockingCallbacks,
            FailurePoint::Http2KeepAlive => SessionInitializationOperation::Http2KeepAlive,
            FailurePoint::Http3KeepAlive => SessionInitializationOperation::Http3KeepAlive,
            FailurePoint::SetStatusCallback => SessionInitializationOperation::SetStatusCallback,
        }
    }

    /// Transport options whose keep-alive policy exercises the probe options.
    fn active_transport_options() -> TransportOptions {
        let mut options = TransportOptions::default();
        options.connection_keep_alive = ConnectionKeepAlive::active_connections(Duration::from_nanos(1), Duration::from_secs(30));
        options
    }

    fn raw_handle() -> RawHandle {
        RawHandle::new(std::ptr::dangling_mut::<c_void>()).unwrap()
    }

    #[test]
    fn every_initialization_step_describes_itself_distinctly_in_the_failure_message() {
        let operations = [
            SessionInitializationOperation::Open,
            SessionInitializationOperation::SetTimeouts,
            SessionInitializationOperation::DisableGlobalPooling,
            SessionInitializationOperation::ConnectionIdleTimeout,
            SessionInitializationOperation::AssuredNonBlockingCallbacks,
            SessionInitializationOperation::Http2KeepAlive,
            SessionInitializationOperation::Http3KeepAlive,
            SessionInitializationOperation::SetStatusCallback,
        ];

        let mut descriptions = Vec::with_capacity(operations.len());

        for operation in operations {
            let description = operation.to_string();

            assert!(!description.is_empty(), "{operation:?} has no description");

            // The failure surfaces which setup step failed and its Win32 code,
            // so both must reach the rendered message.
            let failure = SessionInitializationFailure::new(operation, WinHttpError::new(87, WinHttpOperation::SetOption));
            assert_eq!(failure.to_string(), format!("{description} failed with Win32 error 87"));

            descriptions.push(description);
        }

        let mut unique = descriptions.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), descriptions.len(), "step descriptions must be distinct");
    }

    #[test]
    fn an_initialization_failure_exposes_the_win_http_error_as_its_source() {
        let failure = SessionInitializationFailure::new(SessionInitializationOperation::Open, WinHttpError::new(8, WinHttpOperation::Open));

        let source = failure.source().expect("the originating WinHTTP error is the source");

        assert_eq!(source.to_string(), WinHttpError::new(8, WinHttpOperation::Open).to_string());
    }
}
