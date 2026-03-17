// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Request;
use templated_uri::uri::TargetPathAndQuery;

use crate::UrlTemplateLabel;

/// Extensions for HTTP requests.
pub trait RequestExt: sealed::Sealed {
    /// Returns the path and query associated with this request, if any.
    fn path_and_query(&self) -> Option<&TargetPathAndQuery>;

    /// Returns the URL template label for this request, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. From an explicit [`UrlTemplateLabel`] extension attached to the request
    /// 2. From a templated URI's label (if set via `#[templated(label = "...")]`)
    /// 3. From a templated URI's template string
    ///
    /// Returns `None` if no template information is available.
    fn url_template_label(&self) -> Option<UrlTemplateLabel>;
}

impl<B> RequestExt for Request<B> {
    fn path_and_query(&self) -> Option<&TargetPathAndQuery> {
        self.extensions().get()
    }

    fn url_template_label(&self) -> Option<UrlTemplateLabel> {
        self.extensions().get::<UrlTemplateLabel>().cloned().or_else(|| {
            self.path_and_query()
                .map(|path| UrlTemplateLabel::new(path.label().unwrap_or_else(|| path.template())))
        })
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<B> Sealed for Request<B> {}
}

#[cfg(test)]
mod tests {
    use http::Uri;

    use super::*;
    use crate::HttpBodyBuilder;

    #[test]
    fn template_extension() {
        let uri = Uri::from_static("https://example.com/path");
        let mut request = crate::Request::builder().uri(uri.clone()).body(()).unwrap();
        request
            .extensions_mut()
            .insert(TargetPathAndQuery::from_path_and_query(uri.path_and_query().cloned().unwrap()));

        assert_eq!(request.path_and_query().unwrap().to_uri_string(), "/path");
    }

    #[test]
    fn url_template_label_from_url_template_label_extension() {
        let mut request = http::Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(UrlTemplateLabel::new("/api/users/{id}"));

        let result = request.url_template_label();
        assert_eq!(result.as_ref().map(UrlTemplateLabel::as_str), Some("/api/users/{id}"));
    }

    #[test]
    fn url_template_label_returns_none_without_template() {
        let request = http::Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let result = request.url_template_label();
        assert!(result.is_none());
    }
}
