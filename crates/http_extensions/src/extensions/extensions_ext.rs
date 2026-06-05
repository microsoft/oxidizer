// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Extensions;

use crate::{RequestInfo, UriTemplateLabel};

/// Extensions for [`http::Extensions`].
pub trait ExtensionsExt: sealed::Sealed {
    /// Returns the URI template label from extensions, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. A [`UriTemplateLabel`] attached directly to the extensions
    /// 2. From the [`RequestInfo`] explicit [`UriTemplateLabel`]
    /// 3. From the [`RequestInfo`] original URI path label (if set via `#[templated(label = "...")]`)
    /// 4. From the [`RequestInfo`] original URI path template string
    ///
    /// Returns `None` if no template information is available.
    fn uri_template_label(&self) -> Option<UriTemplateLabel>;
}

impl ExtensionsExt for Extensions {
    fn uri_template_label(&self) -> Option<UriTemplateLabel> {
        // A label attached directly to the extensions overrides anything derived
        // from `RequestInfo`.
        if let Some(label) = self.get::<UriTemplateLabel>() {
            return Some(label.clone());
        }

        let info = self.get::<RequestInfo>()?;

        if let Some(label) = &info.uri_template_label {
            return Some(label.clone());
        }
        if let Some(path) = info.original_uri.as_ref().and_then(|uri| uri.path_and_query()) {
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
    use templated_uri::{PathAndQuery, Uri};

    use super::*;

    fn request_info_with_path(path_and_query: impl Into<PathAndQuery>) -> RequestInfo {
        RequestInfo {
            original_uri: Some(Uri::new().with_path_and_query(path_and_query)),
            ..Default::default()
        }
    }

    #[test]
    fn returns_explicit_uri_template_label() {
        let mut extensions = Extensions::new();
        extensions.insert(RequestInfo {
            uri_template_label: Some(UriTemplateLabel::new("/api/users/{id}")),
            ..Default::default()
        });

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/api/users/{id}")
        );
    }

    #[test]
    fn returns_template_as_fallback_from_uri_path() {
        let mut extensions = Extensions::new();
        extensions.insert(request_info_with_path(PathAndQuery::from_static("/path")));

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
        extensions.insert(request_info_with_path(PathAndQuery::from_template(UserPosts {
            user_id: EscapedString::from_static("123"),
        })));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("user_posts")
        );
    }

    #[test]
    fn explicit_label_takes_precedence_over_target_path() {
        let mut extensions = Extensions::new();
        extensions.insert(RequestInfo {
            original_uri: Some(Uri::new().with_path_and_query(PathAndQuery::from_static("/path"))),
            uri_template_label: Some(UriTemplateLabel::new("/explicit")),
            ..Default::default()
        });

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/explicit")
        );
    }

    #[test]
    fn returns_label_from_standalone_extension() {
        let mut extensions = Extensions::new();
        extensions.insert(UriTemplateLabel::new("/api/orders/{id}"));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/api/orders/{id}")
        );
    }

    #[test]
    fn standalone_extension_takes_precedence_over_request_info() {
        let mut extensions = Extensions::new();
        extensions.insert(RequestInfo {
            uri_template_label: Some(UriTemplateLabel::new("/from-request-info")),
            ..Default::default()
        });
        extensions.insert(UriTemplateLabel::new("/from-extension"));

        assert_eq!(
            extensions.uri_template_label().as_ref().map(UriTemplateLabel::as_str),
            Some("/from-extension")
        );
    }

    #[test]
    fn returns_none_without_any_template_info() {
        let extensions = Extensions::new();
        assert!(extensions.uri_template_label().is_none());
    }
}
