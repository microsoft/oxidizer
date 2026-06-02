// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transport-level HTTP client configuration.

use std::time::Duration;

use http::{Extensions, Version};

use crate::{ConnectionKeepAlive, ConnectionPoolOptions, DEFAULT_CONNECT_TIMEOUT, Http2Options, RequestFilter};

/// Public, transport-relevant subset of an HTTP client's configuration.
///
/// This is the view of the client configuration handed to custom transport handlers.
/// It deliberately excludes plumbing that is the pipeline's concern rather than
/// the transport's (base URI rewriting, redaction, `TLS` backend selection, I/O
/// model overrides, etc.).
///
/// Values on this struct are written through builder setters; a custom transport reads
/// them to size its own resources (pool limits, keep-alive timers, supported protocol
/// versions, ...).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TransportOptions {
    /// How long to wait for a TCP connection to be established.
    pub connect_timeout: Duration,
    /// Keep-alive policy for established connections.
    pub connection_keep_alive: ConnectionKeepAlive,
    /// Whether plain `http://` requests are allowed alongside `https://`.
    pub request_filter: RequestFilter,
    /// HTTP versions the client is willing to negotiate. An empty `Vec` is
    /// treated as "no preference" by the bundled transports.
    pub supported_http_versions: Vec<Version>,
    /// Connection pool sizing and lifetime configuration.
    pub connection_pool: ConnectionPoolOptions,
    /// HTTP/2-specific tuning knobs.
    pub http_2: Http2Options,
    /// Extra extensions to be applied to the underlying transport.
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

#[cfg(not(miri))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use insta::assert_debug_snapshot;

    use super::*;

    #[test]
    fn transport_options_default() {
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(std::env::consts::OS);
        settings.bind(|| {
            assert_debug_snapshot!(TransportOptions::default());
        });
    }

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
