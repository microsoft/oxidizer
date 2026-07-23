// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/favicon.ico")]

//! Blazingly fast HTTP route resolution and query string processing.
//!
//! `routerama` resolves an HTTP method and path to a typed route, extracting
//! captured path variables. It is designed as a routing layer for HTTP
//! frameworks. It also provides query string processing, efficiently converting
//! an incoming query string into a useful Rust data structure.
//!
//! # Route resolution
//!
//! You describe every route your application serves as an enum annotated with
//! [`#[resolver]`](macro@resolver). Variants in the enum are either:
//!
//! - **static** — annotated with `#[route(METHOD, "path")]` and compiled into
//!   the resolver.
//!
//! - **dynamic** — unannotated, registered at run time through the generated
//!   builder, and restricted to owned fields.
//!
//! ```
//! use routerama::{ResolveError, resolver};
//!
//! #[resolver]
//! enum BookRoute<'p> {
//!     #[route(GET, "/books/{book}")]
//!     GetBook { book: &'p str },
//!
//!     #[route(GET, "/health")]
//!     Health,
//! }
//!
//! let resolver = BookRoute::resolver();
//!
//! match resolver.resolve("GET", "/books/rust") {
//!     Ok(BookRoute::GetBook { book }) => get_book(book),
//!     Ok(BookRoute::Health) => health(),
//!     Err(ResolveError::NotFound(path)) => not_found(path),
//!     Err(error) => bad_request(error),
//! }
//!
//! fn get_book(_book: &str) {}
//! fn health() {}
//! fn bad_request(_error: ResolveError<'_>) {}
//! fn not_found(_path: &str) {}
//! ```
//!
//! ## What gets generated
//!
//! For an enum named `BookRoute`, Routerama generates a `BookRouteResolver`
//! implementing [`Resolver`]. Static-only enums get an infallible `resolver`
//! constructor. Enums containing dynamic routes instead get a
//! `BookRouteResolverBuilder` and an `add_<variant>(method, path)` method for
//! each dynamic variant. `#[resolver(name = ApiResolver)]` explicitly names
//! the resolver `ApiResolver` and its builder `ApiResolverBuilder`.
//! [`Resolver::resolve`] returns `Result<Route, ResolveError>`.
//!
//! Each capturing variant declares one field per `{capture}`; the field type
//! controls conversion. Borrowing field types — `&'p str` (no allocation) and
//! `Cow<'p, str>` (percent-decoded) — are allowed only on **static** variants;
//! owned types — `String` (percent-decoded) and any `T: FromStr`
//! (decode-then-parse) — work on either kind. A dynamic route must use capture
//! names that are valid Rust identifiers matching its field names.
//!
//! # Query string processing
//!
//! You describe a query schema as a named-field struct deriving
//! [`FromQuery`](https://docs.rs/routerama/latest/routerama/query/derive.FromQuery.html),
//! [`ToQuery`](https://docs.rs/routerama/latest/routerama/query/derive.ToQuery.html),
//! or both. Its fields may be:
//!
//! - **scalar** — required while decoding, with string forms handled directly
//!   and other values parsed through `FromStr` and produced through `Display`;
//!
//! - **optional** — represented by `Option<T>` and omitted when absent;
//!
//! - **repeated** — represented by `Vec<T>`, with one value per occurrence of
//!   the parameter; or
//!
//! - **flattened** — another query type whose fields share the same query
//!   string.
//!
//! ```
//! # #[cfg(feature = "query")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::borrow::Cow;
//!
//! use routerama::query::{FromQuery, ToQuery};
//!
//! #[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
//! #[query(rename_all = "camelCase", deny_unknown_fields)]
//! struct SearchQuery<'q> {
//!     search_term: Cow<'q, str>,
//!     page: Option<u32>,
//!
//!     #[query(rename = "tag")]
//!     tags: Vec<Cow<'q, str>>,
//! }
//!
//! let query = SearchQuery::from_query("searchTerm=rust+language&page=2&tag=fast&tag=safe")?;
//! assert_eq!(query.search_term, "rust language");
//! assert_eq!(query.page, Some(2));
//! assert_eq!(query.tags, ["fast", "safe"]);
//!
//! assert_eq!(
//!     query.to_query_string()?,
//!     "searchTerm=rust+language&page=2&tag=fast&tag=safe"
//! );
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "query"))]
//! # fn main() {}
//! ```
//!
//! ## What gets generated
//!
//! `FromQuery` generates direct field dispatch and a function-local decoder
//! state. Parsing processes each `application/x-www-form-urlencoded` pair once,
//! in any order, while detecting missing or duplicate scalar values and
//! enforcing `query::QueryLimits`. Borrowed `&str` fields allocate nothing but
//! reject values that require decoding; `Cow<'q, str>` borrows unchanged input
//! and owns only decoded values.
//!
//! `ToQuery` generates direct field encoding in declaration order, producing a
//! canonical parameter name for every field. Neither derive uses a run-time
//! field map, Serde, dynamic dispatch, or a generated top-level state type.
//!
//! The `#[query(...)]` helper attributes rename fields, add decoding aliases,
//! provide defaults, flatten nested query types, skip fields, and reject
//! unknown parameters. The
//! [`FromQuery`](https://docs.rs/routerama/latest/routerama/query/derive.FromQuery.html)
//! and
//! [`ToQuery`](https://docs.rs/routerama/latest/routerama/query/derive.ToQuery.html)
//! derive pages document the complete attribute contract.
//!
//! # Securing route resolution and query processing
//!
//! Routerama applies defensive parsing checks and configurable query limits,
//! but applications must still enforce limits and validate values for their
//! specific environment. Take these precautions:
//!
//! - Enforce an HTTP request-target size limit before resolution. Route
//!   resolution does not impose a total path-length limit.
//!
//! - Pass a consistent URI path with the query removed. Routerama deliberately
//!   does not normalize dot segments, repeated slashes, or percent-encoded
//!   equivalents.
//!
//! - Validate captures for their eventual use. A percent-decoded capture can
//!   contain `/` and is not inherently safe as a filesystem component.
//!
//! - Discard streaming query output if encoding returns an error, because the
//!   destination struct may already contain a partial query string.
//!
//! # Cargo features
//!
//! - **`query`** — enabled by default. Exposes the `query` module and the
//!   `FromQuery` and `ToQuery` derive macros. Disable default features to build
//!   with routing support only.
//!
//! # `no_std`
//!
//! This crate is `#![no_std]` (it requires `alloc`). The generated resolvers run
//! on bare-metal targets; the `#[resolver]` macro expansion runs on the host and
//! requires `std`.
//!
//! [`FromStr`]: core::str::FromStr

