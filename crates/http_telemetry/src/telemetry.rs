// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The telemetry middleware, and the layer that creates it.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use http_extensions::HttpBodyBuilder;
use layered::Layer;
use observed::Sink;
use templated_uri::BasePath;
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
    pub(crate) body_builder: HttpBodyBuilder,
    pub(crate) clock: SimpleClock,
    pub(crate) excluded_paths: Arc<[Box<str>]>,
}

impl Telemetry<()> {
    /// Creates a [`Layer::layer`] whose middleware produces telemetry about incoming HTTP requests.
    #[must_use]
    pub fn server(sink: Sink, body_builder: HttpBodyBuilder) -> TelemetryLayer {
        TelemetryLayer {
            sink,
            body_builder,
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

    /// Sets request base paths to be excluded from telemetry.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_extensions::HttpBodyBuilder;
    /// use http_telemetry::Telemetry;
    /// use observed::Sink;
    /// use templated_uri::BasePath;
    ///
    /// # fn build(sink: Sink, body_builder: HttpBodyBuilder) {
    /// let layer =
    ///     Telemetry::server(sink, body_builder).excluded_paths([BasePath::from_static("/health/")]);
    /// # let _ = layer;
    /// # }
    /// ```
    #[must_use]
    pub fn excluded_paths(mut self, paths: impl IntoIterator<Item = impl Into<BasePath>>) -> Self {
        self.excluded_paths = paths
            .into_iter()
            .map(|path| {
                let path = path.into();
                path.as_str().strip_suffix('/').unwrap_or(path.as_str()).into()
            })
            .collect();
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
