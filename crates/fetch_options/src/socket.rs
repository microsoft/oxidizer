// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Socket-level tuning knobs applied to outbound TCP connections.
//!
//! This module defines [`SocketOptions`], the set of `TCP` socket settings a connector
//! applies to every connection it opens. It covers the Nagle algorithm ([`no_delay`][SocketOptions::no_delay])
//! and the kernel send/receive buffer sizes.
//!
//! Every option defaults to `None`, meaning "use the operating system default". Values left
//! as `None` resolve to that default rather than to a value of this crate's choosing, so
//! adding this struct never changes the observable behavior of an existing client.
//!
//! These settings are applied by transports that dial their own `TCP` sockets. They are
//! deliberately not part of [`TransportOptions`][crate::TransportOptions]: a transport that
//! does not own its sockets cannot honor them, and silently ignoring a tuning request is
//! worse than not offering it. The bundled Tokio transport accepts them through
//! `fetch::tokio::TokioTransportOptions`.
//!
//! # Example
//!
//! ```
//! use fetch_options::SocketOptions;
//!
//! let options = SocketOptions::default()
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
//! [`SocketOptions`] is consumed by a transport's connector rather than by
//! [`TransportOptions`][crate::TransportOptions]. It tunes the socket underneath a connection,
//! while [`Http2Options`][crate::Http2Options] tunes the protocol layer running on top of it.

/// Socket-level settings applied to outbound `TCP` connections.
///
/// Each field is `None` by default, which leaves the operating system default untouched.
///
/// These settings are honored only by transports that dial their own `TCP` sockets, which is
/// why they are deliberately not part of [`TransportOptions`][crate::TransportOptions]: a
/// transport that does not own its sockets cannot apply them, and silently ignoring a tuning
/// request is worse than not offering it. The bundled Tokio transport accepts them through
/// `fetch::tokio::TokioTransportOptions`.
// The reference above is intentionally not an intra-doc link: `fetch` depends on this crate,
// so linking the other way round would invert the dependency direction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SocketOptions {
    /// Whether the Nagle algorithm is disabled (`TCP_NODELAY`).
    ///
    /// `Some(true)` sends small writes immediately at the cost of extra packets.
    /// `None` keeps the operating system default, which enables Nagle on every platform
    /// `fetch` supports and is therefore equivalent to `Some(false)` in practice.
    pub no_delay: Option<bool>,
    /// Size of the kernel receive buffer (`SO_RCVBUF`), in bytes.
    ///
    /// `None` keeps the operating system default. The transport and kernel may clamp the
    /// requested value to their supported ranges.
    pub receive_buffer_size: Option<u32>,
    /// Size of the kernel send buffer (`SO_SNDBUF`), in bytes.
    ///
    /// `None` keeps the operating system default. The transport and kernel may clamp the
    /// requested value to their supported ranges.
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
    /// The default is `None`, which keeps the operating system default. The transport and
    /// kernel may clamp the requested value.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::SocketOptions;
    ///
    /// let options = SocketOptions::default().receive_buffer_size(128 * 1024);
    /// assert_eq!(options.receive_buffer_size, Some(131_072));
    /// ```
    #[must_use]
    pub fn receive_buffer_size(mut self, size: impl Into<Option<u32>>) -> Self {
        self.receive_buffer_size = size.into();
        self
    }

    /// Sets the kernel send buffer size, in bytes.
    ///
    /// The default is `None`, which keeps the operating system default. The transport and
    /// kernel may clamp the requested value.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::SocketOptions;
    ///
    /// let options = SocketOptions::default().send_buffer_size(128 * 1024);
    /// assert_eq!(options.send_buffer_size, Some(131_072));
    /// ```
    #[must_use]
    pub fn send_buffer_size(mut self, size: impl Into<Option<u32>>) -> Self {
        self.send_buffer_size = size.into();
        self
    }
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

    #[test]
    fn buffer_sizes_preserve_requested_values() {
        let options = SocketOptions::default().receive_buffer_size(0).send_buffer_size(u32::MAX);
        assert_eq!(options.receive_buffer_size, Some(0));
        assert_eq!(options.send_buffer_size, Some(u32::MAX));
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
}
