<div align="center">
 <img src="./logo.png" alt="Rest Over Grpc Logo" width="96">

# REST Over gRPC

[![crate.io](https://img.shields.io/crates/v/rest_over_grpc.svg)](https://crates.io/crates/rest_over_grpc)
[![docs.rs](https://docs.rs/rest_over_grpc/badge.svg)](https://docs.rs/rest_over_grpc)
[![MSRV](https://img.shields.io/crates/msrv/rest_over_grpc)](https://crates.io/crates/rest_over_grpc)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Framework-neutral runtime primitives for transcoding gRPC services into
REST/JSON endpoints.

This crate is the runtime half of a gRPC→REST transcoding system. It is
deliberately decoupled from any particular web stack (hyper, axum, tonic,
tower, …): it knows nothing about sockets, bodies, or executors. Instead it
provides the small, allocation-conscious building blocks that generated code
(emitted by the companion `rest_over_grpc_build` crate from `google.api.http`
annotations) plugs into:

* [`Code`][__link0] and [`map_code_to_http`][__link1] / [`map_http_to_code`][__link2] translate between
  gRPC status codes and HTTP status codes following the conventions used by
  the reference gRPC-Gateway and Google API gateways.
* [`Status`][__link3] and [`HttpResponse`][__link4] are the neutral request/response value
  types that generated dispatchers return.
* The [`transcode`][__link5] module provides serde-based JSON⇄message request/response
  transcoding, and the dispatch primitives ([`scan_segments`][__link6],
  [`RouteMatch`][__link7], [`Binding`][__link8], …) back the generated static router.

Path-template parsing (the `google.api.http` pattern grammar such as
`shelves/{shelf}/books/{book=**}`) lives in the separate `http_path_template`
crate. A generated router needs it only at build time: `rest_over_grpc_build`
lowers the parsed templates into a static match tree, so no template parsing
or matching happens at runtime.

## Features

The core (always available) provides status mapping, the neutral message
types, the generated-router dispatch primitives, and JSON⇄protobuf message
coding (via `pbjson`) in [`transcode`][__link9]. Feature-gated modules add the rest:

* `tower`: a [`tower_service::Service`][__link10] adapter ([`adapter::RestService`][__link11]).
* `layered`: a [`layered::Service`][__link12] adapter (the repository’s `async fn`-based
  service trait), on the same [`adapter::RestService`][__link13].
* `streaming`: server-streaming response encodings — JSON array, NDJSON, and
  Server-Sent Events ([`stream`][__link14]).

## Examples

Decode captured path/query values into a request type, then encode the
response as JSON:

```rust
use rest_over_grpc::Binding;
use rest_over_grpc::transcode::{BodyKind, ResponseBodyKind, decode_request, encode_response};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct GetShelf {
    shelf: String,
    theme: String,
}

let bindings = [Binding::new(&["shelf"], "7")];
let request: GetShelf =
    decode_request(&bindings, &[("theme", "history")], b"", BodyKind::None)?;

assert_eq!(request.shelf, "7");
assert_eq!(request.theme, "history");

let body = encode_response(&request, ResponseBodyKind::Whole)?;
let value: serde_json::Value = serde_json::from_slice(&body)?;
assert_eq!(value["shelf"], "7");
assert_eq!(value["theme"], "history");
```

See `examples/tower_service.rs` and `examples/layered_service.rs` for
feature-gated programs that wrap a neutral dispatcher as an
[`adapter::RestService`][__link15] and serve `http::Request`s through the `tower` and
`layered` ecosystems.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbAH2OBSwtoaQb4nB7b656fN8bZ-VLpf8ib_obkzWsxNntj_RhZIOCZ2xheWVyZWRlMC4zLjWCbnJlc3Rfb3Zlcl9ncnBjZTAuMS4wgm10b3dlcl9zZXJ2aWNlZTAuMy4z
 [__link0]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=Code
 [__link1]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=map_code_to_http
 [__link10]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link11]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=adapter::RestService
 [__link12]: https://docs.rs/layered/0.3.5/layered/?search=Service
 [__link13]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=adapter::RestService
 [__link14]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/stream/index.html
 [__link15]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=adapter::RestService
 [__link2]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=map_http_to_code
 [__link3]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=Status
 [__link4]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=HttpResponse
 [__link5]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/transcode/index.html
 [__link6]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=scan_segments
 [__link7]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=RouteMatch
 [__link8]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=Binding
 [__link9]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/transcode/index.html
