// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use layered::{Layer, Service};

use crate::{HttpRequest, HttpResponse, RequestHandler, Result};

/// Buffers the entire HTTP response body into memory.
///
/// Wraps a [`RequestHandler`] and calls
/// [`HttpBody::into_buffered`][crate::HttpBody::into_buffered] on each response. This loads the
/// body in memory and releases the network connection.
///
/// Use it when later handlers must read the body more than once, for example for cloning, retries,
/// or inspection. It also returns the connection to the pool before later processing.
#[derive(Debug)]
pub struct Buffering<T> {
    inner: T,
}

/// Layer for creating [`Buffering`] instances.
#[derive(Debug, Clone, Copy)]
pub struct BufferingLayer;

impl Buffering<()> {
    /// Creates a new buffering handler layer.
    #[must_use]
    pub fn layer() -> BufferingLayer {
        BufferingLayer
    }
}

impl<S> Layer<S> for BufferingLayer {
    type Service = Buffering<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Buffering { inner }
    }
}

impl<T: RequestHandler> Service<HttpRequest> for Buffering<T> {
    type Out = Result<HttpResponse>;

    async fn execute(&self, input: HttpRequest) -> Result<HttpResponse> {
        let response = self.inner.execute(input).await?;

        let (parts, body) = response.into_parts();
        let body = body.into_buffered().await?;

        Ok(HttpResponse::from_parts(parts, body))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::{StatusCode, Uri};
    use http_extensions::{FakeHandler, HttpBodyBuilder, HttpRequestBuilder};

    use super::*;
    use crate::error_labels::collect_error_labels;
    use crate::handlers::Dispatch;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn buffers_response_body() {
        let inner = Dispatch::new_fake(FakeHandler::from(StatusCode::OK));
        let handler = Buffering { inner };

        let request = HttpRequestBuilder::new_fake().uri("https://example.com/path").build().unwrap();

        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // A buffered body is cloneable.
        assert!(response.body().try_clone().is_some());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn propagates_inner_handler_error() {
        let inner = Dispatch::new_fake(FakeHandler::never_completes());
        let handler = Buffering { inner };

        // Request without scheme/authority triggers a validation error in Dispatch.
        let request = http::Request::get(Uri::from_static("/no-authority"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let result: Result<HttpResponse> = handler.execute(request).await;
        let error = result.unwrap_err();
        assert_eq!(collect_error_labels(&error), "uri_origin_missing");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn layer_constructs_handler() {
        let layer = Buffering::layer();
        let inner = Dispatch::new_fake(FakeHandler::from(StatusCode::NO_CONTENT));
        let handler = layer.layer(inner);

        let request = HttpRequestBuilder::new_fake().uri("https://example.com/test").build().unwrap();

        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
