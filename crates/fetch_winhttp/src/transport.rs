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
use crate::request::ContextPool;
use crate::session::{SessionInitializationFailure, WinHttpSession};
use crate::telemetry::Telemetry;

const UNAVAILABLE_MESSAGE: &str = "WinHTTP request handling is unavailable";

#[derive(Debug)]
pub(crate) struct WinHttpTransport {
    telemetry: Telemetry,
    state: TransportState,
}

impl WinHttpTransport {
    pub(crate) fn new(inputs: TransportInputs, bindings: Facade) -> Self {
        let telemetry = Telemetry::new(inputs.sink);
        let state = match WinHttpSession::new(bindings, &inputs.session_options) {
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

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        self.telemetry.request_attempted();

        let error = match &self.state {
            TransportState::Ready(ready) => {
                let _ = (
                    ready.session.handle(),
                    &ready.body_builder,
                    &ready.clock,
                    &ready.global_pool,
                    &ready.contexts,
                    &ready.options,
                    &ready.tls,
                );
                HttpError::other(UNAVAILABLE_MESSAGE, RecoveryInfo::never(), error_labels::REQUEST_WINHTTP)
            }
            TransportState::Failed(failed) => HttpError::other(failed.failure.clone(), RecoveryInfo::never(), error_labels::INITIALIZATION),
        }
        .with_request(input);

        self.telemetry.request_failed();
        Err(error)
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

    use bytesbuf::mem::GlobalPool;
    use fetch::options::TransportOptions;
    use fetch::{HttpBodyBuilder, HttpRequest, HttpRequestBuilder, Recovery, RecoveryInfo};
    use layered::Service;
    use observed::Sink;
    use observed_testing::{TEST_ID, test_emitter};
    use ohno::Labeled as _;
    use static_assertions::assert_impl_all;
    use tick::{Clock, ClockControl};

    use super::{ReadyTransport, TransportInputs, TransportState, UNAVAILABLE_MESSAGE, WinHttpTransport};
    use crate::WinHttpTlsConfig;
    use crate::bindings::{Facade, MockBindings};
    use crate::context::RequestContext;
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::request::ContextPool;

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
    fn ready_transport_retains_inputs_without_request_io() {
        let (sink, processor) = test_emitter(TEST_ID);
        let mut bindings = successful_bindings();
        bindings.expect_connect().never();
        bindings.expect_open_request().never();
        let transport = WinHttpTransport::new(inputs(sink), Facade::mock(Arc::new(bindings)));

        let mut error =
            futures::executor::block_on(transport.execute(request("https://example.com/"))).expect_err("request lifecycle is unavailable");

        assert!(error.to_string().contains(UNAVAILABLE_MESSAGE));
        assert_eq!(error.label(), "request_winhttp");
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_eq!(
            error.take_request().expect("the request is attached").uri().to_string(),
            "https://example.com/"
        );
        let names = processor
            .events()
            .into_iter()
            .map(|event| event.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["fetch.winhttp.request", "fetch.winhttp.request.error",]);
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
        let clock = clock();
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

    fn raw_handle() -> crate::handle::RawHandle {
        crate::handle::RawHandle::new(std::ptr::dangling_mut::<c_void>()).expect("the standard dangling pointer is non-null")
    }

    fn clock() -> Clock {
        ClockControl::new().to_clock()
    }
}
