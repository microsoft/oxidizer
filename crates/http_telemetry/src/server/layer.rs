// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The server layer: its configuration and the methods that set it.

use std::any::type_name;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use data_privacy::{DataClass, IntoDataClass};
use http::{HeaderMap, HeaderName};
use layered::Layer;
use observed::Sink;
use routerama::RouteTable;
use templated_uri::BasePath;

use super::excluded_paths::ExcludedPaths;
use super::media_types::{MediaTypeError, MediaTypes};
use super::route_fields::RouteFields;
use crate::header_fields::{HeaderField, REQUEST_HEADER_PREFIX, RESPONSE_HEADER_PREFIX, header_fields};
use crate::telemetry::Role;
use crate::{Telemetry, TelemetryLayer};

/// Default logged body size in bytes.
const DEFAULT_BODY_SIZE_LIMIT: usize = 32 * 1024;

/// The maximum supported value for `body_size_limit`.
const MAX_BODY_SIZE_LIMIT: usize = 1536 * 1024;

/// Server-side configuration held by a [`TelemetryLayer`].
///
/// A server accepts requests and responds to them. Create its layer with [`Telemetry::server`].
#[derive(Clone)]
#[non_exhaustive]
pub struct Server {
    pub(crate) excluded_paths: ExcludedPaths,
    pub(crate) route_resolver: Option<Arc<dyn RouteTable + Send + Sync>>,
    pub(crate) route_parameter_data_classes: BTreeMap<String, DataClass>,
    pub(crate) route_fields: Option<Arc<RouteFields>>,
    pub(crate) request_header_data_classes: HeaderMap<DataClass>,
    pub(crate) request_header_fields: Option<Arc<[HeaderField]>>,
    pub(crate) response_header_data_classes: HeaderMap<DataClass>,
    pub(crate) response_header_fields: Option<Arc<[HeaderField]>>,
    pub(crate) request_body_media_types: MediaTypes,
    pub(crate) response_body_media_types: MediaTypes,
    pub(crate) body_size_limit: usize,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Telemetry<()> {
    /// Creates a [`Layer::layer`](layered::Layer::layer) whose middleware produces telemetry about incoming HTTP
    /// requests.
    #[must_use]
    pub fn server(sink: Sink) -> TelemetryLayer<Server> {
        TelemetryLayer::new(
            sink,
            Server {
                excluded_paths: ExcludedPaths::default(),
                route_resolver: None,
                route_parameter_data_classes: BTreeMap::new(),
                route_fields: None,
                request_header_data_classes: HeaderMap::default(),
                request_header_fields: None,
                response_header_data_classes: HeaderMap::default(),
                response_header_fields: None,
                request_body_media_types: MediaTypes::default(),
                response_body_media_types: MediaTypes::default(),
                body_size_limit: DEFAULT_BODY_SIZE_LIMIT,
            },
        )
    }
}

/// Server-only configuration.
impl TelemetryLayer<Server> {
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
        self.role.excluded_paths = ExcludedPaths::new(paths);
        self
    }

    /// Sets the resolver that matches each request to its route.
    ///
    /// The middleware does not see how the application routes a request, so it matches the request again with this
    /// resolver. If a route matches, the events of the request have the field `http.route`.
    #[must_use]
    pub fn route_resolver(mut self, resolver: impl RouteTable + Send + Sync + 'static) -> Self {
        self.role.route_resolver = Some(Arc::new(resolver));
        self.role.route_fields = RouteFields::new(&self.role).map(Arc::new);
        self
    }

    /// Adds route parameters to log and their data classes for redaction.
    ///
    /// By default, no route parameter is logged. A layer logs route parameters only if it has a
    /// [resolver](Self::route_resolver).
    #[must_use]
    pub fn route_parameter_data_classes(mut self, data_classes: impl IntoIterator<Item = (impl Into<String>, impl IntoDataClass)>) -> Self {
        for (name, class) in data_classes {
            self.role.route_parameter_data_classes.insert(name.into(), class.into_data_class());
        }
        self.role.route_fields = RouteFields::new(&self.role).map(Arc::new);
        self
    }

    /// Adds request headers to log and their data classes for redaction.
    ///
    /// By default, no request header is logged.
    #[must_use]
    pub fn request_header_data_classes(mut self, data_classes: impl IntoIterator<Item = (HeaderName, impl IntoDataClass)>) -> Self {
        for (name, class) in data_classes {
            self.role.request_header_data_classes.insert(name, class.into_data_class());
        }
        self.role.request_header_fields = header_fields(&self.role.request_header_data_classes, REQUEST_HEADER_PREFIX);
        self
    }

    /// Adds response headers to log and their data classes for redaction.
    ///
    /// By default, no response header is logged.
    #[must_use]
    pub fn response_header_data_classes(mut self, data_classes: impl IntoIterator<Item = (HeaderName, impl IntoDataClass)>) -> Self {
        for (name, class) in data_classes {
            self.role.response_header_data_classes.insert(name, class.into_data_class());
        }
        self.role.response_header_fields = header_fields(&self.role.response_header_data_classes, RESPONSE_HEADER_PREFIX);
        self
    }

    /// Sets the media types of the request bodies to log, such as `application/json`.
    ///
    /// If the list is not empty, the middleware also logs all `text/*`, `application/*+json` and `application/*+xml`
    /// bodies. [Bodies](crate#bodies) tells which `Content-Type` values the types cover.
    ///
    /// # Errors
    ///
    /// Returns an error if an entry is not a media type, such as `json` or `text/*`.
    pub fn request_body_media_types(mut self, types: impl IntoIterator<Item = &'static str>) -> std::result::Result<Self, MediaTypeError> {
        self.role.request_body_media_types = MediaTypes::parse(types)?;
        Ok(self)
    }

    /// Sets the media types of the response bodies to log, such as `application/json`.
    ///
    /// If the list is not empty, the middleware also logs all `text/*`, `application/*+json` and `application/*+xml`
    /// bodies. [Bodies](crate#bodies) tells which `Content-Type` values the types cover.
    ///
    /// # Errors
    ///
    /// Returns an error if an entry is not a media type, such as `json` or `text/*`.
    pub fn response_body_media_types(mut self, types: impl IntoIterator<Item = &'static str>) -> std::result::Result<Self, MediaTypeError> {
        self.role.response_body_media_types = MediaTypes::parse(types)?;
        Ok(self)
    }

    /// Sets the number of bytes to log from each body.
    ///
    /// The default is 32 KiB. Values above 1.5 MiB become 1.5 MiB.
    #[must_use]
    pub fn body_size_limit(mut self, limit: usize) -> Self {
        self.role.body_size_limit = limit.min(MAX_BODY_SIZE_LIMIT);
        self
    }
}

impl<S> Layer<S> for TelemetryLayer<Server> {
    type Service = Telemetry<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Telemetry {
            inner,
            sink: self.sink.clone(),
            clock: self.clock.clone(),
            role: Role::Server(self.role.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use observed::Sink;

    use crate::Telemetry;

    #[test]
    fn server_debug_output_names_its_type() {
        assert!(format!("{:?}", Telemetry::server(Sink::noop()).role).contains("Server"));
    }
}
