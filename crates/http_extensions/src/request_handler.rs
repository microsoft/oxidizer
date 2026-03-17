// Copyright (c) Microsoft Corporation.

use layered::Service;

use crate::{HttpBodyBuilder, HttpRequest, HttpRequestBuilder, HttpResponse, Result};

/// A type alias for HTTP request handlers in the middleware pipeline.
///
/// `RequestHandler` is specialized [`Service`] that processes [`HttpRequest`]s
/// and returns [`HttpResponse`]s. It serves as the foundation for building HTTP
/// middleware pipelines.
///
/// # Creating Custom Handlers
///
/// **Note**: `RequestHandler` is a sealed trait and should not be implemented directly.
/// Instead, implement [`Service<HttpRequest>`][layered::Service] with
/// `Out = Result<HttpResponse>`, and it will automatically implement `RequestHandler`.
///
/// For detailed information on creating services and middleware, see the
/// [`layered`] documentation.
///
/// # Examples
///
/// ```rust
/// # use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
/// # use layered::Service;
/// struct MyHandler<S>(S);
///
/// // My handler wraps another service constrained to `RequestHandler`
/// // and implements the `Service` trait with particular input and output types.
/// impl<S: RequestHandler> Service<HttpRequest> for MyHandler<S> {
///     type Out = Result<HttpResponse>;
///
///     async fn execute(&self, request: HttpRequest) -> Self::Out {
///         // do some custom processing and call the inner handler
///         self.0.execute(request).await
///     }
/// }
/// ```
pub trait RequestHandler: Send + Sync + sealed::Sealed {
    /// Processes an HTTP request and returns a response.
    fn execute(&self, request: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send;
}

impl<S> RequestHandler for S
where
    S: Service<HttpRequest, Out = Result<HttpResponse>>,
{
    fn execute(&self, request: HttpRequest) -> impl Future<Output = S::Out> + Send {
        self.execute(request)
    }
}

/// Extension trait for types that implement `RequestHandler`.
pub trait RequestHandlerExt: RequestHandler
where
    Self: Sized,
{
    /// Creates a new HTTP request builder associated with this handler.
    fn request_builder(&self) -> HttpRequestBuilder<'_, Self>;
}

impl<T> RequestHandlerExt for T
where
    T: RequestHandler + AsRef<HttpBodyBuilder>,
{
    fn request_builder(&self) -> HttpRequestBuilder<'_, Self> {
        HttpRequestBuilder::with_request_handler(self, self.as_ref())
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<S> Sealed for S where S: Service<HttpRequest, Out = Result<HttpResponse>> {}
}
