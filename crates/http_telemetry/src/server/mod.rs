// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server side, which produces telemetry about incoming HTTP requests.

mod drop_guard;
mod events;
mod response_body;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use drop_guard::{RequestEnd, TelemetryDropGuard, UriAuthority};
use events::{ActiveRequests, RequestAbandoned, RequestComplete, RequestError};
use futures::FutureExt;
use futures::future::Either;
use http::header::HOST;
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

        let path = input.uri().path().as_bytes();
        let excluded = layer.excluded_paths.iter().any(|excluded_path| {
            path.get(..excluded_path.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(excluded_path.as_bytes()))
        });
        if excluded {
            return Either::Left(self.inner.execute(input));
        }

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

        let (scheme, default_port) = match input.uri().scheme_str() {
            Some("http") => (Some("http"), Some(80)),
            Some("https") => (Some("https"), Some(443)),
            Some(_) => (Some(OTHER), None),
            None => (None, None),
        };

        let mut guard = TelemetryDropGuard {
            sink: layer.sink.clone(),
            stopwatch: layer.clock.stopwatch(),
            enrichment: layer.sink.transfer_context(),
            method: input.method().clone(),
            version: input.version(),
            scheme,
            uri_authority: match input.uri().authority() {
                Some(authority) => Some(UriAuthority::Uri(authority.clone(), default_port)),
                None => input.headers().get(HOST).cloned().map(UriAuthority::Host),
            },
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
