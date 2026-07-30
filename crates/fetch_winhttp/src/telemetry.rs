// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::{Sink, emit, event};

use crate::session::{SessionInitializationFailure, SessionInitializationOperation};

#[derive(Clone, Debug)]
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

    pub(crate) fn request_failed(&self) {
        emit!(self.sink, RequestError);
    }
}

#[event("fetch.winhttp.session.initialization.failure")]
#[error("WinHTTP transport initialization failed")]
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
struct RequestAttempt;

#[event("fetch.winhttp.request.error")]
#[counter(
    name = "fetch.winhttp.request.error.count",
    desc = "Failed WinHTTP transport request attempts",
    unit = "{error}"
)]
struct RequestError;

const fn operation_name(operation: SessionInitializationOperation) -> &'static str {
    match operation {
        SessionInitializationOperation::Open => "open",
        SessionInitializationOperation::SetTimeouts => "set_timeouts",
        SessionInitializationOperation::DisableGlobalPooling => "disable_global_pooling",
        SessionInitializationOperation::AssuredNonBlockingCallbacks => "assured_non_blocking_callbacks",
        SessionInitializationOperation::SetStatusCallback => "set_status_callback",
    }
}

#[cfg(test)]
mod tests {
    use observed::metadata::InstrumentKind;
    use observed::{Event as _, Severity};
    use observed_testing::{ExpectedEvent, ExpectedEventDescription, TEST_ID, test_emitter};

    use super::{InitializationFailure, RequestAttempt, RequestError, Telemetry};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::session::{SessionInitializationFailure, SessionInitializationOperation};

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
            ExpectedEventDescription::new("fetch.winhttp.request.error", Severity::Info)
                .event_metric("fetch.winhttp.request.error.count", InstrumentKind::Counter),
        );

        let request_metric = RequestAttempt::DESCRIPTION.metric().expect("request event has a counter");
        assert_eq!(request_metric.description(), "WinHTTP transport request attempts");
        assert_eq!(request_metric.unit(), "{request}");

        let error_metric = RequestError::DESCRIPTION.metric().expect("request-error event has a counter");
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
        telemetry.request_failed();

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
            ExpectedEvent::new("fetch.winhttp.request.error", Severity::Info).metric(),
        );
        assert!(events[1].dimensions().is_empty());
        assert!(events[2].dimensions().is_empty());
    }
}
