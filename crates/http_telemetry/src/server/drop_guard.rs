// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The drop guard that emits the event of one request.

use std::sync::Arc;

use http::{Method, StatusCode};
use http_extensions::{HttpError, collect_error_labels};
use observed::context::Transfer;
use observed::{Sink, Text, emit};
use tick::Stopwatch;

use super::OTHER;
use super::events::{RequestAbandoned, RequestComplete, RequestError};

/// The texts of the statuses 500 to 599, three digits each. `StatusCode::as_str` returns a text that
/// borrows the status, so the `error.type` of a complete event comes from here.
const SERVER_ERROR_STATUSES: &str = concat!(
    "500501502503504505506507508509",
    "510511512513514515516517518519",
    "520521522523524525526527528529",
    "530531532533534535536537538539",
    "540541542543544545546547548549",
    "550551552553554555556557558559",
    "560561562563564565566567568569",
    "570571572573574575576577578579",
    "580581582583584585586587588589",
    "590591592593594595596597598599",
);

#[derive(Clone, Copy)]
pub(crate) enum RequestEnd<'a> {
    /// The handler returned a response with this status.
    Complete(StatusCode),
    /// The handler returned this error.
    Error(&'a HttpError),
    /// The handler panicked.
    Panic,
    /// The caller dropped the request future before the handler returned.
    Abandoned,
}

/// Drop guard that emits the event of one request, also when the caller drops the request future.
///
/// The service creates the guard in `execute`, so the duration starts at the call. [`emit`]
/// emits the event of the request. If the caller drops the request future before that, the guard
/// emits `http.server.request.abandoned`.
///
/// [`emit`]: Self::emit
pub(crate) struct TelemetryDropGuard<'a> {
    pub(crate) sink: &'a Sink,
    pub(crate) stopwatch: Stopwatch,
    pub(crate) enrichment: Transfer,
    pub(crate) method: Method,
    pub(crate) protocol_version: &'static str,
    pub(crate) finished: bool,
}

impl TelemetryDropGuard<'_> {
    pub(crate) fn emit(&mut self, end: RequestEnd<'_>) {
        self.finished = true;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "observed has no u128 value, and the range of u64 is more than enough for a request duration."
        )]
        let duration_ms = self.stopwatch.elapsed().as_millis() as u64;
        let _enrichment = self.enrichment.apply_current_thread();
        let method = http_method_name(&self.method);
        let method_original = (method == OTHER).then(|| Text::from(Arc::<str>::from(self.method.as_str())));
        let protocol_version = self.protocol_version;
        match end {
            RequestEnd::Complete(status_code) => {
                let error_type = status_code.as_u16().checked_sub(500).and_then(|index| {
                    let start = usize::from(index) * 3;
                    SERVER_ERROR_STATUSES.get(start..start + 3)
                });
                emit!(
                    self.sink,
                    RequestComplete {
                        method,
                        method_original,
                        protocol_version,
                        status_code: i64::from(status_code.as_u16()),
                        error_type,
                        duration_ms,
                    }
                );
            }
            RequestEnd::Error(error) => emit!(
                self.sink,
                RequestError {
                    method,
                    method_original,
                    protocol_version,
                    error_type: Text::from(collect_error_labels(error).into_cow()),
                    duration_ms,
                }
            ),
            RequestEnd::Panic => emit!(
                self.sink,
                RequestError {
                    method,
                    method_original,
                    protocol_version,
                    error_type: Text::from("panic"),
                    duration_ms,
                }
            ),
            RequestEnd::Abandoned => emit!(
                self.sink,
                RequestAbandoned {
                    method,
                    method_original,
                    protocol_version,
                    error_type: "abandoned",
                    duration_ms,
                }
            ),
        }
    }
}

/// The value of `http.request.method` for `method`.
const fn http_method_name(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => OTHER,
    }
}

impl Drop for TelemetryDropGuard<'_> {
    fn drop(&mut self) {
        // Emit nothing while the thread panics, because a panic in a processor now aborts the process.
        if self.finished || std::thread::panicking() {
            return;
        }

        self.emit(RequestEnd::Abandoned);
    }
}
