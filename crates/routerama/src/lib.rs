// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama/favicon.ico")]

//! Blazingly fast HTTP routing.
//!
//! `routerama` is designed to rapidly decode incoming HTTP requests to route them to
//! handler functions that can respond to the request. In the common case where all
//! possible routes are known at compile time, you can use the [`routes!`] macro
//! to define all your routes and get extremely fast custom routing logic.
//! If on the other hand, you need to define routes dynamically, you can use [`DynRouter`]
//! which provides you the needed flexibility, at the cost of a little performance.
//!
//! At runtime, you can take an HTTP method and URL path and get in return a well-known route
//! and set of variables extracted from the path.
//!
//! # Defining a static router
//!
//! There are two ways to define static routers; both ways produce identical code:
//!
//! - **[`routes!`] macro** — the simplest way, and the one to
//!   reach for by default.
//!
//!   ```
//!   # #[cfg(feature = "macros")]
//!   # fn main() {
//!   routerama::routes! {
//!       pub enum BookRoute {
//!           ListBooks GET  "/books",
//!           GetBook   GET  "/books/{book}",
//!       }
//!   }
//!
//!   assert!(matches!(BookRoute::resolve("GET", "/books/rust"), Some(BookRoute::GetBook { book }) if book == "rust"));
//!   # }
//!   # #[cfg(not(feature = "macros"))]
//!   # fn main() {}
//!   ```
//!
//! - **[`Generator`]** — for use in a `build.rs` context:
//!
//!   ```ignore
//!   // build.rs
//!   use std::path::PathBuf;
//!
//!   use routerama::{Generator, RouteRule, HttpMethod};
//!
//!   let mut generator = Generator::new();
//!   generator.add_all([
//!       RouteRule::new("ListBooks", HttpMethod::Get, "/books".parse()?),
//!       RouteRule::new("GetBook", HttpMethod::Get, "/books/{book}".parse()?),
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
//! # Defining a dynamic router
//!
//! When the route set is known only at run time — loaded from config, a
//! database, or a plugin registry — build a [`DynRouter`] from [`RouteRule`]s
//! (requires the `dynamic` feature).
//!
//! ```
//! # #[cfg(feature = "dynamic")]
//! # fn main() {
//! use routerama::{DynRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};
//!
//! let router = DynRouter::new([
//!     RouteRule::new(
//!         "ListBooks",
//!         HttpMethod::Get,
//!         "/books".parse().expect("valid"),
//!     ),
//!     RouteRule::new(
//!         "GetBook",
//!         HttpMethod::Get,
//!         "/books/{book}".parse().expect("valid"),
//!     ),
//! ]);
//!
//! let matched = router.resolve("GET", "/books/rust").expect("a match");
//! assert_eq!(matched.name(), "GetBook");
//! assert_eq!(matched.capture("book"), Some("rust"));
//! assert!(router.resolve("POST", "/books/rust").is_none());
//! # }
//! # #[cfg(not(feature = "dynamic"))]
//! # fn main() {}
//! ```
//!
//! See the `dynamic_routing` example for a fuller tour.
//!
//! # Defining a hybrid static/dynamic router
//!
//! A fixed set of built-in routes plus a set registered at run time can be served
//! through one [`EitherRouter`] (requires the `dynamic` feature): declare a
//! companion `struct` in `routes!` to get a zero-sized [`Router`] for the static
//! core, then compose it with a [`DynRouter`] overlay. The primary (static) router
//! is tried first and the secondary (dynamic) is the fallback:
//!
//! ```
//! # #[cfg(all(feature = "dynamic", feature = "macros"))]
//! # fn main() {
//! use routerama::{DynRouter, EitherRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};
//!
//! // The built-in routes, plus a `struct` to get a zero-sized `ApiRouter`.
//! routerama::routes! {
//!     pub enum Api {
//!         ListBooks GET "/books",
//!         GetBook   GET "/books/{book}",
//!     }
//!     pub struct ApiRouter;
//! }
//!
//! // The run-time overlay, on a disjoint subtree.
//! let plugins = DynRouter::new([RouteRule::new(
//!     "Plugin",
//!     HttpMethod::Get,
//!     "/plugins/{name}".parse().expect("valid"),
//! )]);
//! let router = EitherRouter::new(ApiRouter, plugins);
//!
//! // Built-ins resolve through the static core...
//! assert_eq!(
//!     router
//!         .resolve("GET", "/books/rust")
//!         .expect("a match")
//!         .name(),
//!     "GetBook"
//! );
//! // ...and plugin routes fall through to the dynamic overlay.
//! let plugin = router.resolve("GET", "/plugins/auth").expect("a match");
//! assert_eq!(plugin.name(), "Plugin");
//! assert_eq!(plugin.capture("name"), Some("auth"));
//! # }
//! # #[cfg(not(all(feature = "dynamic", feature = "macros")))]
//! # fn main() {}
//! ```
//!
//! See the `hybrid_routing` example for a fuller tour.
//!
//! # Path template syntax
//!
//! Each route's path is a [`google.api.http`](https://github.com/googleapis/googleapis/blob/master/google/api/http.proto)
//! path template, parsed by the [`http_path_template`](https://crates.io/crates/http_path_template)
//! crate. A template is a `/`-separated sequence of segments:
//!
//! - a **literal** (`books`) matches that segment verbatim;
//! - **`{var}`** captures exactly one segment into the field `var` (the shorthand
//!   for `{var=*}`);
//! - **`*`** matches exactly one segment without capturing it;
//! - **`**`** matches all remaining segments and may appear only as the final
//!   element;
//! - **`{var=sub-template}`** captures the portion matched by `sub-template`
//!   (e.g. `{book=shelves/*}`);
//! - a trailing **`:verb`** declares a custom method verb, as in
//!   `/books/{book}:archive`.
//!
//! An **extended grammar** additionally allows one variable to be wrapped in
//! literal text *within* a single segment — a prefix and/or suffix — such as
//! `/files/{name}.json`, `/v{version}/x`, or `/img-{id}.png`. Both the [`routes!`]
//! macro and [`Generator`] accept it.
//!
//! # Generated code
//!
//! Generated router code is an enum with one variant per route, carrying any
//! captured path parameters as fields. The inherent [`resolve`](Route::resolve)
//! associated function converts an HTTP method and path into a variant. The enum
//! also implements [`Route`] and [`RouteMatch`].
//!
//! ```ignore
//! #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
//! pub enum BookRoute<'p> {
//!     ListBooks,                  // no path variables: a unit variant
//!     GetBook { book: &'p str },  // captures borrow from the request path
//! }
//!
//! impl<'p> BookRoute<'p> {
//!     // Recovers the name string each variant was declared with, independent
//!     // of any captured values.
//!     pub const fn name(&self) -> &'static str { /* ... */ }
//!
//!     // Scans the path into segments, then walks the compile-time trie,
//!     // most-specific-first, capturing any path variables (e.g. `{book}`)
//!     // into the matched variant's fields.
//!     pub fn resolve<P: AsRef<str> + ?Sized>(
//!         method: impl AsRef<str>,
//!         path: &'p P,
//!     ) -> Option<BookRoute<'p>> { /* ... */ }
//! }
//!
//! // The enum implements the `Route` trait, forwarding to the inherent methods
//! // above, so you can write code generic over any generated router.
//! impl<'p> routerama::Route<'p> for BookRoute<'p> {
//!     fn resolve<P: AsRef<str> + ?Sized>(
//!         method: impl AsRef<str>,
//!         path: &'p P,
//!     ) -> Option<Self> { BookRoute::resolve(method, path) }
//!
//!     fn name(&self) -> &'static str { BookRoute::name(self) }
//! }
//! ```
//!
//! Declaring a companion `struct` in the macro *also* generates a zero-sized
//! [`Router`] under that name and a [`RouteMatch`] impl on the enum, so the
//! static router can be used through the same traits as a runtime
//! [`DynRouter`] and composed with one via [`EitherRouter`]:
//!
//! ```ignore
//! routerama::routes! {
//!     pub enum BookRoute { /* ... */ }
//!     pub struct BookRouter;   // opt in to the ZST router
//! }
//!
//! // expands to, in addition to the above:
//! #[derive(Clone, Copy, Debug, Default)]
//! pub struct BookRouter;
//!
//! impl routerama::Router for BookRouter {
//!     type Match<'p> = BookRoute<'p>;
//!     fn resolve<'p>(&'p self, method: &str, path: &'p str) -> Option<BookRoute<'p>> {
//!         BookRoute::resolve(method, path)
//!     }
//! }
//!
//! impl<'p> routerama::RouteMatch<'p> for BookRoute<'p> {
//!     fn name(&self) -> &str { BookRoute::name(self) }
//!     // Returns the captured variable named `name` (e.g. `"book"`), or `None`.
//!     fn capture(&self, name: &str) -> Option<&'p str> { /* ... */ }
//! }
//! ```
//!
//! You dispatch by matching on the variant that `resolve` returns:
//!
//! ```ignore
//! match BookRoute::resolve("GET", "/v1/books/MyLittlePony") {
//!     Some(BookRoute::ListBooks) => { /* ... */ }
//!     Some(BookRoute::GetBook { book }) => {
//!         // book == "MyLittlePony"
//!     }
//!     None => { /* no route matched */ }
//! }
//! ```
//!
//! # Crate features
//!
//! This crate has the following optional features:
//!
//! - **`macros`** (default) — enables the `routes!` macro.
//!
//! - **`build`** (default) — enables the build-time `Generator` code-generation
//!   API (plus its `GeneratorBuilder`) for use from a `build.rs`.
//!
//! - **`dynamic`** (default) — enables the runtime router: a [`DynRouter`] built from a route set
//!   known only at run time, resolving identically to the static path, plus an [`EitherRouter`]
//!   that combines a static core with a dynamic overlay.

