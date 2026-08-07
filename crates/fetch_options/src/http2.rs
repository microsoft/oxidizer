// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP/2 specific connection options.

/// Configuration options for HTTP/2 connections.
///
/// Controls HTTP/2-specific behavior such as stream limits and protocol settings.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Http2Options {
    /// Initial maximum number of streams that can be sent over HTTP/2 connections.
    ///
    /// `None` requests no override. The transport chooses its initial cap and updates it from
    /// the peer's HTTP/2 settings.
    pub initial_max_send_streams: Option<usize>,
    /// Whether adaptive tuning of the HTTP/2 flow-control window is enabled.
    ///
    /// Defaults to `false`.
    pub adaptive_window: bool,
    /// Initial flow-control window size, in bytes, for each HTTP/2 stream.
    ///
    /// `None` requests no override; the transport chooses its initial value. The bundled Hyper
    /// transport clamps values above the HTTP/2 maximum. Ignored when
    /// [`adaptive_window`][Self::adaptive_window] is enabled.
    pub initial_stream_window_size: Option<u32>,
}

impl Http2Options {
    /// Sets the initial maximum number of streams that can be sent over HTTP/2 connections.
    ///
    /// The default is `None`, which requests no override. The transport chooses its initial cap
    /// and updates it from the peer's HTTP/2 settings.
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
    /// The default is `None`, which requests no override. The bundled Hyper transport clamps
    /// values above the HTTP/2 maximum. This value is ignored when
    /// [`adaptive_window`][Self::adaptive_window] is enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::Http2Options;
    ///
    /// let options = Http2Options::default().initial_stream_window_size(1024 * 1024);
    /// assert_eq!(options.initial_stream_window_size, Some(1_048_576));
    /// ```
    #[must_use]
    pub fn initial_stream_window_size(mut self, size: impl Into<Option<u32>>) -> Self {
        self.initial_stream_window_size = size.into();
        self
    }
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
    fn stream_window_size_preserves_requested_value() {
        let configured = Http2Options::default().initial_stream_window_size(u32::MAX);
        assert_eq!(configured.initial_stream_window_size, Some(u32::MAX));

        let cleared = Http2Options::default().initial_stream_window_size(None);
        assert_eq!(cleared.initial_stream_window_size, None);
    }
}
