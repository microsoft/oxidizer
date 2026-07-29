// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::route::Route;

/// A single REST RPC registered on a [`ServiceDefinition`](crate::build::ServiceDefinition).
#[derive(Debug, Clone)]
pub(crate) struct ServiceMethod {
    rpc: String,
    request_type: String,
    response_type: String,
    routes: Vec<Route>,
    server_streaming: bool,
    doc: Option<String>,
    enum_fields: Vec<(Vec<String>, String)>,
    response_body_defaults: Vec<Option<String>>,
}

impl ServiceMethod {
    /// Creates a method binding `rpc` (with the given request/response Rust type
    /// paths) to its lowered `routes`. `server_streaming` marks a server-streaming
    /// RPC, whose handler yields a stream of responses. `doc` is the RPC's
    /// documentation (e.g. its proto leading comment), used verbatim on the
    /// generated trait method when present. `enum_fields` maps each enum-typed
    /// path variable (by dotted field path) to its generated Rust enum type, so
    /// the transcoder can accept the value by name as well as by number.
    /// `response_body_defaults` is aligned with `routes`: it holds the proto3-JSON
    /// default literal for each route's `response_body` field (`None` for a whole
    /// message or an unknown default).
    #[expect(
        clippy::too_many_arguments,
        reason = "aggregates one lowered method's codegen inputs; a params struct would only shuffle them"
    )]
    pub(crate) fn new(
        rpc: impl Into<String>,
        request_type: impl Into<String>,
        response_type: impl Into<String>,
        routes: Vec<Route>,
        server_streaming: bool,
        doc: Option<String>,
        enum_fields: Vec<(Vec<String>, String)>,
        response_body_defaults: Vec<Option<String>>,
    ) -> Self {
        Self {
            rpc: rpc.into(),
            request_type: request_type.into(),
            response_type: response_type.into(),
            routes,
            server_streaming,
            doc,
            enum_fields,
            response_body_defaults,
        }
    }

    /// The RPC name this method binds.
    pub(crate) fn rpc(&self) -> &str {
        &self.rpc
    }

    /// The fully-qualified request message type.
    pub(crate) fn request_type(&self) -> &str {
        &self.request_type
    }

    /// The fully-qualified response message type.
    pub(crate) fn response_type(&self) -> &str {
        &self.response_type
    }

    /// The lowered HTTP routes for this method.
    pub(crate) fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// Whether this RPC is server-streaming (its handler yields a stream).
    pub(crate) fn server_streaming(&self) -> bool {
        self.server_streaming
    }

    /// The RPC's documentation (its proto leading comment), if any.
    pub(crate) fn doc(&self) -> Option<&str> {
        self.doc.as_deref()
    }

    /// The enum-typed path variables (dotted field path, generated Rust enum
    /// type) this method binds.
    pub(crate) fn enum_fields(&self) -> &[(Vec<String>, String)] {
        &self.enum_fields
    }

    /// The proto3-JSON default literals for each route's `response_body` field,
    /// aligned with [`routes`](Self::routes).
    pub(crate) fn response_body_defaults(&self) -> &[Option<String>] {
        &self.response_body_defaults
    }
}

#[cfg(test)]
mod tests {
    use http_path_template::{Grammar, PathTemplate};
    use routerama::HttpMethod;

    use super::*;
    use crate::build::HttpRule;

    #[test]
    fn accessors_return_method_parts() {
        let template = PathTemplate::parse("/v1/shelves/{shelf}", Grammar::default()).expect("valid path template");
        let routes = HttpRule::new("GetShelf", HttpMethod::GET, template).lower();
        let method = ServiceMethod::new("GetShelf", "crate::Req", "crate::Resp", routes, false, None, Vec::new(), Vec::new());

        assert_eq!(method.rpc(), "GetShelf");
        assert_eq!(method.request_type(), "crate::Req");
        assert_eq!(method.response_type(), "crate::Resp");
        assert_eq!(method.routes().len(), 1);
        assert!(!method.server_streaming());
        assert_eq!(method.doc(), None);
        assert!(method.enum_fields().is_empty());
    }

    #[test]
    fn response_body_defaults_returns_stored_defaults() {
        let template = PathTemplate::parse("/v1/shelves/{shelf}", Grammar::default()).expect("valid path template");
        let routes = HttpRule::new("GetShelf", HttpMethod::GET, template).lower();
        let method = ServiceMethod::new(
            "GetShelf",
            "crate::Req",
            "crate::Resp",
            routes,
            false,
            None,
            Vec::new(),
            vec![Some("\"\"".to_owned())],
        );

        assert_eq!(method.response_body_defaults(), [Some("\"\"".to_owned())]);
    }
}
