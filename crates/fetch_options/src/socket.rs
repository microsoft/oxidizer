// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Socket-level tuning requests for outbound TCP connections.
//!
//! [`SocketOptions`] requests `TCP_NODELAY` and kernel send/receive buffer overrides. `None`
//! requests no override; the transport and its lower layers choose the effective behavior.
//!
//! # Contents
//!
//! - [`SocketOptions`] - requested socket overrides.
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
//! Use these knobs for latency-sensitive small writes or high-throughput streaming connections.
//!
//! # Relationship to other modules
//!
//! Socket-owning transports consume [`SocketOptions`] alongside their transport-specific
//! configuration. [`Http2Options`][crate::Http2Options] configures the protocol layer instead.

/// Requested socket-level settings for outbound `TCP` connections.
///
/// Each field defaults to `None`, which requests no override from the transport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SocketOptions {
    /// Whether the Nagle algorithm is disabled (`TCP_NODELAY`).
    ///
    /// `Some(true)` requests immediate small writes at the cost of extra packets.
    /// `None` requests no override; the transport's lower layer chooses the effective value.
    pub no_delay: Option<bool>,
    /// Size of the kernel receive buffer (`SO_RCVBUF`), in bytes.
    ///
    /// `None` requests no override. The transport and kernel may accept, adjust, or reject the
    /// requested value.
    pub receive_buffer_size: Option<u32>,
    /// Size of the kernel send buffer (`SO_SNDBUF`), in bytes.
    ///
    /// `None` requests no override. The transport and kernel may accept, adjust, or reject the
    /// requested value.
    pub send_buffer_size: Option<u32>,
}

impl SocketOptions {
    /// Sets whether the Nagle algorithm is disabled on connected sockets.
    ///
    /// The default is `None`, which requests no override from the transport.
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
    /// The default is `None`, which requests no override. The transport and kernel may accept,
    /// adjust, or reject the requested value.
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
    /// The default is `None`, which requests no override. The transport and kernel may accept,
    /// adjust, or reject the requested value.
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
