// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_path_template::PathTemplate;
#[cfg(feature = "build")]
use http_path_template::Segment;
use routerama_build::HttpMethod;

use crate::build::binding::Binding;
use crate::build::request_body::RequestBody;
use crate::build::response_body::ResponseBody;
use crate::build::route::Route;

/// A declarative HTTP binding for a single gRPC method, mirroring
/// [`google.api.HttpRule`](https://github.com/googleapis/googleapis/blob/master/google/api/http.proto).
///
/// This type lets you define the path a specific gRPC method will be exposed at in the generated
/// REST API layer, and how the request and response data should be handled.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::build::{Binding, HttpMethod, HttpRule, RequestBody, ResponseBody};
///
/// let rule = HttpRule::new(
///     "UpdateBook",
///     HttpMethod::Patch,
///     "/v1/books/{book}".parse().expect("valid"),
/// )
/// .request_body(RequestBody::Field("book".to_owned()))
/// .response_body(ResponseBody::Field("book".to_owned()))
/// .add_binding(Binding::new(
///     HttpMethod::Patch,
///     "/v1/shelves/{shelf}/books/{book}".parse().expect("valid"),
/// ));
///
/// assert_eq!(rule.name(), "UpdateBook");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HttpRule {
    rpc: String,
    method: HttpMethod,
    template: PathTemplate,
    body: RequestBody,
    response_body: ResponseBody,
    additional_bindings: Vec<Binding>,
    enum_path_fields: Vec<(Vec<String>, String)>,
}

