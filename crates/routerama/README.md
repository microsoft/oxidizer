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

Blazingly fast HTTP routing, response composition, and query/form processing.

Routerama exposes four feature-gated capabilities through canonical
modules:

* [`resolve`][__link0] provides
  typed method-and-path resolution and the
  [`resolve::resolver`][__link1]
  macro.
* [`response`][__link2]
  provides standalone HTTP response bodies and typed response composition.
* [`route`][__link3] provides HTTP
  request extraction, generated dispatch, and the
  [`route::router`][__link4]
  macro. Its additive `mount` feature exposes explicitly erased runtime
  services under `routerama::route::mount`.
* [`query`][__link5] provides
  query string codecs and derives.

`response`, `resolve`, and `query` can be selected independently. `route`
implies `response`; the additive `mount` feature implies `route` without
adding another dependency. The additive `json` feature implies `route` and adds
bounded `routerama::route::json` request decoding. The additive `form`
feature implies both `route` and `query` and adds bounded
`routerama::route::form` decoding through the existing query codec. The
additive `tower` feature implies `route` and adds only the `tower-service`
trait crate, exposing the transport adapter under
`routerama::route::tower`. No capability is enabled by default, and no types
or macros are re-exported at the crate root.

## Typed resolution

```rust
use routerama::resolve::{ResolveError, resolver};

#[resolver]
enum BookRoute<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/health")]
    Health,
}

let resolver = BookRoute::resolver();
assert!(matches!(
    resolver.resolve("GET", "/books/rust"),
    Ok(BookRoute::GetBook { book: "rust" })
));
assert!(matches!(
    resolver.resolve("GET", "/missing"),
    Err(ResolveError::NotFound("/missing"))
));
```

## Handler routing

```rust
use http_body_util::BodyExt as _;
use routerama::response::Body;
use routerama::route::{
    FromRequestParts, HeaderMap, Method, Request, RequestParts, RouteFailure, State,
    StatusCode, TextBody,
};
struct UserAgent<'request>(&'request str);
impl<'request, S: ?Sized> FromRequestParts<'request, S> for UserAgent<'request> {
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &'request RequestParts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .map(Self)
            .ok_or(StatusCode::BAD_REQUEST)
    }
}
#[derive(Clone)]
struct AppState {
    label: &'static str,
}
struct BooksApi;
#[routerama::route::router(state = AppState)]
impl BooksApi {
    #[route(
        POST,
        "/books/{id}",
        host = "library.example",
        consumes = "text/plain",
        produces = "text/plain"
    )]
    async fn update_book(
        &self,
        method: Method,
        id: u32,
        #[body] title: TextBody<1024>,
        headers: &HeaderMap,
        user_agent: UserAgent<'_>,
        state: State<AppState>,
    ) -> (StatusCode, HeaderMap, String) {
        assert_eq!(
            user_agent.0.as_ptr(),
            headers["user-agent"].as_bytes().as_ptr()
        );
        let mut response_headers = HeaderMap::new();
        response_headers.insert(
            "x-library",
            state.label.parse().expect("static header value is valid"),
        );
        (
            StatusCode::ACCEPTED,
            response_headers,
            format!("{method} {id}: {} ({})", title.as_str(), user_agent.0),
        )
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
        failure.status()
    }
}
let api = BooksApi;
let state = AppState { label: "central" };
let request = Request::builder()
    .method("POST")
    .uri("/books/42")
    .header("host", "LIBRARY.EXAMPLE")
    .header("content-type", "text/plain; charset=utf-8")
    .header("accept", "text/*")
    .header("user-agent", "routerama-docs")
    .body(Body::from("Rust"))
    .expect("static request metadata is valid");
let response = api.route(request, &state).await;
assert_eq!(response.status(), StatusCode::ACCEPTED);
let body = response
    .into_body()
    .collect()
    .await
    .expect("the generated body succeeds")
    .to_bytes();
assert_eq!(body, b"POST 42: Rust (routerama-docs)"[..]);
```

