// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};

use bytesbuf::mem::GlobalPool;
use fetch::options::TransportOptions;
use fetch::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, RecoveryInfo};
use layered::Service;
use observed::Sink;
use tick::Clock;

use crate::bindings::BindingsFacade;
use crate::operation::ContextPool;
use crate::request::RequestDriver;
use crate::session::{SessionInitializationFailure, WinHttpSession};
use crate::telemetry::Telemetry;
use crate::{WinHttpTlsConfig, error_labels};

#[derive(Debug)]
/// Runs one materialized WinHTTP handler for a core and pool slot.
///
/// A ready instance owns one WinHTTP session, its OS connection pool, a request
/// context pool, body-building services, and transport configuration. Request
/// execution is delegated to a [`RequestDriver`] that owns request-specific
/// handles and drives the asynchronous WinHTTP operation sequence.
///
/// The custom-transport factory cannot return an error, so session setup failure
/// is retained as a permanent state. Requests in that state return initialization
/// errors without creating request or connect handles and without issuing I/O.
pub(crate) struct WinHttpTransport {
    telemetry: Telemetry,
    state: TransportState,
}

impl WinHttpTransport {
    pub(crate) fn new(inputs: TransportInputs, bindings: BindingsFacade) -> Self {
        let telemetry = Telemetry::new(inputs.sink);
        let state = match WinHttpSession::new(bindings, &inputs.options) {
            Ok(session) => TransportState::Ready(Box::new(ReadyTransport {
                session: Arc::new(session),
                body_builder: inputs.body_builder,
                clock: inputs.clock,
                global_pool: inputs.global_pool,
                contexts: Mutex::new(plurality::Pool::new()),
                options: inputs.options,
                tls: inputs.tls,
            })),
            Err(failure) => {
                telemetry.initialization_failed(&failure);
                TransportState::Failed(FailedTransport { failure })
            }
        };

        Self { telemetry, state }
    }
}

impl Service<HttpRequest> for WinHttpTransport {
    type Out = fetch::Result<HttpResponse>;

    async fn execute(&self, mut input: HttpRequest) -> Self::Out {
        self.telemetry.request_attempted();

        let replay_body = input.body().try_clone();
        let mut body_polled = false;
        let result = match &self.state {
            TransportState::Ready(ready) => match RequestDriver::new(
                &mut input,
                Arc::clone(&ready.session),
                ready.body_builder.clone(),
                &ready.clock,
                ready.global_pool.clone(),
                &ready.contexts,
                &ready.options,
                &ready.tls,
            ) {
                Ok(driver) => driver.execute(&mut body_polled).await.map_err(|failure| {
                    let cold_connect_duration = failure.cold_connect_duration();
                    (failure.into_error(), cold_connect_duration)
                }),
                Err(error) => Err((error, None)),
            },
            TransportState::Failed(failed) => Err((
                HttpError::other(failed.failure.clone(), RecoveryInfo::never(), error_labels::INITIALIZATION),
                None,
            )),
        };

        match result {
            Ok(response) => Ok(response),
            Err((mut error, cold_connect_duration)) => {
                self.telemetry.request_failed(cold_connect_duration);
                let _ = error.take_request();
                if !body_polled {
                    Err(error.with_request(input))
                } else if let Some(replay_body) = replay_body {
                    *input.body_mut() = replay_body;
                    Err(error.with_request(input))
                } else {
                    Err(error)
                }
            }
        }
    }
}

#[derive(Debug)]
/// Separates an initialized handler from a permanent setup failure.
///
/// Initialization happens once when the core and pool-slot handler is
/// materialized. The state is not retried on later requests, representing failed
/// session setup within the infallible custom-transport factory contract.
enum TransportState {
    Ready(Box<ReadyTransport>),
    Failed(FailedTransport),
}

#[derive(Debug)]
/// Owns resources shared by requests handled by one transport instance.
///
/// These resources belong to one materialized core and pool slot: the session
/// and its native connection pool, response-body services, read-buffer pool,
/// request-context pool, and finalized options. A [`RequestDriver`] clones or
/// rents what one request needs and owns its per-request handles and lifecycle.
struct ReadyTransport {
    session: Arc<WinHttpSession>,
    body_builder: HttpBodyBuilder,
    clock: Clock,
    global_pool: GlobalPool,
    contexts: ContextPool,
    options: TransportOptions,
    tls: WinHttpTlsConfig,
}

