// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::String;
use alloc::vec::Vec;

use proc_macro2::TokenStream;
use quote::quote;

use crate::route::Route;

/// Generates a static resolver from a set of routes.
///
/// Create one with [`new`](Self::new) (naming the generated enum and choosing
/// its visibility), add routes with [`add`](Self::add) / [`add_all`](Self::add_all),
/// then call [`generate`](Self::generate) to lower them into a route `enum` plus
/// its inherent `resolve` and `RouteMatch` impl (see the [module-level
/// docs](crate)).
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use routerama_build::{Generator, Route};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut generator = Generator::new("Route", true);
/// generator.add(Route::new(
///     "ListBooks",
///     "GET",
///     PathTemplate::parse("/v1/shelves/{shelf}/books", Grammar::default())?,
/// ));
///
/// let code = generator.generate().to_string();
/// assert!(code.contains("fn resolve"));
/// assert!(code.contains("ListBooks"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Generator {
    route_type: String,
    public: bool,
    full_api: bool,
    routes: Vec<Route>,
    runtime: TokenStream,
}

impl Generator {
    /// Creates an empty generator that emits an enum named `route_type` (which
    /// must be a valid Rust identifier), `pub` when `public` and private
    /// otherwise.
    #[must_use]
    pub fn new(route_type: impl Into<String>, public: bool) -> Self {
        Self {
            route_type: route_type.into(),
            public,
            full_api: true,
            routes: Vec::new(),
            runtime: quote! { ::routerama::codegen_helpers },
        }
    }

    /// Overrides the path to the generated-code runtime module.
    ///
    /// The resolver macro uses this when the `routerama` dependency was renamed
    /// in `Cargo.toml`.
    pub fn runtime_path(&mut self, runtime: TokenStream) -> &mut Self {
        self.runtime = runtime;
        self
    }

    /// Selects whether to emit the general-purpose derives and `RouteMatch`
    /// implementation in addition to the resolver.
    ///
    /// The resolver macro disables these for its private intermediate enum,
    /// which is immediately converted into the user-facing typed enum.
    pub fn full_api(&mut self, enabled: bool) -> &mut Self {
        self.full_api = enabled;
        self
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

    /// Generates the static resolver, returning the
    /// [`TokenStream`] of the route `enum` and its `resolve` function.
    #[must_use]
    pub fn generate(&self) -> TokenStream {
        crate::codegen::generate(&self.routes, &self.route_type, self.public, self.full_api, &self.runtime)
    }
}
