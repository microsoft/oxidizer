// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;

use crate::generator_builder::GeneratorBuilder;
use crate::route_rule::RouteRule;

/// Generates a static router from a set of routes.
///
/// Add routes with [`add`](Self::add) / [`add_all`](Self::add_all), then call
/// [`generate`](Self::generate) to lower them into a `Route` enum whose inherent
/// `resolve` associated function matches an HTTP method + path (see the
/// [module-level docs](crate) for the generated shape). Use [`new`](Self::new)
/// for the default options, or [`builder`](Self::builder) to configure the enum
/// name, its visibility, the runtime path, and an optional companion router.
///
/// # Examples
///
/// ```
/// use routerama_build::{Generator, HttpMethod, RouteRule};
///
/// let mut generator = Generator::new();
/// generator.add(RouteRule::new(
///     "ListBooks",
///     HttpMethod::Get,
///     "/v1/shelves/{shelf}/books".parse().expect("valid"),
/// ));
///
/// let code = generator.generate().to_string();
/// assert!(code.contains("fn resolve"));
/// assert!(code.contains("ListBooks"));
/// ```
#[derive(Debug, Clone)]
pub struct Generator {
    builder: GeneratorBuilder,
    routes: Vec<RouteRule>,
}

impl Default for Generator {
    fn default() -> Self {
        Self::from_builder(GeneratorBuilder::default())
    }
}

impl Generator {
    /// Creates an empty generator with the default code-generation options (a
    /// `pub` `Route` enum, the `::routerama::codegen_helpers` runtime, no
    /// companion router). Use [`builder`](Self::builder) to change them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a [`GeneratorBuilder`] to configure the code-generation options
    /// before [`build`](GeneratorBuilder::build) produces the [`Generator`].
    #[must_use]
    // `GeneratorBuilder::default()` and `Default::default()` are the same value
    // here (the return type is `GeneratorBuilder`), so that mutant is equivalent.
    #[cfg_attr(test, mutants::skip)]
    pub fn builder() -> GeneratorBuilder {
        GeneratorBuilder::default()
    }

    /// Wraps a configured builder into an empty generator (the routes are added
    /// afterwards). Called by [`GeneratorBuilder::build`].
    pub(crate) fn from_builder(builder: GeneratorBuilder) -> Self {
        Self {
            builder,
            routes: Vec::new(),
        }
    }

    /// Adds one route.
    ///
    /// Returns `&mut Self` so calls can be chained.
    pub fn add(&mut self, route: RouteRule) -> &mut Self {
        self.routes.push(route);
        self
    }

    /// Adds every route from an iterator.
    ///
    /// Returns `&mut Self` so calls can be chained.
    pub fn add_all(&mut self, routes: impl IntoIterator<Item = RouteRule>) -> &mut Self {
        self.routes.extend(routes);
        self
    }

    /// Generates the static router for the added routes, returning the
    /// [`TokenStream`] of the `Route` enum and its `resolve` function.
    #[must_use]
    pub fn generate(&self) -> TokenStream {
        crate::codegen::generate(&self.routes, &self.builder)
    }
}

#[cfg(test)]
mod tests {
    use http_path_template::{Grammar, PathTemplate};

    use super::*;
    use crate::http_method::HttpMethod;

    fn rule(name: &str, template: &str) -> RouteRule {
        RouteRule::new(
            name,
            HttpMethod::Get,
            PathTemplate::parse(template, Grammar::default()).expect("valid template"),
        )
    }

    #[test]
    fn add_appends_a_route_to_the_generated_output() {
        // `add` must push into `self` and return that same generator: if it did
        // not push (or returned a fresh generator), the route would be missing.
        let mut generator = Generator::new();
        generator.add(rule("GetShelf", "/v1/shelves/{shelf}"));
        let code = generator.generate().to_string();
        assert!(code.contains("GetShelf"), "add must include the route: {code}");
    }

    #[test]
    fn add_all_appends_every_route() {
        let mut generator = Generator::new();
        generator.add_all([rule("ListShelves", "/v1/shelves"), rule("GetShelf", "/v1/shelves/{shelf}")]);
        let code = generator.generate().to_string();
        assert!(
            code.contains("ListShelves") && code.contains("GetShelf"),
            "add_all must include every route: {code}"
        );
    }
}