impl HttpRule {
    /// Creates a rule binding the RPC named `rpc` to `method` + `template`.
    ///
    /// By default, the request body is set to [`RequestBody::None`] and the
    /// response body is set to [`ResponseBody::Whole`].
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::build::{HttpMethod, HttpRule};
    ///
    /// let rule = HttpRule::new(
    ///     "GetBook",
    ///     HttpMethod::Get,
    ///     "/v1/books/{book}".parse().expect("valid"),
    /// );
    ///
    /// assert_eq!(rule.name(), "GetBook");
    /// ```
    #[must_use]
    pub fn new(rpc: impl Into<String>, method: HttpMethod, template: PathTemplate) -> Self {
        Self {
            rpc: rpc.into(),
            method,
            template,
            body: RequestBody::None,
            response_body: ResponseBody::Whole,
            additional_bindings: Vec::new(),
            enum_path_fields: Vec::new(),
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

    /// Adds an additional HTTP binding for the same gRPC RPC. Call repeatedly to
    /// add more than one.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::build::{Binding, HttpMethod, HttpRule};
    ///
    /// let rule = HttpRule::new(
    ///     "GetBook",
    ///     HttpMethod::Get,
    ///     "/v1/books/{book}".parse().expect("valid"),
    /// )
    /// .add_binding(Binding::new(
    ///     HttpMethod::Get,
    ///     "/v1/books/{book}:read".parse().expect("valid"),
    /// ));
    ///
    /// assert_eq!(rule.name(), "GetBook");
    /// ```
    #[must_use]
    pub fn add_binding(mut self, binding: Binding) -> Self {
        self.additional_bindings.push(binding);
        self
    }

    /// Declares that the path variable at `field_path` (the dotted message-field
    /// path it captures, e.g. `"state"` or `"shelf.state"`) targets a proto
    /// `enum` field whose generated Rust type is `enum_type` (a path usable from
    /// the transcoder's scope, e.g. `"crate::pb::State"` or `"super::State"`).
    ///
    /// A proto enum field is a bare `i32` in the generated message, so — unlike
    /// scalar, `bytes`, and `optional` fields, which the runtime resolves from
    /// the field's Rust type alone — the generator needs the concrete enum type
    /// to accept the value's *name* (as well as its number), matching proto3
    /// JSON. Only enum path variables need to be declared; every other field is
    /// handled automatically. When a rule is decoded from a descriptor, these
    /// declarations are populated automatically.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::build::{HttpMethod, HttpRule};
    ///
    /// let rule = HttpRule::new(
    ///     "GetBooksByState",
    ///     HttpMethod::Get,
    ///     "/v1/books/state/{state}".parse().expect("valid"),
    /// )
    /// .path_field_enum("state", "crate::pb::BookState");
    ///
    /// assert_eq!(rule.name(), "GetBooksByState");
    /// ```
    #[must_use]
    pub fn path_field_enum(mut self, field_path: impl AsRef<str>, enum_type: impl Into<String>) -> Self {
        let path = field_path.as_ref().split('.').map(str::to_owned).collect();
        self.enum_path_fields.push((path, enum_type.into()));
        self
    }

    /// The name of the gRPC method this rule binds.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.rpc
    }

    /// The enum path-field declarations (dotted field path, generated enum Rust
    /// type) registered via [`path_field_enum`](Self::path_field_enum).
    pub(crate) fn enum_path_fields(&self) -> &[(Vec<String>, String)] {
        &self.enum_path_fields
    }

    /// The distinct path-variable field paths captured across this rule's primary
    /// template and every additional binding, in first-seen order. Used when
    /// decoding from a descriptor to classify which captures target `enum`
    /// fields (all bindings share the one request message, so a field path is an
    /// enum regardless of which binding captures it).
    #[cfg(feature = "build")]
    pub(crate) fn path_variable_field_paths(&self) -> Vec<Vec<String>> {
        let mut paths: Vec<Vec<String>> = Vec::new();
        let templates = std::iter::once(&self.template).chain(self.additional_bindings.iter().map(Binding::template));
        for template in templates {
            for segment in template.segments() {
                let path = match segment {
                    Segment::Variable(variable) => variable.field_path().to_vec(),
                    Segment::Affix { name, .. } => name.clone(),
                    _ => continue,
                };
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// Lowers this rule (and its `additional_bindings`) into one [`Route`] per
    /// binding.
    ///
    /// Infallible: the path templates are already parsed and a [`Binding`] cannot
    /// nest, so there is nothing left to validate.
    pub(crate) fn lower(self) -> Vec<Route> {
        let Self {
            rpc,
            method,
            template,
            body,
            response_body,
            additional_bindings,
            ..
        } = self;

        let mut routes = Vec::with_capacity(1 + additional_bindings.len());
        routes.push(Route::new(rpc.clone(), method, template, body, response_body));
        for binding in additional_bindings {
            routes.push(binding.into_route(rpc.clone()));
        }
        routes
    }
}

#[cfg(test)]
mod tests {
    use http_path_template::Grammar;

    use super::*;

    fn template(pattern: &str) -> PathTemplate {
        PathTemplate::parse(pattern, Grammar::default()).expect("valid path template")
    }

    #[test]
    fn lowers_single_rule() {
        let rule = HttpRule::new("GetShelf", HttpMethod::Get, template("/v1/shelves/{shelf}"));
        let routes = rule.lower();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].rpc(), "GetShelf");
        assert_eq!(routes[0].method().as_str(), "GET");
    }

    #[cfg(feature = "build")]
    #[test]
    fn path_variable_field_paths_collects_variable_and_affix_captures() {
        // A plain `{shelf}` variable and an affix segment (`img-{id}.png`, from the
        // extended grammar) both contribute their captured field paths.
        let template = PathTemplate::parse("/v1/shelves/{shelf}/img-{id}.png", Grammar::default().with_segment_affixes())
            .expect("valid extended template");
        let rule = HttpRule::new("GetImage", HttpMethod::Get, template);
        let paths = rule.path_variable_field_paths();
        assert_eq!(paths, vec![vec!["shelf".to_owned()], vec!["id".to_owned()]]);
    }

    #[test]
    fn lowers_additional_bindings() {
        let rule = HttpRule::new("GetShelf", HttpMethod::Get, template("/v1/shelves/{shelf}"))
            .add_binding(Binding::new(HttpMethod::Get, template("/v1/shelves/{shelf}/info")));
        let routes = rule.lower();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[1].rpc(), "GetShelf");
    }

    #[test]
    fn builder_setters_are_preserved() {
        let rule = HttpRule::new("UpdateShelf", HttpMethod::Patch, template("/v1/shelves/{shelf}"))
            .request_body(RequestBody::Field("shelf".into()))
            .response_body(ResponseBody::Field("shelf".into()));
        let routes = rule.lower();
        assert_eq!(routes[0].rpc(), "UpdateShelf");
        assert!(matches!(routes[0].method(), HttpMethod::Patch));
        assert!(matches!(routes[0].body(), RequestBody::Field(field) if field == "shelf"));
        assert!(matches!(routes[0].response_body(), ResponseBody::Field(field) if field == "shelf"));
        assert_eq!(routes[0].template().verb(), None);
    }
}
