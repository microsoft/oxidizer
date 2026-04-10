// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use layered::Service;

use crate::{HttpRequest, HttpResponse, Result};

/// Trait alias for `Service<HttpRequest, Out = Result<HttpResponse>>`.
///
/// Use `RequestHandler` as a trait bound to avoid spelling out the full
/// `Service<HttpRequest, Out = Result<HttpResponse>>` constraint.
///
/// `RequestHandler` is sealed — implement [`Service<HttpRequest>`][layered::Service]
/// with `Out = Result<HttpResponse>` and it is derived automatically.
///
/// # Examples
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
///         self.0.execute(request).await
///     }
/// }
/// ```
pub trait RequestHandler: Service<HttpRequest, Out = Result<HttpResponse>> {}

impl<S> RequestHandler for S where S: Service<HttpRequest, Out = Result<HttpResponse>> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use layered::DynamicService;

    use super::*;

    static_assertions::assert_impl_all!(DynamicService<HttpRequest, crate::Result<HttpResponse>>: RequestHandler);
}
