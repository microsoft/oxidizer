// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt;

use fetch::options::ConnectionKeepAlive;
use widestring::U16CString;
use windows::Win32::Networking::WinHttp::{
    WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE, WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE, WINHTTP_CALLBACK_FLAG_GETPROXYSETTINGS_COMPLETE,
    WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE, WINHTTP_CALLBACK_FLAG_READ_COMPLETE, WINHTTP_CALLBACK_FLAG_REQUEST_ERROR,
    WINHTTP_CALLBACK_FLAG_SECURE_FAILURE, WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_FLAG_WRITE_COMPLETE,
    WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
    WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS, WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
};

use crate::WinHttpOptions;
use crate::bindings::{Bindings as _, Facade};
use crate::callback::status_callback;
use crate::error::WinHttpError;
use crate::handle::SessionHandle;
use crate::options::{
    WINHTTP_FLAG_ASYNC, WINHTTP_OPTION_HTTP2_KEEPALIVE, WINHTTP_OPTION_HTTP3_KEEPALIVE, dword_bytes, http2_keep_alive_millis,
    http3_keep_alive_millis, timeout_millis,
};

const USER_AGENT: &str = "fetch_winhttp";
const UNLIMITED_TIMEOUT: i32 = -1;
const TRUE_BYTES: [u8; size_of::<i32>()] = 1_i32.to_ne_bytes();

const ALL_COMPLETIONS: u32 = WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
    | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
    | WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE
    | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
    | WINHTTP_CALLBACK_FLAG_WRITE_COMPLETE
    | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
    | WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE
    | WINHTTP_CALLBACK_FLAG_GETPROXYSETTINGS_COMPLETE;
const HANDLES: u32 = WINHTTP_CALLBACK_STATUS_HANDLE_CREATED | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;
const CONNECT_TO_SERVER: u32 = WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER | WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER;

pub(crate) const SESSION_NOTIFICATION_FLAGS: u32 = ALL_COMPLETIONS | WINHTTP_CALLBACK_FLAG_SECURE_FAILURE | HANDLES | CONNECT_TO_SERVER;

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
    pub(crate) fn new(
        bindings: Facade,
        options: &WinHttpOptions,
        keep_alive: &ConnectionKeepAlive,
    ) -> Result<Self, SessionInitializationFailure> {
        let user_agent = U16CString::from_str(USER_AGENT).expect("the static WinHTTP user agent contains no NUL characters");
        let raw = bindings
            .open(&user_agent, WINHTTP_FLAG_ASYNC)
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::Open, error))?;
        let handle = SessionHandle::new(raw, bindings);

        handle
            .bindings()
            .set_timeouts(
                handle.raw(),
                timeout_millis(options.resolve_timeout()),
                UNLIMITED_TIMEOUT,
                UNLIMITED_TIMEOUT,
                UNLIMITED_TIMEOUT,
            )
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::SetTimeouts, error))?;
        handle
            .bindings()
            .set_option(handle.raw(), WINHTTP_OPTION_DISABLE_GLOBAL_POOLING, &TRUE_BYTES)
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::DisableGlobalPooling, error))?;
        handle
            .bindings()
            .set_option(handle.raw(), WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS, &TRUE_BYTES)
            .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::AssuredNonBlockingCallbacks, error))?;
        if let Some(interval) = Self::keep_alive_interval(keep_alive) {
            handle
                .bindings()
                .set_option(
                    handle.raw(),
                    WINHTTP_OPTION_HTTP2_KEEPALIVE,
                    &dword_bytes(http2_keep_alive_millis(interval)),
                )
                .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::Http2KeepAlive, error))?;
            handle
                .bindings()
                .set_option(
                    handle.raw(),
                    WINHTTP_OPTION_HTTP3_KEEPALIVE,
                    &dword_bytes(http3_keep_alive_millis(interval)),
                )
                .map_err(|error| SessionInitializationFailure::new(SessionInitializationOperation::Http3KeepAlive, error))?;
        }
        handle
            .bindings()
            .set_status_callback(handle.raw(), Some(status_callback), SESSION_NOTIFICATION_FLAGS)
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
mod tests {
    use std::ffi::c_void;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;
    use std::time::Duration;

