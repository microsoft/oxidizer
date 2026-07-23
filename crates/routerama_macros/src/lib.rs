// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Procedural macros for [`routerama`](https://docs.rs/routerama).

use proc_macro::TokenStream;

/// Derives direct query-string decoding for a named-field struct.
///
/// Fields may be scalar values, [`Option`] values, or [`Vec`] values. String
/// fields may be owned or may borrow from the input through one of the struct's
/// lifetime parameters. Other scalar values are decoded through
/// [`core::str::FromStr`]. Each query parameter may appear in any order.
///
/// # Helper attributes
///
/// `FromQuery` recognizes the following `#[query(...)]` container attributes:
///
/// - `rename_all = "..."` renames fields using `camelCase`, `snake_case`,
///   `kebab-case`, or `SCREAMING_SNAKE_CASE`.
/// - `deny_unknown_fields` rejects parameters that are not recognized by this
///   type or one of its flattened fields.
///
/// It recognizes these field attributes:
///
/// - `rename = "name"` changes the parameter's canonical name.
/// - `alias = "name"` accepts an additional name while decoding. The attribute
///   may be repeated.
/// - `default` uses [`Default::default`] when a scalar parameter is absent. It
///   is also accepted on [`Option`] and [`Vec`] fields, whose ordinary missing
///   values are already `None` and an empty vector.
/// - `flatten` delegates unmatched parameters to another `FromQuery` type.
/// - `skip` ignores the field and initializes it with [`Default::default`].
///
/// `Option<T>` fields are absent when their parameter is missing. Scalar and
/// optional fields reject duplicate parameters. [`Vec`] fields preserve values
/// in query-string order.
///
/// A struct may have type parameters, const parameters, and multiple
/// lifetimes. All fields that borrow query data must use the same lifetime;
/// unrelated lifetimes are unrestricted.
///
/// Compatible `serde` attributes - `rename`, `rename_all`, `alias`, `default`,
/// `flatten`, and `skip` - are also honored. If both attribute sets specify a
/// rename, their values must agree.
///
/// # Example
///
/// ```ignore
/// use routerama::query::FromQuery;
///
/// #[derive(Debug, PartialEq, FromQuery)]
/// #[query(rename_all = "camelCase", deny_unknown_fields)]
/// struct Search<'q> {
///     search_term: &'q str,
///     #[query(alias = "limit")]
///     max_results: Option<u32>,
///     #[query(rename = "tag")]
///     tags: Vec<String>,
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let value = Search::from_query("searchTerm=rust&limit=10&tag=fast&tag=safe")?;
/// assert_eq!(value.search_term, "rust");
/// assert_eq!(value.max_results, Some(10));
/// assert_eq!(value.tags, ["fast", "safe"]);
/// # Ok(())
/// # }
/// ```
///
/// [`Default::default`]: core::default::Default::default
/// [`Option`]: core::option::Option
/// [`Vec`]: std::vec::Vec
///
/// The obsolete `repeated` marker is rejected because [`Vec`] alone expresses
/// repeated parameters:
///
/// ```compile_fail
/// #[derive(routerama::query::FromQuery)]
/// struct Unsupported {
///     #[query(repeated)]
///     values: Vec<String>,
/// }
/// ```
///
/// Borrowing through distinct query lifetimes is rejected:
///
/// ```compile_fail
/// #[derive(routerama::query::FromQuery)]
/// struct Unsupported<'a, 'b> {
///     first: &'a str,
///     second: &'b str,
/// }
/// ```
#[proc_macro_derive(FromQuery, attributes(query, serde))]
#[cfg_attr(test, mutants::skip)]
pub fn derive_from_query(input: TokenStream) -> TokenStream {
    routerama_build::macro_impl::derive_from_query(input.into()).into()
}

