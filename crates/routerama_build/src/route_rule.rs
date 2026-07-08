// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_path_template::PathTemplate;

use crate::http_method::HttpMethod;

/// A single route rule to generate a router from.
///
/// The `name` identifies the route. A [`Generator`](crate::Generator)
/// collects the distinct names into a `Route` enum — one variant per name,
/// carrying that route's captured path variables as named fields — that the
/// generated `resolve` returns, so dispatching on a match is an `O(1)`
/// jump-table `match` and captured variables are read straight from the
/// variant's fields. Because each name becomes an enum variant it **must be a
/// valid Rust identifier**; by convention it is `UpperCamelCase` (e.g.
/// `GetBook`), matching Rust's enum-variant style — not the `GET_BOOK`
/// screaming-snake style used for constants. A name that is not a valid
/// identifier is reported as a [`compile_error!`] in the generated code.
///
/// A name may appear on more than one route (e.g. the same handler bound to
/// several method/path pairs); each such [`RouteRule`] contributes one route to the
/// generated router, and they share the single enum variant for that name. Such
/// routes must therefore capture the **same** path variables; binding one name
/// to routes with different captures is a [`compile_error!`].
///
/// # Examples
///
/// ```
/// use routerama_build::{HttpMethod, RouteRule};
///
/// let rule = RouteRule::new(
///     "GetBook",
///     HttpMethod::Get,
///     "/v1/books/{book}".parse().expect("valid path template"),
/// );
/// assert_eq!(rule.name(), "GetBook");
/// assert_eq!(rule.method().as_str(), "GET");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteRule {
    name: String,
    method: HttpMethod,
    template: PathTemplate,
}

impl RouteRule {
    /// Creates a route binding `name` to `method` + `template`.
    ///
    /// `name` must be a valid Rust identifier (it becomes a variant of the
    /// generated `Route` enum); `UpperCamelCase` such as `GetBook` is
    /// conventional. See the [type documentation](Self) for details.
    #[must_use]
    pub fn new(name: impl Into<String>, method: HttpMethod, template: PathTemplate) -> Self {
        Self {
            name: name.into(),
            method,
            template,
        }
    }

    /// The name identifying this route (the `Route` enum variant a match of it
    /// resolves to).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
}
