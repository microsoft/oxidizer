// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Extensions;
use templated_uri::Path;

use crate::UriTemplateLabel;

/// Extensions for [`http::Extensions`].
pub trait ExtensionsExt: sealed::Sealed {
    /// Returns the URL template label from extensions, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. From an explicit [`UriTemplateLabel`] extension
    /// 2. From a [`Path`] label (if set via `#[templated(label = "...")]`)
    /// 3. From a [`Path`] template string
    ///
    /// Returns `None` if no template information is available.
    fn uri_template_label(&self) -> Option<UriTemplateLabel>;
}

impl ExtensionsExt for Extensions {
    fn uri_template_label(&self) -> Option<UriTemplateLabel> {
        if let Some(label) = self.get::<UriTemplateLabel>() {
            return Some(label.clone());
        }
        if let Some(path) = self.get::<Path>() {
            return Some(UriTemplateLabel::new(path.label().unwrap_or_else(|| path.template())));
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
    fn returns_explicit_uri_template_label() {
        let mut extensions = Extensions::new();
        extensions.insert(UriTemplateLabel::new("/api/users/{id}"));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/api/users/{id}")
        );
    }

    #[test]
    fn returns_template_as_fallback_from_uri_path() {
        let mut extensions = Extensions::new();
        extensions.insert(Path::from_static("/path"));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/path")
        );
    }

    #[test]
    fn returns_label_from_templated_uri_path() {
        use templated_uri::{EscapedString, templated};

        #[templated(template = "/api/{user_id}/posts", label = "user_posts", unredacted)]
        #[derive(Clone)]
        struct UserPosts {
            user_id: EscapedString,
        }

        let mut extensions = Extensions::new();
        extensions.insert(Path::from_template(UserPosts {
            user_id: EscapedString::from_static("123"),
        }));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("user_posts")
        );
    }

    #[test]
    fn explicit_label_takes_precedence_over_target_path() {
        let mut extensions = Extensions::new();
        extensions.insert(UriTemplateLabel::new("/explicit"));
        extensions.insert(Path::from_static("/path"));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/explicit")
        );
    }

    #[test]
    fn returns_none_without_any_template_info() {
        let extensions = Extensions::new();
        assert!(extensions.uri_template_label().is_none());
    }
}
