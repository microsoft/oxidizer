// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_path_template::{Grammar, PathTemplate};
use routerama::HttpMethod;

use super::request_body::RequestBody;
use super::response_body::ResponseBody;
use super::route::Route;

/// An additional HTTP binding for the same RPC as its parent [`HttpRule`](crate::build::HttpRule).
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use rest_over_grpc::build::{Binding, RequestBody};
/// use routerama::HttpMethod;
///
/// let binding = Binding::new(
///     HttpMethod::GET,
///     PathTemplate::parse("/v1/shelves/{shelf}/info", Grammar::default()).expect("valid"),
/// )
/// .request_body(RequestBody::Whole);
/// assert_eq!(binding.method().as_str(), "GET");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Binding {
    method: HttpMethod,
    pattern: String,
    body: RequestBody,
    response_body: ResponseBody,
    response_body_default: Option<String>,
}

impl Binding {
    /// Creates a binding for `method` + `template`.
    ///
    /// This defaults to the request body being configured with [`RequestBody::None`]
    /// and the response body being configured with [`ResponseBody::Whole`].
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the template is rendered to its stored text; by-value keeps the builder call ergonomic"
    )]
    pub fn new(method: HttpMethod, template: PathTemplate<'_>) -> Self {
        Self {
            method,
            pattern: template.to_string(),
            body: RequestBody::None,
            response_body: ResponseBody::Whole,
            response_body_default: None,
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

    /// Records the proto3-JSON literal to emit when the selected `response_body`
    /// field holds its default value (computed from the response descriptor).
    #[must_use]
    pub(crate) fn with_response_body_default(mut self, default: Option<String>) -> Self {
        self.response_body_default = default;
        self
    }

    /// The proto3-JSON default literal for this binding's `response_body` field,
    /// if one was recorded.
    pub(crate) fn response_body_default(&self) -> Option<&str> {
        self.response_body_default.as_deref()
    }

    /// The HTTP method this binding matches.
    #[must_use]
    pub fn method(&self) -> &HttpMethod {
        &self.method
    }

    /// The parsed path template this binding matches.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn template(&self) -> PathTemplate<'_> {
        // Parse with affix support so rendered affixes round-trip.
        PathTemplate::parse(&self.pattern, Grammar::default().with_segment_affixes())
            .expect("pattern was validated when the Binding was created")
    }

    /// Lowers this binding into a [`Route`] for the RPC named `rpc`.
    pub(crate) fn into_route(self, rpc: impl Into<String>) -> Route {
        Route::new(rpc.into(), self.method, self.pattern, self.body, self.response_body)
    }
}
