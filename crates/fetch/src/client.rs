// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::ready;
use std::sync::Arc;
use std::time::Duration;

use bytesbuf::BytesBuf;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use futures::FutureExt;
use futures::future::Either;
use http::Method;
use http_extensions::routing::{BaseUriConflict, Router, RouterContext};
use http_extensions::timeout::ResponseTimeout;
use http_extensions::{HttpRequestBuilder, HttpRequestBuilderExt, RequestExt};
use layered::Service;
use templated_uri::{BaseUri, Uri};
use thread_aware::{PerCore, ThreadAware};
use tick::{Clock, FutureExt as TimeoutExt};

use crate::pipeline::Pipeline;
use crate::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, Result};

/// A runtime-agnostic HTTP client for sending HTTP requests.
///
/// `HttpClient` provides a high-level, fluent API for common HTTP operations over a
/// configurable transport. It runs on the Tokio runtime by default, but any runtime
/// and transport can be plugged in (see the [`custom`](crate::custom) module).
///
/// > **Tip**: Cloning the client is cheap and results in instances that share the underlying connection
/// > pool and configuration.
///
/// # Examples
///
/// ```
/// # use http::header::USER_AGENT;
/// # use fetch::HttpClient;
/// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
/// // Make a GET request
/// let response = client
///     .get("https://example.com")
///     .header(USER_AGENT, "MyApp/1.0")
///     .fetch()
///     .await?;
///
/// println!("Status: {}", response.status());
/// # Ok(())
/// # }
/// ```
///
/// See [crate-level][`crate`] documentation for more details on available configuration options
/// and advanced usage scenarios.
#[derive(Debug, Clone, ThreadAware)]
pub struct HttpClient {
    pipeline: HttpClientPipeline,
    body_builder: HttpBodyBuilder,
    clock: Clock,
    #[thread_aware(skip)]
    router: Router,
}

