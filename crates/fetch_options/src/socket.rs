// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Socket-level tuning knobs applied to outbound TCP connections.
//!
//! This module defines [`SocketOptions`], the set of `TCP` socket settings a connector
//! applies to every connection it opens. It covers the Nagle algorithm ([`no_delay`][SocketOptions::no_delay])
//! and the kernel send/receive buffer sizes.
//!
//! Every option defaults to `None`, meaning "leave the operating system default in place".
//! Only explicitly configured values are applied, so adding this struct never changes
//! the behavior of an existing client.
//!
//! These settings are honored by the connectors bundled with `fetch`. A custom connector
//! is responsible for applying them itself.
//!
//! # Example
//!
//! ```
//! use fetch_options::{SocketOptions, TransportOptions};
//!
//! let mut options = TransportOptions::default();
//! options.socket = SocketOptions::default()
//!     .no_delay(true)
//!     .send_buffer_size(256 * 1024);
//! ```
//!
//! # When to use
//!
//! Reach for these knobs on latency-sensitive, high-throughput links where the OS defaults
//! are a poor fit, for example a service-to-service hop that sends many small messages
//! (`no_delay`) or streams large payloads over a high bandwidth-delay-product path
//! (buffer sizes).
//!
//! # Relationship to other modules
//!
//! [`SocketOptions`] is reachable through [`TransportOptions::socket`][crate::TransportOptions::socket],
//! alongside [`Http2Options`][crate::Http2Options] which tunes the protocol layer running on
//! top of these sockets.

/// Smallest accepted socket buffer size, in bytes.
///
/// Requested sizes are clamped up to this value. Zero in particular must not reach the
/// kernel because on Windows `SO_SNDBUF = 0` disables send buffering entirely rather than
/// meaning "use the default", which is a surprising and hard-to-diagnose behavior change.
pub const MIN_SOCKET_BUFFER_SIZE: u32 = 4 * 1024;

/// Largest accepted socket buffer size, in bytes.
///
/// Requested sizes are clamped down to this value. 64 MiB is far above any realistic
/// bandwidth-delay product for a service-to-service hop, so a larger request indicates a
/// misconfiguration rather than an intent to reserve that much kernel memory per connection.
pub const MAX_SOCKET_BUFFER_SIZE: u32 = 64 * 1024 * 1024;

/// Socket-level settings applied to outbound `TCP` connections.
///
/// Each field is `None` by default, which leaves the operating system default untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SocketOptions {
    /// Whether the Nagle algorithm is disabled (`TCP_NODELAY`).
    ///
    /// `Some(true)` sends small writes immediately at the cost of extra packets.
    /// `None` keeps the operating system default.
    pub no_delay: Option<bool>,
    /// Size of the kernel receive buffer (`SO_RCVBUF`), in bytes.
    ///
    /// `None` keeps the operating system default. Values are clamped into
    /// <code>[MIN_SOCKET_BUFFER_SIZE]..=[MAX_SOCKET_BUFFER_SIZE]</code> when set. The kernel may
    /// clamp or scale the request further, so the effective size can still differ.
    pub receive_buffer_size: Option<u32>,
    /// Size of the kernel send buffer (`SO_SNDBUF`), in bytes.
    ///
    /// `None` keeps the operating system default. Values are clamped into
    /// <code>[MIN_SOCKET_BUFFER_SIZE]..=[MAX_SOCKET_BUFFER_SIZE]</code> when set. The kernel may
    /// clamp or scale the request further, so the effective size can still differ.
    pub send_buffer_size: Option<u32>,
}

impl SocketOptions {
    /// Sets whether the Nagle algorithm is disabled on connected sockets.
    ///
    /// The default is `None`, which keeps the operating system default.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::SocketOptions;
    ///
    /// let options = SocketOptions::default().no_delay(true);
    /// assert_eq!(options.no_delay, Some(true));
    /// ```
    #[must_use]
    pub fn no_delay(mut self, no_delay: impl Into<Option<bool>>) -> Self {
        self.no_delay = no_delay.into();
        self
    }

    /// Sets the kernel receive buffer size, in bytes.
    ///
    /// The default is `None`, which keeps the operating system default. A requested size is
    /// clamped into <code>[MIN_SOCKET_BUFFER_SIZE]..=[MAX_SOCKET_BUFFER_SIZE]</code>.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::SocketOptions;
    ///
    /// let options = SocketOptions::default().receive_buffer_size(128 * 1024);
    /// assert_eq!(options.receive_buffer_size, Some(131_072));
    ///
    /// // Out-of-range requests are clamped rather than passed to the kernel.
    /// let clamped = SocketOptions::default().receive_buffer_size(0);
    /// assert_eq!(
    ///     clamped.receive_buffer_size,
    ///     Some(fetch_options::MIN_SOCKET_BUFFER_SIZE)
    /// );
    /// ```
    #[must_use]
    pub fn receive_buffer_size(mut self, size: impl Into<Option<u32>>) -> Self {
        self.receive_buffer_size = size.into().map(clamp_buffer_size);
        self
    }

