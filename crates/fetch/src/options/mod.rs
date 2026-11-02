// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP client configuration.
//!
//! [`fetch_options`] defines the transport options and this module re-exports them. `ClientOptions`
//! combines transport options with response-body, routing, redaction, TLS, and decompression
//! configuration.
//!
//! Transport-specific builders can use [`fetch_options::SocketOptions`]. It is separate from
//! [`fetch_options::TransportOptions`] because only a transport that owns sockets can apply it.

use data_privacy::RedactionEngine;

mod decompression;

pub use decompression::{DecompressionFormat, DecompressionOptions};
pub use fetch_options::{
    ConnectionIdleTimeout, ConnectionKeepAlive, ConnectionLifetime, ConnectionPoolOptions, Http2Options, PoolIndex, PoolSelection,
    RequestFilter, SocketOptions, TransportOptions,
};
pub use http_extensions::HttpBodyOptions;
use http_extensions::routing::Router;

use crate::tls::TlsOptions;

/// Aggregated configuration for an [`HttpClient`](crate::HttpClient).
///
/// [`TransportOptions`] contains transport configuration. The request pipeline owns the other
/// fields.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClientOptions {
    /// Transport-level configuration handed to transport handlers.
    pub transport: TransportOptions,
    /// Body-level policies (idle timeout, buffer limit) applied to every response body.
    pub response_body_options: HttpBodyOptions,
    /// Base-URI rewriting rules.
    pub router: Router,
    /// Redaction engine used for logging and telemetry.
    pub redaction_engine: RedactionEngine,
    /// TLS configuration used by the bundled transports.
    pub tls: TlsOptions,
    /// Formats and resource limits for automatic response decompression.
    pub decompression: DecompressionOptions,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn client_options_default_uses_https_filter() {
        let options = ClientOptions::default();
        assert!(matches!(options.transport.request_filter, RequestFilter::Https));
    }
}
