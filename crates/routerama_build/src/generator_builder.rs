// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::generator::Generator;

/// Configures a [`Generator`] before building it.
///
/// Obtain one from [`Generator::builder`]; set any of the options below and call
/// [`build`](Self::build) to produce the [`Generator`] (the routes themselves are
/// added to the built generator via [`Generator::add`] / [`Generator::add_all`]).
/// The defaults are a `pub` enum named `Route` and a companion zero-sized
/// resolver named `RouteResolver` whose `resolve` method calls the
/// [`routerama`](https://crates.io/crates/routerama) runtime by its absolute path
/// (`::routerama::codegen_helpers`).
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
    resolver: Option<(TokenStream, String)>,
    emit_enum: bool,
}

impl Default for GeneratorBuilder {
    fn default() -> Self {
        Self {
            route_type: "Route".to_owned(),
            visibility: quote! { pub },
            runtime_path: quote! { ::routerama::codegen_helpers },
            resolver: None,
            emit_enum: true,
        }
    }
}

impl GeneratorBuilder {
    /// Sets the name of the generated route enum.
    ///
    /// Defaults to `Route`. The name must be a valid Rust identifier;
    /// configuring it lets several generated resolvers coexist in one scope.
    #[must_use]
    pub fn route_type(mut self, name: impl Into<String>) -> Self {
        self.route_type = name.into();
        self
    }

    /// Sets the visibility of the generated route enum (and, unless
    /// [`resolver_type`](Self::resolver_type) overrides it, the default resolver).
    ///
    /// Pass an empty stream (`quote! {}`) to make them private — for example when
    /// embedding the resolver inside another item.
    #[must_use]
    pub fn visibility(mut self, visibility: TokenStream) -> Self {
        self.visibility = visibility;
        self
    }

    /// Sets the absolute path the generated code uses to reach the resolver
    /// runtime: the [`Resolver`] / [`RouteMatch`] traits it implements and the
    /// scan primitives (`scan_segments`, `split_verb`) its `resolve` calls.
    ///
    /// Defaults to `::routerama::codegen_helpers`. Override it when those items are
    /// re-exported by a different crate (which must expose all of them under the
    /// given path).
    ///
    /// [`Resolver`]: https://docs.rs/routerama/latest/routerama/trait.Resolver.html
    /// [`RouteMatch`]: https://docs.rs/routerama/latest/routerama/trait.RouteMatch.html
    #[must_use]
    pub fn runtime_path(mut self, runtime_path: TokenStream) -> Self {
        self.runtime_path = runtime_path;
        self
    }

    /// Sets the name and visibility of the generated zero-sized [`Resolver`].
    ///
    /// A resolver is always emitted; by default it is named `{route_type}Resolver`
    /// with the enum's visibility. Call this to choose a different name or
    /// visibility. The `#[resolver(name = ...)]` attribute drives it from the
    /// given name.
    ///
    /// [`Resolver`]: https://docs.rs/routerama/latest/routerama/trait.Resolver.html
    /// [`RouteMatch`]: https://docs.rs/routerama/latest/routerama/trait.RouteMatch.html
    #[must_use]
    pub fn resolver_type(mut self, visibility: TokenStream, name: impl Into<String>) -> Self {
        self.resolver = Some((visibility, name.into()));
        self
    }

    /// Emits only the impls (not the `enum` definition), for the `#[resolver]`
    /// path where the caller writes the enum themselves. The configured
    /// `route_type` must name that enum, its variants must match the route names,
    /// and each capturing variant's fields must match the path captures.
    #[must_use]
    pub fn impls_only(mut self) -> Self {
        self.emit_enum = false;
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

    /// The resolver `(visibility, name)` override to emit, if any. When `None`,
    /// the resolver defaults to `{route_type}Resolver` with the enum's visibility.
    pub(crate) fn resolver_spec(&self) -> Option<(&TokenStream, &str)> {
        self.resolver.as_ref().map(|(visibility, name)| (visibility, name.as_str()))
    }

    /// Whether to emit the `enum` definition (false for the `#[resolver]`
    /// path, where the caller wrote it).
    pub(crate) fn emits_enum(&self) -> bool {
        self.emit_enum
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
        // No resolver override is set unless requested; the resolver defaults.
        assert!(options.resolver_spec().is_none());
        // By default the enum definition is emitted (the `Generator`/build.rs path).
        assert!(options.emits_enum());
    }

    #[test]
    fn impls_only_disables_enum_emission() {
        // The `#[resolver]` path writes the enum itself, so `impls_only` suppresses
        // the generated enum definition; `emits_enum` reports that.
        let options = GeneratorBuilder::default().impls_only();
        assert!(!options.emits_enum());
        // Other settings are untouched.
        assert_eq!(options.route_type_ident().to_string(), "Route");
        assert_eq!(options.visibility_tokens().to_string(), "pub");
    }

    #[test]
    fn resolver_type_sets_the_resolver_override() {
        let options = GeneratorBuilder::default().resolver_type(quote! { pub(crate) }, "BookResolver");
        let (visibility, name) = options.resolver_spec().expect("resolver override set");
        assert_eq!(visibility.to_string(), "pub (crate)");
        assert_eq!(name, "BookResolver");
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
        // vice versa.
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