/// Derives direct query-string encoding for a named-field struct.
///
/// Fields are written in declaration order. Scalar values use
/// [`core::fmt::Display`], [`Option::None`] values are omitted, and [`Vec`]
/// values produce one parameter per element. Flattened fields write their
/// parameters at the flattened field's position.
///
/// # Helper attributes
///
/// `ToQuery` recognizes the following `#[query(...)]` container attribute:
///
/// - `rename_all = "..."` renames fields using `camelCase`, `snake_case`,
///   `kebab-case`, or `SCREAMING_SNAKE_CASE`.
///
/// It recognizes these field attributes:
///
/// - `rename = "name"` changes the emitted parameter name.
/// - `alias = "name"` declares a decoding-only alias. Encoding always uses the
///   canonical field name, so aliases do not affect output.
/// - `default` is accepted for symmetry with `FromQuery` and does not suppress
///   the encoded value.
/// - `flatten` writes the fields of another `ToQuery` value.
/// - `skip` omits the field.
///
/// `deny_unknown_fields` is accepted as a container attribute for types that
/// derive both query traits, but it affects decoding only.
///
/// Compatible `serde` attributes - `rename`, `rename_all`, `alias`, `default`,
/// `flatten`, and `skip` - are also honored. If both attribute sets specify a
/// rename, their values must agree.
///
/// # Example
///
/// ```ignore
/// use routerama::query::ToQuery;
///
/// #[derive(ToQuery)]
/// #[query(rename_all = "camelCase")]
/// struct Search<'q> {
///     search_term: &'q str,
///     #[query(rename = "tag")]
///     tags: Vec<&'q str>,
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let value = Search {
///     search_term: "rust language",
///     tags: vec!["fast", "safe"],
/// };
/// assert_eq!(
///     value.to_query_string()?,
///     "searchTerm=rust+language&tag=fast&tag=safe"
/// );
/// # Ok(())
/// # }
/// ```
///
/// [`Option::None`]: core::option::Option::None
/// [`Vec`]: std::vec::Vec
#[proc_macro_derive(ToQuery, attributes(query, serde))]
#[cfg_attr(test, mutants::skip)]
pub fn derive_to_query(input: TokenStream) -> TokenStream {
    routerama_build::macro_impl::derive_to_query(input.into()).into()
}

/// Generates a [`routerama`](https://docs.rs/routerama) resolver for a route
/// `enum` containing static routes, dynamic routes, or both.
///
/// Apply `#[resolver]` to an `enum`, or use
/// `#[resolver(name = ApiResolver)]` to name its generated resolver type
/// explicitly. Annotate each *static* variant with `#[route(METHOD, "path")]`
/// (or use a string method such as `"M-SEARCH"`); leave each *dynamic* variant
/// unannotated. By default, an enum named `Api` generates `ApiResolver`. A
/// static-only enum gets an infallible `resolver` constructor and no builder
/// type. An enum with dynamic variants gets an `ApiResolverBuilder`; an
/// explicitly named `ApiResolver` similarly gets `ApiResolverBuilder`.
/// Resolution returns the declared route through `Result`, with failures
/// represented by `routerama::ResolveError`. See the
/// [`routerama`](https://docs.rs/routerama) crate documentation for the full
/// model.
#[cfg_attr(test, mutants::skip)]
#[proc_macro_attribute]
pub fn resolver(attr: TokenStream, item: TokenStream) -> TokenStream {
    routerama_build::macro_impl::resolver(attr.into(), item.into()).into()
}

/// Generates static and dynamic route dispatch for an annotated inherent impl.
///
/// Use `#[route(METHOD, "path")]` for compile-time static routes and
/// `#[route(dynamic)]` for handlers whose method and path are registered at
/// startup. Every handler must be async, begin with `&self`, and return the
/// same explicit response type. With bare `#[service]`, handlers also accept
/// one shared request-context reference.
///
/// `#[service(context)]` instead reserves the first parameter after `&self` as
/// context and forwards its exact concrete type. It may be `Context`,
/// `&Context`, or `&mut Context`, but must be identical for every handler.
/// Dispatch continues to accept context as its final argument.
///
/// Static capture arguments are named after the path captures and may borrow
/// the path. Dynamic capture arguments are inferred from every non-context
/// parameter and must be owned.
///
/// Static-only services receive
/// `service.dispatch(method, path, context)`. Services with dynamic handlers
/// receive `Service::router_builder()`, generated `add_<handler>` methods, and
/// a persistent router whose
/// `router.dispatch(&service, method, path, context)` method performs the same
/// exhaustive direct dispatch.
///
/// # Example
///
/// ```ignore
/// #[routerama::service(context)]
/// impl Api {
///     #[route(GET, "/health")]
///     async fn health(&self, context: &mut Context) -> Response {
///         // ...
///     }
///
///     #[route(dynamic)]
///     async fn plugin(&self, context: &mut Context, name: String) -> Response {
///         // ...
///     }
/// }
///
/// let router = Api::router_builder()
///     .add_plugin(routerama::HttpMethod::GET, "/plugins/{name}")
///     .build()?;
/// let response = router.dispatch(&api, method, path, &mut context).await?;
/// ```
///
/// Router construction reports `routerama::ConfigurationError`; dispatch
/// reports `routerama::ResolveError`. The generated implementation uses
/// `routerama`'s existing tries and direct handler calls, without a runtime
/// handler registry or trait objects.
#[cfg_attr(test, mutants::skip)]
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    routerama_build::macro_impl::service(attr.into(), item.into()).into()
}
