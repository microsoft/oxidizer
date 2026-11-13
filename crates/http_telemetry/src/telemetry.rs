// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The telemetry middleware, and the layer that creates it.

use std::any::type_name;
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use data_privacy::{DataClass, IntoDataClass};
use futures::FutureExt;
use futures::future::Either;
use http::header::HOST;
use http::{HeaderMap, HeaderName};
use http_body::Body;
use http_body_util::BodyExt;
use http_extensions::body::{Content, StreamingBody};
use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
use layered::{Layer, Service};
use observed::{Event, Sink};
use parking_lot::Mutex;
use routerama::RouteTable;
use templated_uri::BasePath;
use tick::SimpleClock;

use crate::server::{
    ActiveRequests, BodyBuffer, HeaderField, HeaderValues, MediaTypeError, MediaTypes, OTHER, RequestAbandoned, RequestAndResponseHeaders,
    RequestComplete, RequestEnd, RequestError, RouteFields, RouteValues, TelemetryDropGuard, UriAuthority, emit_when_body_ends,
    header_fields,
};

const DEFAULT_BODY_SIZE_LIMIT: usize = 32 * 1024;
const MAX_BODY_SIZE_LIMIT: usize = 1536 * 1024;

/// Produces telemetry about the requests that go through it, in the form of `observed` events.
///
/// Create the layer with [`Telemetry::server`], set its configuration, and apply it with
/// [`Layer::layer`].
#[derive(Clone)]
pub struct Telemetry<T> {
    pub(crate) inner: T,
    pub(crate) layer: TelemetryLayer,
    pub(crate) route_fields: Option<Arc<RouteFields>>,
    pub(crate) request_header_fields: Option<Arc<[HeaderField]>>,
    pub(crate) response_header_fields: Option<Arc<[HeaderField]>>,
}

impl Telemetry<()> {
    /// Creates a [`Layer::layer`] whose middleware produces telemetry about incoming HTTP requests.
    #[must_use]
    pub fn server(sink: Sink) -> TelemetryLayer {
        TelemetryLayer {
            sink,
            clock: SimpleClock::new_system(),
            excluded_paths: ExcludedPaths::default(),
            route_resolver: None,
            route_parameter_data_classes: BTreeMap::new(),
            request_header_data_classes: HeaderMap::default(),
            response_header_data_classes: HeaderMap::default(),
            request_body_media_types: MediaTypes::default(),
            response_body_media_types: MediaTypes::default(),
            body_size_limit: DEFAULT_BODY_SIZE_LIMIT,
        }
    }
}

impl<T: RequestHandler> Service<HttpRequest> for Telemetry<T> {
    type Out = Result<HttpResponse>;

