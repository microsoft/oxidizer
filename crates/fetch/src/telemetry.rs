// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry types for enriching `fetch` metrics and inspecting connections.
//!
//! [`TelemetryAttributes`] lets you attach custom [`KeyValue`] attributes to a
//! request so they are merged into the metrics recorded for it.
//! [`ConnectionInfo`] reports details about the connection that served a response.
//!
//! For the full list of emitted metrics and their attributes, see the
//! [telemetry reference](crate::_documentation::telemetry).

/// Diagnostic information about the connection that served an HTTP response.
///
/// Re-exported from [`fetch_options`]. Attached as a response extension by real
/// network connections (not by [`FakeHandler`](crate::fake::FakeHandler));
/// retrieve via `response.extensions().get::<ConnectionInfo>()`.
pub use fetch_options::ConnectionInfo;
use http::Version;
use http::uri::Scheme;
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::{InstrumentationScope, KeyValue, Value};

pub(crate) const METER_NAME: &str = "fetch";

/// Instrumentation-scope attribute identifying the runtime a client is associated with.
pub(crate) const FETCH_RUNTIME_ATTRIBUTE: &str = "fetch.runtime";

/// Instrumentation-scope attribute identifying the transport handler a client uses.
pub(crate) const FETCH_TRANSPORT_ATTRIBUTE: &str = "fetch.transport";

/// Instrumentation-scope attribute identifying the name of a client instance.
pub(crate) const HTTP_CLIENT_NAME_ATTRIBUTE: &str = "http.client.name";

/// A set of key-value attributes that enrich `fetch` telemetry.
///
/// Attach these to a request (via its extensions) to merge custom dimensions
/// into the metrics recorded for that request.
#[derive(Debug, Clone, Default)]
pub struct TelemetryAttributes(smallvec::SmallVec<[KeyValue; 9]>);

impl TelemetryAttributes {
    /// Creates an empty set of telemetry attributes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a [`KeyValue`] attribute to the set.
    pub fn push(&mut self, attribute: KeyValue) {
        self.0.push(attribute);
    }

    /// Returns a slice of the telemetry attributes.
    #[must_use]
    pub fn values(&self) -> &[KeyValue] {
        &self.0
    }
}

impl FromIterator<KeyValue> for TelemetryAttributes {
    fn from_iter<I: IntoIterator<Item = KeyValue>>(iter: I) -> Self {
        Self(smallvec::SmallVec::from_iter(iter))
    }
}