extern crate alloc;
extern crate self as routerama;
#[cfg(test)]
extern crate std;

mod route_match;

// Referenced by generated code in downstream crates.
#[doc(hidden)]
pub mod codegen_helpers;

mod http_method;
pub use http_method::HttpMethod;
/// Generates a resolver for a route `enum` mixing static and dynamic routes.
///
/// Apply `#[resolver]` to a route `enum`, optionally specifying the generated
/// resolver type as `#[resolver(name = ApiResolver)]`. Annotate each *static* variant with
/// `#[route(METHOD, "path")]`, or use a string for a method that is not a Rust
/// identifier, such as `#[route("M-SEARCH", "/devices")]`. Its path is compiled
/// into a trie and fields may borrow the path as `&'p str`. Leave each *dynamic*
/// variant unannotated (its path is registered at run time via the generated
/// builder; fields must be owned). An enum named `BookRoute` generates a
/// `BookRouteResolver`. Static-only enums get an infallible enum-associated
/// `resolver` constructor and no builder type. Enums with dynamic variants get
/// an enum-associated `builder` and a `BookRouteResolverBuilder` with one
/// `add_<variant>` method per dynamic variant. Its fallible `build` returns the
/// generated resolver, whose `resolve(method, path)` tries static routes before
/// dynamic ones and returns [`ResolveError`] for a path containing `?` or `#`,
/// on a miss, or on capture failure. See
/// the [crate-level docs](crate) for the full tour.
///
/// # Example
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// #[routerama::resolver]
/// enum BookRoute<'p> {
///     #[route(GET, "/books/{book}")]
///     GetBook {
///         book: &'p str,
///     }, // static, zero-copy borrow
///
///     Plugin {
///         name: String,
///     }, // dynamic, registered at run time
/// }
/// # let _ = BookRoute::builder()
/// #     .add_plugin(routerama::HttpMethod::GET, "/plugins/{name}")
/// #     .build()?;
/// # Ok(())
/// # }
/// ```
pub use routerama_macros::resolver;

mod affix_edge;
mod build_error_entry;
mod captures;
mod configuration_error;
mod decode;
mod dyn_builder;
mod dyn_route;
#[path = "extract_helpers.rs"]
mod extract_helpers;
mod literal_edge;
#[cfg(feature = "query")]
pub mod query;
mod raw_match;
mod raw_resolver;
mod resolve_error;
mod resolver;
mod rt_node;
mod walk;

pub use configuration_error::ConfigurationError;
pub use resolve_error::ResolveError;
pub use resolver::Resolver;

/// Runtime support for generated resolvers.
#[doc(hidden)]
pub mod __rt {
    pub use routerama_build::Route;

    pub use crate::captures::Captures;
    pub use crate::dyn_builder::DynBuilder;
    pub use crate::dyn_route::DynRoute;
    pub use crate::extract_helpers::{coerce_cow, coerce_owned, coerce_parse, owned, parse};
    pub use crate::raw_match::RawMatch;
    pub use crate::raw_resolver::RawResolver;
    pub use crate::route_match::RouteMatch;
}
