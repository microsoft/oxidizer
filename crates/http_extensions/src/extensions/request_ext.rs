// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::Request;
use recoverable::Attempt;
use templated_uri::{PathAndQuery, Uri};

use crate::extensions::ExtensionsExt;
use crate::{HttpError, RequestInfo, UriTemplateLabel};

/// Extensions for HTTP requests.
pub trait RequestExt: sealed::Sealed {
    /// Returns the [`RequestInfo`] attached to this request, if any.
    ///
    /// [`RequestInfo`] aggregates routing and resilience metadata (URIs,
    /// template label, attempt). Returns `None` when no [`RequestInfo`] has been
    /// attached yet.
    fn request_info(&self) -> Option<&RequestInfo>;

    /// Returns a mutable reference to this request's [`RequestInfo`].
    ///
    /// Attaches a default [`RequestInfo`] first if none is present, so the
    /// returned reference can always be used to record metadata.
    fn request_info_mut(&mut self) -> &mut RequestInfo;

    /// Returns the URI path and query associated with this request, if any.
    fn path_and_query(&self) -> Option<&PathAndQuery>;

    /// Returns the URI template label for this request, if available.
    ///
    /// This method checks for a template label in the following order:
    /// 1. A [`UriTemplateLabel`] attached directly to the request's extensions
    /// 2. From the [`RequestInfo`] explicit [`UriTemplateLabel`]
    /// 3. From the [`RequestInfo`] original URI path label (if set via `#[templated(label = "...")]`)
    /// 4. From the [`RequestInfo`] original URI path template string
    ///
    /// Returns `None` if no template information is available.
    fn uri_template_label(&self) -> Option<UriTemplateLabel>;

    /// Returns the [`Attempt`] recorded on this request, if any.
    ///
    /// Reads the [`attempt`](RequestInfo::attempt) field of the [`RequestInfo`]
    /// extension. Returns `None` when no attempt has been recorded.
    fn attempt(&self) -> Option<Attempt>;

    /// Records the [`Attempt`] for this request.
    ///
    /// Stores it in the [`attempt`](RequestInfo::attempt) field of the
    /// [`RequestInfo`] extension, attaching a [`RequestInfo`] if none is present
    /// and preserving any other fields.
    fn set_attempt(&mut self, attempt: Attempt);

