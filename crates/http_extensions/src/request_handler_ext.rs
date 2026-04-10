// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use layered::{DynamicService, DynamicServiceExt, Service};

use crate::{HttpRequest, HttpResponse, Result};

/// Extension trait for [`RequestHandler`][crate::RequestHandler] that provides type erasure.
pub trait RequestHandlerExt: crate::RequestHandler + 'static {
    /// Converts this handler into a type-erased [`DynamicService`].
    fn into_dynamic_service(self) -> DynamicService<HttpRequest, Result<HttpResponse>>;
}

impl<T: crate::RequestHandler + 'static> RequestHandlerExt for T {
    fn into_dynamic_service(self) -> DynamicService<HttpRequest, Result<HttpResponse>> {
        ServiceAdapter(self).into_dynamic()
    }
}

/// Adapter that converts a `RequestHandler` into a `Service`.
#[derive(Debug)]
struct ServiceAdapter<T>(T);

impl<T: crate::RequestHandler> Service<HttpRequest> for ServiceAdapter<T> {
    type Out = crate::Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        self.0.execute_request(input)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;
    use http::StatusCode;

    use crate::{FakeHandler, HttpRequestBuilder};

    use super::*;
    #[test]
    fn into_dynamic_service_ok() {
        let service = FakeHandler::from(StatusCode::INTERNAL_SERVER_ERROR).into_dynamic_service();

        let response = block_on(service.execute(HttpRequestBuilder::new_fake().uri("https://dummy.com").build().unwrap())).unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