mod route;

// Runtime plumbing the generated code calls by absolute path
// (`::routerama::codegen_helpers`); public so generated code can reach it, but
// hidden from the docs as it is not a human-facing API.
#[doc(hidden)]
pub mod codegen_helpers;

#[doc(inline)]
pub use route::{Route, RouteMatch, Router};
// The build-time code-generation API, re-exported at the crate root.
#[cfg(feature = "build")]
#[cfg_attr(docsrs, doc(cfg(feature = "build")))]
pub use routerama_build::{Generator, GeneratorBuilder, route_field_name};

// The runtime router, re-exported at the crate root from a private module.
#[cfg(feature = "dynamic")]
mod dynamic;
#[cfg(feature = "dynamic")]
#[cfg_attr(docsrs, doc(cfg(feature = "dynamic")))]
pub use dynamic::{DynMatch, DynRouter, EitherMatch, EitherRouter};
// `HttpMethod` and `RouteRule` describe a route to either backend, so they are
// shared by the `build` and `dynamic` features and re-exported once.
#[cfg(any(feature = "build", feature = "dynamic"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "build", feature = "dynamic"))))]
pub use routerama_build::{HttpMethod, RouteRule};
/// Generates a static router from a table of routes.
///
/// See the [crate-level docs](crate) for the syntax.
///
/// # Example
///
/// ```ignore
/// routerama::routes! {
///     pub enum BookRoute {
///         ListBooks   GET  "/books",
///         GetBook     GET  "/books/{book}",
///         ArchiveBook POST "/books/{book}:archive",
///     }
///     // Optional: also emit a zero-sized `Router` named `BookRouter`.
///     pub struct BookRouter;
/// }
#[cfg(feature = "macros")]
#[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
pub use routerama_macros::routes;
