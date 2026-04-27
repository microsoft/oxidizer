// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Extensions;
use templated_uri::uri::TargetPathAndQuery;

use crate::UrlTemplateLabel;

/// Extensions for [`http::Extensions`].
pub trait ExtensionsExt: sealed::Sealed {
    /// Returns the URL template label from extensions, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. From an explicit [`UrlTemplateLabel`] extension
    /// 2. From a [`TargetPathAndQuery`] label (if set via `#[templated(label = "...")]`)
    /// 3. From a [`TargetPathAndQuery`] template string
    ///
    /// Returns `None` if no template information is available.
    fn url_template_label(&self) -> Option<UrlTemplateLabel>;
}

impl ExtensionsExt for Extensions {
    fn url_template_label(&self) -> Option<UrlTemplateLabel> {
        if let Some(label) = self.get::<UrlTemplateLabel>() {
            return Some(label.clone());
        }
        if let Some(path) = self.get::<TargetPathAndQuery>() {
            return Some(UrlTemplateLabel::new(path.label().unwrap_or_else(|| path.template())));
        }

        None
    }
}

pub(crate) mod sealed {
    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl Sealed for super::Extensions {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn returns_explicit_url_template_label() {
        let mut extensions = Extensions::new();
        extensions.insert(UrlTemplateLabel::new("/api/users/{id}"));

        assert_eq!(
            extensions.url_template_label().as_ref().map(UrlTemplateLabel::as_str),
            Some("/api/users/{id}")
        );
    }

    #[test]
    fn returns_template_as_fallback_from_target_path_and_query() {
        let mut extensions = Extensions::new();
        extensions.insert(TargetPathAndQuery::from_path_and_query("/path".parse().unwrap()));

        assert_eq!(
            extensions.url_template_label().as_ref().map(UrlTemplateLabel::as_str),
            Some("/path")
        );
    }

    #[test]
    fn returns_label_from_templated_target_path_and_query() {
        use templated_uri::{UriSafeString, templated};

        #[templated(template = "/api/{user_id}/posts", label = "user_posts", bypass_redaction)]
        #[derive(Clone)]
        struct UserPosts {
            user_id: UriSafeString,
        }

        let mut extensions = Extensions::new();
        extensions.insert(TargetPathAndQuery::from_templated(UserPosts {
            user_id: UriSafeString::from_static("123"),
        }));

        assert_eq!(
            extensions.url_template_label().as_ref().map(UrlTemplateLabel::as_str),
            Some("user_posts")
        );
    }

    #[test]
    fn explicit_label_takes_precedence_over_target_path() {
        let mut extensions = Extensions::new();
        extensions.insert(UrlTemplateLabel::new("/explicit"));
        extensions.insert(TargetPathAndQuery::from_path_and_query("/path".parse().unwrap()));

        assert_eq!(
            extensions.url_template_label().as_ref().map(UrlTemplateLabel::as_str),
            Some("/explicit")
        );
    }

    #[test]
    fn returns_none_without_any_template_info() {
        let extensions = Extensions::new();
        assert!(extensions.url_template_label().is_none());
    }
}