Request metadata references are zero-copy and remain valid through the
handler’s `.await` because generated dispatch owns the request parts until
the handler completes. Owned `Uri` and `HeaderMap` parameters are explicit
clones; `Version` is copied and `Method` is cloned. Custom borrowed
extractors use `FromRequestParts<'request, S>` and handler-side `'_`, never
a macro-generated lifetime name.

Bare `#[routerama::route::router]` keeps `route<B, S>(request, &S)`
generic for reusable services. Adding `state = AppState` removes `S` from
the generated method, accepts only `&AppState`, and validates every
state-dependent parts/body extractor and `State<T>` projection when the
service is defined. Custom body extractors provide one compile-time
`route::BodyStateWitness` because their actual transport body remains a
call-specific type parameter. These witnesses add no request-time work;
fixed and generic routes use the same direct extraction and dispatch body.

Handler routes may add `host`, `consumes`, and `produces` string arguments.
They run in that order before extraction, rejecting with 404, 415, and 406
respectively. Host matching prefers URI authority over `Host`; consumes
validates one parameter-tolerant `Content-Type`; produces performs
specificity- and quality-aware `Accept` negotiation and replaces the
invoked handler response’s `Content-Type`. Routes without predicates emit
no predicate work. See [the `route`
module][__link6] for exact
authority, media, alias, dynamic-handler, and response-mutation semantics.
Exact method/template overlaps are opt-in: every candidate declares a
distinct integer `priority`, compatible captures, and is tried from highest
priority to lowest by its predicates before any extractor runs.
`#[fallback]` customizes typed
[`route::RouteFailure`][__link7]
values, while
`#[catch(RejectionType)]` customizes exact built-in extractor rejections and
`#[catch(RejectionType, from = ExtractorType)]` associates a custom
extractor explicitly. Policy futures and concrete streaming bodies remain
unboxed and need not be `Send`; services without policy annotations retain
the ordinary direct arm with no candidate or policy work.

Response tuples keep every body concrete. Metadata values implement
`routerama::response::IntoResponseParts`, whose typed error converts
independently through `routerama::response::IntoResponse`. In `(first, second, body)`, `body` is converted first, then `second`, then `first`;
this preserves leftmost-wins status/header overrides. The first failure in
that application order short-circuits and discards the partially composed
success response.
Success and rejection bodies use unboxed `routerama::response::EitherBody`
sums. `routerama::response::BoxBody` is used only when an application
requests that explicit dynamic boundary. One rejection path is exempt: a
router that is generic over its state cannot name the rejection of an
uncaught request-parts extractor, so that rejection response converts once
through `routerama::response::SendBoxBody`. That uncaught response body
must therefore be `Send + 'static` and its error `Send + Sync + 'static`.
Naming the rejection with `#[catch(RejectionType)]`, or fixing the state
with `#[router(state = T)]`, preserves local bodies and keeps that path
unboxed too; success paths never box.

## Explicitly erased mounted services

The additive `mount` feature supports runtime-chosen handler
implementations under `routerama::route::mount`. Its names expose the
erasure boundary: an `ErasedMountService<B, S>` is registered in an
immutable `ErasedMountRouter<B, S>` by HTTP method and path template.
Invalid methods/templates and conflicting mounted shapes fail at startup.
Raw captures are zero-copy URI slices retained through precomputed offsets;
explicit decoded and typed access performs decoding/parsing only when
requested.

This is distinct from a generated `#[route(dynamic)]` handler. A generated
dynamic handler is still statically known and called directly; only its
aliases are registered at startup. A mounted handler is genuinely open and
therefore makes one service-vtable call, boxes one service future, and
converts its concrete response body once through `response::BoxBody`.
Neither boundary requires `Send`.

