// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::HttpRequest;

/// Request cloning with body support.
///
/// Provides methods to clone HTTP requests including their bodies when possible.
pub trait HttpRequestExt: sealed::Sealed {
    /// Tries to clone the entire request, including its body.
    ///
    /// Returns `Some(HttpRequest)` if the body supports cloning/replay, or
    /// `None` if the body cannot be cloned. On success, the clone preserves the
    /// method, URI, HTTP version, headers, and extensions of the source.
    fn try_clone(&self) -> Option<Self>
    where
        Self: Sized;
}

impl HttpRequestExt for HttpRequest {
    fn try_clone(&self) -> Option<Self> {
        self.body().try_clone().map(|body| {
            let mut request = Self::new(body);
            *request.method_mut() = self.method().clone();
            *request.uri_mut() = self.uri().clone();
            *request.version_mut() = self.version();
            *request.headers_mut() = self.headers().clone();
            *request.extensions_mut() = self.extensions().clone();
            request
        })
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl Sealed for HttpRequest {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;
    use http::{Request, Uri};
    use http_body_util::Empty;

    use super::*;
    use crate::HttpBodyBuilder;

    #[test]
    fn clone_http_request_ok() {
        let body = HttpBodyBuilder::new_fake().text("dummy");
        let mut request = Request::new(body);
        *request.method_mut() = http::Method::POST;
        *request.uri_mut() = Uri::from_static("https://example.com/path");
        *request.version_mut() = http::Version::HTTP_11;
        request.headers_mut().insert("x-test", "value".parse().unwrap());
        request.extensions_mut().insert(42_u32);

        let cloned = request.try_clone().unwrap();
        assert_eq!(cloned.method(), request.method());
        assert_eq!(cloned.uri(), request.uri());
        assert_eq!(cloned.version(), request.version());
        assert_eq!(cloned.headers(), request.headers());
        assert_eq!(cloned.extensions().get::<u32>(), Some(&42_u32));
        assert_eq!(block_on(cloned.into_body().into_text()).unwrap(), "dummy");
    }

    #[test]
    fn clone_http_request_non_cloneable() {
        let body = HttpBodyBuilder::new_fake().external(Empty::default());
        let request = Request::new(body);

        assert!(request.try_clone().is_none());
    }
}
