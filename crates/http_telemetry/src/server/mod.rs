// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server side, which produces telemetry about incoming HTTP requests.

mod drop_guard;
mod events;

use drop_guard::{RequestEnd, TelemetryDropGuard};
use events::{RequestAbandoned, RequestComplete};
use futures::future::Either;
use http::Version;
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::Service;
use observed::Event;

use crate::telemetry::Telemetry;

/// The replacement value for unknown HTTP methods or HTTP protocol versions. We cannot support
/// arbitrary value there and must have finite lists to avoid dimension cardinality blow up.
const OTHER: &str = "_OTHER";

impl<T: RequestHandler> Service<HttpRequest> for Telemetry<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let layer = &self.layer;

        let interested = [RequestComplete::DESCRIPTION, RequestAbandoned::DESCRIPTION]
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

        let mut guard = TelemetryDropGuard {
            sink: &layer.sink,
            stopwatch: layer.clock.stopwatch(),
            enrichment: layer.sink.transfer_context(),
            method: input.method().clone(),
            protocol_version,
            finished: false,
        };
        let response = self.inner.execute(input);
        Either::Right(async move {
            match response.await {
                Ok(response) => {
                    guard.emit(RequestEnd::Complete(response.status()));
                    Ok(response)
                }
                Err(error) => {
                    guard.finished = true;
                    drop(guard);
                    Err(error)
                }
            }
        })
    }
}
