// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};

use bytesbuf::mem::GlobalPool;
use fetch::options::TransportOptions;
use fetch::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, RecoveryInfo};
use layered::Service;
use observed::Sink;
use tick::Clock;

use crate::WinHttpTlsConfig;
use crate::bindings::Facade;
use crate::error_labels;
use crate::request::{ContextPool, RequestDriver};
use crate::session::{SessionInitializationFailure, WinHttpSession};
use crate::telemetry::Telemetry;

#[derive(Debug)]
pub(crate) struct WinHttpTransport {
    telemetry: Telemetry,
    state: TransportState,
}

impl WinHttpTransport {
    pub(crate) fn new(inputs: TransportInputs, bindings: Facade) -> Self {
        let telemetry = Telemetry::new(inputs.sink);
        let state = match WinHttpSession::new(bindings, &inputs.session_options, &inputs.options.connection_keep_alive) {
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
enum TransportState {
    Ready(Box<ReadyTransport>),
    Failed(FailedTransport),
}

#[derive(Debug)]
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
struct FailedTransport {
    failure: SessionInitializationFailure,
}

#[derive(Debug)]
pub(crate) struct TransportInputs {
    pub(crate) body_builder: HttpBodyBuilder,
    pub(crate) clock: Clock,
    pub(crate) global_pool: GlobalPool,
    pub(crate) sink: Sink,
    pub(crate) options: TransportOptions,
    pub(crate) tls: WinHttpTlsConfig,
    pub(crate) session_options: crate::WinHttpOptions,
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use fetch::options::TransportOptions;
    use fetch::{HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, Recovery, RecoveryInfo};
    use http_extensions::HttpBodyOptions;
    use layered::Service;
    use observed::Severity;
    use observed::Sink;
    use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};
    use ohno::Labeled as _;
    use static_assertions::assert_impl_all;
    use tick::{Clock, ClockControl};

    use super::{ReadyTransport, TransportInputs, TransportState, WinHttpTransport};
    use crate::WinHttpTlsConfig;
    use crate::bindings::{Facade, MockBindings};
    use crate::callback::dispatch_completion;
    use crate::context::RequestContext;
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::options::{
        WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_RAW_HEADERS_CRLF,
        WINHTTP_QUERY_STATUS_CODE,
    };
    use crate::request::ContextPool;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
    };

    assert_impl_all!(WinHttpTransport: Send, Sync, std::fmt::Debug);
    assert_impl_all!(ReadyTransport: Send, Sync, std::fmt::Debug);
    assert_impl_all!(ContextPool: Send, Sync, std::fmt::Debug);
    assert_impl_all!(plurality::Box<RequestContext>: Send, Sync);

    #[test]
    fn failed_transport_returns_fresh_never_recoverable_errors_with_requests() {
        let (sink, processor) = test_emitter(TEST_ID);
        let mut bindings = MockBindings::new();
        bindings
            .expect_open()
            .times(1)
            .returning(|_, _| Err(WinHttpError::new(12029, WinHttpOperation::Open)));
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));

