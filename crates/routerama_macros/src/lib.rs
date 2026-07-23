// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Procedural macros for [`routerama`](https://docs.rs/routerama).

#[cfg(any(feature = "query", feature = "resolve", feature = "route"))]
use proc_macro::TokenStream;

/// Derives direct query-string decoding for a named-field struct.
///
/// Fields may be scalar values, [`Option`] values, or [`Vec`] values. Strings
/// may own or borrow input. Other scalar values use [`core::str::FromStr`].
///
/// Container attributes:
///
/// - `rename_all = "..."` supports `camelCase`, `snake_case`, `kebab-case`,
///   and `SCREAMING_SNAKE_CASE`.
/// - `deny_unknown_fields` rejects unrecognized parameters.
///
/// Field attributes:
///
/// - `rename = "name"` changes the canonical parameter name.
/// - repeatable `alias = "name"` adds decoding aliases.
/// - `default` supplies [`Default::default`] for a missing scalar.
/// - `flatten` delegates unmatched parameters to another query type.
/// - `skip` ignores the field and supplies its default.
///
/// Scalar and optional fields reject duplicates; [`Vec`] preserves repeated
/// values in input order. Compatible `serde` attributes are also accepted.
///
/// [`Default::default`]: core::default::Default::default
/// [`Option`]: core::option::Option
/// [`Vec`]: std::vec::Vec
#[proc_macro_derive(FromQuery, attributes(query, serde))]
#[cfg(feature = "query")]
#[cfg_attr(test, mutants::skip)]
pub fn derive_from_query(input: TokenStream) -> TokenStream {
    routerama_build::macro_impl::derive_from_query(input.into()).into()
}

/// Derives direct query-string encoding for a named-field struct.
///
/// Fields are written in declaration order. Scalars use
/// [`core::fmt::Display`], [`Option::None`] is omitted, and [`Vec`] emits one
/// parameter per element. The derive supports `rename_all`, `rename`, `alias`,
/// `default`, `flatten`, and `skip`, plus compatible `serde` attributes.
/// Aliases and `deny_unknown_fields` affect decoding only.
///
/// [`Option::None`]: core::option::Option::None
/// [`Vec`]: std::vec::Vec
#[proc_macro_derive(ToQuery, attributes(query, serde))]
#[cfg(feature = "query")]
#[cfg_attr(test, mutants::skip)]
pub fn derive_to_query(input: TokenStream) -> TokenStream {
    routerama_build::macro_impl::derive_to_query(input.into()).into()
}

/// Generates a typed resolver for a route enum.
///
/// Static variants use `#[route(METHOD, "path")]`; dynamic variants use
/// `#[route(dynamic)]` and are registered through the generated builder.
/// Static-only enums provide an infallible `resolver()` constructor. Use
/// `#[resolver(name = ApiResolver)]` to select the generated resolver name.
///
/// Resolver routes accept only method and path. Request predicates and
/// priorities belong to [`router`](macro@router).
#[cfg_attr(test, mutants::skip)]
#[cfg(feature = "resolve")]
#[proc_macro_attribute]
pub fn resolver(attr: TokenStream, item: TokenStream) -> TokenStream {
    routerama_build::macro_impl::resolver(attr.into(), item.into()).into()
}

/// Generates static and runtime-configured HTTP handler routing.
///
/// Apply the attribute to an inherent impl whose route handlers are async
/// methods beginning with `&self` and returning
/// `routerama::response::IntoResponse`.
///
/// Static handlers use `#[route(METHOD, "path")]`; dynamic handlers use
/// `#[route(dynamic)]` and receive method/path registrations through a
/// generated builder. Routes may declare `host`, `consumes`, and `produces`
/// predicates. Static response headers use
/// `headers(insert("name", "value"), append("name", "value"))`; names and
/// values are validated while the router is generated, operations run in
/// source order, negotiated `Content-Type` is applied afterwards, and
/// `#[after]` interceptors observe the result. Exact static overlaps require
/// compatible captures and distinct explicit `priority` values.
///
/// Handler parameters are classified independently of position:
///
/// - static captures match template variable names;
/// - dynamic captures use `#[capture]` and must be owned;
/// - one `#[body]` parameter implements `FromRequestBody`;
/// - all remaining parameters implement `FromRequestParts`.
///
/// Bare `#[router]` keeps shared state generic. `#[router(state = T)]` fixes
/// the state contract and validates state extraction at the annotated impl.
/// `#[router(state = T, erased_mounts)]` also generates the mount integration
/// entry when the `mount` feature is enabled. Add `tower` to generate an
/// allocation-free `tower_service` constructor whose opaque response type
/// retains the service's exact concrete body; this requires Routerama's
/// additive `tower` Cargo feature and an all-`Send` service contract.
/// Add `heterogeneous_data` when handlers intentionally return bodies with
/// different `http_body::Body::Data` types, such as `bytes::Bytes` and
/// `bytesbuf::BytesView`. The generated service-specific body maps each frame
/// into a nested `EitherData` sum without copying payload bytes. The default
/// contract remains `Data = bytes::Bytes` so homogeneous routers pay no
/// discriminant or branch cost.
///
/// Optional policy methods are:
///
/// - `#[fallback]` for `RouteFailure`;
/// - `#[catch(Rejection)]`, with `from = Extractor` for custom extractors;
/// - router-wide or per-handler `#[before]` and `#[after]` interceptors;
/// - `#[transform(limit = N, ...)]` for bounded buffering;
/// - `#[transform(stream, ...)]` for a generic body wrapper.
///
/// Generated dispatch preserves concrete futures and response bodies. It
/// forwards body frames, trailers, errors, and auto traits without mandatory
/// boxing or `Send` bounds. Uncaught parts-extractor rejections on a
/// generic-state router use `SendBoxBody` because their associated body type
/// cannot be named in the generated signature.
///
/// Static services dispatch through `service.route(request, state)`. Services
/// with dynamic handlers also receive `router_builder()`, registration
/// methods, and `router.route(&service, request, state)`. A static
/// `#[router(..., tower)]` service constructs its adapter with
/// `Service::tower_service(service, state)`; configured services use
/// `Router::tower_service(router, service, state)`.
#[cfg_attr(test, mutants::skip)]
#[cfg(feature = "route")]
#[proc_macro_attribute]
pub fn router(attr: TokenStream, item: TokenStream) -> TokenStream {
    routerama_build::macro_impl::router(attr.into(), item.into()).into()
}
