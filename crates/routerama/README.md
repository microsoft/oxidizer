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

Blazingly fast HTTP route resolution.

`routerama` rapidly *resolves* an incoming HTTP method + path to a typed
route, extracting any variables captured from the path. Once you have the typed
route, you trivially invoke the requisite HTTP handler method.

`routerama` is typically integrated with a higher-level framework that builds additional
capabilities, such as strongly-typed variables. `routerama` is an extremely fast foundational
building block used by these higher layers.

In the common case when your routes are known at compile time, you generate a
resolver with [`#[resolver]`][__link0]; when they are known only at run
time, build a [`DynResolver`][__link1] instead.

## Defining a static resolver

The common case is that you know all the routes in your application statically, so you can
use the fast static route model. You declare an enum representing each of your routes,
annotate the enum with the [`#[resolver]`][__link2] attribute,
and you annotate each variant with `#[route(METHOD, "path")]`.

```rust
use routerama::{Resolver as _, resolver};

#[resolver(name = BookResolver)]
#[derive(Clone, Copy)]
enum BookRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,

    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
}

// this is how you resolve a verb + path pair and dispatch to the handler method.
match BookResolver.resolve("GET", "/books/rust") {
    Some(BookRoute::ListBooks) => { /* list books */ }
    Some(BookRoute::GetBook { book }) => assert_eq!(book, "rust"),
    None => { /* 404 */ }
}
```

As you can see above, the static resolver returns an enum with the variants holding the raw
variable values, ready to be consumed. You then need to dispatch to the specific handler to
perform the actual work.

For a build-script alternative that generates the enum for you (rather than
deriving on one you write), use [`Generator`][__link3]:

```rust
// use in build.rs
use std::path::PathBuf;

use http_path_template::{Grammar, PathTemplate};
use routerama::{Generator, Route, HttpMethod};

let mut generator = Generator::new();
generator.add_all([
    Route::new("ListBooks", HttpMethod::Get, PathTemplate::parse("/books", Grammar::default())?),
    Route::new("GetBook", HttpMethod::Get, PathTemplate::parse("/books/{book}", Grammar::default())?),
]);

let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
std::fs::write(out_dir.join("router.rs"), generator.generate().to_string())?;
```

```rust
// src/lib.rs
include!(concat!(env!("OUT_DIR"), "/router.rs"));
```

## Defining a dynamic resolver

When the route set is known only at run time — loaded from config, a
database, or a plugin registry — build a [`DynResolver`][__link4] from [`Route`][__link5]s
(requires the `dynamic` feature). Attach a value to each route — typically a
handler — with [`with_values`][__link6]; a match hands that
value straight back through [`value`][__link7]:

```rust
use http_path_template::{Grammar, PathTemplate};
use routerama::{DynResolver, HttpMethod, Resolver as _, Route, RouteMatch};

type Handler = fn(&dyn RouteMatch<'_>) -> String;

let resolver = DynResolver::with_values([
    (
        Route::new(
            "ListBooks",
            HttpMethod::Get,
            PathTemplate::parse("/books", Grammar::default()).expect("valid"),
        ),
        (|_m| "list all books".to_owned()) as Handler,
    ),
    (
        Route::new(
            "GetBook",
            HttpMethod::Get,
            PathTemplate::parse("/books/{book}", Grammar::default()).expect("valid"),
        ),
        (|m| format!("get book {}", m.capture("book").unwrap_or("?"))) as Handler,
    ),
]);

// The matched route hands back its handler directly
match resolver.resolve("GET", "/books/rust") {
    Some(matched) => {
        let handler = *matched.value();
        assert_eq!(handler(&matched), "get book rust");
    }
    None => { /* 404 */ }
}
assert!(resolver.resolve("POST", "/books/rust").is_none());
```

See the `dynamic_routing` example for a fuller tour.

## Defining a hybrid static/dynamic resolver

A fixed set of built-in routes plus a set registered at run time can be served
through one [`EitherResolver`][__link8] (requires the `dynamic` feature): apply
`#[resolver(name = ...)]` to an enum to create a static resolver
and compose it with a [`DynResolver`][__link9] overlay. The
primary (static) resolver is tried first and the secondary (dynamic) is the
fallback:

