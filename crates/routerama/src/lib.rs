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
//! ## Generated service dispatch
//!
//! [`#[service]`](macro@service) can make async handler methods the source of
//! truth and generate the route enum plus exhaustive dispatch:
//!
//! ```
//! struct BooksApi;
//! struct RequestContext;
//!
//! #[routerama::service]
//! impl BooksApi {
//!     #[route(GET, "/books")]
//!     async fn list_books(&self, request: &RequestContext) -> &'static str {
//!         let _ = request;
//!         "books"
//!     }
//!
//!     #[route(GET, "/books/{id}")]
//!     async fn get_book(&self, id: u32, request: &RequestContext) -> &'static str {
//!         let _ = (id, request);
//!         "book"
//!     }
//! }
//!
//! # async fn example() -> Result<(), routerama::ResolveError<'static>> {
//! let api = BooksApi;
//! let context = RequestContext;
//! assert_eq!(api.dispatch("GET", "/books/42", &context).await?, "book");
//! # Ok(())
//! # }
//! ```
//!
//! Use `#[service(context)]` when the first handler parameter after `&self`
//! should be forwarded as an owned, shared, or mutable context. Services with
//! `#[route(dynamic)]` handlers additionally generate a persistent router and
//! builder. See [`service`] for the complete contract and a mixed-route
//! example.
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
/// Generates route resolution and asynchronous dispatch from service methods.
///
/// `#[service]` makes annotated handler methods the source of truth. It
/// generates a private route enum, applies [`resolver`] to that enum, and
/// generates an exhaustive match that invokes each handler directly. There is
/// no handler registry, trait object, function-pointer table, or per-request
/// initialization.
///
/// # Static routes
///
/// Annotate a static handler with `#[route(METHOD, "path")]`. Capture
/// parameters must have the same names as the path captures. Static captures
/// support the same types as [`resolver`], including zero-copy `&str`,
/// decode-on-demand `Cow<'_, str>`, owned `String`, and parsed `FromStr`
/// values.
///
/// A service containing only static routes receives:
///
/// ```text
/// service.dispatch(method, path, context).await
/// ```
///
/// Construction remains infallible because its route trie is compiled into
/// the program.
///
/// # Context forwarding
///
/// Use `#[service(context)]` to reserve the first handler parameter after
/// `&self` as the context. Its concrete type is inferred from the handlers and
/// must be identical across all routes. The type is forwarded unchanged, so
/// contexts may be owned, shared, or mutable:
///
/// ```text
/// async fn handler(&self, context: Context, capture: u32) -> Response
/// async fn handler(&self, context: &Context, capture: u32) -> Response
/// async fn handler(&self, context: &mut Context, capture: u32) -> Response
/// ```
///
/// Context remains the final argument to `dispatch`; only the handler position
/// is reserved:
///
/// ```text
/// service.dispatch(method, path, context).await
/// router.dispatch(&service, method, path, context).await
/// ```
///
/// Bare `#[service]` retains the original convention: one shared borrowed
/// context argument may appear anywhere that does not correspond to a static
/// capture. The explicit context mode is recommended when the context is
/// owned or mutable, and removes ambiguity between context and dynamic capture
/// parameters.
///
/// # Dynamic routes
///
/// Annotate a handler with `#[route(dynamic)]` when its method and path
/// template are supplied at run time. All non-context parameters become path
/// captures. Dynamic captures must be owned: use `String` for decoded text or
/// another owned `FromStr` type.
///
/// A service containing any dynamic handler receives a generated
/// `<Service>RouterBuilder` and `<Service>Router`. Configure the dynamic paths
/// once, retain the built router, and dispatch through it:
///
/// ```
/// use routerama::{HttpMethod, service};
///
/// struct RequestContext {
///     request_id: u64,
/// }
///
/// struct BooksApi;
///
/// #[service(context)]
/// impl BooksApi {
///     #[route(GET, "/health")]
///     async fn health(&self, context: &mut RequestContext) -> String {
///         context.request_id += 1;
///         std::future::ready(format!("healthy:{}", context.request_id)).await
///     }
///
///     #[route(dynamic)]
///     async fn get_book(&self, context: &mut RequestContext, id: u32) -> String {
///         context.request_id += 1;
///         std::future::ready(format!("book:{id}:{}", context.request_id)).await
///     }
/// }
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let api = BooksApi;
/// let mut context = RequestContext { request_id: 7 };
/// let router = BooksApi::router_builder()
///     .add_get_book(HttpMethod::GET, "/books/{id}")
///     .add_get_book(HttpMethod::GET, "/library/{id}")
///     .build()?;
///
/// assert_eq!(
///     router
///         .dispatch(&api, "GET", "/health", &mut context)
///         .await?,
///     "healthy:8"
/// );
/// assert_eq!(
///     router
///         .dispatch(&api, "GET", "/library/42", &mut context)
///         .await?,
///     "book:42:9"
/// );
/// # Ok(())
/// # }
/// ```
///
/// Calling an `add_<handler>` method more than once registers aliases for that
/// handler. Every dynamic handler must be registered at least once.
/// [`ConfigurationError`] returned by `build` aggregates missing
/// registrations, invalid templates, capture mismatches, and conflicting
/// dynamic routes. Static routes are resolved before dynamic routes.
///
/// # Handler contract
///
/// Every annotated handler must:
///
/// - be an `async` method in a non-generic inherent impl;
/// - begin with shared `&self`;
/// - contain either one shared borrowed context argument in bare mode or one
///   context argument immediately after `&self` in context mode;
/// - use the same context type and explicit response type as every other
///   handler; and
/// - use simple identifier parameter patterns.
///
/// Non-annotated methods remain unchanged. Generic or conditionally compiled
/// impl blocks and handlers are not currently supported.
///
/// # Errors
///
/// Dispatch returns [`ResolveError::InvalidPath`] for a path containing a
/// query or fragment, [`ResolveError::NotFound`] when nothing matches, and the
/// corresponding capture error when conversion fails. Handler return values
/// are not interpreted; for example, if every handler returns
/// `Result<Response, AppError>`, dispatch returns
/// `Result<Result<Response, AppError>, ResolveError>`.
///
/// # Performance
///
/// Static and dynamic matching use `routerama`'s existing tries. After
/// resolution, dispatch performs one enum match and a direct monomorphized
/// handler call. Dynamic routing adds persistent startup-built resolver state,
/// but does not add indirect handler dispatch or allocations beyond those
/// required by the selected capture types.
pub use routerama_macros::service;

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
