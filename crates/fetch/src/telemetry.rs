// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Types for custom `fetch` metric attributes and connection diagnostics.
//!
//! Attach [`TelemetryAttributes`][crate::telemetry::TelemetryAttributes] to a request to add custom
//! [`KeyValue`][opentelemetry::KeyValue] values to its metrics.
//! [`ConnectionInfo`][crate::telemetry::ConnectionInfo] describes the connection that served a
//! response.
//!
//! For the full list of emitted metrics and their attributes, see the
//! [telemetry reference](crate::documentation::telemetry).

use std::borrow::Cow;
use std::fmt;

/// Diagnostic information about the connection that served an HTTP response.
///
/// Re-exported from [`fetch_options`]. The bundled Hyper transport adds it as a response extension.
/// A custom transport must add it if it supports this diagnostic.
/// [`FakeHandler`](crate::fake::FakeHandler) does not add it. Read it through
/// `response.extensions().get::<ConnectionInfo>()`.
pub use fetch_options::ConnectionInfo;
use http::Version;
use http::uri::Scheme;
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::{InstrumentationScope, KeyValue, Value};
use performables::arc::Arc;

pub(crate) const METER_NAME: &str = "fetch";

/// Instrumentation-scope attribute identifying the runtime a client is associated with.
pub(crate) const FETCH_RUNTIME_ATTRIBUTE: &str = "fetch.runtime";

/// Instrumentation-scope attribute identifying the transport handler a client uses.
pub(crate) const FETCH_TRANSPORT_ATTRIBUTE: &str = "fetch.transport";

/// Instrumentation-scope attribute identifying the name of a client instance.
pub(crate) const HTTP_CLIENT_NAME_ATTRIBUTE: &str = "http.client.name";

/// Built-in HTTP attributes fit inline. Larger custom sets use the heap.
const TELEMETRY_ATTRIBUTES_INLINE_CAPACITY: usize = 9;

/// A set of key-value attributes that enrich `fetch` telemetry.
///
/// Attach this type to request extensions to add attributes to request metrics.
#[derive(Debug, Clone, Default)]
pub struct TelemetryAttributes(smallvec::SmallVec<[KeyValue; TELEMETRY_ATTRIBUTES_INLINE_CAPACITY]>);

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

/// How a client's `fetch` metrics are metered.
///
/// Stores the runtime, transport, client name, and optional [provider][MeterProvider]. It uses the
/// global meter provider if no custom provider is set. It creates the [`InstrumentationScope`] with
/// the [`Meter`], so the client name can change after provider selection.
#[derive(Clone)]
pub(crate) struct Metering {
    provider: Option<Arc<dyn MeterProvider + Send + Sync>>,
    runtime: Cow<'static, str>,
    transport: Cow<'static, str>,
    client_name: Cow<'static, str>,
}

impl fmt::Debug for Metering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metering")
            .field("runtime", &self.runtime)
            .field("transport", &self.transport)
            .field("client_name", &self.client_name)
            .field("custom_provider", &self.provider.is_some())
            .finish()
    }
}

impl Metering {
    /// Creates metering that records against the global meter provider.
    pub(crate) fn new(runtime: Cow<'static, str>, transport: Cow<'static, str>, client_name: Cow<'static, str>) -> Self {
        Self {
            provider: None,
            runtime,
            transport,
            client_name,
        }
    }

    /// Records against the given custom meter provider instead of the global one.
    ///
    /// The meter scope uses a client name set before or after this call.
    pub(crate) fn with_provider<P>(mut self, provider: P) -> Self
    where
        P: MeterProvider + Send + Sync + 'static,
    {
        let provider: Box<dyn MeterProvider + Send + Sync> = Box::new(provider);
        self.provider = Some(provider.into());
        self
    }

    /// Updates the client name recorded on the eventual meter's scope.
    pub(crate) fn with_client_name(mut self, client_name: Cow<'static, str>) -> Self {
        self.client_name = client_name;
        self
    }

    /// Returns `true` if the client uses a custom meter provider.
    #[cfg(test)]
    pub(crate) fn has_custom_provider(&self) -> bool {
        self.provider.is_some()
    }
}

impl From<Metering> for Meter {
    fn from(metering: Metering) -> Self {
        let scope = client_scope(metering.runtime, metering.transport, metering.client_name);
        match metering.provider {
            Some(provider) => provider.meter_with_scope(scope),
            None => opentelemetry::global::meter_with_scope(scope),
        }
    }
}

/// Builds the client scope with `fetch.runtime`, `fetch.transport`, and `http.client.name`.
fn client_scope(runtime: impl Into<Value>, transport: impl Into<Value>, client_name: impl Into<Value>) -> InstrumentationScope {
    InstrumentationScope::builder(METER_NAME)
        .with_attributes([
            KeyValue::new(FETCH_RUNTIME_ATTRIBUTE, runtime),
            KeyValue::new(FETCH_TRANSPORT_ATTRIBUTE, transport),
            KeyValue::new(HTTP_CLIENT_NAME_ATTRIBUTE, client_name),
        ])
        .build()
}

/// Builds the scope for a standalone [`MetricsLayer`](crate::handlers::Metrics), which has no
/// transport attributes.
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

    fn test_metering(client_name: &'static str) -> Metering {
        Metering::new(Cow::Borrowed("tokio"), Cow::Borrowed("hyper"), Cow::Borrowed(client_name))
    }

    fn test_provider() -> opentelemetry_sdk::metrics::SdkMeterProvider {
        opentelemetry_sdk::metrics::SdkMeterProvider::builder().build()
    }

    #[test]
    fn new_metering_uses_global_provider() {
        let metering = test_metering("http_client");
        assert!(metering.provider.is_none());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn with_provider_sets_custom_provider() {
        let metering = test_metering("http_client").with_provider(test_provider());
        assert!(metering.provider.is_some());
    }

    #[test]
    fn from_global_metering_materializes_meter() {
        let _meter: Meter = test_metering("http_client").into();
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn with_provider_preserves_client_name() {
        let metering = test_metering("preserved_client").with_provider(test_provider());
        assert_eq!(metering.client_name, "preserved_client");
        assert!(metering.provider.is_some());
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
    fn with_client_name_updates_global_client_name() {
        let metering = test_metering("http_client").with_client_name(Cow::Borrowed("renamed_client"));
        assert_eq!(metering.client_name, "renamed_client");
        assert!(metering.provider.is_none());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn with_client_name_updates_custom_client_name() {
        let metering = test_metering("http_client")
            .with_provider(test_provider())
            .with_client_name(Cow::Borrowed("renamed_client"));
        assert_eq!(metering.client_name, "renamed_client");
        assert!(metering.provider.is_some());
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
