// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_path_template::PathTemplate;
use routerama_build::HttpMethod;

use crate::build::request_body::RequestBody;
use crate::build::response_body::ResponseBody;
use crate::build::route::Route;

/// An additional HTTP binding for the same RPC as its parent [`HttpRule`](crate::build::HttpRule).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::{Binding, HttpMethod, RequestBody};
///
/// let binding = Binding::new(
///     HttpMethod::Get,
///     "/v1/shelves/{shelf}/info".parse().expect("valid"),
/// )
/// .request_body(RequestBody::Whole);
/// assert_eq!(binding.method().as_str(), "GET");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Binding {
    method: HttpMethod,
    template: PathTemplate,
    body: RequestBody,
    response_body: ResponseBody,
}

impl Binding {
    /// Creates a binding for `method` + `template`.
    ///
    /// This defaults to the request body being configured with [`RequestBody::None`]
    /// and the response body being configured with [`ResponseBody::Whole`].
    #[must_use]
    pub fn new(method: HttpMethod, template: PathTemplate) -> Self {
        Self {
            method,
            template,
            body: RequestBody::None,
            response_body: ResponseBody::Whole,
        }
    }

    /// Sets how the request body maps onto the RPC request message.
    #[must_use]
    pub fn request_body(mut self, body: RequestBody) -> Self {
        self.body = body;
        self
    }

    /// Sets how the RPC response message maps onto the HTTP response body.
    #[must_use]
    pub fn response_body(mut self, response_body: ResponseBody) -> Self {
        self.response_body = response_body;
        self
    }

    /// The HTTP method this binding matches.
    #[must_use]
    pub fn method(&self) -> &HttpMethod {
        &self.method
    }

    /// The parsed path template this binding matches.
    #[must_use]
    pub fn template(&self) -> &PathTemplate {
        &self.template
    }

    /// Lowers this binding into a [`Route`] for the RPC named `rpc`.
    pub(crate) fn into_route(self, rpc: impl Into<String>) -> Route {
        Route::new(rpc.into(), self.method, self.template, self.body, self.response_body)
    }
}