impl HttpClient {
    /// Creates a request builder with the specified method and URI.
    ///
    /// This is the most flexible way to build requests. For common HTTP methods like
    /// GET or POST, you can use the dedicated helper methods instead.
    ///
    /// # Performance tip
    ///
    /// While you can provide strings for the method and URI, using pre-created
    /// [`Method`] and [`Uri`] instances is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::header::USER_AGENT;
    /// use fetch::HttpClient;
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Using strings (convenient but with parsing overhead)
    /// let response = client
    ///     .request("GET", "https://example.com/api")
    ///     .fetch()
    ///     .await?;
    ///
    /// // Using pre-parsed values (more efficient) and additional customization
    /// // before fetching the response.
    /// let method = http::Method::GET;
    /// let uri = "https://example.com/api".parse::<http::Uri>()?;
    /// let response = client
    ///     .request(method, uri)
    ///     .header(USER_AGENT, "MyApp/1.0")
    ///     .fetch()
    ///     .await?;
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn request(
        &self,
        method: impl TryInto<Method, Error: Into<http::Error>>,
        uri: impl TryInto<Uri, Error: Into<HttpError>>,
    ) -> HttpRequestBuilder<'_, Self> {
        self.request_builder().method(method).uri(uri)
    }

    /// Creates a GET request to the specified URI.
    ///
    /// This is a convenient shortcut for `request(Method::GET, uri)`.
    ///
    /// # Performance tip
    ///
    /// While you can provide a string for the URI, using a pre-created [`Uri`]
    /// instance is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::{HttpClient, HttpResponse, Response};
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Basic GET request
    /// let response: HttpResponse = client.get("https://example.com").fetch().await?;
    ///
    /// // GET with additional customization
    /// let response: HttpResponse = client
    ///     .get("https://api.example.com/users")
    ///     .header("X-API-Key", "my-key")
    ///     .fetch()
    ///     .await?;
    ///
    /// // Get response as text
    /// let body: Response<String> = client.get("https://example.com").fetch_text().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn get(&self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> HttpRequestBuilder<'_, Self> {
        self.request(Method::GET, uri)
    }

    /// Creates a POST request to the specified URI.
    ///
    /// This is a convenient shortcut for `request(Method::POST, uri)`.
    ///
    /// # Performance tip
    ///
    /// While you can provide a string for the URI, using a pre-created [`Uri`]
    /// instance is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::header::USER_AGENT;
    /// # use fetch::HttpClient;
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    ///
    /// // Simple POST without a body
    /// let response = client.post("https://api.example.com/users").fetch().await?;
    ///
    /// // POST with text body and additional customization
    /// let response = client
    ///     .post("https://api.example.com/users")
    ///     .header(USER_AGENT, "MyApp/1.0")
    ///     .text("my-text")
    ///     .fetch()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn post(&self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> HttpRequestBuilder<'_, Self> {
        self.request(Method::POST, uri)
    }

    /// Creates a DELETE request to the specified URI.
    ///
    /// This is a convenient shortcut for `request(Method::DELETE, uri)`.
    ///
    /// # Performance tip
    ///
    /// While you can provide a string for the URI, using a pre-created [`Uri`]
    /// instance is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::HttpClient;
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Delete a resource
    /// let response = client
    ///     .delete("https://api.example.com/users/123")
    ///     .header("Authorization", "Bearer token")
    ///     .fetch()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete(&self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> HttpRequestBuilder<'_, Self> {
        self.request(Method::DELETE, uri)
    }

    /// Creates a HEAD request to the specified URI.
    ///
    /// This is a convenient shortcut for `request(Method::HEAD, uri)`. HEAD requests are
    /// similar to GET but return only headers without a body, useful for checking if a
    /// resource exists or has been modified.
    ///
    /// # Performance tip
    ///
    /// While you can provide a string for the URI, using a pre-created [`Uri`]
    /// instance is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::HttpClient;
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Check if a resource exists without downloading it
    /// let response = client
    ///     .head("https://example.com/large-file.zip")
    ///     .fetch()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn head(&self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> HttpRequestBuilder<'_, Self> {
        self.request(Method::HEAD, uri)
    }

    /// Creates a PUT request to the specified URI.
    ///
    /// This is a convenient shortcut for `request(Method::PUT, uri)`.
    ///
    /// # Performance tip
    ///
    /// While you can provide a string for the URI, using a pre-created [`Uri`]
    /// instance is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::header::USER_AGENT;
    /// # use fetch::HttpClient;
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    ///
    /// // Simple PUT without a body
    /// let response = client.put("https://api.example.com/users").fetch().await?;
    ///
    /// // PUT with text body and additional customization
    /// let response = client
    ///     .put("https://api.example.com/users")
    ///     .header(USER_AGENT, "MyApp/1.0")
    ///     .text("my-text")
    ///     .fetch()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn put(&self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> HttpRequestBuilder<'_, Self> {
        self.request(Method::PUT, uri)
    }

    /// Creates a PATCH request to the specified URI.
    ///
    /// This is a convenient shortcut for `request(Method::PATCH, uri)`. PATCH requests are used
    /// for partial updates to resources, modifying only the specified fields rather than
    /// replacing the entire resource.
    ///
    /// # Performance tip
    ///
    /// While you can provide a string for the URI, using a pre-created [`Uri`]
    /// instance is more efficient as it avoids parsing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::header::USER_AGENT;
    /// # use fetch::HttpClient;
    /// # async fn example(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Partially update a resource with JSON payload
    /// let response = client
    ///     .patch("https://api.example.com/users/123")
    ///     .header(USER_AGENT, "MyApp/1.0")
    ///     .text("some content")
    ///     .fetch()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn patch(&self, uri: impl TryInto<Uri, Error = templated_uri::UriError>) -> HttpRequestBuilder<'_, Self> {
        self.request(Method::PATCH, uri)
    }

    pub(super) fn new(pipeline: HttpClientPipeline, body_builder: HttpBodyBuilder, clock: Clock, router: Router) -> Self {
        Self {
            pipeline,
            body_builder,
            clock,
            router,
        }
    }

    /// Returns a new `HttpClient` that uses the given base URI for all requests.
    ///
    /// The new client shares this client's pipeline and configuration; only the
    /// base URI differs. The original client is left unchanged.
    #[must_use]
    pub fn with_base_uri(&self, base_uri: BaseUri) -> Self {
        Self {
            pipeline: self.pipeline.clone(),
            body_builder: self.body_builder.clone(),
            clock: self.clock.clone(),
            // Preserve historical semantics: the client's base URI overrides any endpoint
            // already present on the request URI.
            router: Router::fixed(base_uri).conflict_policy(BaseUriConflict::UseRouted),
        }
    }

    pub(super) fn pipeline(&self) -> &Pipeline {
        match &self.pipeline {
            HttpClientPipeline::Shared(p) => p,
            HttpClientPipeline::Isolated(p) => p,
        }
    }
}

