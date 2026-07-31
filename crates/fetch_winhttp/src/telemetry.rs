// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use observed::{Sink, emit, event};

use crate::session::{SessionInitializationFailure, SessionInitializationOperation};

#[derive(Clone, Debug)]
/// Emits bounded WinHTTP lifecycle events through the configured sink.
pub(crate) struct Telemetry {
    sink: Sink,
}

impl Telemetry {
    pub(crate) const fn new(sink: Sink) -> Self {
        Self { sink }
    }

    pub(crate) fn initialization_failed(&self, failure: &SessionInitializationFailure) {
        emit!(
            self.sink,
            InitializationFailure {
                operation: operation_name(failure.operation()),
                code: failure.code(),
            }
        );
    }

    pub(crate) fn request_attempted(&self) {
        emit!(self.sink, RequestAttempt);
    }

    pub(crate) fn request_failed(&self, cold_connect_duration: Option<Duration>) {
        emit!(
            self.sink,
            RequestError {
                fresh_connection: cold_connect_duration.map(|_| true),
                connect_duration: cold_connect_duration.map(|duration| duration.as_secs_f64()),
            }
        );
    }
}

#[event("fetch.winhttp.session.initialization.failure")]
#[error("WinHTTP transport initialization failed")]
/// Records the failed session setup step and operating-system error code.
struct InitializationFailure {
    #[dimension(log = "winhttp.operation")]
    #[unredacted]
    operation: &'static str,
    #[dimension(log = "winhttp.error_code")]
    #[unredacted]
    code: u32,
}

#[event("fetch.winhttp.request")]
#[counter(
    name = "fetch.winhttp.request.count",
    desc = "WinHTTP transport request attempts",
    unit = "{request}"
)]
/// Counts requests accepted by the WinHTTP transport.
struct RequestAttempt;

#[event("fetch.winhttp.request.error")]
#[error("WinHTTP transport request failed")]
#[counter(
    name = "fetch.winhttp.request.error.count",
    desc = "Failed WinHTTP transport request attempts",
    unit = "{error}"
)]
/// Records failed requests and optional fresh-connection timing.
struct RequestError {
    #[dimension(log = "winhttp.connection.fresh")]
    #[if_none(drop)]
    #[unredacted]
    fresh_connection: Option<bool>,
    #[dimension(log = "winhttp.connect.duration")]
    #[if_none(drop)]
    #[unredacted]
    connect_duration: Option<f64>,
}

const fn operation_name(operation: SessionInitializationOperation) -> &'static str {
    match operation {
        SessionInitializationOperation::Open => "open",
        SessionInitializationOperation::SetTimeouts => "set_timeouts",
        SessionInitializationOperation::DisableGlobalPooling => "disable_global_pooling",
        SessionInitializationOperation::AssuredNonBlockingCallbacks => "assured_non_blocking_callbacks",
        SessionInitializationOperation::Http2KeepAlive => "http2_keep_alive",
        SessionInitializationOperation::Http3KeepAlive => "http3_keep_alive",
        SessionInitializationOperation::SetStatusCallback => "set_status_callback",
    }
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::time::Duration;

    use observed::metadata::InstrumentKind;
    use observed::{Event as _, Severity};
    use observed_testing::{ExpectedEvent, ExpectedEventDescription, TEST_ID, test_emitter};
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{InitializationFailure, RequestAttempt, RequestError, Telemetry};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::session::{SessionInitializationFailure, SessionInitializationOperation};

    // Sink contains a user-erased event emitter without unwind-safety bounds.
    assert_not_impl_any!(Telemetry: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InitializationFailure: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestAttempt: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestError: UnwindSafe, RefUnwindSafe);

    #[test]
    fn event_metadata_uses_exact_names_and_zero_dimensional_metrics() {
        assert_eq!(
            InitializationFailure::DESCRIPTION,
            ExpectedEventDescription::new("fetch.winhttp.session.initialization.failure", Severity::Error)
                .body("WinHTTP transport initialization failed")
                .log(),
        );
        assert_eq!(
            RequestAttempt::DESCRIPTION,
            ExpectedEventDescription::new("fetch.winhttp.request", Severity::Info)
                .event_metric("fetch.winhttp.request.count", InstrumentKind::Counter),
        );
        assert_eq!(
            RequestError::DESCRIPTION,
            ExpectedEventDescription::new("fetch.winhttp.request.error", Severity::Error)
                .body("WinHTTP transport request failed")
                .log()
                .event_metric("fetch.winhttp.request.error.count", InstrumentKind::Counter),
        );

        let request_metric = RequestAttempt::DESCRIPTION.metric().unwrap();
        assert_eq!(request_metric.description(), "WinHTTP transport request attempts");
        assert_eq!(request_metric.unit(), "{request}");

        let error_metric = RequestError::DESCRIPTION.metric().unwrap();
        assert_eq!(error_metric.description(), "Failed WinHTTP transport request attempts");
        assert_eq!(error_metric.unit(), "{error}");
    }

    #[test]
    fn emissions_have_only_the_declared_bounded_fields() {
        let (sink, processor) = test_emitter(TEST_ID);
        let telemetry = Telemetry::new(sink);
        let failure = SessionInitializationFailure::new(
            SessionInitializationOperation::AssuredNonBlockingCallbacks,
            WinHttpError::new(1234, WinHttpOperation::SetOption),
        );

        telemetry.initialization_failed(&failure);
        telemetry.request_attempted();
        telemetry.request_failed(None);

        let events = processor.events();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            ExpectedEvent::new("fetch.winhttp.session.initialization.failure", Severity::Error)
                .body("WinHTTP transport initialization failed")
                .dimension("winhttp.error_code", 1234_u32)
                .dimension("winhttp.operation", "assured_non_blocking_callbacks")
                .log(),
        );
        assert_eq!(events[1], ExpectedEvent::new("fetch.winhttp.request", Severity::Info).metric(),);
        assert_eq!(
            events[2],
            ExpectedEvent::new("fetch.winhttp.request.error", Severity::Error)
                .body("WinHTTP transport request failed")
                .log()
                .metric(),
        );
        assert!(events[1].dimensions().is_empty());
        assert!(events[2].dimensions().is_empty());
    }

    #[test]
    fn cold_connect_fields_are_log_only() {
        let (sink, processor) = test_emitter(TEST_ID);
        let telemetry = Telemetry::new(sink);

        telemetry.request_failed(Some(Duration::from_millis(250)));

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("fetch.winhttp.request.error", Severity::Error)
                .body("WinHTTP transport request failed")
                .dimension("winhttp.connect.duration", 0.25_f64)
                .dimension("winhttp.connection.fresh", true)
                .log()
                .metric(),
        );

        let event = RequestError {
            fresh_connection: Some(true),
            connect_duration: Some(0.25),
        };
        let mut routing = Vec::new();
        let mut visitor = |field: &observed::metadata::FieldDescriptor, _: &observed::processing::FieldValueFn<'_>| {
            routing.push((
                field.field_name(),
                field.log().map(observed::metadata::LogFieldEntry::key),
                field.metric().map(observed::metadata::MetricFieldEntry::key),
            ));
            ControlFlow::Continue(())
        };
        assert_eq!(event.visit_fields(&mut visitor), ControlFlow::Continue(()));
        assert_eq!(
            routing,
            [
                ("fresh_connection", Some("winhttp.connection.fresh"), None),
                ("connect_duration", Some("winhttp.connect.duration"), None),
            ]
        );
    }
}
