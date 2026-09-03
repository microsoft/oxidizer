// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use observed::{Sink, emit, event};

use crate::session::{SessionInitializationFailure, SessionInitializationOperation};

#[derive(Clone, Debug)]
/// Routes WinHTTP lifecycle signals to bounded metrics and diagnostic logs.
///
/// Request attempts and failures increment zero-dimensional counters so metrics
/// remain low-cardinality. Initialization details and per-request diagnostic
/// context are carried only by log fields through the configured sink.
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
/// Describes the session setup operation that prevented materialization.
///
/// Its operation and operating-system code are diagnostic log fields; session
/// initialization failures do not introduce metric dimensions.
struct InitializationFailure {
    #[dimension(log = "winhttp.operation")]
    #[unredacted]
    operation: &'static str,
    #[dimension(log = "winhttp.error_code")]
    #[unredacted]
    code: u32,
}

#[event("fetch.winhttp.request.accepted")]
#[counter(
    name = "fetch.winhttp.request.accepted.count",
    desc = "WinHTTP transport request attempts",
    unit = "{request}"
)]
/// Represents one transport-accepted request for the request counter.
///
/// The event deliberately has no fields so every request contributes to one
/// low-cardinality metric series.
struct RequestAttempt;

#[event("fetch.winhttp.request.error")]
#[error("WinHTTP transport request failed")]
#[counter(
    name = "fetch.winhttp.request.error.count",
    desc = "Failed WinHTTP transport request attempts",
    unit = "{error}"
)]
/// Describes one failed request for metrics and diagnostic logs.
///
/// Every event increments the same fieldless error counter. When WinHTTP
/// observed that the request began a fresh physical connection, the event may
/// also carry that attribution and its connection duration as log-only fields;
/// those values never become metric dimensions.
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
        SessionInitializationOperation::ConnectionIdleTimeout => "connection_idle_timeout",
        SessionInitializationOperation::AssuredNonBlockingCallbacks => "assured_non_blocking_callbacks",
        SessionInitializationOperation::Http2KeepAlive => "http2_keep_alive",
        SessionInitializationOperation::Http3KeepAlive => "http3_keep_alive",
        SessionInitializationOperation::SetStatusCallback => "set_status_callback",
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ops::ControlFlow;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::time::Duration;

    use observed::metadata::InstrumentKind;
    use observed::{Event as _, Severity};
    use observed_testing::{ExpectedEvent, ExpectedEventDescription, TEST_ID, test_emitter};
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{InitializationFailure, RequestAttempt, RequestError, Telemetry, operation_name};
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
            // The severity argument is inert for a metric-only event: without `.log()` the
            // comparison asserts only that the description carries no log signal.
            ExpectedEventDescription::new("fetch.winhttp.request.accepted", Severity::Info)
                .event_metric("fetch.winhttp.request.accepted.count", InstrumentKind::Counter),
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
                .dimension("winhttp.operation", "assured_non_blocking_callbacks"),
        );
        assert_eq!(
            events[1],
            ExpectedEvent::without_severity("fetch.winhttp.request.accepted").metric()
        );
        assert_eq!(
            events[2],
            ExpectedEvent::new("fetch.winhttp.request.error", Severity::Error)
                .body("WinHTTP transport request failed")
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

    #[test]
    fn every_initialization_step_has_a_stable_dimension_name() {
        // These names are emitted as telemetry dimension values, so downstream
        // queries depend on them staying exactly as written.
        let cases = [
            (SessionInitializationOperation::Open, "open"),
            (SessionInitializationOperation::SetTimeouts, "set_timeouts"),
            (SessionInitializationOperation::DisableGlobalPooling, "disable_global_pooling"),
            (SessionInitializationOperation::ConnectionIdleTimeout, "connection_idle_timeout"),
            (
                SessionInitializationOperation::AssuredNonBlockingCallbacks,
                "assured_non_blocking_callbacks",
            ),
            (SessionInitializationOperation::Http2KeepAlive, "http2_keep_alive"),
            (SessionInitializationOperation::Http3KeepAlive, "http3_keep_alive"),
            (SessionInitializationOperation::SetStatusCallback, "set_status_callback"),
        ];

        for (operation, expected) in cases {
            assert_eq!(operation_name(operation), expected);
        }
    }
}
