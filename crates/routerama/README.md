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

Blazingly fast HTTP route resolution and query string processing.

`routerama` resolves an HTTP method and path to a typed route, extracting
captured path variables. It is designed as a routing layer for HTTP
frameworks. It also provides query string processing, efficiently converting
an incoming query string into a useful Rust data structure.

## Route resolution

You describe every route your application serves as an enum annotated with
[`#[resolver]`][__link0]. Variants in the enum are either:

* **static** — annotated with `#[route(METHOD, "path")]` and compiled into
  the resolver.

* **dynamic** — unannotated, registered at run time through the generated
  builder, and restricted to owned fields.

```rust
use routerama::{ResolveError, resolver};

#[resolver]
enum BookRoute<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },

    #[route(GET, "/health")]
    Health,
}

let resolver = BookRoute::resolver();

match resolver.resolve("GET", "/books/rust") {
    Ok(BookRoute::GetBook { book }) => get_book(book),
    Ok(BookRoute::Health) => health(),
    Err(ResolveError::NotFound(path)) => not_found(path),
    Err(error) => bad_request(error),
}

fn get_book(_book: &str) {}
fn health() {}
fn bad_request(_error: ResolveError<'_>) {}
fn not_found(_path: &str) {}
```

### What gets generated

For an enum named `BookRoute`, Routerama generates a `BookRouteResolver`
implementing [`Resolver`][__link1]. Static-only enums get an infallible `resolver`
constructor. Enums containing dynamic routes instead get a
`BookRouteResolverBuilder` and an `add_<variant>(method, path)` method for
each dynamic variant. `#[resolver(name = ApiResolver)]` explicitly names
the resolver `ApiResolver` and its builder `ApiResolverBuilder`.
[`Resolver::resolve`][__link2] returns `Result<Route, ResolveError>`.

Each capturing variant declares one field per `{capture}`; the field type
controls conversion. Borrowing field types — `&'p str` (no allocation) and
`Cow<'p, str>` (percent-decoded) — are allowed only on **static** variants;
owned types — `String` (percent-decoded) and any `T: FromStr`
(decode-then-parse) — work on either kind. A dynamic route must use capture
names that are valid Rust identifiers matching its field names.

### Generated service dispatch

[`#[service]`][__link3] can make async handler methods the source of
truth and generate the route enum plus exhaustive dispatch:

```rust
struct BooksApi;
struct RequestContext;

#[routerama::service]
impl BooksApi {
    #[route(GET, "/books")]
    async fn list_books(&self, request: &RequestContext) -> &'static str {
        let _ = request;
        "books"
    }

    #[route(GET, "/books/{id}")]
    async fn get_book(&self, id: u32, request: &RequestContext) -> &'static str {
        let _ = (id, request);
        "book"
    }
}

let api = BooksApi;
let context = RequestContext;
assert_eq!(api.dispatch("GET", "/books/42", &context).await?, "book");
```

Use `#[service(context)]` when the first handler parameter after `&self`
should be forwarded as an owned, shared, or mutable context. Services with
`#[route(dynamic)]` handlers additionally generate a persistent router and
builder. See [`service`][__link4] for the complete contract and a mixed-route
example.

## Query string processing

You describe a query schema as a named-field struct deriving
[`FromQuery`][__link5],
[`ToQuery`][__link6],
or both. Its fields may be:

* **scalar** — required while decoding, with string forms handled directly
  and other values parsed through `FromStr` and produced through `Display`;

* **optional** — represented by `Option<T>` and omitted when absent;

* **repeated** — represented by `Vec<T>`, with one value per occurrence of
  the parameter; or

* **flattened** — another query type whose fields share the same query
  string.

```rust
use std::borrow::Cow;

use routerama::query::{FromQuery, ToQuery};

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(rename_all = "camelCase", deny_unknown_fields)]
struct SearchQuery<'q> {
    search_term: Cow<'q, str>,
    page: Option<u32>,

    #[query(rename = "tag")]
    tags: Vec<Cow<'q, str>>,
}

let query = SearchQuery::from_query("searchTerm=rust+language&page=2&tag=fast&tag=safe")?;
assert_eq!(query.search_term, "rust language");
assert_eq!(query.page, Some(2));
assert_eq!(query.tags, ["fast", "safe"]);

assert_eq!(
    query.to_query_string()?,
    "searchTerm=rust+language&page=2&tag=fast&tag=safe"
);
```

### What gets generated

`FromQuery` generates direct field dispatch and a function-local decoder
state. Parsing processes each `application/x-www-form-urlencoded` pair once,
in any order, while detecting missing or duplicate scalar values and
enforcing `query::QueryLimits`. Borrowed `&str` fields allocate nothing but
reject values that require decoding; `Cow<'q, str>` borrows unchanged input
and owns only decoded values.

`ToQuery` generates direct field encoding in declaration order, producing a
canonical parameter name for every field. Neither derive uses a run-time
field map, Serde, dynamic dispatch, or a generated top-level state type.

The `#[query(...)]` helper attributes rename fields, add decoding aliases,
provide defaults, flatten nested query types, skip fields, and reject
unknown parameters. The
[`FromQuery`][__link7]
and
[`ToQuery`][__link8]
derive pages document the complete attribute contract.

## Securing route resolution and query processing

Routerama applies defensive parsing checks and configurable query limits,
but applications must still enforce limits and validate values for their
specific environment. Take these precautions:

* Enforce an HTTP request-target size limit before resolution. Route
  resolution does not impose a total path-length limit.

* Pass a consistent URI path with the query removed. Routerama deliberately
  does not normalize dot segments, repeated slashes, or percent-encoded
  equivalents.

* Validate captures for their eventual use. A percent-decoded capture can
  contain `/` and is not inherently safe as a filesystem component.

* Discard streaming query output if encoding returns an error, because the
  destination struct may already contain a partial query string.

## Cargo features

* **`query`** — enabled by default. Exposes the `query` module and the
  `FromQuery` and `ToQuery` derive macros. Disable default features to build
  with routing support only.

## `no_std`

This crate is `#![no_std]` (it requires `alloc`). The generated resolvers run
on bare-metal targets; the `#[resolver]` macro expansion runs on the host and
requires `std`.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/routerama">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbmWTWA1L2sRMbV_7lnVOeH-UbOPaYM91svj0biHjKqME0YzZhZIKCaXJvdXRlcmFtYWUwLjEuMIJwcm91dGVyYW1hX21hY3Jvc2UwLjEuMA
 [__link0]: macro@resolver
 [__link1]: https://docs.rs/routerama/0.1.0/routerama/?search=Resolver
 [__link2]: https://docs.rs/routerama/0.1.0/routerama/?search=Resolver::resolve
 [__link3]: macro@service
 [__link4]: https://docs.rs/routerama_macros/0.1.0/routerama_macros/?search=service
 [__link5]: https://docs.rs/routerama/latest/routerama/query/derive.FromQuery.html
 [__link6]: https://docs.rs/routerama/latest/routerama/query/derive.ToQuery.html
 [__link7]: https://docs.rs/routerama/latest/routerama/query/derive.FromQuery.html
 [__link8]: https://docs.rs/routerama/latest/routerama/query/derive.ToQuery.html
