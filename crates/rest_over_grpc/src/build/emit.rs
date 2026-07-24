// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static REST router code generation.
//!
//! This module lowers gRPC-specific [`HttpRule`]s and [`Route`]s into
//! [`routerama_build::Route`]s and points the generated code at the
//! `rest_over_grpc` runtime re-export.

use proc_macro2::TokenStream;
use quote::quote;
use routerama_build::Generator;

use super::http_rule::HttpRule;
use super::route::Route;

/// The runtime path the generated router uses to reach the [`routerama`] scan
/// primitives (`scan_segments`, `split_verb`), which `rest_over_grpc`
/// re-exports.
///
/// [`routerama`]: https://crates.io/crates/routerama
fn runtime_path() -> TokenStream {
    quote! { ::rest_over_grpc::codegen_helpers }
}

/// Lowers a [`Route`] to the [`routerama_build::Route`] the router codegen
/// consumes: the route's RPC becomes the route name; body/response-body
/// configuration is irrelevant to routing and dropped here.
fn route_to_routerama(route: &Route) -> routerama_build::Route {
    routerama_build::Route::new(route.rpc().to_owned(), route.method().as_str(), route.template())
}

/// Generates a standalone static REST router for a set of
/// [`HttpRule`](crate::build::HttpRule)s.
///
/// This is an internal primitive (re-exported as `#[doc(hidden)]`): the
/// customer-facing codegen ([`Generator`](crate::build::Generator)) uses
/// `generate_router_with_visibility` directly. It remains `pub` only so the
/// workspace benchmark/coverage tooling (the `rest_over_grpc_tests` crate) can
/// build a bare `Route::resolve` to benchmark against `matchit` and to test
/// routing behavior in isolation.
///
/// The returned [`TokenStream`] defines a `Route` enum whose inherent `resolve`
/// associated function maps an HTTP method + path to the resolved RPC and its
/// captured path variables:
///
/// ```ignore
/// pub enum Route<'p> { /* one variant per RPC, capturing path variables */ }
/// impl<'p> Route<'p> {
///     pub fn resolve<P: AsRef<str> + ?Sized>(method: impl AsRef<str>, path: &'p P)
///         -> Option<Route<'p>>;
/// }
/// ```
///
/// Each rule (with its `additional_bindings`) contributes one or more routes,
/// lowered into a nested `match` over path segments (a compile-time trie);
/// overlapping templates resolve most-specific-first.
///
/// If two routes match an identical set of requests (same HTTP method, custom
/// verb, and path shape) but map to different RPCs, only the first could ever be
/// reached; the generated code contains a [`compile_error!`] naming the conflict
/// so the mistake surfaces at build time.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
/// use rest_over_grpc::build::{HttpRule, generate_router};
/// use routerama::HttpMethod;
///
/// let rule = HttpRule::new(
///     "ListBooks",
///     HttpMethod::GET,
///     PathTemplate::parse("/v1/shelves/{shelf}/books", Grammar::default()).expect("valid"),
/// );
/// let code = generate_router([rule]).to_string();
///
/// assert!(!code.is_empty());
/// assert!(code.contains("ListBooks"));
/// ```
#[must_use]
pub fn generate_router(rules: impl IntoIterator<Item = HttpRule>) -> TokenStream {
    let routes: Vec<Route> = rules.into_iter().flat_map(HttpRule::lower).collect();
    generate_router_with_visibility(&routes, true)
}

/// Emits the router with a public or private `resolve` function.
///
/// The public [`generate_router`] uses `pub`; the generated `Transcoder` embeds
/// the router (merged across all services) inside its `try_transcode` method and
/// requests a private resolver so `resolve` stays an implementation detail.
pub(crate) fn generate_router_with_visibility(routes: &[Route], public: bool) -> TokenStream {
    let mut generator = Generator::new("Route", public);
    generator.runtime_path(runtime_path());
    generator.add_all(routes.iter().map(route_to_routerama));
    generator.generate()
}

#[cfg(test)]
mod tests {
    use http_path_template::{Grammar, PathTemplate};
    use routerama::HttpMethod;

    use super::*;

    fn rules(specs: &[(&str, HttpMethod, &str)]) -> Vec<HttpRule> {
        specs
            .iter()
            .map(|(rpc, method, pattern)| {
                let template = PathTemplate::parse(pattern, Grammar::default()).expect("valid rule");
                HttpRule::new(*rpc, method.clone(), template)
            })
            .collect()
    }

    #[test]
    fn generated_code_is_valid_rust_and_uses_the_rest_runtime() {
        let code = generate_router(rules(&[
            ("GetShelf", HttpMethod::GET, "/v1/shelves/{shelf}"),
            ("ListBooks", HttpMethod::GET, "/v1/shelves/{shelf}/books"),
            ("CreateBook", HttpMethod::POST, "/v1/shelves/{shelf}/books"),
            ("GetName", HttpMethod::GET, "/v1/{name=shelves/*/books/**}"),
        ]));
        let file: syn::File = syn::parse2(code).expect("generated router must be syntactically valid Rust");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("pub fn resolve"));
        assert!(pretty.contains("\"GetShelf\""));
        assert!(pretty.contains("rest_over_grpc :: codegen_helpers") || pretty.contains("rest_over_grpc::codegen_helpers"));
    }

    #[test]
    fn embedded_router_can_be_emitted_private() {
        let routes: Vec<Route> = rules(&[("GetShelf", HttpMethod::GET, "/v1/shelves/{shelf}")])
            .into_iter()
            .flat_map(HttpRule::lower)
            .collect();
        let code = generate_router_with_visibility(&routes, false).to_string();
        assert!(code.contains("fn resolve"));
        assert!(!code.contains("pub fn resolve"));
    }

    #[test]
    fn empty_router_is_valid() {
        let file: syn::File = syn::parse2(generate_router(std::iter::empty())).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("resolve"));
    }

    #[test]
    fn custom_verb_router_is_valid() {
        let code = generate_router(rules(&[
            ("Get", HttpMethod::GET, "/v1/shelves/{shelf}"),
            ("Archive", HttpMethod::POST, "/v1/shelves/{shelf}:archive"),
        ]));
        let file: syn::File = syn::parse2(code).expect("valid");
        let pretty = prettyplease::unparse(&file);
        assert!(pretty.contains("split_verb"));
        assert!(pretty.contains("\"archive\""));
    }

    #[test]
    fn conflicting_routes_emit_a_compile_error() {
        let code = generate_router(rules(&[
            ("GetBook", HttpMethod::GET, "/v1/books/{book}"),
            ("LookupBook", HttpMethod::GET, "/v1/books/{name}"),
        ]));
        let pretty = prettyplease::unparse(&syn::parse2(code).expect("still parses with compile_error!"));
        assert!(pretty.contains("compile_error!"));
        assert!(pretty.contains("GetBook"));
        assert!(pretty.contains("LookupBook"));
    }

    #[test]
    fn distinct_method_or_verb_or_shape_do_not_conflict() {
        let code = generate_router(rules(&[
            ("GetBook", HttpMethod::GET, "/v1/books/{book}"),
            ("UpdateBook", HttpMethod::PATCH, "/v1/books/{book}"),
            ("ArchiveBook", HttpMethod::POST, "/v1/books/{book}:archive"),
            ("PublishBook", HttpMethod::POST, "/v1/books/{book}:publish"),
            ("GetFeatured", HttpMethod::GET, "/v1/books/featured"),
        ]));
        let pretty = prettyplease::unparse(&syn::parse2(code).expect("valid"));
        assert!(!pretty.contains("compile_error!"));
    }
}