`#[router(state = S, erased_mounts)]` explicitly generates the
`route_with_erased_mounts` integration entry; merely enabling the feature
does not alter ordinary generated services. Generated static and
configured-dynamic routes have deterministic precedence. Only a complete
generated miss reaches mounts; capture/predicate/extractor failures do not
fall through. The mount table is the final backstop: if it also misses, the
result is a plain `404 Not Found` and a generated custom `#[fallback]` is
not invoked. Its response body is
`EitherBody<GeneratedBody, BoxBody>`, so a generated hit neither boxes its
body nor invokes a mounted service. The original `route` entry and all
ordinary generated services remain unchanged. See the runnable
`mounted_services` example and the `route::mount` module for ownership,
alias, failure, lifetime, and exact cost contracts.

## Generated interceptors

`#[before]`, `#[after]`, and `#[transform]` methods add generated
request/response interceptors and terminal request-body transforms that the
macro calls directly, with no boxed future or service and no per-request
allocation.

A bare `#[before]` is router-wide: it runs before route resolution, owns the
whole request head, may rewrite the URI, and also guards mounted delegation.
A `#[before(handler, ...)]` runs after route selection and takes a split
request head that keeps the URI readable, so a guard composes with zero-copy
borrowed path captures.

A bare `#[after]` observes **every response this router generates** —
handler responses, `#[before]`/`#[transform]` short-circuits, extractor
rejections and `#[catch]` responses, predicate rejections, and routing
failures or `#[fallback]` responses — but not a response produced by a
mounted service, whose request head is moved into that service. An
`#[after(handler, ...)]` observes only its own handlers’ responses.

`#[transform]` is the terminal request-body owner in one of two explicit
modes: `#[transform(limit = N, ...)]` collects a bounded buffer, while
`#[transform(stream, ...)]` is generic over the transport body and returns a
wrapper the macro substitutes into the handler’s `#[body]` extraction, so
decompression- or signature-style middleware never buffers. Either way,
single-consumer body ownership stays a compile-time property. See the
runnable `interceptors` example and
[the `route` module][__link8] for
ordering, scope, short-circuit, and body-ownership rules.

The runnable `auth_tracing` example composes the two cross-cutting concerns
most services need first: a router-wide `#[before]` authenticates and
inserts a typed principal a handler borrows, while a bare `#[after]` records
the status of every generated response inside a request span carried through
the typed extensions. Interceptors are ordinary `async` methods, so a span
is never entered across the handler’s await; it is stored, entered
synchronously with `Span::in_scope`, and attached to the handler’s own
future with `Instrument::instrument`. The library itself emits no telemetry
and depends on no telemetry crate.

## Tower transport

The additive `tower` feature exposes `routerama::route::tower`, a
[`tower_service::Service`][__link9]
adapter for any routing call: a generated static entry, a configured
dynamic or mixed router, the mount integration entry, or a standalone
`ErasedMountRouter`. It adds only the `tower-service` trait crate and needs
no macro attribute, so nothing about code generation changes.

```rust
use std::sync::Arc;

use routerama::response::Body;
use routerama::route::tower::RouteService;
use routerama::route::{Request, StatusCode, router};

pub struct Api;

#[router(state = ())]
impl Api {
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

pub fn service()
-> impl tower_service::Service<Request<Body>, Error = core::convert::Infallible> + Clone {
    RouteService::new(
        Arc::new(Api),
        (),
        |api: Arc<Api>, (): (), request: Request<Body>| async move { api.route(request, &()).await },
    )
    .send_boxed_body()
}
```

Tower’s associated future type cannot borrow the service, so the adapter
hands the callable owned clones of the router and state; applications pick
their own sharing strategy and the adapter adds no `Arc`, trait object, or
boxed future of its own. Routing has no backpressure, so readiness is
honestly always ready, and the service error type is `Infallible` because
every routing failure is already a response. Body errors remain body errors.

Service-level auto traits are inherited rather than imposed. The explicit
response boundary pays the transport body’s requirements: the default keeps
the router’s own unboxed body, while `send_boxed_body()` and
`local_boxed_body()` each add one allocation to normalize it into the
nameable `routerama::response::SendBoxBody` or
`routerama::response::BoxBody`. Select that boundary once; applying it to an
already boxed body deliberately nests another erasure. See the runnable
`tower_service` example and
[the `route::tower`
module][__link10] for the
complete ownership, readiness, and cost contracts.

