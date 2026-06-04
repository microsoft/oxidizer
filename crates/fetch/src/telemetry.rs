// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry related APIs for fetch.

/// Diagnostic information about the connection that served an HTTP response.
///
/// Re-exported from [`fetch_options`]. Attached as a response extension by real
/// network connections (not by [`FakeHandler`](crate::fake::FakeHandler));
/// retrieve via `response.extensions().get::<ConnectionInfo>()`.
pub use fetch_options::ConnectionInfo;
use http::Version;
use http::uri::Scheme;
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::{KeyValue, Value};

pub(crate) const METER_NAME: &str = "fetch";

/// A collection of key value attributes that can be attached to the request.
///
/// These attributes are used to enrich the fetch telemetry if available.
#[derive(Debug, Clone, Default)]
pub struct TelemetryAttributes(smallvec::SmallVec<[KeyValue; 9]>);

impl TelemetryAttributes {
    /// Creates an empty telemetry attributes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an [`KeyValue`] to the telemetry attributes.
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

#[derive(Debug, Default, Clone)]
pub(crate) enum Metering {
    #[default]
    Global,
    Custom(Meter),
}

impl Metering {
    pub fn custom(meter_provider: &dyn MeterProvider) -> Self {
        Self::Custom(meter_provider.meter(METER_NAME))
    }
}

impl From<Metering> for Meter {
    fn from(metering: Metering) -> Self {
        match metering {
            Metering::Global => opentelemetry::global::meter(METER_NAME),
            Metering::Custom(meter) => meter,
        }
    }
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
    fn metering_default() {
        let metering = Metering::default();
        assert!(matches!(metering, Metering::Global));
    }

    #[test]
    fn metering_custom() {
        let metering = Metering::custom(opentelemetry::global::meter_provider().as_ref());
        assert!(matches!(metering, Metering::Custom(_expected_meter)));
    }

    #[test]
    fn from_metering_global() {
        let metering = Metering::Global;
        let _meter: Meter = metering.into();
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