    #[expect(
        clippy::too_many_lines,
        reason = "a request goes through each step here in order: the pass-through checks, the drop guard, the copy of the request body and the call of the handler"
    )]
    fn execute(&self, mut input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
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
            route_values: self.route_fields.as_ref().and_then(|fields| {
                let path_and_query = input.uri().path_and_query()?.clone();
                Some(RouteValues {
                    route_fields: Arc::clone(fields),
                    path_and_query,
                })
            }),
            headers: (self.request_header_fields.is_some() || self.response_header_fields.is_some()).then(|| {
                Box::new(RequestAndResponseHeaders {
                    request: self
                        .request_header_fields
                        .clone()
                        .map(|fields| HeaderValues::new(fields, input.headers())),
                    response: None,
                })
            }),
            request_body_buffer: None,
            response_body_buffer: None,
            finished: false,
        };
        guard.add_active_request(1);
        if layer.request_body_media_types.matches(input.headers()) {
            let buffer = Arc::new(Mutex::new(BodyBuffer::new(layer.body_size_limit)));
            if !input.body().is_end_stream() {
                input = input.map(|body| {
                    body.map(|content| {
                        Ok(match content {
                            Content::Bytes(bytes) => {
                                buffer.lock().push(&bytes);
                                Content::Bytes(bytes)
                            }
                            Content::Streaming(inner) => {
                                let buffer = Arc::clone(&buffer);
                                Content::Streaming(StreamingBody::new(inner.inspect_frame(move |frame| {
                                    if let Some(data) = frame.data_ref() {
                                        buffer.lock().push(data);
                                    }
                                })))
                            }
                        })
                    })
                    .expect("map fails only for a consumed body, which is at its end, so the check above rules it out")
                });
            }
            guard.request_body_buffer = Some(buffer);
        }
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
                Ok(Ok(response)) => {
                    if let Some(headers) = &mut guard.headers {
                        headers.response = self
                            .response_header_fields
                            .clone()
                            .map(|fields| HeaderValues::new(fields, response.headers()));
                    }
                    if layer.response_body_media_types.matches(response.headers()) {
                        guard.response_body_buffer = Some(BodyBuffer::new(layer.body_size_limit));
                    }
                    Ok(emit_when_body_ends(guard, response))
                }
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
    pub(crate) route_resolver: Option<Arc<dyn RouteTable + Send + Sync>>,
    pub(crate) route_parameter_data_classes: BTreeMap<String, DataClass>,
    pub(crate) request_header_data_classes: HeaderMap<DataClass>,
    pub(crate) response_header_data_classes: HeaderMap<DataClass>,
    pub(crate) request_body_media_types: MediaTypes,
    pub(crate) response_body_media_types: MediaTypes,
    pub(crate) body_size_limit: usize,
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

    /// Sets the resolver that matches each request to its route.
    ///
    /// The middleware does not see how the application routes a request, so it matches the request again with this
    /// resolver. If a route matches, the events of the request have the field `http.route`.
    #[must_use]
    pub fn route_resolver(mut self, resolver: Arc<dyn RouteTable + Send + Sync>) -> Self {
        self.route_resolver = Some(resolver);
        self
    }

    /// Sets the route parameters to log and their data classes for redaction.
    ///
    /// By default, no route parameter is logged.
    #[must_use]
    pub fn route_parameter_data_classes(mut self, data_classes: impl IntoIterator<Item = (impl Into<String>, impl IntoDataClass)>) -> Self {
        for (name, class) in data_classes {
            self.route_parameter_data_classes.insert(name.into(), class.into_data_class());
        }
        self
    }

    /// Sets the request headers to log and their data classes for redaction.
    ///
    /// By default, no request header is logged.
    #[must_use]
    pub fn request_header_data_classes(mut self, data_classes: impl IntoIterator<Item = (HeaderName, impl IntoDataClass)>) -> Self {
        for (name, class) in data_classes {
            self.request_header_data_classes.insert(name, class.into_data_class());
        }
        self
    }

    /// Sets the response headers to log and their data classes for redaction.
    ///
    /// By default, no response header is logged.
    #[must_use]
    pub fn response_header_data_classes(mut self, data_classes: impl IntoIterator<Item = (HeaderName, impl IntoDataClass)>) -> Self {
        for (name, class) in data_classes {
            self.response_header_data_classes.insert(name, class.into_data_class());
        }
        self
    }

    /// Sets the media types of the request bodies to log, such as `application/json`. [Bodies](crate#bodies) tells
    /// which `Content-Type` values the types cover.
    ///
    /// # Errors
    ///
    /// Returns an error if an entry is not a media type, such as `json` or `text/*`.
    pub fn request_body_media_types(mut self, types: impl IntoIterator<Item = &'static str>) -> std::result::Result<Self, MediaTypeError> {
        self.request_body_media_types = MediaTypes::parse(types)?;
        Ok(self)
    }

    /// Sets the media types of the response bodies to log, such as `application/json`. [Bodies](crate#bodies) tells
    /// which `Content-Type` values the types cover.
    ///
    /// # Errors
    ///
    /// Returns an error if an entry is not a media type, such as `json` or `text/*`.
    pub fn response_body_media_types(mut self, types: impl IntoIterator<Item = &'static str>) -> std::result::Result<Self, MediaTypeError> {
        self.response_body_media_types = MediaTypes::parse(types)?;
        Ok(self)
    }

    /// Sets the number of bytes to log from each body.
    ///
    /// The default is 32 KiB. Values above 1.5 MiB become 1.5 MiB.
    #[must_use]
    pub fn body_size_limit(mut self, limit: usize) -> Self {
        self.body_size_limit = limit.min(MAX_BODY_SIZE_LIMIT);
        self
    }
}

impl<S> Layer<S> for TelemetryLayer {
    type Service = Telemetry<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Telemetry {
            inner,
            layer: self.clone(),
            route_fields: RouteFields::new(self).map(Arc::new),
            request_header_fields: header_fields(&self.request_header_data_classes, "http.request.header."),
            response_header_fields: header_fields(&self.response_header_data_classes, "http.response.header."),
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
