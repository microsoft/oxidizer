// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`HttpRule`] type and rule lowering.

use http_path_template::PathTemplate;

use crate::body::Body;
use crate::http_method::HttpMethod;
use crate::response_body::ResponseBody;
use crate::route::Route;
use crate::rule_error::RuleError;

/// A declarative HTTP binding for a single gRPC RPC, mirroring
/// [`google.api.HttpRule`](https://github.com/googleapis/googleapis/blob/master/google/api/http.proto).
///
/// Build one with [`HttpRule::new`] and refine it with the builder-style
/// setters, then [`HttpRule::lower`] it into one or more [`Route`]s.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::{Body, HttpMethod, HttpRule, ResponseBody};
///
/// let rule = HttpRule::new("UpdateBook", HttpMethod::Patch, "/v1/books/{book}")
///     .with_body(Body::Field("book".to_owned()))
///     .with_response_body(ResponseBody::Field("book".to_owned()))
///     .with_additional_binding(HttpRule::new(
///         "UpdateBook",
///         HttpMethod::Patch,
///         "/v1/shelves/{shelf}/books/{book}",
///     ));
///
/// assert_eq!(rule.rpc(), "UpdateBook");
///
/// let routes = rule.lower().expect("the path templates are valid");
/// assert_eq!(routes.len(), 2);
/// assert_eq!(routes[0].method().as_str(), "PATCH");
/// assert_eq!(routes[0].body(), &Body::Field("book".to_owned()));
/// assert_eq!(
///     routes[0].response_body(),
///     &ResponseBody::Field("book".to_owned())
/// );
/// assert_eq!(routes[1].rpc(), "UpdateBook");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HttpRule {
    rpc: String,
    method: HttpMethod,
    pattern: String,
    body: Body,
    response_body: ResponseBody,
    additional_bindings: Vec<Self>,
}

impl HttpRule {
    /// Creates a rule binding the RPC named `rpc` to `method` + `pattern` (a
    /// `google.api.http` path template).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule};
    ///
    /// let rule = HttpRule::new("GetBook", HttpMethod::Get, "/v1/books/{book}");
    ///
    /// assert_eq!(rule.rpc(), "GetBook");
    /// assert_eq!(rule.lower().expect("valid path template").len(), 1);
    /// ```
    #[must_use]
    pub fn new(rpc: impl Into<String>, method: HttpMethod, pattern: impl Into<String>) -> Self {
        Self {
            rpc: rpc.into(),
            method,
            pattern: pattern.into(),
            body: Body::None,
            response_body: ResponseBody::Whole,
            additional_bindings: Vec::new(),
        }
    }

    /// Sets how the request body maps onto the RPC request message.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{Body, HttpMethod, HttpRule};
    ///
    /// let routes = HttpRule::new("CreateBook", HttpMethod::Post, "/v1/books")
    ///     .with_body(Body::Whole)
    ///     .lower()
    ///     .expect("valid path template");
    ///
    /// assert_eq!(routes[0].body(), &Body::Whole);
    /// ```
    #[must_use]
    pub fn with_body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    /// Sets how the RPC response message maps onto the HTTP response body.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule, ResponseBody};
    ///
    /// let routes = HttpRule::new("GetBook", HttpMethod::Get, "/v1/books/{book}")
    ///     .with_response_body(ResponseBody::Field("book".to_owned()))
    ///     .lower()
    ///     .expect("valid path template");
    ///
    /// assert_eq!(
    ///     routes[0].response_body(),
    ///     &ResponseBody::Field("book".to_owned())
    /// );
    /// ```
    #[must_use]
    pub fn with_response_body(mut self, response_body: ResponseBody) -> Self {
        self.response_body = response_body;
        self
    }

