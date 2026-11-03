// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server side, which produces telemetry about incoming HTTP requests.

mod drop_guard;
mod events;
mod response_body;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use drop_guard::{RequestEnd, TelemetryDropGuard};
use events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};
use futures::FutureExt;
use futures::future::Either;
use http::Version;
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::Service;
use observed::Event;
use response_body::emit_when_body_ends;

use crate::telemetry::Telemetry;

/// The replacement value for unknown HTTP methods, URL schemes, or HTTP protocol versions. We cannot support
/// arbitrary value there and must have finite lists to avoid dimension cardinality blow up.
const OTHER: &str = "_OTHER";

impl<T: RequestHandler> Service<HttpRequest> for Telemetry<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let layer = &self.layer;

        let interested = [
            RequestComplete::DESCRIPTION,
            RequestError::DESCRIPTION,
            RequestAbandoned::DESCRIPTION,
            ActiveRequests::DESCRIPTION,
        ]
        .iter()
        .any(|description| layer.sink.is_interested(description));
        if !interested {
            return Either::Left(self.inner.execute(input));
        }

        let protocol_version = match input.version() {
            Version::HTTP_10 => "1.0",
            Version::HTTP_11 => "1.1",
            Version::HTTP_2 => "2",
            Version::HTTP_3 => "3",
            _ => OTHER,
        };

        let scheme = input.uri().scheme_str().map(|scheme| match scheme {
            "http" => "http",
            "https" => "https",
            _ => OTHER,
        });

        let mut guard = TelemetryDropGuard {
            sink: layer.sink.clone(),
            stopwatch: layer.clock.stopwatch(),
            enrichment: layer.sink.transfer_context(),
            method: input.method().clone(),
            protocol_version,
            scheme,
            finished: false,
        };
        guard.add_active_request(1);
        // The middleware resumes each panic that it catches and never polls the handler again, so the
        // handler does not need to be unwind safe. The handler can panic in the call too, because
        // `Execute` runs the handler closure in `execute`.
        let response = match catch_unwind(AssertUnwindSafe(|| self.inner.execute(input))) {
            Ok(response) => response,
            Err(payload) => {
                guard.emit(RequestEnd::Panic(None));
                resume_unwind(payload)
            }
        };
        Either::Right(async move {
            match AssertUnwindSafe(response).catch_unwind().await {
                Ok(Ok(response)) => Ok(emit_when_body_ends(guard, response)),
                Ok(Err(error)) => {
                    guard.emit(RequestEnd::Error(&error, None));
                    Err(error)
                }
                Err(payload) => {
                    guard.emit(RequestEnd::Panic(None));
                    resume_unwind(payload)
                }
            }
        })
    }
}
