// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::UriTemplateLabel;
use crate::extensions::ExtensionsExt;
use http::Request;
use templated_uri::Path;

/// Extensions for HTTP requests.
pub trait RequestExt: sealed::Sealed {
    /// Returns the URI path associated with this request, if any.
    fn path(&self) -> Option<&Path>;

    /// Returns the URL template label for this request, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. From an explicit [`UriTemplateLabel`] extension attached to the request
    /// 2. From a templated URIs label (if set via `#[templated(label = "...")]`)
    /// 3. From a templated URIs template string
    ///
    /// Returns `None` if no template information is available.
    fn uri_template_label(&self) -> Option<UriTemplateLabel>;
}

impl<B> RequestExt for Request<B> {
    fn path(&self) -> Option<&Path> {
        self.extensions().get()
    }

    fn uri_template_label(&self) -> Option<UriTemplateLabel> {
        self.extensions().uri_template_label()
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<B> Sealed for Request<B> {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::Uri;

    use super::*;
    use crate::HttpBodyBuilder;

    #[test]
    fn template_extension() {
        let uri = Uri::from_static("https://example.com/path");
        let mut request = crate::Request::builder().uri(uri.clone()).body(()).unwrap();
        request.extensions_mut().insert(Path::from(uri.path_and_query().cloned().unwrap()));

        assert_eq!(request.path().unwrap().to_string().declassify_ref(), "/path");
    }

    #[test]
    fn uri_template_label_from_uri_template_label_extension() {
        let mut request = http::Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(UriTemplateLabel::new("/api/users/{id}"));

        let result = request.uri_template_label();
        assert_eq!(result.as_ref().map(UriTemplateLabel::as_str), Some("/api/users/{id}"));
    }

    #[test]
    fn uri_template_label_returns_none_without_template() {
        let request = http::Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let result = request.uri_template_label();
        assert!(result.is_none());
    }

    #[test]
    fn uri_template_label_falls_back_to_path_template() {
        let uri = Uri::from_static("https://example.com/api/users");
        let mut request = http::Request::get("https://example.com/api/users")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        // Attach a Path but no UriTemplateLabel.
        // For a plain PathAndQuery, label() returns None so the fallback
        // to template() is exercised.
        request.extensions_mut().insert(Path::from(uri.path_and_query().cloned().unwrap()));

        let result = request.uri_template_label();
        assert_eq!(result.as_ref().map(UriTemplateLabel::as_str), Some("/api/users"));
    }
}
