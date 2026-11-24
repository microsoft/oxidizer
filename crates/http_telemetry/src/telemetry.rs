// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The telemetry middleware and the layer that creates it: the parts that both sides share.

use std::any::type_name;
use std::fmt;

use futures::future::Either;
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::Service;
use observed::Sink;
use observed::metadata::EventDescription;
use tick::SimpleClock;

use crate::client::Client;
use crate::server::Server;

/// Produces telemetry about the requests that go through it, in the form of `observed` events.
///
/// Create the layer with [`Telemetry::client`] or [`Telemetry::server`], set its configuration, and
/// apply it with [`Layer::layer`](layered::Layer::layer).
#[derive(Clone)]
pub struct Telemetry<T> {
    pub(crate) inner: T,
    pub(crate) sink: Sink,
    pub(crate) clock: SimpleClock,
    pub(crate) role: Role,
}

/// The side of a [`Telemetry`] middleware, with its configuration.
#[derive(Clone)]
pub(crate) enum Role {
    Client(Client),
    Server(Server),
}

impl<T: RequestHandler> Service<HttpRequest> for Telemetry<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        match &self.role {
            Role::Client(client) => Either::Left(self.execute_client(client, input)),
            Role::Server(server) => Either::Right(self.execute_server(server, input)),
        }
    }
}

impl<T> Telemetry<T> {
    /// Whether a processor of the sink is interested in one of the events `descriptions`.
    pub(crate) fn is_interested(&self, descriptions: &[EventDescription]) -> bool {
        descriptions.iter().any(|description| self.sink.is_interested(description))
    }
}

impl<T: fmt::Debug> fmt::Debug for Telemetry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// The layer that creates the [`Telemetry`] middleware, on the side `R`: [`Client`] or [`Server`].
///
/// Create one with [`Telemetry::client`] or [`Telemetry::server`].
#[derive(Clone)]
pub struct TelemetryLayer<R> {
    pub(crate) sink: Sink,
    pub(crate) clock: SimpleClock,
    pub(crate) role: R,
}

impl<R> TelemetryLayer<R> {
    pub(crate) fn new(sink: Sink, role: R) -> Self {
        Self {
            sink,
            clock: SimpleClock::new_system(),
            role,
        }
    }
}

/// Configuration shared by client and server layers.
impl<R> TelemetryLayer<R> {
    /// Sets the clock that measures the duration of each request.
    ///
    /// The default is the system clock, [`SimpleClock::new_system`].
    #[must_use]
    pub fn clock(mut self, clock: impl Into<SimpleClock>) -> Self {
        self.clock = clock.into();
        self
    }
}

impl<R> fmt::Debug for TelemetryLayer<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}
