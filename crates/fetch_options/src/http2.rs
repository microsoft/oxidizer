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
    /// `None` means no client-side cap is applied and the server's settings are used.
    pub initial_max_send_streams: Option<usize>,
    /// Whether adaptive tuning of the HTTP/2 flow-control window is enabled.
    ///
    /// Defaults to `false`.
    pub adaptive_window: bool,
}

impl Http2Options {
    /// Sets the initial maximum number of streams that can be sent over HTTP/2 connections.
    ///
    /// The default is `None`, which means no limit is set, and the maximum number of streams is determined by the server.
    #[must_use]
    pub fn initial_max_send_streams(mut self, max_send_streams: impl Into<Option<usize>>) -> Self {
        self.initial_max_send_streams = max_send_streams.into();
        self
    }

    /// Enables adaptive tuning of the window size.
    ///
    /// Defaults to `false`, which keeps the initial window size fixed.
    /// If `true`, the client enables adaptive flow control.
    #[must_use]
    pub fn adaptive_window(mut self, enabled: bool) -> Self {
        self.adaptive_window = enabled;
        self
    }
}

#[cfg(not(miri))]
#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;

    use super::*;

    #[test]
    fn http2_options_default() {
        assert_debug_snapshot!(Http2Options::default());
    }
}
