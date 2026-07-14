// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;

use crate::generator_builder::GeneratorBuilder;
use crate::route::Route;

/// Generates a static router from a set of routes.
///
/// Add routes with [`add`](Self::add) / [`add_all`](Self::add_all), then call
/// [`generate`](Self::generate) to lower them into a `Route` enum plus a
/// zero-sized resolver whose `resolve` method matches an HTTP method + path (see
/// the [module-level docs](crate) for the generated shape). Use [`new`](Self::new)
/// for the default options, or [`builder`](Self::builder) to configure the enum
/// name, its visibility, the runtime path, and the resolver name.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use routerama_build::{Generator, HttpMethod, Route};
///
/// let mut generator = Generator::new();
/// generator.add(Route::new(
///     "ListBooks",
///     HttpMethod::Get,
///     PathTemplate::parse("/v1/shelves/{shelf}/books", Grammar::default()).expect("valid"),
/// ));
///
/// let code = generator.generate().to_string();
/// assert!(code.contains("fn resolve"));
/// assert!(code.contains("ListBooks"));
/// ```
#[derive(Debug, Clone)]
pub struct Generator {
    builder: GeneratorBuilder,
    routes: Vec<Route>,
}

impl Default for Generator {
    fn default() -> Self {
        Self::from_builder(GeneratorBuilder::default())
    }
}

impl Generator {
    /// Creates an empty generator with the default code-generation options (a
    /// `pub` `Route` enum with a `pub` `RouteResolver`, using the
    /// `::routerama::codegen_helpers` runtime). Use [`builder`](Self::builder) to
    /// change them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a [`GeneratorBuilder`] to configure the code-generation options
    /// before [`build`](GeneratorBuilder::build) produces the [`Generator`].
    #[must_use]
    // `mutants::skip`: `GeneratorBuilder::default()` and `Default::default()`
    // return the same value here, so swapping them is equivalent.
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
    pub fn add(&mut self, route: Route) -> &mut Self {
        self.routes.push(route);
        self
    }

    /// Adds every route from an iterator.
    ///
    /// Returns `&mut Self` so calls can be chained.
    pub fn add_all(&mut self, routes: impl IntoIterator<Item = Route>) -> &mut Self {
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

    fn rule(name: &str, template: &str) -> Route {
        Route::new(
            name,
            HttpMethod::Get,
            PathTemplate::parse(template, Grammar::default()).expect("valid template"),
        )
    }

    #[test]
    fn add_appends_a_route_to_the_generated_output() {
        // `add` pushes into `self` and returns that same generator.
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