```rust
use http_path_template::{Grammar, PathTemplate};
use routerama::{
    DynResolver, EitherResolver, HttpMethod, Resolver as _, Route, RouteMatch as _, resolver,
};

// The built-in routes; `#[resolver(name = ...)]` names the zero-sized `ApiResolver`.
#[resolver(name = ApiResolver)]
#[derive(Clone, Copy)]
enum Api<'p> {
    #[route(GET, "/books")]
    ListBooks,

    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
}

// The run-time overlay, on a disjoint subtree.
let plugins = DynResolver::new([Route::new(
    "Plugin",
    HttpMethod::Get,
    PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
)]);

let resolver = EitherResolver::new(ApiResolver, plugins);

// Built-ins resolve through the static core...
assert_eq!(
    resolver
        .resolve("GET", "/books/rust")
        .expect("a match")
        .name(),
    "GetBook"
);
// ...and plugin routes fall through to the dynamic overlay.
let plugin = resolver.resolve("GET", "/plugins/auth").expect("a match");
assert_eq!(plugin.name(), "Plugin");
assert_eq!(plugin.capture("name"), Some("auth"));
```

See the `hybrid_routing` example for a fuller tour.

## What the macro generates

`#[resolver(name = X)]` leaves your `enum` as written and generates, for it:

* a zero-sized resolver named `X`, implementing [`Resolver`][__link10]; its
  `resolve(method, path) -> Option<YourEnum>` scans the path and walks a
  compile-time trie, most-specific-first;

* a [`RouteMatch`][__link11] impl on the enum, exposing [`name`][__link12] and
  [`capture`][__link13].

Each capturing variant declares one `&'p str` field per `{capture}` (dotted
and reserved names are sanitized — `{item.id}` → `item_id`, `{type}` →
`_f_type`); the macro reports a clear error when a variant declares fields
that disagree with its path captures. The enum uses the lifetime `'p` for its
captures.

## Crate features

This crate has the following optional features:

* **`macros`** (default) — enables `#[resolver]`.

* **`build`** (default) — enables the build-time `Generator` code-generation
  API for use from a `build.rs`.

* **`dynamic`** (default) — enables the runtime resolver: a [`DynResolver`][__link14] built from a route set
  known only at run time, resolving identically to the static path, plus an [`EitherResolver`][__link15]
  that combines a static core with a dynamic overlay.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/routerama">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbKX-ImnsXunMb7-WKwVuZwy8bbcVlSpDUtfcbAFe5zWihOTdhZIKCaXJvdXRlcmFtYWUwLjEuMIJvcm91dGVyYW1hX2J1aWxkZTAuMS4w
 [__link0]: macro@resolver
 [__link1]: https://docs.rs/routerama/0.1.0/routerama/?search=DynResolver
 [__link10]: https://docs.rs/routerama/0.1.0/routerama/?search=Resolver
 [__link11]: https://docs.rs/routerama/0.1.0/routerama/?search=RouteMatch
 [__link12]: https://docs.rs/routerama/0.1.0/routerama/?search=RouteMatch::name
 [__link13]: https://docs.rs/routerama/0.1.0/routerama/?search=RouteMatch::capture
 [__link14]: https://docs.rs/routerama/0.1.0/routerama/?search=DynResolver
 [__link15]: https://docs.rs/routerama/0.1.0/routerama/?search=EitherResolver
 [__link2]: macro@resolver
 [__link3]: https://docs.rs/routerama_build/0.1.0/routerama_build/?search=Generator
 [__link4]: https://docs.rs/routerama/0.1.0/routerama/?search=DynResolver
 [__link5]: https://docs.rs/routerama_build/0.1.0/routerama_build/?search=Route
 [__link6]: https://docs.rs/routerama/0.1.0/routerama/?search=DynResolver::with_values
 [__link7]: https://docs.rs/routerama/0.1.0/routerama/?search=DynMatch::value
 [__link8]: https://docs.rs/routerama/0.1.0/routerama/?search=EitherResolver
 [__link9]: https://docs.rs/routerama/0.1.0/routerama/?search=DynResolver
