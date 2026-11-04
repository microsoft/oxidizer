// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The telemetry middleware, and the layer that creates it.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use layered::Layer;
use observed::Sink;
use tick::SimpleClock;

/// Produces telemetry about the requests that go through it, in the form of `observed` events.
///
/// Create the layer with [`Telemetry::server`], set its configuration, and apply it with
/// [`Layer::layer`].
#[derive(Clone)]
pub struct Telemetry<T> {
    pub(crate) inner: T,
    pub(crate) layer: TelemetryLayer,
}

/// The layer that creates the [`Telemetry`] middleware.
///
/// Create one with [`Telemetry::server`].
#[derive(Clone)]
pub struct TelemetryLayer {
    pub(crate) sink: Sink,
    pub(crate) clock: SimpleClock,
    pub(crate) excluded_paths: Arc<[Box<str>]>,
}

impl Telemetry<()> {
    /// Creates a [`Layer::layer`] whose middleware produces telemetry about incoming HTTP requests.
    #[must_use]
    pub fn server(sink: Sink) -> TelemetryLayer {
        TelemetryLayer {
            sink,
            clock: SimpleClock::new_system(),
            excluded_paths: Arc::default(),
        }
    }
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

    /// Sets request paths to be excluded from telemetry.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_telemetry::Telemetry;
    /// use observed::Sink;
    ///
    /// # fn build(sink: Sink) {
    /// let layer = Telemetry::server(sink).excluded_paths(["/health", "/metrics"]);
    /// # let _ = layer;
    /// # }
    /// ```
    #[must_use]
    pub fn excluded_paths(mut self, paths: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        self.excluded_paths = paths.into_iter().map(|path| path.as_ref().into()).collect();
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

impl<T> fmt::Debug for Telemetry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl fmt::Debug for TelemetryLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}
