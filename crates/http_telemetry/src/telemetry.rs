// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The telemetry middleware, and the layer that creates it.

use std::any::type_name;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use futures::FutureExt;
use futures::future::Either;
use http::header::HOST;
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::{Layer, Service};
use observed::{Event, Sink};
use templated_uri::BasePath;
use tick::SimpleClock;

use crate::server::{
    ActiveRequests, OTHER, RequestAbandoned, RequestComplete, RequestEnd, RequestError, TelemetryDropGuard, UriAuthority,
    emit_when_body_ends,
};

/// Produces telemetry about the requests that go through it, in the form of `observed` events.
///
/// Create the layer with [`Telemetry::server`], set its configuration, and apply it with
/// [`Layer::layer`].
#[derive(Clone)]
pub struct Telemetry<T> {
    pub(crate) inner: T,
    pub(crate) layer: TelemetryLayer,
}

impl Telemetry<()> {
    /// Creates a [`Layer::layer`] whose middleware produces telemetry about incoming HTTP requests.
    #[must_use]
    pub fn server(sink: Sink) -> TelemetryLayer {
        TelemetryLayer {
            sink,
            clock: SimpleClock::new_system(),
            excluded_paths: ExcludedPaths::default(),
        }
    }
}

impl<T: RequestHandler> Service<HttpRequest> for Telemetry<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let layer = &self.layer;

        if layer.excluded_paths.contains(input.uri().path()) {
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

impl<T> fmt::Debug for Telemetry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// The layer that creates the [`Telemetry`] middleware.
///
/// Create one with [`Telemetry::server`].
#[derive(Clone)]
pub struct TelemetryLayer {
    pub(crate) sink: Sink,
    pub(crate) clock: SimpleClock,
    pub(crate) excluded_paths: ExcludedPaths,
}

impl TelemetryLayer {
    /// Sets the clock that measures the duration of each request.
    ///
    /// The default is the system clock, [`SimpleClock::new_system`].
    #[must_use]
    pub fn clock(mut self, clock: impl Into<SimpleClock>) -> Self {
        self.clock = clock.into();
        self
    }

    /// Sets request base paths to be excluded from telemetry.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_telemetry::Telemetry;
    /// use observed::Sink;
    /// use templated_uri::BasePath;
    ///
    /// # fn build(sink: Sink) {
    /// let layer = Telemetry::server(sink).excluded_paths([BasePath::from_static("/health/")]);
    /// # let _ = layer;
    /// # }
    /// ```
    #[must_use]
    pub fn excluded_paths(mut self, paths: impl IntoIterator<Item = impl Into<BasePath>>) -> Self {
        self.excluded_paths = ExcludedPaths::new(paths);
        self
    }
}

impl<S> Layer<S> for TelemetryLayer {
    type Service = Telemetry<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Telemetry {
            inner,
            layer: self.clone(),
        }
    }
}

impl fmt::Debug for TelemetryLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// The base paths whose requests get no telemetry, each without its final `/`.
#[derive(Clone, Default)]
pub(crate) struct ExcludedPaths(Arc<[Box<str>]>);

impl ExcludedPaths {
    fn new(paths: impl IntoIterator<Item = impl Into<BasePath>>) -> Self {
        let paths = paths.into_iter().map(|path| {
            let path = path.into();
            path.as_str().strip_suffix('/').unwrap_or(path.as_str()).into()
        });
        Self(paths.collect())
    }

    /// Whether `path` is one of the base paths or a path under one, ignoring ASCII case. For example, `/health/`
    /// covers `/health` and `/HEALTH/live`, but not `/healths`.
    pub(crate) fn contains(&self, path: &str) -> bool {
        self.0.iter().any(|base| {
            let Some((start, rest)) = path.as_bytes().split_at_checked(base.len()) else {
                return false;
            };
            start.eq_ignore_ascii_case(base.as_bytes()) && matches!(rest.first(), None | Some(b'/'))
        })
    }
}