#[derive(Debug)]
/// Retains setup failure without permitting request I/O.
///
/// Every request receives a fresh non-recoverable initialization error sourced
/// from the same failure; no session recovery or native request construction is
/// attempted by this state.
struct FailedTransport {
    failure: SessionInitializationFailure,
}

#[derive(Debug)]
/// Carries finalized factory inputs into one materialized transport.
///
/// The inputs are per-instance clones or relocated services, not request state.
/// [`WinHttpTransport::new`] consumes them to create the session, telemetry, and
/// resource pools used by that core and pool-slot handler.
pub(crate) struct TransportInputs {
    pub(crate) body_builder: HttpBodyBuilder,
    pub(crate) clock: Clock,
    pub(crate) global_pool: GlobalPool,
    pub(crate) sink: Sink,
    pub(crate) options: TransportOptions,
    pub(crate) tls: WinHttpTlsConfig,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ffi::c_void;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use fetch::options::TransportOptions;
    use fetch::{HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, Recovery, RecoveryInfo};
    use http_extensions::HttpBodyOptions;
    use layered::Service;
    use observed::{Severity, Sink};
    use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};
    use ohno::Labeled as _;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use tick::{Clock, ClockControl};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
    };

    use super::{FailedTransport, ReadyTransport, TransportInputs, TransportState, WinHttpTransport};
    use crate::WinHttpTlsConfig;
    use crate::bindings::{
        BindingsFacade, MockBindings, WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE,
    };
    use crate::context::RequestContext;
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::ConnectHandle;
    use crate::mocks::{closing, complete, complete_request_error, context_pointer, drive, installed_context_value};
    use crate::operation::ContextPool;
    use crate::session::SESSION_OPTIONS_WITHOUT_KEEP_ALIVE;

    assert_impl_all!(WinHttpTransport: Send, Sync, std::fmt::Debug);
    assert_impl_all!(ReadyTransport: Send, Sync, std::fmt::Debug);
    // Transport resources contain user-erased memory, body-builder, clock, or sink state.
    assert_not_impl_any!(WinHttpTransport: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(TransportState: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(ReadyTransport: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(FailedTransport: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(TransportInputs: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ContextPool: Send, Sync, std::fmt::Debug, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(plurality::Box<RequestContext>: Send, Sync);

    #[test]
    fn failed_transport_returns_fresh_never_recoverable_errors_with_requests() {
        let (sink, processor) = test_emitter(TEST_ID);
        let mut bindings = MockBindings::new();
        bindings
            .expect_open()
            .times(1)
            .returning(|_, _| Err(WinHttpError::new(12029, WinHttpOperation::Open)));
        let transport = WinHttpTransport::new(inputs(sink), BindingsFacade::mock(Arc::new(bindings)));

        for uri in ["https://first.example/", "https://second.example/"] {
            let mut error = drive(transport.execute(request(uri))).unwrap_err();

            assert_eq!(error.label(), "winhttp_initialization");
            assert_eq!(error.recovery(), RecoveryInfo::never());
            assert_eq!(error.take_request().unwrap().uri().to_string(), uri);
            assert!(error.take_request().is_none(), "the request is attached exactly once");
        }

        let names = processor
            .events()
            .into_iter()
            .map(|event| event.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "fetch.winhttp.session.initialization.failure",
                "fetch.winhttp.request.accepted",
                "fetch.winhttp.request.error",
                "fetch.winhttp.request.accepted",
                "fetch.winhttp.request.error",
            ]
        );
    }

    #[test]
    fn ready_transport_rejects_http_before_request_io() {
        let (sink, processor) = test_emitter(TEST_ID);
        let mut bindings = successful_bindings();
        bindings.expect_connect().never();
        bindings.expect_open_request().never();
        let transport = WinHttpTransport::new(inputs(sink), BindingsFacade::mock(Arc::new(bindings)));

        let mut input = request("http://example.com/");
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("untouched"));
        let mut error = drive(transport.execute(input)).unwrap_err();

        assert!(error.to_string().contains("plain HTTP requests are disabled"));
        assert_eq!(error.label(), "invalid_request");
        assert_eq!(error.recovery(), RecoveryInfo::never());
        let attached = error.take_request().unwrap();
        assert_eq!(attached.method(), http::Method::GET);
        assert_eq!(attached.uri().to_string(), "http://example.com/");
        assert_eq!(attached.version(), http::Version::HTTP_2);
        assert_eq!(
            attached.headers().get("x-original"),
            Some(&http::HeaderValue::from_static("untouched"))
        );
        assert_eq!(attached.body().content_length(), Some(0));
        assert!(error.take_request().is_none(), "the request is attached exactly once");
        let names = processor
            .events()
            .into_iter()
            .map(|event| event.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["fetch.winhttp.request.accepted", "fetch.winhttp.request.error",]);
    }

    #[test]
    fn successful_request_emits_one_attempt_without_an_error_event() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = successful_request_bindings(Arc::clone(&context), Arc::clone(&closes));
        let transport = WinHttpTransport::new(inputs(sink), BindingsFacade::mock(Arc::new(bindings)));

        let response = drive(transport.execute(request("https://example.com/"))).unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(response.version(), http::Version::HTTP_2);
        assert!(!response.body().is_empty());
        assert_eq!(response.headers().get("content-length"), Some(&http::HeaderValue::from_static("0")));
        assert_eq!(
            processor
                .events()
                .into_iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>(),
            ["fetch.winhttp.request.accepted"]
        );
        assert_eq!(closes.load(Ordering::SeqCst), 0, "the response body owns the request guard");
        drop(response);
        assert_eq!(
            closes.load(Ordering::SeqCst),
            1,
            "dropping the response body closes the request handle"
        );

        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // mock registered this context at installation and nothing has
        // reclaimed it; the response body owning the request guard is dropped
        // above, which the close count asserted, so no notification is in
        // flight and the mock raises none of its own; the transport reaches the
        // context only through that guard, sharedly; and the pointer is not
        // used again.
        unsafe {
            closing(context_pointer(context.load(Ordering::SeqCst)));
        }
        drop(transport);

        assert_eq!(closes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn polled_non_cloneable_body_error_omits_request_and_emits_telemetry_once() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = upload_failure_bindings(&context, Arc::clone(&closes), UploadFailure::Body, 0);
        let transport = WinHttpTransport::new(inputs(sink), BindingsFacade::mock(Arc::new(bindings)));
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &clock());
        let body_error = HttpError::unavailable("upload stream failed").with_request(request("https://unrelated.example/attached-by-body"));
        let body = body_builder.stream(
            futures::stream::iter([Err::<BytesView, _>(body_error)]),
            &HttpBodyOptions::default(),
        );
        let mut input = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://example.com/upload")
            .body(body)
            .unwrap();
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("preserved"));

        let mut error = drive(transport.execute(input)).unwrap_err();

        assert_eq!(error.label(), "unavailable");
        assert_eq!(error.recovery(), RecoveryInfo::unavailable());
        assert!(error.to_string().contains("upload stream failed"));
        assert!(error.take_request().is_none(), "a consumed streaming request is not replayable");
        assert_eq!(
            processor
                .events()
                .into_iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>(),
            ["fetch.winhttp.request.accepted", "fetch.winhttp.request.error"]
        );
        finish_failed_request(transport, &context, &closes);
    }

    #[test]
    fn send_failure_attaches_untouched_non_cloneable_streaming_request() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = upload_failure_bindings(&context, Arc::clone(&closes), UploadFailure::Send, 0);
        let transport = WinHttpTransport::new(inputs(sink), BindingsFacade::mock(Arc::new(bindings)));
        let memory = GlobalPool::new();
        let body_builder = HttpBodyBuilder::new(memory.clone(), &clock());
        let body = body_builder.stream(
            futures::stream::iter([Ok(BytesView::copied_from_slice(b"streaming", &memory))]),
            &HttpBodyOptions::default(),
        );
        let mut input = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://example.com/upload")
            .body(body)
            .unwrap();
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("preserved"));
        input.extensions_mut().insert(42_u32);

        let mut error = drive(transport.execute(input)).unwrap_err();

        assert_eq!(error.recovery(), RecoveryInfo::retry());
        let attached = error.take_request().unwrap();
        assert_eq!(attached.method(), http::Method::POST);
        assert_eq!(attached.uri().to_string(), "https://example.com/upload");
        assert_eq!(attached.version(), http::Version::HTTP_2);
        assert_eq!(
            attached.headers().get("x-original"),
            Some(&http::HeaderValue::from_static("preserved"))
        );
        assert_eq!(attached.extensions().get::<u32>(), Some(&42));
        let text = drive(attached.into_body().into_text()).unwrap();
        assert_eq!(text, "streaming");
        assert_eq!(
            processor
                .events()
                .into_iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>(),
            ["fetch.winhttp.request.accepted", "fetch.winhttp.request.error"]
        );
        finish_failed_request(transport, &context, &closes);
    }

    #[test]
    fn write_failure_attaches_replayable_body_from_original_clone() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = upload_failure_bindings(&context, Arc::clone(&closes), UploadFailure::Write, 10);
        let transport = WinHttpTransport::new(inputs(sink), BindingsFacade::mock(Arc::new(bindings)));
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &clock());
        let mut input = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://example.com/upload")
            .body(body_builder.text("replayable"))
            .unwrap();
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("preserved"));
        input.extensions_mut().insert(42_u32);

        let mut error = drive(transport.execute(input)).unwrap_err();

        assert_eq!(error.recovery(), RecoveryInfo::retry());
        let attached = error.take_request().unwrap();
        assert_eq!(attached.method(), http::Method::POST);
        assert_eq!(attached.uri().to_string(), "https://example.com/upload");
        assert_eq!(attached.version(), http::Version::HTTP_2);
        assert_eq!(
            attached.headers().get("x-original"),
            Some(&http::HeaderValue::from_static("preserved"))
        );
        assert_eq!(attached.extensions().get::<u32>(), Some(&42));
        let text = drive(attached.into_body().into_text()).unwrap();
        assert_eq!(text, "replayable");
        assert_eq!(
            processor
                .events()
                .into_iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>(),
            ["fetch.winhttp.request.accepted", "fetch.winhttp.request.error"]
        );
        finish_failed_request(transport, &context, &closes);
    }

    #[test]
    fn cold_connect_failure_emits_log_only_attribution() {
        let (sink, processor) = test_emitter(TEST_ID);
        let control = ClockControl::new();
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = cold_connect_failure_bindings(&context, Arc::clone(&closes), control.clone());
        let transport = WinHttpTransport::new(
            inputs_with_clock(sink, control.to_clock()),
            BindingsFacade::mock(Arc::new(bindings)),
        );

        let mut error = drive(transport.execute(request("https://example.com/"))).unwrap_err();

        assert_eq!(error.label(), "connect");
        assert!(error.take_request().is_some());
        assert_eq!(
            processor.events(),
            [
                ExpectedEvent::without_severity("fetch.winhttp.request.accepted").metric(),
                ExpectedEvent::new("fetch.winhttp.request.error", Severity::Error)
                    .body("WinHTTP transport request failed")
                    .dimension("winhttp.connect.duration", 0.25_f64)
                    .dimension("winhttp.connection.fresh", true)
                    .metric(),
            ]
        );
        finish_failed_request(transport, &context, &closes);
    }

    /// Reclaims the context of a request that failed and asserts the teardown.
    ///
    /// A failed request drops its guard as it unwinds, which closes the request
    /// handle; the connect handle and the session are released only when
    /// `HANDLE_CLOSING` reclaims the context, and the session's own handle
    /// closes when the last transport reference goes away.
    fn finish_failed_request(transport: WinHttpTransport, context: &Arc<AtomicUsize>, closes: &Arc<AtomicUsize>) {
        assert_eq!(closes.load(Ordering::SeqCst), 1);

        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards. The
        // mock registered this context at installation and nothing has
        // reclaimed it; the failed request dropped its guard, which the close
        // count asserted above, so no notification is in flight and the mock
        // raises none of its own; the transport reaches the context only
        // through that guard, sharedly; and the pointer is not used again.
        unsafe {
            closing(context_pointer(context.load(Ordering::SeqCst)));
        }
        drop(transport);

        assert_eq!(closes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn execute_future_is_send() {
        let mut bindings = MockBindings::new();
        bindings
            .expect_open()
            .times(1)
            .returning(|_, _| Err(WinHttpError::new(12029, WinHttpOperation::Open)));
        let transport = WinHttpTransport::new(inputs(Sink::noop()), BindingsFacade::mock(Arc::new(bindings)));

        assert_send(transport.execute(request("https://example.com/")));
    }

    /// Nothing is shared between two materialized transports: each opens its own
    /// `WinHTTP` session and rents callback contexts from its own pool. Both halves are
    /// observable - a shared session hands both instances the same native handle, and a
    /// shared pool reports a live rental made through one instance as occupancy in the
    /// other.
    #[test]
    fn ready_transports_own_distinct_sessions_and_context_pools() {
        let first = WinHttpTransport::new(
            inputs(Sink::noop()),
            BindingsFacade::mock(Arc::new(session_bindings(raw_handle_value(1)))),
        );
        let second = WinHttpTransport::new(
            inputs(Sink::noop()),
            BindingsFacade::mock(Arc::new(session_bindings(raw_handle_value(2)))),
        );
        // The rented context needs a connect handle; its own bindings keep that
        // handle's close out of either transport's expectations.
        let mut connect_bindings = MockBindings::new();
        connect_bindings.expect_close_handle().once().returning(|_| Ok(()));
        let connect = ConnectHandle::new(raw_handle_value(3), BindingsFacade::mock(Arc::new(connect_bindings)));

        match (&first.state, &second.state) {
            (TransportState::Ready(first), TransportState::Ready(second)) => {
                assert_ne!(first.session.handle().raw(), second.session.handle().raw());

                let rented = first
                    .contexts
                    .lock()
                    .unwrap()
                    .alloc_box(RequestContext::new(connect, Arc::clone(&first.session)));

                assert_eq!(first.contexts.lock().unwrap().len(), 1);
                assert_eq!(second.contexts.lock().unwrap().len(), 0);
                drop(rented);
            }
            _ => panic!("both transports must initialize successfully"),
        }
    }

    fn assert_send<T: Send>(_value: T) {}

    fn inputs(sink: Sink) -> TransportInputs {
        inputs_with_clock(sink, clock())
    }

    fn inputs_with_clock(sink: Sink, clock: Clock) -> TransportInputs {
        let global_pool = GlobalPool::new();

        TransportInputs {
            body_builder: HttpBodyBuilder::new(global_pool.clone(), &clock),
            clock,
            global_pool,
            sink,
            options: TransportOptions::default(),
            tls: WinHttpTlsConfig::default(),
        }
    }

    fn request(uri: &str) -> HttpRequest {
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &clock());
        HttpRequestBuilder::new(&body_builder).get(uri).build().unwrap()
    }

    fn successful_bindings() -> MockBindings {
        session_bindings(raw_handle())
    }

    fn session_bindings(session: crate::handle::RawHandle) -> MockBindings {
        let mut bindings = MockBindings::new();
        bindings.expect_open().once().returning(move |_, _| Ok(session));
        bindings.expect_set_timeouts().once().returning(|_, _, _, _, _| Ok(()));
        bindings
            .expect_set_option()
            .times(SESSION_OPTIONS_WITHOUT_KEEP_ALIVE)
            .returning(|_, _, _| Ok(()));
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_close_handle().once().returning(|_| Ok(()));
        bindings
    }

    /// Records the context the transport installs and reports it through `record`.
    ///
    /// A transport owns its request pipeline, so its mock bindings observe the
    /// installation only as the pointer-sized value `WinHTTP` is handed. That
    /// value is registered here, where the installation happens, which is what
    /// admits the context to the dispatch helpers; registering it at delivery
    /// would admit every address a script could name.
    fn expect_context_option(bindings: &mut MockBindings, record: Arc<AtomicUsize>) {
        bindings.expect_set_option().returning(move |_, option, value| {
            if option == WINHTTP_OPTION_CONTEXT_VALUE {
                let context = installed_context_value(usize::from_ne_bytes(value.try_into().unwrap()));
                record.store(context.addr(), Ordering::SeqCst);
            }
            Ok(())
        });
    }

    fn cold_connect_failure_bindings(context: &Arc<AtomicUsize>, closes: Arc<AtomicUsize>, control: ClockControl) -> MockBindings {
        let mut bindings = MockBindings::new();
        bindings.expect_open().once().returning(|_, _| Ok(raw_handle_value(1)));
        bindings.expect_set_timeouts().once().returning(|_, _, _, _, _| Ok(()));

        expect_context_option(&mut bindings, Arc::clone(context));
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_connect().once().returning(|_, _, _| Ok(raw_handle_value(2)));
        bindings
            .expect_open_request()
            .once()
            .returning(|_, _, _, _| Ok(raw_handle_value(3)));
        bindings.expect_send_request().once().returning(move |_, _, total_len, context| {
            assert_eq!(total_len, 0);
            let context = context_pointer(context);
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload matching the notification, no overlapping
            // notification, no outstanding exclusive borrow, and no use of the
            // context after the reclaiming notification. The context option
            // above registered the value the transport installed and only
            // `finish_failed_request` reclaims it; a connect-progress
            // notification carries no payload; this runs inside the send the
            // transport submitted, on the submitting thread, with nothing else
            // dispatching, so it overlaps no other notification; and a
            // submission runs with no exclusive borrow of the context
            // outstanding.
            unsafe {
                complete(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
            }
            control.advance(Duration::from_millis(250));
            // SAFETY: complete_request_error carries the obligations of
            // complete except the payload, which it supplies. They hold as for
            // the connect-progress notification above, which returned before
            // this one begins.
            unsafe {
                complete_request_error(context, 12029);
            }
            Ok(())
        });
        bindings.expect_write_data().never();
        bindings.expect_receive_response().never();
        bindings.expect_query_headers().never();
        bindings.expect_query_option().never();
        bindings.expect_close_handle().times(3).returning(move |_| {
            closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        bindings
    }

    fn successful_request_bindings(context: Arc<AtomicUsize>, closes: Arc<AtomicUsize>) -> MockBindings {
        let mut bindings = MockBindings::new();
        bindings.expect_open().once().returning(|_, _| Ok(raw_handle_value(1)));
        bindings.expect_set_timeouts().once().returning(|_, _, _, _, _| Ok(()));

        expect_context_option(&mut bindings, Arc::clone(&context));
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_connect().once().returning(|_, _, _| Ok(raw_handle_value(2)));
        bindings
            .expect_open_request()
            .once()
            .returning(|_, _, _, _| Ok(raw_handle_value(3)));

        bindings.expect_send_request().once().returning(|_, _, total_len, context| {
            assert_eq!(total_len, 0);
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload matching the notification, no overlapping
            // notification, no outstanding exclusive borrow, and no use of the
            // context after the reclaiming notification. The context option
            // above registered the value the transport installed and only the
            // test's own `HANDLE_CLOSING` reclaims it; a send completion
            // carries no payload; this runs inside the send the transport
            // submitted, on the submitting thread, with nothing else
            // dispatching, so it overlaps no other notification; and a
            // submission runs with no exclusive borrow of the context
            // outstanding.
            unsafe {
                complete(
                    context_pointer(context),
                    WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                    std::ptr::null_mut(),
                    0,
                );
            }
            Ok(())
        });
        let receive_context = Arc::clone(&context);
        bindings.expect_receive_response().once().returning(move |_| {
            // SAFETY: as for the send completion above, except that this runs
            // inside the response reception the transport submitted after that
            // send completed, so the two cannot overlap. A headers-available
            // completion likewise carries no payload.
            unsafe {
                complete(
                    context_pointer(receive_context.load(Ordering::SeqCst)),
                    WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
                    std::ptr::null_mut(),
                    0,
                );
            }
            Ok(())
        });
        bindings.expect_query_headers().returning(|_, info_level, buffer, byte_len| {
            if info_level == (WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER) {
                let output = buffer.unwrap().cast::<u32>();
                // SAFETY: the transport supplies a writable DWORD.
                unsafe { output.as_ptr().write(200) };
                *byte_len = 4;
                return Ok(());
            }
            assert_eq!(info_level, WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING);
            let bytes = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
            let required = u32::try_from(bytes.len() + 1).unwrap();
            let Some(buffer) = buffer else {
                *byte_len = required;
                return Err(WinHttpError::new(122, WinHttpOperation::QueryHeaders));
            };
            // SAFETY: the sizing query reserved space for the content and NUL.
            unsafe { buffer.as_ptr().copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
            // SAFETY: required includes one byte after the copied content.
            let terminator = unsafe { buffer.as_ptr().add(bytes.len()) };
            // SAFETY: terminator points to the final writable byte.
            unsafe { terminator.write(0) };
            *byte_len = u32::try_from(bytes.len()).unwrap();
            Ok(())
        });
        bindings.expect_query_option().once().returning(|_, option, buffer, byte_len| {
            assert_eq!(option, WINHTTP_OPTION_HTTP_PROTOCOL_USED);
            let output = buffer.unwrap().cast::<u32>();
            // SAFETY: the transport supplies a writable DWORD.
            unsafe { output.as_ptr().write(1) };
            *byte_len = 4;
            Ok(())
        });

        bindings.expect_close_handle().times(3).returning(move |_| {
            closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        drop(context);
        bindings
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UploadFailure {
        Send,
        Body,
        Write,
    }

    fn upload_failure_bindings(
        context: &Arc<AtomicUsize>,
        closes: Arc<AtomicUsize>,
        failure: UploadFailure,
        expected_total_len: u32,
    ) -> MockBindings {
        let mut bindings = MockBindings::new();
        bindings.expect_open().once().returning(|_, _| Ok(raw_handle_value(1)));
        bindings.expect_set_timeouts().once().returning(|_, _, _, _, _| Ok(()));

        expect_context_option(&mut bindings, Arc::clone(context));
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_connect().once().returning(|_, _, _| Ok(raw_handle_value(2)));
        bindings
            .expect_open_request()
            .once()
            .returning(|_, _, _, _| Ok(raw_handle_value(3)));
        bindings.expect_send_request().once().returning(move |_, _, total_len, context| {
            assert_eq!(total_len, expected_total_len);
            if failure == UploadFailure::Send {
                return Err(WinHttpError::new(12029, WinHttpOperation::SendRequest));
            }
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload matching the notification, no overlapping
            // notification, no outstanding exclusive borrow, and no use of the
            // context after the reclaiming notification. The context option
            // above registered the value the transport installed and only
            // `finish_failed_request` reclaims it; a send completion carries no
            // payload; this runs inside the send the transport submitted, on
            // the submitting thread, with nothing else dispatching, so it
            // overlaps no other notification; and a submission runs with no
            // exclusive borrow of the context outstanding.
            unsafe {
                complete(
                    context_pointer(context),
                    WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                    std::ptr::null_mut(),
                    0,
                );
            }
            Ok(())
        });
        bindings
            .expect_write_data()
            .times(usize::from(failure == UploadFailure::Write))
            .returning(|_, _, _| Err(WinHttpError::new(12030, WinHttpOperation::WriteData)));
        bindings.expect_receive_response().never();
        bindings.expect_query_headers().never();
        bindings.expect_query_option().never();
        bindings.expect_close_handle().times(3).returning(move |_| {
            closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        bindings
    }

    fn raw_handle() -> crate::handle::RawHandle {
        crate::handle::RawHandle::new(std::ptr::dangling_mut::<c_void>()).unwrap()
    }

    fn raw_handle_value(value: usize) -> crate::handle::RawHandle {
        crate::handle::RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).unwrap()
    }

    fn clock() -> Clock {
        ClockControl::new().to_clock()
    }
}
