// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_options/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_options/favicon.ico")]

//! HTTP client transport configuration.
//!
//! This crate defines connect timeouts, request filtering, connection information,
//! keep-alive, connection pools, idle timeouts, pool selection, connection lifetime,
//! HTTP versions, HTTP/2 stream limits and flow control, and socket configuration.
//!
//! # Example
//!
//! ```
//! use std::time::Duration;
//!
//! use fetch_options::{ConnectionLifetime, RequestFilter, TransportOptions};
//!
//! fn configure(mut options: TransportOptions) -> TransportOptions {
//!     options.connect_timeout = Duration::from_secs(10);
//!     options.request_filter = RequestFilter::Https;
//!     options.connection_pool = options
//!         .connection_pool
//!         .max_connections(64)
//!         .connection_idle_timeout(Duration::from_secs(90))
//!         .connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(300)));
//!
//!     options
//! }
//!
//! let options = configure(TransportOptions::default());
//! ```

use std::time::Duration;

mod connection_info;
mod connection_keep_alive;
mod http2;
mod pooling;
mod request_filter;
mod socket;
mod transport;

pub use connection_info::ConnectionInfo;
pub use connection_keep_alive::ConnectionKeepAlive;
#[doc(inline)]
pub use http2::Http2Options;
pub use pooling::{ConnectionIdleTimeout, ConnectionLifetime, ConnectionPoolOptions, PoolIndex, PoolSelection};
pub use request_filter::RequestFilter;
#[doc(inline)]
pub use socket::SocketOptions;
pub use transport::TransportOptions;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

// This matches the 20-second value of `SocketsHttpHandler.KeepAlivePingTimeout` in .NET.
const DEFAULT_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

// .NET disables `KeepAlivePingDelay` by default with `Timeout.InfiniteTimeSpan`.
// This crate uses 20 seconds only when the caller enables keep-alive without an interval.
const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(20);

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn constants_have_expected_values() {
        assert_eq!(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(DEFAULT_KEEP_ALIVE_TIMEOUT, Duration::from_secs(20));
        assert_eq!(DEFAULT_KEEP_ALIVE_INTERVAL, Duration::from_secs(20));
    }
}