        for uri in ["https://first.example/", "https://second.example/"] {
            let mut error =
                futures::executor::block_on(transport.execute(request(uri))).expect_err("failed transport rejects every request");

            assert_eq!(error.label(), "winhttp_initialization");
            assert_eq!(error.recovery(), RecoveryInfo::never());
            assert_eq!(
                error.take_request().expect("the current request is attached").uri().to_string(),
                uri
            );
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
                "fetch.winhttp.request",
                "fetch.winhttp.request.error",
                "fetch.winhttp.request",
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
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));

        let mut input = request("http://example.com/");
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("untouched"));
        let mut error = futures::executor::block_on(transport.execute(input)).expect_err("plain HTTP is disabled");

        assert!(error.to_string().contains("plain HTTP requests are disabled"));
        assert_eq!(error.label(), "invalid_request");
        assert_eq!(error.recovery(), RecoveryInfo::never());
        let attached = error.take_request().expect("the request is attached");
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
        assert_eq!(names, ["fetch.winhttp.request", "fetch.winhttp.request.error",]);
    }

    #[test]
    fn successful_request_emits_one_attempt_without_an_error_event() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = successful_request_bindings(Arc::clone(&context), Arc::clone(&closes));
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));

        let response = futures::executor::block_on(transport.execute(request("https://example.com/"))).expect("request succeeds");

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
            ["fetch.winhttp.request"]
        );
        assert_eq!(closes.load(Ordering::SeqCst), 0, "the response body owns the request guard");
        drop(response);
        assert_eq!(
            closes.load(Ordering::SeqCst),
            1,
            "dropping the response body closes the request handle"
        );

        let context = std::ptr::with_exposed_provenance_mut(context.load(Ordering::SeqCst));
        // SAFETY: the mock records the live installed RequestContext pointer,
        // and HANDLE_CLOSING is its final synthetic callback.
        unsafe {
            dispatch_completion(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
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
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));
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
            .expect("test request is valid");
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("preserved"));

        let mut error = futures::executor::block_on(transport.execute(input)).expect_err("body error fails the request");

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
            ["fetch.winhttp.request", "fetch.winhttp.request.error"]
        );
        finish_failed_request(transport, &context, &closes);
    }

    #[test]
    fn send_failure_attaches_untouched_non_cloneable_streaming_request() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = upload_failure_bindings(&context, Arc::clone(&closes), UploadFailure::Send, 0);
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));
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
            .expect("test request is valid");
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("preserved"));
        input.extensions_mut().insert(42_u32);

        let mut error = futures::executor::block_on(transport.execute(input)).expect_err("send failure reaches the caller");

        assert_eq!(error.recovery(), RecoveryInfo::retry());
        let attached = error.take_request().expect("the unpolled request is attached");
        assert_eq!(attached.method(), http::Method::POST);
        assert_eq!(attached.uri().to_string(), "https://example.com/upload");
        assert_eq!(attached.version(), http::Version::HTTP_2);
        assert_eq!(
            attached.headers().get("x-original"),
            Some(&http::HeaderValue::from_static("preserved"))
        );
        assert_eq!(attached.extensions().get::<u32>(), Some(&42));
        let text = futures::executor::block_on(attached.into_body().into_text()).expect("the untouched stream remains readable");
        assert_eq!(text, "streaming");
        assert_eq!(
            processor
                .events()
                .into_iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>(),
            ["fetch.winhttp.request", "fetch.winhttp.request.error"]
        );
        finish_failed_request(transport, &context, &closes);
    }

    #[test]
    fn write_failure_attaches_replayable_body_from_original_clone() {
        let (sink, processor) = test_emitter(TEST_ID);
        let context = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let bindings = upload_failure_bindings(&context, Arc::clone(&closes), UploadFailure::Write, 10);
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &clock());
        let mut input = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://example.com/upload")
            .body(body_builder.text("replayable"))
            .expect("test request is valid");
        *input.version_mut() = http::Version::HTTP_2;
        input
            .headers_mut()
            .insert("x-original", http::HeaderValue::from_static("preserved"));
        input.extensions_mut().insert(42_u32);

        let mut error = futures::executor::block_on(transport.execute(input)).expect_err("write failure reaches the caller");

        assert_eq!(error.recovery(), RecoveryInfo::retry());
        let attached = error.take_request().expect("the replayable request is attached");
        assert_eq!(attached.method(), http::Method::POST);
        assert_eq!(attached.uri().to_string(), "https://example.com/upload");
        assert_eq!(attached.version(), http::Version::HTTP_2);
        assert_eq!(
            attached.headers().get("x-original"),
            Some(&http::HeaderValue::from_static("preserved"))
        );
        assert_eq!(attached.extensions().get::<u32>(), Some(&42));
        let text = futures::executor::block_on(attached.into_body().into_text()).expect("the restored body remains readable");
        assert_eq!(text, "replayable");
        assert_eq!(
            processor
                .events()
                .into_iter()
                .map(|event| event.name().to_owned())
                .collect::<Vec<_>>(),
            ["fetch.winhttp.request", "fetch.winhttp.request.error"]
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
        let transport = WinHttpTransport::new(inputs_with_clock(sink, control.to_clock()), Facade::mock(Arc::new(bindings)));

        let mut error = futures::executor::block_on(transport.execute(request("https://example.com/"))).expect_err("cold connect fails");

        assert_eq!(error.label(), "connect");
        assert!(error.take_request().is_some());
        assert_eq!(
            processor.events(),
            [
                ExpectedEvent::new("fetch.winhttp.request", Severity::Info).metric(),
                ExpectedEvent::new("fetch.winhttp.request.error", Severity::Error)
                    .body("WinHTTP transport request failed")
                    .dimension("winhttp.connect.duration", 0.25_f64)
                    .dimension("winhttp.connection.fresh", true)
                    .log()
                    .metric(),
            ]
        );
        finish_failed_request(transport, &context, &closes);
    }

    fn finish_failed_request(transport: WinHttpTransport, context: &Arc<AtomicUsize>, closes: &Arc<AtomicUsize>) {
        assert_eq!(closes.load(Ordering::SeqCst), 1);

        let context = std::ptr::with_exposed_provenance_mut(context.load(Ordering::SeqCst));
        // SAFETY: the mock records the live installed RequestContext pointer,
        // and HANDLE_CLOSING is its final synthetic callback.
        unsafe {
            dispatch_completion(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
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
        let transport = WinHttpTransport::new(inputs(Sink::noop()), Facade::mock(Arc::new(bindings)));

        assert_send(transport.execute(request("https://example.com/")));
    }

    #[test]
    fn ready_transports_own_distinct_context_pools() {
        let first = WinHttpTransport::new(inputs(Sink::noop()), Facade::mock(Arc::new(successful_bindings())));
        let second = WinHttpTransport::new(inputs(Sink::noop()), Facade::mock(Arc::new(successful_bindings())));

        match (&first.state, &second.state) {
            (TransportState::Ready(first), TransportState::Ready(second)) => {
                assert!(!std::ptr::eq(&raw const first.contexts, &raw const second.contexts));
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
            session_options: crate::WinHttpOptions::default(),
        }
    }

    fn request(uri: &str) -> HttpRequest {
        let body_builder = HttpBodyBuilder::new(GlobalPool::new(), &clock());
        HttpRequestBuilder::new(&body_builder)
            .get(uri)
            .build()
            .expect("test request is valid")
    }

    fn successful_bindings() -> MockBindings {
        let mut bindings = MockBindings::new();
        bindings.expect_open().once().returning(|_, _| Ok(raw_handle()));
        bindings.expect_set_timeouts().once().returning(|_, _, _, _, _| Ok(()));
        bindings.expect_set_option().times(2).returning(|_, _, _| Ok(()));
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_close_handle().once().returning(|_| Ok(()));
        bindings
    }

    fn cold_connect_failure_bindings(context: &Arc<AtomicUsize>, closes: Arc<AtomicUsize>, control: ClockControl) -> MockBindings {
        let mut bindings = MockBindings::new();
        bindings.expect_open().once().returning(|_, _| Ok(raw_handle_value(1)));
        bindings.expect_set_timeouts().once().returning(|_, _, _, _, _| Ok(()));

        let context_option = Arc::clone(context);
        bindings.expect_set_option().returning(move |_, option, value| {
            if option == WINHTTP_OPTION_CONTEXT_VALUE {
                context_option.store(
                    usize::from_ne_bytes(value.try_into().expect("the context option is pointer-sized")),
                    Ordering::SeqCst,
                );
            }
            Ok(())
        });
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_connect().once().returning(|_, _, _| Ok(raw_handle_value(2)));
        bindings
            .expect_open_request()
            .once()
            .returning(|_, _, _, _| Ok(raw_handle_value(3)));
        bindings.expect_send_request().once().returning(move |_, _, total_len, context| {
            assert_eq!(total_len, 0);
            let context = std::ptr::with_exposed_provenance_mut(context);
            // SAFETY: send receives the installed live context and dispatch is
            // synchronous while the operation is armed.
            unsafe {
                dispatch_completion(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
            }
            control.advance(Duration::from_millis(250));
            let mut result = WINHTTP_ASYNC_RESULT {
                dwResult: 0,
                dwError: 12029,
            };
            // SAFETY: the result storage remains valid for the synchronous
            // callback, and the context still owns the active send operation.
            unsafe {
                dispatch_completion(
                    context,
                    WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                    (&raw mut result).cast(),
                    u32::try_from(size_of::<WINHTTP_ASYNC_RESULT>()).expect("status info length fits a DWORD"),
                );
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

        let context_option = Arc::clone(&context);
        bindings.expect_set_option().returning(move |_, option, value| {
            if option == WINHTTP_OPTION_CONTEXT_VALUE {
                context_option.store(
                    usize::from_ne_bytes(value.try_into().expect("the context option is pointer-sized")),
                    Ordering::SeqCst,
                );
            }
            Ok(())
        });
        bindings.expect_set_status_callback().once().returning(|_, _, _| Ok(()));
        bindings.expect_connect().once().returning(|_, _, _| Ok(raw_handle_value(2)));
        bindings
            .expect_open_request()
            .once()
            .returning(|_, _, _, _| Ok(raw_handle_value(3)));

        bindings.expect_send_request().once().returning(|_, _, total_len, context| {
            assert_eq!(total_len, 0);
            // SAFETY: send receives the installed live context and dispatch is
            // synchronous while the operation is armed.
            unsafe {
                dispatch_completion(
                    std::ptr::with_exposed_provenance_mut(context),
                    WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                    std::ptr::null_mut(),
                    0,
                );
            }
            Ok(())
        });
        let receive_context = Arc::clone(&context);
        bindings.expect_receive_response().once().returning(move |_| {
            // SAFETY: receive runs after context installation and while the
            // headers operation is armed.
            unsafe {
                dispatch_completion(
                    std::ptr::with_exposed_provenance_mut(receive_context.load(Ordering::SeqCst)),
                    WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
                    std::ptr::null_mut(),
                    0,
                );
            }
            Ok(())
        });
        bindings.expect_query_headers().returning(|_, info_level, buffer, byte_len| {
            if info_level == (WINHTTP_QUERY_STATUS_CODE | 0x2000_0000) {
                let output = buffer.expect("status query supplies a DWORD").cast::<u32>();
                // SAFETY: the transport supplies a writable DWORD.
                unsafe { output.as_ptr().write(200) };
                *byte_len = 4;
                return Ok(());
            }
            assert_eq!(info_level, WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_WIRE_ENCODING);
            let bytes = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
            let required = u32::try_from(bytes.len() + 1).expect("test headers fit a DWORD");
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
            *byte_len = u32::try_from(bytes.len()).expect("test headers fit a DWORD");
            Ok(())
        });
        bindings.expect_query_option().once().returning(|_, option, buffer, byte_len| {
            assert_eq!(option, WINHTTP_OPTION_HTTP_PROTOCOL_USED);
            let output = buffer.expect("protocol query supplies a DWORD").cast::<u32>();
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

        let context_option = Arc::clone(context);
        bindings.expect_set_option().returning(move |_, option, value| {
            if option == WINHTTP_OPTION_CONTEXT_VALUE {
                context_option.store(
                    usize::from_ne_bytes(value.try_into().expect("the context option is pointer-sized")),
                    Ordering::SeqCst,
                );
            }
            Ok(())
        });
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
            // SAFETY: send receives the installed live context and dispatch is
            // synchronous while the operation is armed.
            unsafe {
                dispatch_completion(
                    std::ptr::with_exposed_provenance_mut(context),
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
        crate::handle::RawHandle::new(std::ptr::dangling_mut::<c_void>()).expect("the standard dangling pointer is non-null")
    }

    fn raw_handle_value(value: usize) -> crate::handle::RawHandle {
        crate::handle::RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).expect("test handle values are nonzero")
    }

    fn clock() -> Clock {
        ClockControl::new().to_clock()
    }
}
