// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use layered::Service;

use crate::{HttpRequest, HttpResponse, Result};

/// A trait for HTTP request handlers in the middleware pipeline.
///
/// `RequestHandler` is a specialized [`Service`] that processes [`HttpRequest`]s
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
/// Implement [`Service`] (not `RequestHandler` directly) to create middleware.
/// Use `RequestHandler` as a trait bound on the inner service to constrain it
/// to request-handler–compatible types without spelling out the full
/// `Service<HttpRequest, Out = Result<HttpResponse>>` bound. Call
/// [`execute_request`](RequestHandler::execute_request) to delegate:
///
/// ```rust
/// # use http_extensions::{HttpRequest, HttpResponse, RequestHandler, Result};
/// # use layered::Service;
/// struct MyHandler<S>(S);
///
/// impl<S: RequestHandler> Service<HttpRequest> for MyHandler<S> {
///     type Out = Result<HttpResponse>;
///
///     async fn execute(&self, request: HttpRequest) -> Self::Out {
///         // Custom processing, then delegate to the inner handler.
///         self.0.execute_request(request).await
///     }
/// }
/// ```
pub trait RequestHandler: Send + Sync + sealed::Sealed {
    /// Processes an HTTP request and returns a response.
    fn execute_request(&self, request: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send;
}

impl<S> RequestHandler for S
where
    S: Service<HttpRequest, Out = Result<HttpResponse>>,
{
    fn execute_request(&self, request: HttpRequest) -> impl Future<Output = S::Out> + Send {
        Service::execute(self, request)
    }
}

pub(crate) mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<S> Sealed for S where S: Service<HttpRequest, Out = Result<HttpResponse>> {}
}
