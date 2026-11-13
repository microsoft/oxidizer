// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP request filtering configuration.

/// Controls which URI schemes the HTTP client can use.
///
/// The default is [`RequestFilter::Https`]. A client builder can provide an option
/// for [`RequestFilter::HttpAndHttps`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RequestFilter {
    /// Permits only `https://` requests.
    #[default]
    Https,
    /// Permits `http://` and `https://` requests.
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
