<div align="center">
 <img src="./logo.png" alt="Routerama Logo" width="96">

# Routerama

[![crate.io](https://img.shields.io/crates/v/routerama.svg)](https://crates.io/crates/routerama)
[![docs.rs](https://docs.rs/routerama/badge.svg)](https://docs.rs/routerama)
[![MSRV](https://img.shields.io/crates/msrv/routerama)](https://crates.io/crates/routerama)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Blazingly fast HTTP routing.

`routerama` is designed to rapidly decode incoming HTTP requests to route them to
handler functions that can respond to the request. In the common case where all
possible routes are known at compile time, you can use the [`routes!`][__link0] macro
to define all your routes and get extremely fast custom routing logic.
If on the other hand, you need to define routes dynamically, you can use [`DynRouter`][__link1]
which provides you the needed flexibility, at the cost of a little performance.

At runtime, you can take an HTTP method and URL path and get in return a well-known route
and set of variables extracted from the path.

## Defining a static router

There are two ways to define static routers; both ways produce identical code:

* **[`routes!`][__link2] macro** — the simplest way, and the one to
  reach for by default.
  
  ```rust
  routerama::routes! {
      pub enum BookRoute {
          ListBooks GET  "/books",
          GetBook   GET  "/books/{book}",
      }
  }
  
  assert!(matches!(BookRoute::resolve("GET", "/books/rust"), Some(BookRoute::GetBook { book }) if book == "rust"));
  ```

* **[`Generator`][__link3]** — for use in a `build.rs` context:
  
  ```rust
  // build.rs
  use std::path::PathBuf;
  
  use routerama::{Generator, RouteRule, HttpMethod};
  
  let mut generator = Generator::new();
  generator.add_all([
      RouteRule::new("ListBooks", HttpMethod::Get, "/books".parse()?),
      RouteRule::new("GetBook", HttpMethod::Get, "/books/{book}".parse()?),
  ]);
  
  let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
  std::fs::write(out_dir.join("router.rs"), generator.generate().to_string())?;
  ```
  
  ```rust
  // src/lib.rs
  include!(concat!(env!("OUT_DIR"), "/router.rs"));
  ```

## Defining a dynamic router

When the route set is known only at run time — loaded from config, a
database, or a plugin registry — build a [`DynRouter`][__link4] from [`RouteRule`][__link5]s
(requires the `dynamic` feature).

```rust
use routerama::{DynRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};

let router = DynRouter::new([
    RouteRule::new(
        "ListBooks",
        HttpMethod::Get,
        "/books".parse().expect("valid"),
    ),
    RouteRule::new(
        "GetBook",
        HttpMethod::Get,
        "/books/{book}".parse().expect("valid"),
    ),
]);

let matched = router.resolve("GET", "/books/rust").expect("a match");
assert_eq!(matched.name(), "GetBook");
assert_eq!(matched.capture("book"), Some("rust"));
assert!(router.resolve("POST", "/books/rust").is_none());
```

See the `dynamic_routing` example for a fuller tour.

## Defining a hybrid static/dynamic router

A fixed set of built-in routes plus a set registered at run time can be served
through one [`EitherRouter`][__link6] (requires the `dynamic` feature): declare a
companion `struct` in `routes!` to get a zero-sized [`Router`][__link7] for the static
core, then compose it with a [`DynRouter`][__link8] overlay. The primary (static) router
is tried first and the secondary (dynamic) is the fallback:

```rust
use routerama::{DynRouter, EitherRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};

// The built-in routes, plus a `struct` to get a zero-sized `ApiRouter`.
routerama::routes! {
    pub enum Api {
        ListBooks GET "/books",
        GetBook   GET "/books/{book}",
    }
    pub struct ApiRouter;
}

// The run-time overlay, on a disjoint subtree.
let plugins = DynRouter::new([RouteRule::new(
    "Plugin",
    HttpMethod::Get,
    "/plugins/{name}".parse().expect("valid"),
)]);
let router = EitherRouter::new(ApiRouter, plugins);

// Built-ins resolve through the static core...
assert_eq!(
    router
        .resolve("GET", "/books/rust")
        .expect("a match")
        .name(),
    "GetBook"
);
// ...and plugin routes fall through to the dynamic overlay.
let plugin = router.resolve("GET", "/plugins/auth").expect("a match");
assert_eq!(plugin.name(), "Plugin");
assert_eq!(plugin.capture("name"), Some("auth"));
```

See the `hybrid_routing` example for a fuller tour.

## Path template syntax

Each route’s path is a [`google.api.http`][__link9]
path template, parsed by the [`http_path_template`][__link10]
crate. A template is a `/`-separated sequence of segments:

* a **literal** (`books`) matches that segment verbatim;
* **`{var}`** captures exactly one segment into the field `var` (the shorthand
  for `{var=*}`);
* **`*`** matches exactly one segment without capturing it;
* **`**`** matches all remaining segments and may appear only as the final
  element;
* **`{var=sub-template}`** captures the portion matched by `sub-template`
  (e.g. `{book=shelves/*}`);
* a trailing **`:verb`** declares a custom method verb, as in
  `/books/{book}:archive`.

An **extended grammar** additionally allows one variable to be wrapped in
literal text *within* a single segment — a prefix and/or suffix — such as
`/files/{name}.json`, `/v{version}/x`, or `/img-{id}.png`. Both the [`routes!`][__link11]
macro and [`Generator`][__link12] accept it.

## Generated code

Generated router code is an enum with one variant per route, carrying any
captured path parameters as fields. The inherent [`resolve`][__link13]
associated function converts an HTTP method and path into a variant. The enum
also implements [`Route`][__link14] and [`RouteMatch`][__link15].

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BookRoute<'p> {
    ListBooks,                  // no path variables: a unit variant
    GetBook { book: &'p str },  // captures borrow from the request path
}

impl<'p> BookRoute<'p> {
    // Recovers the name string each variant was declared with, independent
    // of any captured values.
    pub const fn name(&self) -> &'static str { /* ... */ }

    // Scans the path into segments, then walks the compile-time trie,
    // most-specific-first, capturing any path variables (e.g. `{book}`)
    // into the matched variant's fields.
    pub fn resolve<P: AsRef<str> + ?Sized>(
        method: impl AsRef<str>,
        path: &'p P,
    ) -> Option<BookRoute<'p>> { /* ... */ }
}

// The enum implements the `Route` trait, forwarding to the inherent methods
// above, so you can write code generic over any generated router.
impl<'p> routerama::Route<'p> for BookRoute<'p> {
    fn resolve<P: AsRef<str> + ?Sized>(
        method: impl AsRef<str>,
        path: &'p P,
    ) -> Option<Self> { BookRoute::resolve(method, path) }

    fn name(&self) -> &'static str { BookRoute::name(self) }
}
```

Declaring a companion `struct` in the macro *also* generates a zero-sized
[`Router`][__link16] under that name and a [`RouteMatch`][__link17] impl on the enum, so the
static router can be used through the same traits as a runtime
[`DynRouter`][__link18] and composed with one via [`EitherRouter`][__link19]:

```rust
routerama::routes! {
    pub enum BookRoute { /* ... */ }
    pub struct BookRouter;   // opt in to the ZST router
}

// expands to, in addition to the above:
#[derive(Clone, Copy, Debug, Default)]
pub struct BookRouter;

impl routerama::Router for BookRouter {
    type Match<'p> = BookRoute<'p>;
    fn resolve<'p>(&'p self, method: &str, path: &'p str) -> Option<BookRoute<'p>> {
        BookRoute::resolve(method, path)
    }
}

impl<'p> routerama::RouteMatch<'p> for BookRoute<'p> {
    fn name(&self) -> &str { BookRoute::name(self) }
    // Returns the captured variable named `name` (e.g. `"book"`), or `None`.
    fn capture(&self, name: &str) -> Option<&'p str> { /* ... */ }
}
```

You dispatch by matching on the variant that `resolve` returns:

```rust
match BookRoute::resolve("GET", "/v1/books/MyLittlePony") {
    Some(BookRoute::ListBooks) => { /* ... */ }
    Some(BookRoute::GetBook { book }) => {
        // book == "MyLittlePony"
    }
    None => { /* no route matched */ }
}
```

## Crate features

This crate has the following optional features:

* **`macros`** (default) — enables the `routes!` macro.

* **`build`** (default) — enables the build-time `Generator` code-generation
  API (plus its `GeneratorBuilder`) for use from a `build.rs`.

* **`dynamic`** (default) — enables the runtime router: a [`DynRouter`][__link20] built from a route set
  known only at run time, resolving identically to the static path, plus an [`EitherRouter`][__link21]
  that combines a static core with a dynamic overlay.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/routerama">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbedNJbyU3adkb7-9DtpVJbcYbiTnnhh7NRWkbeisCfdFu2uRhZIKCaXJvdXRlcmFtYWUwLjEuMIJvcm91dGVyYW1hX2J1aWxkZTAuMS4w
 [__link0]: `routes!`
 [__link1]: https://docs.rs/routerama/0.1.0/routerama/?search=DynRouter
 [__link10]: https://crates.io/crates/http_path_template
 [__link11]: `routes!`
 [__link12]: https://docs.rs/routerama_build/0.1.0/routerama_build/?search=Generator
 [__link13]: https://docs.rs/routerama/0.1.0/routerama/?search=Route::resolve
 [__link14]: https://docs.rs/routerama/0.1.0/routerama/?search=Route
 [__link15]: https://docs.rs/routerama/0.1.0/routerama/?search=RouteMatch
 [__link16]: https://docs.rs/routerama/0.1.0/routerama/?search=Router
 [__link17]: https://docs.rs/routerama/0.1.0/routerama/?search=RouteMatch
 [__link18]: https://docs.rs/routerama/0.1.0/routerama/?search=DynRouter
 [__link19]: https://docs.rs/routerama/0.1.0/routerama/?search=EitherRouter
 [__link2]: `routes!`
 [__link20]: https://docs.rs/routerama/0.1.0/routerama/?search=DynRouter
 [__link21]: https://docs.rs/routerama/0.1.0/routerama/?search=EitherRouter
 [__link3]: https://docs.rs/routerama_build/0.1.0/routerama_build/?search=Generator
 [__link4]: https://docs.rs/routerama/0.1.0/routerama/?search=DynRouter
 [__link5]: https://docs.rs/routerama_build/0.1.0/routerama_build/?search=RouteRule
 [__link6]: https://docs.rs/routerama/0.1.0/routerama/?search=EitherRouter
 [__link7]: https://docs.rs/routerama/0.1.0/routerama/?search=Router
 [__link8]: https://docs.rs/routerama/0.1.0/routerama/?search=DynRouter
 [__link9]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
