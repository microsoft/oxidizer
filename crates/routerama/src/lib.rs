// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/favicon.ico")]

//! Blazingly fast HTTP route resolution.
//!
//! `routerama` rapidly *resolves* an incoming HTTP method + path to a typed
//! route, extracting any variables captured from the path. Once you have the typed
//! route, you trivially invoke the requisite HTTP handler method.
//!
//! `routerama` is typically integrated with a higher-level framework that builds additional
//! capabilities, such as strongly-typed variables. `routerama` is an extremely fast foundational
//! building block used by these higher layers.
//!
//! In the common case when your routes are known at compile time, you generate a
//! resolver with [`#[resolver]`](macro@resolver); when they are known only at run
//! time, build a [`DynResolver`] instead.
//!
//! # Defining a static resolver
//!
//! The common case is that you know all the routes in your application statically, so you can
//! use the fast static route model. You declare an enum representing each of your routes,
//! annotate the enum with the [`#[resolver]`](macro@resolver) attribute,
//! and you annotate each variant with `#[route(METHOD, "path")]`.
//!
//! ```
//! # #[cfg(feature = "macros")]
//! # fn main() {
//! use routerama::{Resolver as _, resolver};
//!
//! #[resolver(name = BookResolver)]
//! #[derive(Clone, Copy)]
//! enum BookRoute<'p> {
//!     #[route(GET, "/books")]
//!     ListBooks,
//!
//!     #[route(GET, "/books/{book}")]
//!     GetBook { book: &'p str },
//! }
//!
//! // this is how you resolve a verb + path pair and dispatch to the handler method.
//! match BookResolver.resolve("GET", "/books/rust") {
//!     Some(BookRoute::ListBooks) => { /* list books */ }
//!     Some(BookRoute::GetBook { book }) => assert_eq!(book, "rust"),
//!     None => { /* 404 */ }
//! }
//! # }
//! # #[cfg(not(feature = "macros"))]
//! # fn main() {}
//! ```
//!
//! As you can see above, the static resolver returns an enum with the variants holding the raw
//! variable values, ready to be consumed. You then need to dispatch to the specific handler to
//! perform the actual work.
//!
//! For a build-script alternative that generates the enum for you (rather than
//! deriving on one you write), use [`Generator`]:
//!
//!   ```ignore
//!   // use in build.rs
//!   use std::path::PathBuf;
//!
//!   use http_path_template::{Grammar, PathTemplate};
//!   use routerama::{Generator, Route, HttpMethod};
//!
//!   let mut generator = Generator::new();
//!   generator.add_all([
//!       Route::new("ListBooks", HttpMethod::Get, PathTemplate::parse("/books", Grammar::default())?),
//!       Route::new("GetBook", HttpMethod::Get, PathTemplate::parse("/books/{book}", Grammar::default())?),
//!   ]);
//!
//!   let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
//!   std::fs::write(out_dir.join("router.rs"), generator.generate().to_string())?;
//!   ```
//!
//!   ```ignore
//!   // src/lib.rs
//!   include!(concat!(env!("OUT_DIR"), "/router.rs"));
//!   ```
//!
//! # Defining a dynamic resolver
//!
//! When the route set is known only at run time — loaded from config, a
//! database, or a plugin registry — build a [`DynResolver`] from [`Route`]s
//! (requires the `dynamic` feature). Attach a value to each route — typically a
//! handler — with [`with_values`](DynResolver::with_values); a match hands that
//! value straight back through [`value`](DynMatch::value):
//!
//! ```
//! # #[cfg(feature = "dynamic")]
//! # fn main() {
//! use http_path_template::{Grammar, PathTemplate};
//! use routerama::{DynResolver, HttpMethod, Resolver as _, Route, RouteMatch};
//!
//! type Handler = fn(&dyn RouteMatch<'_>) -> String;
//!
//! let resolver = DynResolver::with_values([
//!     (
//!         Route::new(
//!             "ListBooks",
//!             HttpMethod::Get,
//!             PathTemplate::parse("/books", Grammar::default()).expect("valid"),
//!         ),
//!         (|_m| "list all books".to_owned()) as Handler,
//!     ),
//!     (
//!         Route::new(
//!             "GetBook",
//!             HttpMethod::Get,
//!             PathTemplate::parse("/books/{book}", Grammar::default()).expect("valid"),
//!         ),
//!         (|m| format!("get book {}", m.capture("book").unwrap_or("?"))) as Handler,
//!     ),
//! ]);
//!
//! // The matched route hands back its handler directly
//! match resolver.resolve("GET", "/books/rust") {
//!     Some(matched) => {
//!         let handler = *matched.value();
//!         assert_eq!(handler(&matched), "get book rust");
//!     }
//!     None => { /* 404 */ }
//! }
//! assert!(resolver.resolve("POST", "/books/rust").is_none());
//! # }
//! # #[cfg(not(feature = "dynamic"))]
//! # fn main() {}
//! ```
//!
//! See the `dynamic_routing` example for a fuller tour.
//!
//! # Defining a hybrid static/dynamic resolver
//!
//! A fixed set of built-in routes plus a set registered at run time can be served
//! through one [`EitherResolver`] (requires the `dynamic` feature): apply
//! `#[resolver(name = ...)]` to an enum to create a static resolver
//! and compose it with a [`DynResolver`] overlay. The
//! primary (static) resolver is tried first and the secondary (dynamic) is the
//! fallback:
//!
//! ```
//! # #[cfg(all(feature = "dynamic", feature = "macros"))]
//! # fn main() {
//! use http_path_template::{Grammar, PathTemplate};
//! use routerama::{
//!     DynResolver, EitherResolver, HttpMethod, Resolver as _, Route, RouteMatch as _, resolver,
//! };
//!
//! // The built-in routes; `#[resolver(name = ...)]` names the zero-sized `ApiResolver`.
//! #[resolver(name = ApiResolver)]
//! #[derive(Clone, Copy)]
//! enum Api<'p> {
//!     #[route(GET, "/books")]
//!     ListBooks,
//!
//!     #[route(GET, "/books/{book}")]
//!     GetBook { book: &'p str },
//! }
//!
//! // The run-time overlay, on a disjoint subtree.
//! let plugins = DynResolver::new([Route::new(
//!     "Plugin",
//!     HttpMethod::Get,
//!     PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
//! )]);
//!
//! let resolver = EitherResolver::new(ApiResolver, plugins);
//!
//! // Built-ins resolve through the static core...
//! assert_eq!(
//!     resolver
//!         .resolve("GET", "/books/rust")
//!         .expect("a match")
//!         .name(),
//!     "GetBook"
//! );
//! // ...and plugin routes fall through to the dynamic overlay.
//! let plugin = resolver.resolve("GET", "/plugins/auth").expect("a match");
//! assert_eq!(plugin.name(), "Plugin");
//! assert_eq!(plugin.capture("name"), Some("auth"));
//! # }
//! # #[cfg(not(all(feature = "dynamic", feature = "macros")))]
//! # fn main() {}
//! ```
//!
//! See the `hybrid_routing` example for a fuller tour.
//!
//! # What the macro generates
//!
//! `#[resolver(name = X)]` leaves your `enum` as written and generates, for it:
//!
//! - a zero-sized resolver named `X`, implementing [`Resolver`]; its
//!   `resolve(method, path) -> Option<YourEnum>` scans the path and walks a
//!   compile-time trie, most-specific-first;
//!
//! - a [`RouteMatch`] impl on the enum, exposing [`name`](RouteMatch::name) and
//!   [`capture`](RouteMatch::capture).
//!
//! Each capturing variant declares one `&'p str` field per `{capture}` (dotted
//! and reserved names are sanitized — `{item.id}` → `item_id`, `{type}` →
//! `_f_type`); the macro reports a clear error when a variant declares fields
//! that disagree with its path captures. The enum uses the lifetime `'p` for its
//! captures.
//!
//! # Crate features
//!
//! This crate has the following optional features:
//!
//! - **`macros`** (default) — enables `#[resolver]`.
//!
//! - **`build`** (default) — enables the build-time `Generator` code-generation
//!   API for use from a `build.rs`.
//!
//! - **`dynamic`** (default) — enables the runtime resolver: a [`DynResolver`] built from a route set
//!   known only at run time, resolving identically to the static path, plus an [`EitherResolver`]
//!   that combines a static core with a dynamic overlay.