    /// Returns the best-known templated [`Uri`] for this request.
    ///
    /// Resolution order:
    /// 1. [`RequestInfo::routed_uri`] if present (most recent routing result),
    /// 2. otherwise [`RequestInfo::original_uri`] if the extension is attached,
    /// 3. otherwise parsed from [`Request::uri`].
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::validation`] when falling back to step 3 and the
    /// request's [`http::Uri`] cannot be converted to a [`Uri`].
    fn resolve_uri(&self) -> Result<Uri, HttpError>;
}

impl<B> RequestExt for Request<B> {
    fn request_info(&self) -> Option<&RequestInfo> {
        self.extensions().get::<RequestInfo>()
    }

    fn request_info_mut(&mut self) -> &mut RequestInfo {
        self.extensions_mut().get_or_insert_with(RequestInfo::default)
    }

    fn path_and_query(&self) -> Option<&PathAndQuery> {
        self.request_info()
            .and_then(|info| info.original_uri.as_ref())
            .and_then(Uri::path_and_query)
    }

    fn uri_template_label(&self) -> Option<UriTemplateLabel> {
        self.extensions().uri_template_label()
    }

    fn attempt(&self) -> Option<Attempt> {
        self.request_info().and_then(|info| info.attempt)
    }

    fn set_attempt(&mut self, attempt: Attempt) {
        self.request_info_mut().attempt = Some(attempt);
    }

    fn resolve_uri(&self) -> Result<Uri, HttpError> {
        if let Some(uri) = self.request_info().and_then(RequestInfo::resolved_uri) {
            return Ok(uri.clone());
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
        let original = templated_uri::Uri::from_static("https://example.com/path");
        let mut request = crate::Request::builder().uri("https://example.com/path").body(()).unwrap();
        request.extensions_mut().insert(RequestInfo {
            original_uri: Some(original),
            ..Default::default()
        });

        assert_eq!(request.path_and_query().unwrap().to_string().declassify_ref(), "/path");
    }

    #[test]
    fn uri_template_label_from_explicit_label() {
        let mut request = http::Request::get("https://example.com/api/users/123")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(RequestInfo {
            uri_template_label: Some(UriTemplateLabel::new("/api/users/{id}")),
            ..Default::default()
        });

        let result = request.uri_template_label();
        assert_eq!(result.as_ref().map(UriTemplateLabel::as_str), Some("/api/users/{id}"));
    }

    #[test]
    fn uri_template_label_from_standalone_extension() {
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
        let original = templated_uri::Uri::from_static("https://example.com/api/users");
        let mut request = http::Request::get("https://example.com/api/users")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        // Attach a RequestInfo with only the original URI and no explicit label.
        // For a plain path, label() returns None so the fallback
        // to template() is exercised.
        request.extensions_mut().insert(RequestInfo {
            original_uri: Some(original),
            ..Default::default()
        });

        let result = request.uri_template_label();
        assert_eq!(result.as_ref().map(UriTemplateLabel::as_str), Some("/api/users"));
    }

    #[test]
    fn resolve_uri_prefers_routed_from_request_info_extension() {
        use templated_uri::Uri as TemplatedUri;

        let original = TemplatedUri::try_from(Uri::from_static("/v1/items")).unwrap();
        let routed = TemplatedUri::try_from(Uri::from_static("https://api.example.com/v1/items")).unwrap();

        let mut request = http::Request::get("/v1/items").body(HttpBodyBuilder::new_fake().empty()).unwrap();
        request.extensions_mut().insert(RequestInfo {
            original_uri: Some(original),
            routed_uri: Some(routed),
            ..Default::default()
        });

        let resolved = request.resolve_uri().unwrap();
        assert_eq!(resolved.to_string().declassify_into(), "https://api.example.com/v1/items");
    }

    #[test]
    fn resolve_uri_falls_back_to_original_when_routed_is_unset() {
        use templated_uri::Uri as TemplatedUri;

        let original = TemplatedUri::try_from(Uri::from_static("/v1/items")).unwrap();

        let mut request = http::Request::get("https://different.example.com/other")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(RequestInfo {
            original_uri: Some(original),
            ..Default::default()
        });

        // `original_uri` wins over the request's current `http::Uri`.
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

    #[test]
    fn attempt_returns_none_without_extension() {
        let request = http::Request::get("https://example.com/")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        assert!(request.attempt().is_none());
    }

    #[test]
    fn set_attempt_then_attempt_round_trips() {
        let mut request = http::Request::get("https://example.com/")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        request.set_attempt(Attempt::new(2, true));

        let attempt = request.attempt().expect("attempt should be recorded");
        assert_eq!(attempt.index(), 2);
        assert!(attempt.is_last());
    }

    #[test]
    fn set_attempt_preserves_other_request_info_fields() {
        let original = templated_uri::Uri::from_static("https://example.com/path");
        let mut request = http::Request::get("https://example.com/path")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(RequestInfo {
            original_uri: Some(original),
            ..Default::default()
        });

        request.set_attempt(Attempt::new(1, false));

        let attempt = request.attempt().expect("attempt should be recorded");
        assert_eq!(attempt.index(), 1);
        // The previously attached `original_uri` is still present.
        assert_eq!(request.path_and_query().unwrap().to_string().declassify_ref(), "/path");
    }

    #[test]
    fn request_info_returns_none_without_extension() {
        let request = http::Request::get("https://example.com/")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        assert!(request.request_info().is_none());
    }

    #[test]
    fn request_info_returns_attached_info() {
        let original = templated_uri::Uri::from_static("https://example.com/path");
        let mut request = http::Request::get("https://example.com/path")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();
        request.extensions_mut().insert(RequestInfo {
            original_uri: Some(original),
            ..Default::default()
        });

        let info = request.request_info().expect("request info should be present");
        assert!(info.original_uri.is_some());
    }

    #[test]
    fn request_info_mut_inserts_default_when_absent() {
        let mut request = http::Request::get("https://example.com/")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        assert!(request.request_info().is_none());

        request.request_info_mut().attempt = Some(Attempt::new(4, true));

        let attempt = request.attempt().expect("attempt should be recorded");
        assert_eq!(attempt.index(), 4);
    }
}