    /// Sets the kernel send buffer size, in bytes.
    ///
    /// The default is `None`, which keeps the operating system default. A requested size is
    /// clamped into <code>[MIN_SOCKET_BUFFER_SIZE]..=[MAX_SOCKET_BUFFER_SIZE]</code>.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::SocketOptions;
    ///
    /// let options = SocketOptions::default().send_buffer_size(128 * 1024);
    /// assert_eq!(options.send_buffer_size, Some(131_072));
    ///
    /// // Out-of-range requests are clamped rather than passed to the kernel.
    /// let clamped = SocketOptions::default().send_buffer_size(u32::MAX);
    /// assert_eq!(
    ///     clamped.send_buffer_size,
    ///     Some(fetch_options::MAX_SOCKET_BUFFER_SIZE)
    /// );
    /// ```
    #[must_use]
    pub fn send_buffer_size(mut self, size: impl Into<Option<u32>>) -> Self {
        self.send_buffer_size = size.into().map(clamp_buffer_size);
        self
    }

    /// Reports whether any setting must be applied before the socket connects.
    ///
    /// Buffer sizes must be set on an unconnected socket to affect `TCP` window scaling,
    /// whereas [`no_delay`][Self::no_delay] can be applied to an already connected stream.
    /// Connectors use this to skip the more expensive unconnected-socket path when no
    /// pre-connect tuning was requested; it is public so third-party connectors can reuse
    /// the same fast/slow-path split.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::SocketOptions;
    ///
    /// assert!(
    ///     !SocketOptions::default()
    ///         .no_delay(true)
    ///         .requires_pre_connect_setup()
    /// );
    /// assert!(
    ///     SocketOptions::default()
    ///         .send_buffer_size(65_536)
    ///         .requires_pre_connect_setup()
    /// );
    /// ```
    #[must_use]
    pub fn requires_pre_connect_setup(&self) -> bool {
        self.receive_buffer_size.is_some() || self.send_buffer_size.is_some()
    }
}

/// Clamps a requested buffer size into the accepted range.
fn clamp_buffer_size(size: u32) -> u32 {
    size.clamp(MIN_SOCKET_BUFFER_SIZE, MAX_SOCKET_BUFFER_SIZE)
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use insta::assert_debug_snapshot;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_socket_options_type() {
        static_assertions::assert_impl_all!(
            SocketOptions: Send,
            Sync,
            Clone,
            Copy,
            Debug,
            Default
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn buffer_sizes_are_clamped_into_the_accepted_range() {
        // Zero must never reach the kernel: on Windows it disables send buffering outright.
        let too_small = SocketOptions::default().receive_buffer_size(0).send_buffer_size(1);
        assert_eq!(too_small.receive_buffer_size, Some(MIN_SOCKET_BUFFER_SIZE));
        assert_eq!(too_small.send_buffer_size, Some(MIN_SOCKET_BUFFER_SIZE));

        let too_large = SocketOptions::default()
            .receive_buffer_size(u32::MAX)
            .send_buffer_size(MAX_SOCKET_BUFFER_SIZE + 1);
        assert_eq!(too_large.receive_buffer_size, Some(MAX_SOCKET_BUFFER_SIZE));
        assert_eq!(too_large.send_buffer_size, Some(MAX_SOCKET_BUFFER_SIZE));

        // Values already inside the range are passed through untouched.
        let in_range = SocketOptions::default().receive_buffer_size(64 * 1024);
        assert_eq!(in_range.receive_buffer_size, Some(65_536));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn socket_options_default() {
        assert_debug_snapshot!(SocketOptions::default());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn configure_socket_options() {
        let options = SocketOptions::default()
            .no_delay(true)
            .receive_buffer_size(64 * 1024)
            .send_buffer_size(32 * 1024);

        assert_debug_snapshot!(options);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn options_can_be_cleared() {
        let options = SocketOptions::default().no_delay(true).no_delay(None).send_buffer_size(None);

        assert_eq!(options, SocketOptions::default());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn pre_connect_setup_only_needed_for_buffer_sizes() {
        assert!(!SocketOptions::default().requires_pre_connect_setup());
        assert!(!SocketOptions::default().no_delay(false).requires_pre_connect_setup());
        assert!(SocketOptions::default().receive_buffer_size(1).requires_pre_connect_setup());
        assert!(SocketOptions::default().send_buffer_size(1).requires_pre_connect_setup());
    }
}
