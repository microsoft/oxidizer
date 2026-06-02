// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry helpers for the Hyper transport.

use ohno::ErrorLabel;
use opentelemetry::{KeyValue, Value};

pub(crate) fn connection_network_protocol_version(connected: &hyper_util::client::legacy::connect::Connected) -> Value {
    if connected.is_negotiated_h2() {
        Value::from("2")
    } else {
        Value::from("1")
    }
}

pub(crate) fn create_connection_attributes(
    uri: &templated_uri::BaseUri,
    connected: &hyper_util::client::legacy::connect::Connected,
) -> Vec<KeyValue> {
    use opentelemetry_semantic_conventions::trace::{NETWORK_PROTOCOL_VERSION, SERVER_ADDRESS, SERVER_PORT, URL_SCHEME};

    vec![
        KeyValue::new(SERVER_ADDRESS, uri.authority().host().to_string()),
        KeyValue::new(SERVER_PORT, server_port_attribute(uri)),
        KeyValue::new(URL_SCHEME, uri.scheme().to_string()),
        KeyValue::new(NETWORK_PROTOCOL_VERSION, connection_network_protocol_version(connected)),
    ]
}

/// Attributes for failed connection attempts.
pub(crate) fn create_connection_failure_attributes(uri: &templated_uri::BaseUri, error_type: ErrorLabel) -> Vec<KeyValue> {
    use opentelemetry_semantic_conventions::trace::{SERVER_ADDRESS, SERVER_PORT, URL_SCHEME};

    vec![
        KeyValue::new(SERVER_ADDRESS, uri.authority().host().to_string()),
        KeyValue::new(SERVER_PORT, server_port_attribute(uri)),
        KeyValue::new(URL_SCHEME, uri.scheme().to_string()),
        KeyValue::new(opentelemetry_semantic_conventions::attribute::ERROR_TYPE, error_type.into_cow()),
    ]
}

/// Returns the server port as an `i64` attribute, using `-1` as a sentinel
/// when no port is known for the URI scheme. Keeping the attribute present
/// (rather than omitting it) gives a stable attribute set across emissions,
/// which downstream aggregations rely on.
fn server_port_attribute(uri: &templated_uri::BaseUri) -> i64 {
    uri.effective_port().map_or(-1, i64::from)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::error_labels::LABEL_CONNECT;
    use crate::testing::sorted_attributes;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn create_connection_attributes_emits_expected_keys() {
        let uri = templated_uri::BaseUri::from_static("https://example.com:8443");
        let connected = hyper_util::client::legacy::connect::Connected::new();
        let attrs = create_connection_attributes(&uri, &connected);
        insta::assert_debug_snapshot!(sorted_attributes(&attrs));
    }

    #[test]
    fn connection_network_protocol_version_h2() {
        let connected = hyper_util::client::legacy::connect::Connected::new().negotiated_h2();
        assert_eq!(connection_network_protocol_version(&connected).as_str(), "2");
    }

    #[test]
    fn connection_network_protocol_version_default_is_1() {
        let connected = hyper_util::client::legacy::connect::Connected::new();
        assert_eq!(connection_network_protocol_version(&connected).as_str(), "1");
    }

    #[test]
    fn server_port_attribute_uses_negative_one_sentinel_when_unknown() {
        // Pins the sentinel value as `-1` (not `1`) when the scheme has no
        // default port and the URI carries no explicit port.
        use templated_uri::{Authority, BasePath, Origin, Scheme};
        let origin = Origin::from_parts(Scheme::try_from("ftp").unwrap(), Authority::from_static("example.com"));
        let uri = templated_uri::BaseUri::from_parts(origin, BasePath::default());
        assert_eq!(uri.effective_port(), None);
        assert_eq!(server_port_attribute(&uri), -1);
    }

    #[test]
    fn server_port_attribute_returns_explicit_port() {
        let uri = templated_uri::BaseUri::from_static("https://example.com:8443");
        assert_eq!(server_port_attribute(&uri), 8443);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn create_connection_failure_attributes_includes_error_type() {
        let uri = templated_uri::BaseUri::from_static("https://example.com");
        let attrs = create_connection_failure_attributes(&uri, LABEL_CONNECT);
        insta::assert_debug_snapshot!(sorted_attributes(&attrs));
    }
}
