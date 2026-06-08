// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP request filtering configuration.

/// Controls which URI schemes the HTTP client is willing to send to.
///
/// Defaults to [`RequestFilter::Https`]. The builder method typically includes an option
/// to switch to [`RequestFilter::HttpAndHttps`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RequestFilter {
    /// Only `https://` requests are permitted.
    #[default]
    Https,
    /// Both `http://` and `https://` requests are permitted.
    HttpAndHttps,
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use insta::assert_debug_snapshot;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_request_filter_type() {
        static_assertions::assert_impl_all!(
            RequestFilter: Send,
            Sync,
            Clone,
            Debug,
            Default
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn request_filter_default() {
        assert_debug_snapshot!(RequestFilter::default());
    }
}