impl AsRef<HttpBodyBuilder> for HttpClient {
    fn as_ref(&self) -> &HttpBodyBuilder {
        &self.body_builder
    }
}

impl Memory for HttpClient {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.body_builder.memory().reserve(min_bytes)
    }
}

impl HasMemory for HttpClient {
    fn memory(&self) -> impl MemoryShared {
        self.body_builder.clone()
    }
}

impl Service<HttpRequest> for HttpClient {
    type Out = Result<HttpResponse>;

    fn execute(&self, mut input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let timeout = input
            .extensions()
            .get::<ResponseTimeout>()
            .map_or_else(|| Duration::MAX, ResponseTimeout::duration);

        // Make the router available to downstream layers (retry, hedging) via request
        // extensions so they can re-resolve the URI against an alternative endpoint on each
        // attempt. Attaching router only make sense if it can provide alternative endpoints.
        if self.router.has_alternatives() {
            input.extensions_mut().insert(self.router.clone());
        }

        if let Err(e) = self.router.resolve_request_uri(RouterContext::default(), &mut input) {
            // Carry whatever request metadata the builder attached so the
            // error remains diagnosable even though routing never ran.
            let request_info = input.request_info().cloned().unwrap_or_default();
            Either::Left(ready(Err(e.with_request_info(request_info))))
        } else {
            // Capture request metadata before the pipeline consumes the
            // request so it can be attached to errors that are synthesized
            // without request context (e.g. a response timeout, or an
            // internal total/attempt timeout produced by the resilience
            // layers).
            let request_info = input.request_info().cloned().unwrap_or_default();
            Either::Right(
                self.pipeline()
                    .execute(input)
                    .timeout(&self.clock, timeout)
                    .map(move |outcome| match outcome {
                        // A response timeout fires here, outside the
                        // pipeline, so the error carries no request context.
                        Err(_) => Err(HttpError::timeout(timeout).with_request_info(request_info)),
                        Ok(Ok(response)) => Ok(response),
                        // Backfill the metadata on any pipeline error that
                        // did not already carry it, leaving the richer
                        // dispatch-attached info untouched.
                        Ok(Err(error)) if error.request_info().is_none() => Err(error.with_request_info(request_info)),
                        Ok(Err(error)) => Err(error),
                    }),
            )
        }
    }
}