mod route;

// Runtime plumbing the generated code calls by absolute path
// (`::routerama::codegen_helpers`); public so generated code can reach it, but
// hidden from the docs as it is not a human-facing API.
#[doc(hidden)]
pub mod codegen_helpers;

#[doc(inline)]
pub use route::{Resolver, RouteMatch};
// The build-time code-generation API, re-exported at the crate root.
#[cfg(feature = "build")]
#[cfg_attr(docsrs, doc(cfg(feature = "build")))]
pub use routerama_build::{Generator, GeneratorBuilder};

// The runtime resolver, re-exported at the crate root from a private module.
#[cfg(feature = "dynamic")]
mod dynamic;
#[cfg(feature = "dynamic")]
#[cfg_attr(docsrs, doc(cfg(feature = "dynamic")))]
pub use dynamic::{DynMatch, DynResolver, EitherMatch, EitherResolver};
// `HttpMethod` and `Route` describe a route to either backend, so they are
// shared by the `build` and `dynamic` features and re-exported once.
#[cfg(any(feature = "build", feature = "dynamic"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "build", feature = "dynamic"))))]
pub use routerama_build::{HttpMethod, Route};
/// Generates a static resolver for a route `enum`.
///
/// Apply `#[resolver(name = SomeResolver)]` to a route `enum` and annotate each
/// variant with `#[route(METHOD, "path")]`; a capturing variant carries the
/// path's captures as `&'p str` fields. Generates a zero-sized `SomeResolver`
/// implementing [`Resolver`], plus a [`RouteMatch`] impl on the enum. See the
/// [crate-level docs](crate) for a full tour.
///
/// # Example
///
/// ```ignore
/// #[routerama::resolver(name = BookResolver)]
/// #[derive(Clone, Copy)]
/// enum BookRoute<'p> {
///     #[route(GET, "/books")]
///     ListBooks,
///
///     #[route(GET, "/books/{book}")]
///     GetBook { book: &'p str },
///
///     #[route(POST, "/books/{book}:archive")]
///     ArchiveBook { book: &'p str },
/// }
/// ```
#[cfg(feature = "macros")]
#[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
pub use routerama_macros::resolver;
