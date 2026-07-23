<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Routerama Logo" width="96">

# Routerama

[![crate.io](https://img.shields.io/crates/v/routerama.svg)](https://crates.io/crates/routerama)
[![docs.rs](https://docs.rs/routerama/badge.svg)](https://docs.rs/routerama)
[![MSRV](https://img.shields.io/crates/msrv/routerama)](https://crates.io/crates/routerama)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

HTTP routing, response composition, and query/form processing.

Routerama exposes independent, feature-gated modules:

* [`resolve`][__link0] provides typed static and runtime-configured path resolution.
* [`response`][__link1] provides HTTP body types and typed response composition.
* [`route`][__link2] provides generated handler dispatch and request extraction.
* [`query`][__link3] provides bounded query-string decoding and encoding.

`route` enables `response`. The additive `json`, `form`, `mount`, and
`tower` features add bounded JSON/form extraction, erased runtime services,
and a `tower_service::Service` adapter. `bytesbuf` preserves fragmented
`BytesView` request and response data and supports caller-provided-memory
templates under `no_std + alloc`; `bytesbuf-std` adds `GlobalPool` and
standard-I/O JSON decoding. No features are enabled by default, and the
crate root re-exports no feature-specific API.

## Example

```rust
use routerama::response::Body;
use routerama::route::{Request, State, StatusCode, router};

#[derive(Clone)]
struct AppState(&'static str);

struct Api;

#[router(state = AppState)]
impl Api {
    #[route(GET, "/books/{id}")]
    async fn book(&self, id: u32, state: State<AppState>) -> String {
        format!("{}:{id}", state.0.0)
    }

    #[fallback]
    async fn fallback(&self, failure: routerama::route::RouteFailure<'_>) -> StatusCode {
        failure.status()
    }
}

let request = Request::get("/books/42")
    .body(Body::empty())
    .expect("valid request");
let response = Api.route(request, &AppState("main")).await;
assert_eq!(response.status(), StatusCode::OK);
```

See each module and the crate’s runnable examples for extraction,
predicates, dynamic routes, interceptors, mounted services, and transport
integration.

## `no_std`

Routerama is `#![no_std]` and uses `alloc` where owned storage is required.
Procedural macros execute on the host. Features that depend on HTTP response
types enable their required `std` support.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/routerama">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbfuEDZgiWDbwbTVU5z5hrTn8bnheLMBL-n5YbTi8NOUJA_Z1hZIGCaXJvdXRlcmFtYWUwLjEuMA
 [__link0]: https://docs.rs/routerama/0.1.0/routerama/resolve/index.html
 [__link1]: https://docs.rs/routerama/0.1.0/routerama/response/index.html
 [__link2]: https://docs.rs/routerama/0.1.0/routerama/route/index.html
 [__link3]: https://docs.rs/routerama/0.1.0/routerama/query/index.html