#[derive(ThreadAware, Clone, Debug)]
pub(super) enum HttpClientPipeline {
    Shared(#[thread_aware(skip)] Arc<Pipeline>),
    Isolated(thread_aware::Arc<Pipeline, PerCore>),
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::{Request, StatusCode, Uri};
    use http_extensions::FakeHandler;
    use ohno::ErrorExt;
    use seatbelt::{Recovery, RecoveryKind};
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::HttpResponseBuilder;
    use crate::error_labels::collect_error_labels;
    use crate::fake::FakeDeps;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_send() {
        assert_impl_all!(HttpClient: Send, Sync, Clone, ThreadAware);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn assert_fetch_future_not_large() {
        let client = HttpClient::new_fake(StatusCode::OK);

        let future = client.get("http://example.com").fetch();
        let size = size_of_val(&future);

        // last verified future size 784
        assert!(size < 2000, "future size is too large, size: {size}");

        println!("size of future: {size}");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn http_not_allowed_ensure_rejected() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).build();

        let err = client.get("http://example.com").fetch_text().await.unwrap_err();

        assert_eq!(
            err.message(),
            "unable to communicate with 'http://example.com', because the 'http' scheme is not allowed by this HTTP client"
        );
        assert_eq!(collect_error_labels(&err), "scheme_not_allowed");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_post_method() {
        test_method(super::HttpClient::post, Method::POST).await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_delete_method() {
        test_method(super::HttpClient::delete, Method::DELETE).await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_head_method() {
        test_method(super::HttpClient::head, Method::HEAD).await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_put_method() {
        test_method(super::HttpClient::put, Method::PUT).await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_patch_method() {
        test_method(super::HttpClient::patch, Method::PATCH).await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_request_method_with_string_params() {
        let fake = FakeHandler::from_sync_handler(|request| {
            assert_eq!(request.method(), http::Method::DELETE);
            assert_eq!(request.uri().to_string(), "https://example.com/path");
            HttpResponseBuilder::new_fake().status(StatusCode::IM_A_TEAPOT).build()
        });
        let client = HttpClient::new_fake(fake);

        let response = client.request("DELETE", "https://example.com/path").fetch().await.unwrap();

        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fetch_fails_when_router_uri_resolution_conflicts() {
        use http_extensions::routing::BaseUriConflict;

        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .router(Router::fixed(BaseUri::from_static("https://api.example.com")).conflict_policy(BaseUriConflict::Fail))
            .build();

        // The request targets a different absolute base URI than the router's fixed base URI.
        // With a `Fail` conflict policy the router rejects resolution in `execute`, short-circuiting
        // before the request ever reaches the pipeline.
        let err = client.get("https://existing.example.com/items").fetch().await.unwrap_err();

        assert_eq!(collect_error_labels(&err), "uri_conflict");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_request_handler_implementation() {
        let client = HttpClient::new_fake(StatusCode::ACCEPTED);

        // Create a test request
        let request = Request::builder()
            .method(http::Method::GET)
            .uri("https://example.com")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        // Test the RequestHandler implementation
        let response = client.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_minimal_pipeline() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .minimal_pipeline()
            .build();

        let response = client.get("https://example.com").fetch().await.unwrap();
        assert!(matches!(client.pipeline(), Pipeline::Minimal(_)));
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_has_memory() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .minimal_pipeline()
            .build();

        let memory = client.memory();
        let sb = memory.reserve(123_456);
        assert!(sb.capacity() >= 123_456);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_memory() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .minimal_pipeline()
            .build();

        let sb = client.reserve(123_456);
        assert!(sb.capacity() >= 123_456);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_custom_pipeline() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .custom_pipeline(|_root, _ctx| FakeHandler::from(StatusCode::IM_A_TEAPOT))
            .build();

        let response = client.get("https://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }

    // Test error handling with invalid URIs
    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_invalid_uri_handling() {
        let client = HttpClient::new_fake(StatusCode::OK);

        let error = client.get("not a valid uri").fetch().await.unwrap_err();

        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
        assert_eq!(collect_error_labels(&error), "uri_invalid");
        assert!(
            error.to_string().starts_with("invalid uri character"),
            "Unexpected error message: {error}"
        );
    }

    async fn test_method(callback: impl Fn(&HttpClient, Uri) -> HttpRequestBuilder<'_, HttpClient>, method: Method) {
        let uri = Uri::from_static("https://example.com/test");
        let uri_cloned = uri.clone();

        let client = HttpClient::new_fake(FakeHandler::from_sync_handler(move |request| {
            assert_eq!(request.method(), method);
            assert_eq!(request.uri().path(), "/test");
            HttpResponseBuilder::new_fake().build()
        }));

        callback(&client, uri_cloned).fetch().await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn multiple_pools_creates_pooled_dispatch_handler() {
        use crate::handlers::DispatchMode;
        use crate::options::{ConnectionPoolOptions, PoolSelection};
        use crate::pipeline::Pipeline;

        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .minimal_pipeline()
            .connection_pool_options(
                ConnectionPoolOptions::default().multiple_pools(3, PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT)),
            )
            .build();

        let Pipeline::Minimal(dispatch) = client.pipeline() else {
            panic!("Expected minimal pipeline");
        };

        let DispatchMode::Pooled { transports, .. } = &dispatch.mode else {
            panic!("Expected pooled dispatch handler mode");
        };

        assert_eq!(transports.len(), 3);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn multiple_pools_with_count_one_creates_single_dispatch_handler() {
        use crate::handlers::DispatchMode;
        use crate::options::{ConnectionPoolOptions, PoolSelection};
        use crate::pipeline::Pipeline;

        let mut pools = ConnectionPoolOptions::default();
        pools.multiple_pools = Some((1, PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT)));
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .minimal_pipeline()
            .connection_pool_options(pools)
            .build();

        let Pipeline::Minimal(dispatch) = client.pipeline() else {
            panic!("Expected minimal pipeline");
        };

        assert!(
            matches!(&dispatch.mode, DispatchMode::Single(_)),
            "Expected single dispatch handler mode for pool_count=1"
        );
    }
}
