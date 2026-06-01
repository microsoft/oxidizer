// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Request;
use templated_uri::{PathAndQuery, Uri};

use crate::extensions::ExtensionsExt;
use crate::routing::RequestUris;
use crate::{HttpError, UriTemplateLabel};

/// Extensions for HTTP requests.
pub trait RequestExt: sealed::Sealed {
    /// Returns the URI path and query associated with this request, if any.
    fn path_and_query(&self) -> Option<&PathAndQuery>;

    /// Returns the URI template label for this request, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. From an explicit [`UriTemplateLabel`] extension attached to the request
    /// 2. From a templated URIs label (if set via `#[templated(label = "...")]`)
    /// 3. From a templated URIs template string
    ///
    /// Returns `None` if no template information is available.
    fn uri_template_label(&self) -> Option<UriTemplateLabel>;

    /// Returns the best-known templated [`Uri`] for this request.
    ///
    /// Resolution order:
    /// 1. [`RequestUris::routed`] if present (most recent routing result),
    /// 2. otherwise [`RequestUris::original`] if the extension is attached,
    /// 3. otherwise parsed from [`Request::uri`].
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when falling back to step 3 and the
    /// request's [`http::Uri`] cannot be converted to a [`Uri`].
    fn resolve_uri(&self) -> Result<Uri, HttpError>;
}

impl<B> RequestExt for Request<B> {
    fn path_and_query(&self) -> Option<&PathAndQuery> {
        self.extensions().get()
    }

    fn uri_template_label(&self) -> Option<UriTemplateLabel> {
        self.extensions().uri_template_label()
    }

    fn resolve_uri(&self) -> Result<Uri, HttpError> {
        if let Some(uris) = self.extensions().get::<RequestUris>() {
            return Ok(uris.routed().unwrap_or_else(|| uris.original()).clone());
        }
        Ok(self.uri().clone().try_into()?)
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
        request
            .extensions_mut()
            .insert(PathAndQuery::from(uri.path_and_query().cloned().unwrap()));

        assert_eq!(request.path_and_query().unwrap().to_string().declassify_ref(), "/path");
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

        // Attach a PathAndQuery but no UriTemplateLabel.
        // For a plain PathAndQuery, label() returns None so the fallback
        // to template() is exercised.
        request
            .extensions_mut()
            .insert(PathAndQuery::from(uri.path_and_query().cloned().unwrap()));

        let result = request.uri_template_label();
        assert_eq!(result.as_ref().map(UriTemplateLabel::as_str), Some("/api/users"));
    }

    #[test]
    fn resolve_uri_prefers_routed_from_request_uris_extension() {
        use templated_uri::Uri as TemplatedUri;

        let original = TemplatedUri::try_from(Uri::from_static("/v1/items")).unwrap();
        let routed = TemplatedUri::try_from(Uri::from_static("https://api.example.com/v1/items")).unwrap();

        let mut uris = RequestUris::new(original);
        uris.set_routed(routed);

        let mut request = http::Request::get("/v1/items").body(HttpBodyBuilder::new_fake().empty()).unwrap();
        request.extensions_mut().insert(uris);

        let resolved = request.resolve_uri().unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn resolve_uri_falls_back_to_original_when_routed_is_unset() {
        use templated_uri::Uri as TemplatedUri;

        let original = TemplatedUri::try_from(Uri::from_static("/v1/items")).unwrap();
        let uris = RequestUris::new(original);

        let mut request = http::Request::get("https://different.example.com/other")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(uris);

        // `original` wins over the request's current `http::Uri`.
        let resolved = request.resolve_uri().unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "/v1/items");
    }

    #[test]
    fn resolve_uri_falls_back_to_request_uri_without_extension() {
        let request = http::Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let resolved = request.resolve_uri().unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://example.com/api/users/123");
    }
}
