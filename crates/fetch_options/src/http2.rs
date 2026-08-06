// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP/2 specific connection options.

/// Largest legal HTTP/2 flow-control window size, in bytes.
///
/// RFC 9113 caps `SETTINGS_INITIAL_WINDOW_SIZE` at `2^31 - 1`; a larger value is a
/// protocol error that would surface as a connection failure far from the
/// misconfiguration, so requests are clamped to this value instead.
pub const MAX_HTTP2_WINDOW_SIZE: u32 = i32::MAX as u32;

/// Configuration options for HTTP/2 connections.
///
/// Controls HTTP/2-specific behavior such as stream limits and protocol settings.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Http2Options {
    /// Initial maximum number of streams that can be sent over HTTP/2 connections.
    ///
    /// `None` means no client-side cap is applied and the server's settings are used.
    pub initial_max_send_streams: Option<usize>,
    /// Whether adaptive tuning of the HTTP/2 flow-control window is enabled.
    ///
    /// Defaults to `false`.
    pub adaptive_window: bool,
    /// Initial flow-control window size, in bytes, for each HTTP/2 stream.
    ///
    /// `None` means the protocol default is used. Values are clamped to
    /// [`MAX_HTTP2_WINDOW_SIZE`]. Ignored when [`adaptive_window`][Self::adaptive_window]
    /// is enabled, because the window is then sized dynamically.
    pub initial_stream_window_size: Option<u32>,
}

impl Http2Options {
    /// Sets the initial maximum number of streams that can be sent over HTTP/2 connections.
    ///
    /// The default is `None`, which means no limit is set, and the maximum number of streams is determined by the server.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::Http2Options;
    ///
    /// let options = Http2Options::default().initial_max_send_streams(100);
    /// assert_eq!(options.initial_max_send_streams, Some(100));
    /// ```
    #[must_use]
    pub fn initial_max_send_streams(mut self, max_send_streams: impl Into<Option<usize>>) -> Self {
        self.initial_max_send_streams = max_send_streams.into();
        self
    }

    /// Enables adaptive tuning of the window size.
    ///
    /// Defaults to `false`, which keeps the initial window size fixed.
    /// If `true`, the client enables adaptive flow control.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::Http2Options;
    ///
    /// let options = Http2Options::default().adaptive_window(true);
    /// assert!(options.adaptive_window);
    /// ```
    #[must_use]
    pub fn adaptive_window(mut self, enabled: bool) -> Self {
        self.adaptive_window = enabled;
        self
    }

    /// Sets the initial flow-control window size, in bytes, for each HTTP/2 stream.
    ///
    /// The default is `None`, which uses the protocol default. Values are clamped to
    /// [`MAX_HTTP2_WINDOW_SIZE`]. This value is ignored when
    /// [`adaptive_window`][Self::adaptive_window] is enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::{Http2Options, MAX_HTTP2_WINDOW_SIZE};
    ///
    /// let options = Http2Options::default().initial_stream_window_size(1024 * 1024);
    /// assert_eq!(options.initial_stream_window_size, Some(1_048_576));
    ///
    /// // Values beyond the protocol maximum are clamped instead of failing at connect time.
    /// let clamped = Http2Options::default().initial_stream_window_size(u32::MAX);
    /// assert_eq!(
    ///     clamped.initial_stream_window_size,
    ///     Some(MAX_HTTP2_WINDOW_SIZE)
    /// );
    /// ```
    #[must_use]
    pub fn initial_stream_window_size(mut self, size: impl Into<Option<u32>>) -> Self {
        self.initial_stream_window_size = size.into().map(clamp_window_size);
        self
    }

    /// Returns the stream window size clamped to the protocol maximum.
    ///
    /// The builder method already clamps, but the field is public and may have been assigned
    /// directly. HTTP transports must use this method instead of reading
    /// [`initial_stream_window_size`][Self::initial_stream_window_size] verbatim.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::{Http2Options, MAX_HTTP2_WINDOW_SIZE};
    ///
    /// let mut options = Http2Options::default();
    /// options.initial_stream_window_size = Some(u32::MAX);
    ///
    /// assert_eq!(
    ///     options.effective_initial_stream_window_size(),
    ///     Some(MAX_HTTP2_WINDOW_SIZE)
    /// );
    /// ```
    #[must_use]
    #[inline]
    pub fn effective_initial_stream_window_size(&self) -> Option<u32> {
        self.initial_stream_window_size.map(clamp_window_size)
    }
}

/// Clamps an HTTP/2 stream window size to the protocol maximum.
#[inline]
fn clamp_window_size(size: u32) -> u32 {
    size.min(MAX_HTTP2_WINDOW_SIZE)
}

#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn http2_options_default() {
        assert_debug_snapshot!(Http2Options::default());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn configure_http2_options() {
        let options = Http2Options::default()
            .initial_max_send_streams(100)
            .adaptive_window(true)
            .initial_stream_window_size(1024 * 1024);
        assert_debug_snapshot!(options);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn stream_window_size_is_clamped_to_the_protocol_maximum() {
        let clamped = Http2Options::default().initial_stream_window_size(u32::MAX);
        assert_eq!(clamped.initial_stream_window_size, Some(MAX_HTTP2_WINDOW_SIZE));

        let maximum = Http2Options::default().initial_stream_window_size(MAX_HTTP2_WINDOW_SIZE);
        assert_eq!(maximum.initial_stream_window_size, Some(MAX_HTTP2_WINDOW_SIZE));

        let above_maximum = Http2Options::default().initial_stream_window_size(MAX_HTTP2_WINDOW_SIZE + 1);
        assert_eq!(above_maximum.initial_stream_window_size, Some(MAX_HTTP2_WINDOW_SIZE));

        let in_range = Http2Options::default().initial_stream_window_size(65_535);
        assert_eq!(in_range.initial_stream_window_size, Some(65_535));

        let cleared = Http2Options::default().initial_stream_window_size(None);
        assert_eq!(cleared.initial_stream_window_size, None);
    }

    #[test]
    fn effective_stream_window_size_clamps_direct_assignment() {
        let options = Http2Options {
            initial_stream_window_size: Some(u32::MAX),
            ..Default::default()
        };

        assert_eq!(options.effective_initial_stream_window_size(), Some(MAX_HTTP2_WINDOW_SIZE));
        assert_eq!(Http2Options::default().effective_initial_stream_window_size(), None);
    }
}
