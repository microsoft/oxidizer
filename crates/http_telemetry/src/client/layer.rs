// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The client layer: its configuration and the methods that set it.

use std::borrow::Cow;
use std::error::Error;
use std::sync::Arc;

use data_privacy::{DataClass, IntoDataClass};
use http::{HeaderMap, HeaderName};
use layered::Layer;
use observed::metadata::{FieldDescriptor, LogFieldEntry, MetricFieldEntry};
use observed::{Sink, Text};
use ohno::ErrorLabel;

use crate::header_fields::{HeaderField, REQUEST_HEADER_PREFIX, RESPONSE_HEADER_PREFIX, header_fields};
use crate::shared::collect_error_labels;
use crate::telemetry::Role;
use crate::{Telemetry, TelemetryLayer};

/// Client-side configuration held by a [`TelemetryLayer`].
///
/// A client sends requests and receives responses. Create its layer with [`Telemetry::client`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Client {
    pub(crate) report_total_duration: bool,
    pub(crate) error_labels: fn(&(dyn Error + 'static)) -> ErrorLabel,
    pub(crate) dimensions: Vec<(FieldDescriptor, Text)>,
    pub(crate) request_header_data_classes: HeaderMap<DataClass>,
    pub(crate) request_header_fields: Option<Arc<[HeaderField]>>,
    pub(crate) response_header_data_classes: HeaderMap<DataClass>,
    pub(crate) response_header_fields: Option<Arc<[HeaderField]>>,
}

impl Telemetry<()> {
    /// Creates a [`Layer::layer`](layered::Layer::layer) whose middleware produces telemetry about outgoing HTTP
    /// requests.
    #[must_use]
    pub fn client(sink: Sink) -> TelemetryLayer<Client> {
        TelemetryLayer::new(
            sink,
            Client {
                report_total_duration: false,
                error_labels: collect_error_labels,
                dimensions: Vec::new(),
                request_header_data_classes: HeaderMap::default(),
                request_header_fields: None,
                response_header_data_classes: HeaderMap::default(),
                response_header_fields: None,
            },
        )
    }
}

/// Client-only configuration.
impl TelemetryLayer<Client> {
    /// Sets whether the middleware reports the whole request instead of one attempt.
    ///
    /// If enabled, the middleware emits the events `http.client.request.total.complete`,
    /// `http.client.request.total.error` and `http.client.request.total.abandoned`, which record the metric
    /// `http.client.request.total_duration`, instead of the events of an attempt. The middleware measures its handler,
    /// so put this layer outside retry and hedging to measure all attempts of a request. The default is `false`.
    ///
    /// # Example
    ///
    /// ```
    /// use http_telemetry::Telemetry;
    /// use observed::Sink;
    ///
    /// # fn build(sink: Sink) {
    /// let layer = Telemetry::client(sink).report_total_duration(true);
    /// # let _ = layer;
    /// # }
    /// ```
    #[must_use]
    pub fn report_total_duration(mut self, enabled: bool) -> Self {
        self.role.report_total_duration = enabled;
        self
    }

    /// Sets the function that gives `error.type` for an error that the handler returns.
    ///
    /// The default function gives the labels of the error and its sources, joined with dots: the label of each
    /// [`HttpError`](http_extensions::HttpError) and the kind of each I/O error.
    #[must_use]
    pub fn error_labels_with(mut self, labels: fn(&(dyn Error + 'static)) -> ErrorLabel) -> Self {
        self.role.error_labels = labels;
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

    /// Adds the field `key` with `value` to each event of the layer, as a log field and as a metric dimension.
    ///
    /// A second call with the same key replaces the value. The key is not checked against the keys of the other
    /// fields. The method is for `fetch` only, which adds the fields that tell which client sent a request.
    ///
    /// ```
    /// # use http_telemetry::Telemetry;
    /// # use observed::Sink;
    /// let _ = Telemetry::client(Sink::noop()).dimension("http.client.name", "books");
    /// ```
    #[doc(hidden)]
    #[must_use]
    pub fn dimension(mut self, key: &'static str, value: impl Into<Cow<'static, str>>) -> Self {
        let value = Text::from(value.into());
        if let Some((_, old_value)) = self
            .role
            .dimensions
            .iter_mut()
            .find(|(descriptor, _)| descriptor.field_name() == key)
        {
            *old_value = value;
        } else {
            let descriptor = FieldDescriptor::new(key, Some(LogFieldEntry::new(key)), Some(MetricFieldEntry::dimension(key)));
            self.role.dimensions.push((descriptor, value));
        }
        self
    }
}

impl<S> Layer<S> for TelemetryLayer<Client> {
    type Service = Telemetry<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Telemetry {
            inner,
            sink: self.sink.clone(),
            clock: self.clock.clone(),
            role: Role::Client(self.role.clone()),
        }
    }
}