## Query strings

```rust
use routerama::query::{FromQuery, ToQuery};

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct SearchQuery {
    q: String,
    page: Option<usize>,
}

let query = SearchQuery::from_query("q=rust+language&page=2")?;
assert_eq!(query.q, "rust language");
assert_eq!(query.to_query_string()?, "q=rust+language&page=2");
```

## Form bodies

`form` reuses `routerama::query::FromQuery` for explicitly bounded
`application/x-www-form-urlencoded` request bodies. Because the buffered
text is temporary, form schemas must own all data decoded from it:

```rust
use routerama::query::FromQuery;
use routerama::route::form::Form;
use routerama::route::router;

#[derive(FromQuery)]
struct Registration {
    name: String,
    newsletter: Option<bool>,
    topic: Vec<String>,
}

struct Registrations;

#[router]
impl Registrations {
    #[route(POST, "/registrations")]
    async fn register(&self, #[body] form: Form<Registration, 1024>) -> String {
        form.into_inner().name
    }
}
```

See the runnable `form` example for complete request dispatch.

## Runnable examples

Every example in the crate’s `examples/` directory asserts its own
behavior and exits non-zero when it changes. Each declares the smallest
feature set it needs, so `cargo run -p routerama --example <name> --features <feature>` also demonstrates which capability owns it.

|Example|Features|Capability|
|-------|--------|----------|
|`routing`|`resolve`|static resolution and capture coercion|
|`dynamic_routing`|`resolve`|routes registered at run time|
|`hybrid_routing`|`resolve`|static and dynamic routes in one resolver|
|`query_strings`|`query`|`FromQuery` and `ToQuery` codecs|
|`response_composition`|`response`|status, headers, extensions, and fallible response metadata|
|`request_metadata`|`route`|borrowed and owned request metadata, typed extensions|
|`request_predicates`|`route`|`host`, `consumes`, and `produces` route predicates|
|`route_policy`|`route`|overlap priority, typed fallback, extractor catcher|
|`required_state`|`route`|`#[router(state = ...)]`, `State<T>`, `FromRef`|
|`streaming_responses`|`route`|response frames, trailers, and body errors|
|`interceptors`|`route`|`#[before]`, `#[after]`, and both `#[transform]` modes|
|`web_app`|`query`, `route`|typed query extraction behind an HTTP transport|
|`json_api`|`json`|bounded JSON bodies and their rejections|
|`form`|`form`|bounded `x-www-form-urlencoded` bodies|
|`mounted_services`|`mount`|explicitly erased runtime services|
|`tower_service`|`tower`|the router as a `tower_service::Service`|
|`auth_tracing`|`tower`|an authentication guard and request-span correlation|

## `no_std`

Routerama is `#![no_std]` and uses `alloc` for capabilities that require
owned storage. Procedural macro expansion runs on the host. The `response`
feature, and therefore `route` and `form`, enables the `std` support
selected for its HTTP types; featureless, `resolve`-only, and `query`-only
builds do not.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/routerama">source code</a>.
</sub>

 [__link0]: https://docs.rs/routerama/latest/routerama/resolve/
 [__link1]: https://docs.rs/routerama/latest/routerama/resolve/attr.resolver.html
 [__link10]: https://docs.rs/routerama/latest/routerama/route/tower/
 [__link2]: https://docs.rs/routerama/latest/routerama/response/
 [__link3]: https://docs.rs/routerama/latest/routerama/route/
 [__link4]: https://docs.rs/routerama/latest/routerama/route/attr.router.html
 [__link5]: https://docs.rs/routerama/latest/routerama/query/
 [__link6]: https://docs.rs/routerama/latest/routerama/route/
 [__link7]: https://docs.rs/routerama/latest/routerama/route/enum.RouteFailure.html
 [__link8]: https://docs.rs/routerama/latest/routerama/route/
 [__link9]: https://docs.rs/tower-service/latest/tower_service/trait.Service.html