impl Extend<KeyValue> for TelemetryAttributes {
    fn extend<T: IntoIterator<Item = KeyValue>>(&mut self, iter: T) {
        self.0.extend(iter);
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Metering {
    Global(InstrumentationScope),
    Custom(Meter),
}

impl Metering {
    /// Metering against the global meter provider, materialized lazily at build
    /// time. The `scope` is retained so the eventual meter carries it.
    pub(crate) fn global(scope: InstrumentationScope) -> Self {
        Self::Global(scope)
    }

    /// Metering against a caller-supplied provider. The scoped meter is created
    /// eagerly because the provider is only borrowed, not retained.
    pub(crate) fn custom(meter_provider: &dyn MeterProvider, scope: InstrumentationScope) -> Self {
        Self::Custom(meter_provider.meter_with_scope(scope))
    }
}

impl From<Metering> for Meter {
    fn from(metering: Metering) -> Self {
        match metering {
            Metering::Global(scope) => opentelemetry::global::meter_with_scope(scope),
            Metering::Custom(meter) => meter,
        }
    }
}

impl Metering {
    /// Rebuilds the instrumentation scope with an updated client name.
    ///
    /// Only global metering can be rebuilt; custom metering keeps its already
    /// materialized meter (and therefore the scope it was created with), so a
    /// custom meter provider must be supplied after the client name is set.
    pub(crate) fn with_client_name(self, runtime: impl Into<Value>, transport: impl Into<Value>, client_name: impl Into<Value>) -> Self {
        match self {
            Self::Global(_) => Self::Global(client_scope(runtime, transport, client_name)),
            custom @ Self::Custom(_) => custom,
        }
    }
}

/// Builds the `fetch` instrumentation scope carrying the `fetch.runtime`,
/// `fetch.transport`, and `http.client.name` attributes, attached to every
/// metric a client records.
pub(crate) fn client_scope(runtime: impl Into<Value>, transport: impl Into<Value>, client_name: impl Into<Value>) -> InstrumentationScope {
    InstrumentationScope::builder(METER_NAME)
        .with_attributes([
            KeyValue::new(FETCH_RUNTIME_ATTRIBUTE, runtime),
            KeyValue::new(FETCH_TRANSPORT_ATTRIBUTE, transport),
            KeyValue::new(HTTP_CLIENT_NAME_ATTRIBUTE, client_name),
        ])
        .build()
}

/// Builds the attribute-less `fetch` instrumentation scope used by the
/// standalone [`MetricsLayer`](crate::handlers::Metrics), which has no transport.
pub(crate) fn base_scope() -> InstrumentationScope {
    InstrumentationScope::builder(METER_NAME).build()
}

pub(crate) const fn http_method_name(method: &http::Method) -> &'static str {
    match *method {
        http::Method::GET => "GET",
        http::Method::POST => "POST",
        http::Method::PUT => "PUT",
        http::Method::DELETE => "DELETE",
        http::Method::PATCH => "PATCH",
        http::Method::HEAD => "HEAD",
        http::Method::OPTIONS => "OPTIONS",
        http::Method::CONNECT => "CONNECT",
        http::Method::TRACE => "TRACE",
        _ => "_OTHER",
    }
}

#[cfg_attr(test, mutants::skip)] // Some branches are for optimization and cannot be feasibly distinguished in tests.
pub(crate) fn url_scheme(scheme: &Scheme) -> Value {
    match scheme.as_str() {
        "http" => Value::from("http"),
        "https" => Value::from("https"),
        val => Value::from(val.to_string()),
    }
}

#[cfg_attr(test, mutants::skip)] // Some branches are for optimization and cannot be feasibly distinguished in tests.
pub(crate) fn url_scheme_or(scheme: Option<&Scheme>) -> Value {
    scheme.map_or_else(|| Value::from("_OTHER"), url_scheme)
}

pub(crate) fn server_port(uri: &http::Uri) -> Option<Value> {
    match uri.authority()?.port() {
        Some(p) => Some(Value::from(i64::from(p.as_u16()))),
        None if uri.scheme() == Some(&Scheme::HTTPS) => Some(Value::from(443)),
        None if uri.scheme() == Some(&Scheme::HTTP) => Some(Value::from(80)),
        None => None,
    }
}

pub(crate) fn network_protocol_name() -> Value {
    // HTTP client does not support other protocols yet.
    Value::from("http")
}

pub(crate) fn network_protocol_version(version: Version) -> Value {
    match version {
        http::Version::HTTP_11 => Value::from("1.1"),
        http::Version::HTTP_2 => Value::from("2.0"),
        http::Version::HTTP_3 => Value::from("3.0"),
        http::Version::HTTP_10 => Value::from("1.0"),
        _ => Value::from("_OTHER"),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::Method;

    use super::*;

    #[test]
    fn metering_global() {
        let metering = Metering::global(base_scope());
        assert!(matches!(metering, Metering::Global(_)));
    }

    #[test]
    fn metering_custom() {
        let metering = Metering::custom(opentelemetry::global::meter_provider().as_ref(), base_scope());
        assert!(matches!(metering, Metering::Custom(_expected_meter)));
    }

    #[test]
    fn from_metering_global() {
        let metering = Metering::global(client_scope("tokio", "hyper", "http_client"));
        let _meter: Meter = metering.into();
    }

    #[test]
    fn client_scope_carries_runtime_transport_and_client_name_attributes() {
        let scope = client_scope("tokio", "hyper", "my_client");

        let runtime = scope
            .attributes()
            .find(|kv| kv.key.as_str() == FETCH_RUNTIME_ATTRIBUTE)
            .expect("client scope must carry the fetch.runtime attribute");
        assert_eq!(runtime.value, Value::from("tokio"));

        let transport = scope
            .attributes()
            .find(|kv| kv.key.as_str() == FETCH_TRANSPORT_ATTRIBUTE)
            .expect("client scope must carry the fetch.transport attribute");
        assert_eq!(transport.value, Value::from("hyper"));

        let client_name = scope
            .attributes()
            .find(|kv| kv.key.as_str() == HTTP_CLIENT_NAME_ATTRIBUTE)
            .expect("client scope must carry the http.client.name attribute");
        assert_eq!(client_name.value, Value::from("my_client"));
    }

    #[test]
    fn with_client_name_rebuilds_global_scope() {
        let metering = Metering::global(client_scope("tokio", "hyper", "http_client")).with_client_name("tokio", "hyper", "renamed_client");

        let Metering::Global(scope) = metering else {
            panic!("global metering must stay global after renaming");
        };
        let client_name = scope
            .attributes()
            .find(|kv| kv.key.as_str() == HTTP_CLIENT_NAME_ATTRIBUTE)
            .expect("rebuilt scope must carry the http.client.name attribute");
        assert_eq!(client_name.value, Value::from("renamed_client"));
    }

    #[test]
    fn base_scope_is_named_fetch_without_attributes() {
        let scope = base_scope();
        assert_eq!(scope.name(), METER_NAME);
        assert_eq!(scope.attributes().count(), 0);
    }

    #[test]
    fn http_method_name_standard_methods() {
        assert_eq!("GET", http_method_name(&Method::GET));
        assert_eq!("POST", http_method_name(&Method::POST));
        assert_eq!("PUT", http_method_name(&Method::PUT));
        assert_eq!("DELETE", http_method_name(&Method::DELETE));
        assert_eq!("PATCH", http_method_name(&Method::PATCH));
        assert_eq!("HEAD", http_method_name(&Method::HEAD));
        assert_eq!("OPTIONS", http_method_name(&Method::OPTIONS));
        assert_eq!("CONNECT", http_method_name(&Method::CONNECT));
        assert_eq!("TRACE", http_method_name(&Method::TRACE));
    }

    #[test]
    fn http_method_name_custom_method() {
        let custom_method = Method::from_bytes(b"CUSTOM").unwrap();
        assert_eq!("_OTHER", http_method_name(&custom_method));
    }

    #[test]
    fn url_scheme_test() {
        assert_eq!(Value::from("https"), url_scheme(&Scheme::HTTPS));
        assert_eq!(Value::from("http"), url_scheme(&Scheme::HTTP));
        assert_eq!(Value::from("abc"), url_scheme(&Scheme::try_from("abc").unwrap()));

        assert_eq!(Value::from("https"), url_scheme_or(Some(&Scheme::HTTPS)));
        assert_eq!(Value::from("_OTHER"), url_scheme_or(None));
    }

    #[test]
    fn server_port_test() {
        use http::Uri;

        let uri_with_port = Uri::from_static("http://example.com:8080/path");
        assert_eq!(Some(Value::from(8080_i64)), server_port(&uri_with_port));

        let uri_without_port = Uri::from_static("https://example.com/path");
        assert_eq!(Some(Value::from(443_i64)), server_port(&uri_without_port));

        let uri_without_port = Uri::from_static("http://example.com/path");
        assert_eq!(Some(Value::from(80_i64)), server_port(&uri_without_port));

        let uri_without_port = Uri::from_static("ftp://example.com/path");
        assert_eq!(None, server_port(&uri_without_port));

        let relative_uri = Uri::from_static("/path/to/resource");
        assert_eq!(None, server_port(&relative_uri));
    }

    #[test]
    fn network_protocol_name_test() {
        assert_eq!(Value::from("http"), network_protocol_name());
    }

    #[test]
    fn network_protocol_version_test() {
        assert_eq!(Value::from("1.0"), network_protocol_version(Version::HTTP_10));

        assert_eq!(Value::from("1.1"), network_protocol_version(Version::HTTP_11));

        assert_eq!(Value::from("2.0"), network_protocol_version(Version::HTTP_2));

        assert_eq!(Value::from("3.0"), network_protocol_version(Version::HTTP_3));

        assert_eq!(Value::from("_OTHER"), network_protocol_version(Version::HTTP_09));
    }
}