    use fetch::options::ConnectionKeepAlive;
    use mockall::Sequence;
    use static_assertions::assert_impl_all;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE, WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE,
        WINHTTP_CALLBACK_FLAG_GETPROXYSETTINGS_COMPLETE, WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE, WINHTTP_CALLBACK_FLAG_READ_COMPLETE,
        WINHTTP_CALLBACK_FLAG_REQUEST_ERROR, WINHTTP_CALLBACK_FLAG_SECURE_FAILURE, WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE,
        WINHTTP_CALLBACK_FLAG_WRITE_COMPLETE, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS,
        WINHTTP_OPTION_DISABLE_GLOBAL_POOLING,
    };

    use super::{
        SESSION_NOTIFICATION_FLAGS, SessionInitializationFailure, SessionInitializationOperation, TRUE_BYTES, UNLIMITED_TIMEOUT,
        USER_AGENT, WinHttpSession,
    };
    use crate::WinHttpOptions;
    use crate::bindings::{Facade, MockBindings, StatusCallback};
    use crate::callback::status_callback;
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::RawHandle;
    use crate::options::{WINHTTP_FLAG_ASYNC, WINHTTP_OPTION_HTTP2_KEEPALIVE, WINHTTP_OPTION_HTTP3_KEEPALIVE, dword_bytes, timeout_millis};

    assert_impl_all!(WinHttpSession: Send, Sync, std::fmt::Debug, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        SessionInitializationFailure: Send, Sync, Clone, std::fmt::Debug, std::error::Error, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(SessionInitializationOperation: UnwindSafe, RefUnwindSafe);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FailurePoint {
        Open,
        SetTimeouts,
        DisableGlobalPooling,
        AssuredNonBlockingCallbacks,
        Http2KeepAlive,
        Http3KeepAlive,
        SetStatusCallback,
    }

    #[test]
    fn session_setup_succeeds_in_exact_order_with_expected_values() {
        let bindings = configured_bindings(None, timeout_millis(Some(Duration::from_micros(1_500))), true);
        let options = WinHttpOptions::builder().resolve_timeout(Duration::from_micros(1_500)).build();
        let keep_alive = active_keep_alive();

        let session = WinHttpSession::new(Facade::mock(Arc::new(bindings)), &options, &keep_alive).unwrap();

        assert_eq!(session.handle().raw(), raw_handle());
        drop(session);
    }

    #[test]
    fn disabled_keep_alive_leaves_session_probe_options_unset() {
        let bindings = configured_bindings(None, UNLIMITED_TIMEOUT, false);

        let session = WinHttpSession::new(
            Facade::mock(Arc::new(bindings)),
            &WinHttpOptions::default(),
            &ConnectionKeepAlive::Disabled,
        )
        .unwrap();

        drop(session);
    }

    #[test]
    fn notification_mask_contains_every_required_status() {
        let expected = WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
            | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
            | WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE
            | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
            | WINHTTP_CALLBACK_FLAG_WRITE_COMPLETE
            | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
            | WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE
            | WINHTTP_CALLBACK_FLAG_GETPROXYSETTINGS_COMPLETE
            | WINHTTP_CALLBACK_FLAG_SECURE_FAILURE
            | WINHTTP_CALLBACK_STATUS_HANDLE_CREATED
            | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING
            | WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER
            | WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER;

        assert_eq!(SESSION_NOTIFICATION_FLAGS, expected);
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
        let bindings = configured_bindings(Some(point), UNLIMITED_TIMEOUT, true);
        let error = WinHttpSession::new(Facade::mock(Arc::new(bindings)), &WinHttpOptions::default(), &active_keep_alive()).unwrap_err();

        assert_eq!(error.code(), error_code(point));
        assert_eq!(error.operation(), initialization_operation(point));
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the helper keeps the complete ordered WinHTTP session setup script visible in one place"
    )]
    fn configured_bindings(failure: Option<FailurePoint>, expected_resolve_timeout: i32, configure_keep_alive: bool) -> MockBindings {
        let mut bindings = MockBindings::new();
        let mut sequence = Sequence::new();
        let raw = raw_handle();

        bindings
            .expect_open()
            .withf(|user_agent, flags| user_agent.to_string_lossy() == USER_AGENT && *flags == WINHTTP_FLAG_ASYNC)
            .once()
            .in_sequence(&mut sequence)
            .return_once(move |_, _| setup_result(failure, FailurePoint::Open, raw));

        if failure != Some(FailurePoint::Open) {
            bindings
                .expect_set_timeouts()
                .withf(move |handle, resolve, connect, send, receive| {
                    *handle == raw
                        && *resolve == expected_resolve_timeout
                        && *connect == UNLIMITED_TIMEOUT
                        && *send == UNLIMITED_TIMEOUT
                        && *receive == UNLIMITED_TIMEOUT
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _, _, _| setup_unit_result(failure, FailurePoint::SetTimeouts));
        }

        if !matches!(failure, Some(FailurePoint::Open | FailurePoint::SetTimeouts)) {
            bindings
                .expect_set_option()
                .withf(move |handle, option, value| {
                    *handle == raw && *option == WINHTTP_OPTION_DISABLE_GLOBAL_POOLING && value == TRUE_BYTES
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _| setup_unit_result(failure, FailurePoint::DisableGlobalPooling));
        }

        if !matches!(
            failure,
            Some(FailurePoint::Open | FailurePoint::SetTimeouts | FailurePoint::DisableGlobalPooling)
        ) {
            bindings
                .expect_set_option()
                .withf(move |handle, option, value| {
                    *handle == raw && *option == WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS && value == TRUE_BYTES
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _| setup_unit_result(failure, FailurePoint::AssuredNonBlockingCallbacks));
        }

        if configure_keep_alive
            && !matches!(
                failure,
                Some(
                    FailurePoint::Open
                        | FailurePoint::SetTimeouts
                        | FailurePoint::DisableGlobalPooling
                        | FailurePoint::AssuredNonBlockingCallbacks
                )
            )
        {
            bindings
                .expect_set_option()
                .withf(move |handle, option, value| {
                    *handle == raw && *option == WINHTTP_OPTION_HTTP2_KEEPALIVE && value == dword_bytes(5_000)
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _| setup_unit_result(failure, FailurePoint::Http2KeepAlive));
        }

        if configure_keep_alive
            && !matches!(
                failure,
                Some(
                    FailurePoint::Open
                        | FailurePoint::SetTimeouts
                        | FailurePoint::DisableGlobalPooling
                        | FailurePoint::AssuredNonBlockingCallbacks
                        | FailurePoint::Http2KeepAlive
                )
            )
        {
            bindings
                .expect_set_option()
                .withf(move |handle, option, value| *handle == raw && *option == WINHTTP_OPTION_HTTP3_KEEPALIVE && value == dword_bytes(1))
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _| setup_unit_result(failure, FailurePoint::Http3KeepAlive));
        }

        if !matches!(
            failure,
            Some(
                FailurePoint::Open
                    | FailurePoint::SetTimeouts
                    | FailurePoint::DisableGlobalPooling
                    | FailurePoint::AssuredNonBlockingCallbacks
                    | FailurePoint::Http2KeepAlive
                    | FailurePoint::Http3KeepAlive
            )
        ) {
            let expected_callback: StatusCallback = Some(status_callback);
            bindings
                .expect_set_status_callback()
                .withf(move |handle, callback, flags| {
                    *handle == raw && status_callback_matches(*callback, expected_callback) && *flags == SESSION_NOTIFICATION_FLAGS
                })
                .once()
                .in_sequence(&mut sequence)
                .return_once(move |_, _, _| setup_unit_result(failure, FailurePoint::SetStatusCallback));
        }

        if failure != Some(FailurePoint::Open) {
            bindings
                .expect_close_handle()
                .withf(move |handle| *handle == raw)
                .once()
                .in_sequence(&mut sequence)
                .returning(|_| Ok(()));
        }

        bindings
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

    fn setup_result(failure: Option<FailurePoint>, point: FailurePoint, raw: RawHandle) -> crate::error::Result<RawHandle> {
        setup_unit_result(failure, point).map(|()| raw)
    }

    fn setup_unit_result(failure: Option<FailurePoint>, point: FailurePoint) -> crate::error::Result<()> {
        if failure == Some(point) {
            Err(WinHttpError::new(error_code(point), operation(point)))
        } else {
            Ok(())
        }
    }

    const fn error_code(point: FailurePoint) -> u32 {
        match point {
            FailurePoint::Open => 12_000,
            FailurePoint::SetTimeouts => 12_001,
            FailurePoint::DisableGlobalPooling => 12_002,
            FailurePoint::AssuredNonBlockingCallbacks => 12_003,
            FailurePoint::Http2KeepAlive => 12_004,
            FailurePoint::Http3KeepAlive => 12_005,
            FailurePoint::SetStatusCallback => 12_006,
        }
    }

    const fn operation(point: FailurePoint) -> WinHttpOperation {
        match point {
            FailurePoint::Open => WinHttpOperation::Open,
            FailurePoint::SetTimeouts => WinHttpOperation::SetTimeouts,
            FailurePoint::DisableGlobalPooling
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
            FailurePoint::AssuredNonBlockingCallbacks => SessionInitializationOperation::AssuredNonBlockingCallbacks,
            FailurePoint::Http2KeepAlive => SessionInitializationOperation::Http2KeepAlive,
            FailurePoint::Http3KeepAlive => SessionInitializationOperation::Http3KeepAlive,
            FailurePoint::SetStatusCallback => SessionInitializationOperation::SetStatusCallback,
        }
    }

    fn active_keep_alive() -> ConnectionKeepAlive {
        ConnectionKeepAlive::active_connections(Duration::from_nanos(1), Duration::from_secs(30))
    }

    fn raw_handle() -> RawHandle {
        RawHandle::new(std::ptr::dangling_mut::<c_void>()).unwrap()
    }
}
