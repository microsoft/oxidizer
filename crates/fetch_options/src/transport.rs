// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP client transport configuration.

use std::time::Duration;

use http::{Extensions, Version};

use crate::{ConnectionKeepAlive, ConnectionPoolOptions, DEFAULT_CONNECT_TIMEOUT, Http2Options, RequestFilter};

/// HTTP client configuration that a transport uses.
///
/// A client passes this value to a custom transport. It excludes pipeline behavior,
/// such as base URI rewriting, redaction, `TLS` backend selection, and I/O model overrides.
///
/// Set the fields directly. A custom transport reads them to configure its resources.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TransportOptions {
    /// How long to wait for a TCP connection to be established.
    pub connect_timeout: Duration,
    /// Keep-alive policy for established connections.
    pub connection_keep_alive: ConnectionKeepAlive,
    /// Whether plain `http://` requests are allowed alongside `https://`.
    pub request_filter: RequestFilter,
    /// HTTP versions that the client can negotiate.
    ///
    /// The bundled transports treat an empty `Vec` as no preference.
    pub supported_http_versions: Vec<Version>,
    /// Connection pool sizing and lifetime configuration.
    pub connection_pool: ConnectionPoolOptions,
    /// HTTP/2 configuration.
    pub http_2: Http2Options,
    /// Extra extensions for the underlying transport.
    pub extra: Extensions,
}

impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_filter: RequestFilter::Https,
            connection_keep_alive: ConnectionKeepAlive::default(),
            supported_http_versions: vec![Version::HTTP_11, Version::HTTP_2],
            connection_pool: ConnectionPoolOptions::default(),
            http_2: Http2Options::default(),
            extra: Extensions::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use insta::assert_debug_snapshot;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn transport_options_default() {
        assert_debug_snapshot!(TransportOptions::default());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_transport_options_type() {
        static_assertions::assert_impl_all!(
            TransportOptions: Send,
            Sync,
            Clone,
            Debug,
            Default
        );
    }
}
