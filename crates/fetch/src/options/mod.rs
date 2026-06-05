// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configuration options for HTTP client behavior.
//!
//! The transport-level option types are defined in the [`fetch_options`] crate
//! and re-exported here. `ClientOptions` bundles those transport options
//! together with the response-body, routing, redaction, and TLS configuration
//! owned by the `fetch` request pipeline.

use data_privacy::RedactionEngine;
pub use fetch_options::{
    ConnectionIdleTimeout, ConnectionKeepAlive, ConnectionLifetime, ConnectionPoolOptions, Http2Options, PoolIndex, PoolSelection,
    RequestFilter, TransportOptions,
};
pub use http_extensions::HttpBodyOptions;
use http_extensions::routing::Router;

use crate::tls::TlsOptions;

/// Aggregated configuration for an [`HttpClient`](crate::HttpClient).
///
/// Transport-level knobs live in [`TransportOptions`]; the remaining fields are
/// owned by the request pipeline rather than the transport.
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
