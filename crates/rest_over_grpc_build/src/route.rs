// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Route`] type.

use http_path_template::PathTemplate;

use crate::body::Body;
use crate::http_method::HttpMethod;
use crate::response_body::ResponseBody;

/// A single lowered HTTP route: one HTTP method + parsed path template bound to
/// an RPC, together with its body / response-body configuration.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::{Body, HttpMethod, HttpRule, ResponseBody};
///
/// let routes = HttpRule::new("UpdateBook", HttpMethod::Patch, "/v1/books/{book}")
///     .with_body(Body::Field("book".to_owned()))
///     .with_response_body(ResponseBody::Field("book".to_owned()))
///     .lower()
///     .expect("valid path template");
/// let route = &routes[0];
///
/// assert_eq!(route.rpc(), "UpdateBook");
/// assert_eq!(route.method().as_str(), "PATCH");
/// assert_eq!(route.template().verb(), None);
/// assert_eq!(route.body(), &Body::Field("book".to_owned()));
/// assert_eq!(
///     route.response_body(),
///     &ResponseBody::Field("book".to_owned())
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Route {
    rpc: String,
    method: HttpMethod,
    template: PathTemplate,
    body: Body,
    response_body: ResponseBody,
}

impl Route {
    /// Creates a route from its lowered parts.
    pub(crate) fn new(rpc: String, method: HttpMethod, template: PathTemplate, body: Body, response_body: ResponseBody) -> Self {
        Self {
            rpc,
            method,
            template,
            body,
            response_body,
        }
    }

    /// The RPC this route dispatches to.
    #[must_use]
    pub fn rpc(&self) -> &str {
        &self.rpc
    }

    /// The HTTP method this route matches.
    #[must_use]
    pub fn method(&self) -> &HttpMethod {
        &self.method
    }

    /// The parsed path template this route matches.
    #[must_use]
    pub fn template(&self) -> &PathTemplate {
        &self.template
    }

    /// How the request body maps onto the RPC request message.
    #[must_use]
    pub fn body(&self) -> &Body {
        &self.body
    }

    /// How the RPC response maps onto the HTTP response body.
    #[must_use]
    pub fn response_body(&self) -> &ResponseBody {
        &self.response_body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_rule::HttpRule;

    #[test]
    fn accessors_return_lowered_route_parts() {
        let rule = HttpRule::new("UpdateBook", HttpMethod::Patch, "/v1/books/{book}")
            .with_body(Body::Field("book".to_owned()))
            .with_response_body(ResponseBody::Field("book".to_owned()));
        let routes = rule.lower().expect("valid path template");
        let route = &routes[0];

        assert_eq!(route.rpc(), "UpdateBook");
        assert_eq!(route.method().as_str(), "PATCH");
        assert_eq!(route.template().verb(), None);
        assert_eq!(route.body(), &Body::Field("book".to_owned()));
        assert_eq!(route.response_body(), &ResponseBody::Field("book".to_owned()));
    }
}
