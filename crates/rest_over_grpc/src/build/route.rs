// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_path_template::{Grammar, PathTemplate};
use routerama::HttpMethod;

use super::{RequestBody, ResponseBody};

/// A single lowered HTTP route: one HTTP method + parsed path template bound to
/// an RPC, together with its body / response-body configuration.
///
/// Produced internally by [`HttpRule::lower`](crate::build::HttpRule); consumed by the
/// router/transcoder codegen. Not part of the public API.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Route {
    rpc: String,
    method: HttpMethod,
    pattern: String,
    body: RequestBody,
    response_body: ResponseBody,
}

impl Route {
    /// Creates a route from its lowered parts. `pattern` is the raw (already
    /// validated) path-template text.
    pub(crate) fn new(rpc: String, method: HttpMethod, pattern: String, body: RequestBody, response_body: ResponseBody) -> Self {
        Self {
            rpc,
            method,
            pattern,
            body,
            response_body,
        }
    }

    /// The RPC this route transcodes to.
    pub(crate) fn rpc(&self) -> &str {
        &self.rpc
    }

    /// The HTTP method this route matches.
    pub(crate) fn method(&self) -> &HttpMethod {
        &self.method
    }

    /// The raw path-template text this route matches.
    pub(crate) fn pattern(&self) -> &str {
        &self.pattern
    }

    /// The parsed path template this route matches.
    pub(crate) fn template(&self) -> PathTemplate<'_> {
        // The pattern was parsed and validated before this `Route` was created,
        // so re-parsing it with the affix-enabled grammar preserves its AST.
        PathTemplate::parse(&self.pattern, Grammar::default().with_segment_affixes())
            .expect("pattern was validated when the Route was created")
    }

    /// How the request body maps onto the RPC request message.
    pub(crate) fn body(&self) -> &RequestBody {
        &self.body
    }

    /// How the RPC response maps onto the HTTP response body.
    pub(crate) fn response_body(&self) -> &ResponseBody {
        &self.response_body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::HttpRule;

    #[test]
    fn accessors_return_lowered_route_parts() {
        let template = PathTemplate::parse("/v1/books/{book}", Grammar::default()).expect("valid path template");
        let rule = HttpRule::new("UpdateBook", HttpMethod::PATCH, template)
            .request_body(RequestBody::Field("book".to_owned()))
            .response_body(ResponseBody::Field("book".to_owned()));
        let routes = rule.lower();
        let route = &routes[0];

        assert_eq!(route.rpc(), "UpdateBook");
        assert_eq!(route.method().as_str(), "PATCH");
        assert_eq!(route.template().verb(), None);
        assert_eq!(route.body(), &RequestBody::Field("book".to_owned()));
        assert_eq!(route.response_body(), &ResponseBody::Field("book".to_owned()));
    }
}
