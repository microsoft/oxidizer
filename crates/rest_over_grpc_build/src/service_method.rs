// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ServiceMethod`] type.

use crate::message_types::MessageTypes;
use crate::route::Route;

/// A single RPC of a [`Service`]: its name, request/response Rust types, and the
/// HTTP route(s) it is bound to.
///
/// # Examples
///
/// ```
/// use rest_over_grpc_build::{HttpMethod, HttpRule, ServiceMethod};
///
/// let routes = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")
///     .lower()
///     .expect("valid path template");
/// let method = ServiceMethod::new(
///     "GetShelf",
///     ("crate::pb::GetShelfRequest", "crate::pb::Shelf"),
///     routes,
/// );
///
/// let service = rest_over_grpc_build::Service::new("LibraryService", vec![method]);
/// assert!(service.generate().to_string().contains("get_shelf"));
/// ```
#[derive(Debug, Clone)]
pub struct ServiceMethod {
    rpc: String,
    types: MessageTypes,
    routes: Vec<Route>,
}

impl ServiceMethod {
    /// Creates a method binding `rpc` (with the given request/response Rust
    /// [`MessageTypes`]) to its lowered `routes`.
    ///
    /// `routes` are typically the result of [`HttpRule::lower`](crate::HttpRule::lower).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc_build::{HttpMethod, HttpRule, Service, ServiceMethod};
    ///
    /// let routes = HttpRule::new("ListBooks", HttpMethod::Get, "/v1/shelves/{shelf}/books")
    ///     .lower()
    ///     .expect("valid path template");
    /// let method = ServiceMethod::new(
    ///     "ListBooks",
    ///     (
    ///         "crate::pb::ListBooksRequest",
    ///         "crate::pb::ListBooksResponse",
    ///     ),
    ///     routes,
    /// );
    ///
    /// let tokens = Service::new("LibraryService", vec![method]).generate();
    /// assert!(tokens.to_string().contains("list_books"));
    /// ```
    #[must_use]
    pub fn new(rpc: impl Into<String>, types: impl Into<MessageTypes>, routes: Vec<Route>) -> Self {
        Self {
            rpc: rpc.into(),
            types: types.into(),
            routes,
        }
    }

    /// The RPC name this method binds.
    pub(crate) fn rpc(&self) -> &str {
        &self.rpc
    }

    /// The request/response message types this method uses.
    pub(crate) fn types(&self) -> &MessageTypes {
        &self.types
    }

    /// The lowered HTTP routes for this method.
    pub(crate) fn routes(&self) -> &[Route] {
        &self.routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_method::HttpMethod;
    use crate::http_rule::HttpRule;

    #[test]
    fn accessors_return_method_parts() {
        let routes = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")
            .lower()
            .expect("valid path template");
        let method = ServiceMethod::new("GetShelf", ("crate::Req", "crate::Resp"), routes);

        assert_eq!(method.rpc(), "GetShelf");
        assert_eq!(method.types().request(), "crate::Req");
        assert_eq!(method.types().response(), "crate::Resp");
        assert_eq!(method.routes().len(), 1);
    }
}