    /// Adds an additional HTTP binding for the same RPC
    /// (`HttpRule.additional_bindings`).
    ///
    /// Per the `google.api.http` spec, nested `additional_bindings` are not
    /// allowed; any such nesting on `binding` is rejected by [`HttpRule::lower`].
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule};
    ///
    /// let routes = HttpRule::new("GetBook", HttpMethod::Get, "/v1/books/{book}")
    ///     .with_additional_binding(HttpRule::new(
    ///         "GetBook",
    ///         HttpMethod::Get,
    ///         "/v1/shelves/{shelf}/books/{book}",
    ///     ))
    ///     .lower()
    ///     .expect("valid path templates");
    ///
    /// assert_eq!(routes.len(), 2);
    /// assert_eq!(routes[1].rpc(), "GetBook");
    /// ```
    #[must_use]
    pub fn with_additional_binding(mut self, binding: Self) -> Self {
        self.additional_bindings.push(binding);
        self
    }

    /// The name of the RPC this rule binds.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule};
    ///
    /// let rule = HttpRule::new("ListBooks", HttpMethod::Get, "/v1/books");
    ///
    /// assert_eq!(rule.rpc(), "ListBooks");
    /// ```
    #[must_use]
    pub fn rpc(&self) -> &str {
        &self.rpc
    }

    /// Lowers this rule (and its `additional_bindings`) into one [`Route`] per
    /// binding.
    ///
    /// # Errors
    ///
    /// Returns a [`RuleError`] if any path template fails to parse, or if an
    /// `additional_bindings` entry itself carries nested `additional_bindings`
    /// (which the spec forbids).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule};
    ///
    /// let routes = HttpRule::new("GetBook", HttpMethod::Get, "/v1/books/{book}")
    ///     .with_additional_binding(HttpRule::new(
    ///         "GetBook",
    ///         HttpMethod::Get,
    ///         "/v1/books/{book}:read",
    ///     ))
    ///     .lower()
    ///     .expect("valid path templates");
    ///
    /// assert_eq!(routes.len(), 2);
    /// assert_eq!(routes[0].method().as_str(), "GET");
    /// assert_eq!(routes[1].template().verb(), Some("read"));
    /// ```
    pub fn lower(&self) -> Result<Vec<Route>, RuleError> {
        let mut routes = Vec::with_capacity(1 + self.additional_bindings.len());
        routes.push(self.lower_self()?);

        for binding in &self.additional_bindings {
            if !binding.additional_bindings.is_empty() {
                return Err(RuleError::nested_bindings(&binding.rpc));
            }
            routes.push(binding.lower_self()?);
        }

        Ok(routes)
    }

    fn lower_self(&self) -> Result<Route, RuleError> {
        let template = PathTemplate::parse(&self.pattern).map_err(|source| RuleError::bad_template(&self.rpc, &self.pattern, source))?;

        Ok(Route::new(
            self.rpc.clone(),
            self.method.clone(),
            template,
            self.body.clone(),
            self.response_body.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_single_rule() {
        let rule = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}");
        let routes = rule.lower().expect("valid");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].rpc(), "GetShelf");
        assert_eq!(routes[0].method().as_str(), "GET");
    }

    #[test]
    fn lowers_additional_bindings() {
        let rule = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}").with_additional_binding(HttpRule::new(
            "GetShelf",
            HttpMethod::Get,
            "/v1/shelves/{shelf}/info",
        ));
        let routes = rule.lower().expect("valid");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn rejects_nested_additional_bindings() {
        let inner = HttpRule::new("X", HttpMethod::Get, "/a").with_additional_binding(HttpRule::new("X", HttpMethod::Get, "/b"));
        let rule = HttpRule::new("X", HttpMethod::Get, "/c").with_additional_binding(inner);
        let err = rule.lower().expect_err("nested bindings are illegal");
        assert_eq!(err.rpc(), "X");
    }

    #[test]
    fn surfaces_template_parse_errors() {
        let rule = HttpRule::new("Bad", HttpMethod::Get, "no-leading-slash");
        let err = rule.lower().expect_err("bad template");
        assert!(err.to_string().contains("Bad"));
    }

    #[test]
    fn builder_setters_are_preserved() {
        let rule = HttpRule::new("UpdateShelf", HttpMethod::Patch, "/v1/shelves/{shelf}")
            .with_body(Body::Field("shelf".into()))
            .with_response_body(ResponseBody::Field("shelf".into()));
        assert_eq!(rule.rpc(), "UpdateShelf");

        let routes = rule.lower().expect("valid");
        assert!(matches!(routes[0].method(), HttpMethod::Patch));
        assert!(matches!(routes[0].body(), Body::Field(field) if field == "shelf"));
        assert!(matches!(routes[0].response_body(), ResponseBody::Field(field) if field == "shelf"));
        assert_eq!(routes[0].template().verb(), None);
    }
}
