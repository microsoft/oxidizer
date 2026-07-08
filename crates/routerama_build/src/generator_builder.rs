// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::generator::Generator;

/// Configures a [`Generator`] before building it: the generated enum's name and
/// visibility, the runtime crate path the generated code calls, and an optional
/// companion router type.
///
/// Obtain one from [`Generator::builder`]; set any of the options below and call
/// [`build`](Self::build) to produce the [`Generator`] (the routes themselves are
/// added to the built generator via [`Generator::add`] / [`Generator::add_all`]).
/// The defaults are a `pub` enum named `Route` whose inherent `resolve` calls the
/// [`routerama`](https://crates.io/crates/routerama) runtime by its absolute path
/// (`::routerama::codegen_helpers`), and no companion router.
///
/// # Examples
///
/// ```
/// use quote::quote;
/// use routerama_build::Generator;
///
/// // A private `BookRoute` enum whose runtime calls go through a re-exporting crate.
/// let generator = Generator::builder()
///     .route_type("BookRoute")
///     .visibility(quote! {})
///     .runtime_path(quote! { ::my_crate::router_runtime })
///     .build();
/// # let _ = generator;
/// ```
#[derive(Debug, Clone)]
pub struct GeneratorBuilder {
    route_type: String,
    visibility: TokenStream,
    runtime_path: TokenStream,
    router: Option<(TokenStream, String)>,
}

impl Default for GeneratorBuilder {
    fn default() -> Self {
        Self {
            route_type: "Route".to_owned(),
            visibility: quote! { pub },
            runtime_path: quote! { ::routerama::codegen_helpers },
            router: None,
        }
    }
}

impl GeneratorBuilder {
    /// Sets the name of the generated route enum (whose inherent `resolve`
    /// associated function is the router entry point).
    ///
    /// Defaults to `Route`. The name must be a valid Rust identifier;
    /// configuring it lets several generated routers coexist in one scope.
    #[must_use]
    pub fn route_type(mut self, name: impl Into<String>) -> Self {
        self.route_type = name.into();
        self
    }

    /// Sets the visibility of the generated route enum and its associated
    /// functions.
    ///
    /// Pass an empty stream (`quote! {}`) to make them private — for example when
    /// embedding the router inside another item.
    #[must_use]
    pub fn visibility(mut self, visibility: TokenStream) -> Self {
        self.visibility = visibility;
        self
    }

    /// Sets the absolute path the generated code uses to reach the router
    /// runtime: the `Route` trait it implements and the scan primitives
    /// (`scan_segments`, `split_verb`) its `resolve` calls.
    ///
    /// Defaults to `::routerama::codegen_helpers`. Override it when those items are
    /// re-exported by a different crate (which must expose all of them under the
    /// given path).
    #[must_use]
    pub fn runtime_path(mut self, runtime_path: TokenStream) -> Self {
        self.runtime_path = runtime_path;
        self
    }

    /// Also emit a zero-sized [`Router`] type (named `name`, with the given
    /// `visibility`) and a [`RouteMatch`] impl for the route enum, so the static
    /// router plugs into the runtime routing-trait abstraction (and composes via
    /// `EitherRouter`).
    ///
    /// Off by default (no such type is emitted); calling this opts in. The
    /// `routes!` macro drives it from an optional companion `struct` declaration.
    ///
    /// [`Router`]: https://docs.rs/routerama/latest/routerama/trait.Router.html
    /// [`RouteMatch`]: https://docs.rs/routerama/latest/routerama/trait.RouteMatch.html
    #[must_use]
    pub fn router_type(mut self, visibility: TokenStream, name: impl Into<String>) -> Self {
        self.router = Some((visibility, name.into()));
        self
    }

    /// Builds the configured [`Generator`], with no routes added yet.
    #[must_use]
    pub fn build(self) -> Generator {
        Generator::from_builder(self)
    }

    /// The configured route enum name as an [`Ident`].
    ///
    /// # Panics
    ///
    /// Panics if the configured name is not a valid Rust identifier.
    pub(crate) fn route_type_ident(&self) -> Ident {
        Ident::new(&self.route_type, Span::call_site())
    }

    pub(crate) fn visibility_tokens(&self) -> &TokenStream {
        &self.visibility
    }

    pub(crate) fn runtime_tokens(&self) -> &TokenStream {
        &self.runtime_path
    }

    /// The companion router `(visibility, name)` to emit, if any.
    pub(crate) fn router_spec(&self) -> Option<(&TokenStream, &str)> {
        self.router.as_ref().map(|(visibility, name)| (visibility, name.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_public_routerama() {
        let options = GeneratorBuilder::default();
        assert_eq!(options.visibility_tokens().to_string(), "pub");
        assert_eq!(options.runtime_tokens().to_string(), ":: routerama :: codegen_helpers");
        assert_eq!(options.route_type_ident().to_string(), "Route");
        // No companion router is emitted unless explicitly requested.
        assert!(options.router_spec().is_none());
    }

    #[test]
    fn router_type_sets_the_companion_router() {
        let options = GeneratorBuilder::default().router_type(quote! { pub(crate) }, "BookRouter");
        let (visibility, name) = options.router_spec().expect("router requested");
        assert_eq!(visibility.to_string(), "pub (crate)");
        assert_eq!(name, "BookRouter");
        // ...and leaves the other settings at their defaults.
        assert_eq!(options.visibility_tokens().to_string(), "pub");
        assert_eq!(options.route_type_ident().to_string(), "Route");
    }

    #[test]
    fn setters_override_the_defaults() {
        let options = GeneratorBuilder::default()
            .route_type("BookRoute")
            .visibility(quote! {})
            .runtime_path(quote! { ::my_crate::rt });
        // An empty visibility renders as nothing (a private item).
        assert_eq!(options.visibility_tokens().to_string(), "");
        assert_eq!(options.runtime_tokens().to_string(), ":: my_crate :: rt");
        assert_eq!(options.route_type_ident().to_string(), "BookRoute");
    }

    #[test]
    fn each_setter_is_independent() {
        // Setting only the runtime path leaves visibility at its default, and
        // vice versa — pins that each setter returns the mutated `self`.
        let only_runtime = GeneratorBuilder::default().runtime_path(quote! { ::x });
        assert_eq!(only_runtime.visibility_tokens().to_string(), "pub");
        assert_eq!(only_runtime.runtime_tokens().to_string(), ":: x");
        assert_eq!(only_runtime.route_type_ident().to_string(), "Route");

        let only_vis = GeneratorBuilder::default().visibility(quote! { pub(crate) });
        assert_eq!(only_vis.visibility_tokens().to_string(), "pub (crate)");
        assert_eq!(only_vis.runtime_tokens().to_string(), ":: routerama :: codegen_helpers");

        let only_type = GeneratorBuilder::default().route_type("ApiRoute");
        assert_eq!(only_type.route_type_ident().to_string(), "ApiRoute");
        assert_eq!(only_type.visibility_tokens().to_string(), "pub");
    }
}
