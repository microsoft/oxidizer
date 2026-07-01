<div align="center">
 <img src="./logo.png" alt="Rest Over Grpc Build Logo" width="96">

# REST Over gRPC Build

[![crate.io](https://img.shields.io/crates/v/rest_over_grpc_build.svg)](https://crates.io/crates/rest_over_grpc_build)
[![docs.rs](https://docs.rs/rest_over_grpc_build/badge.svg)](https://docs.rs/rest_over_grpc_build)
[![MSRV](https://img.shields.io/crates/msrv/rest_over_grpc_build)](https://crates.io/crates/rest_over_grpc_build)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Build-time code generation that lowers `google.api.http`-annotated gRPC
services into a framework-neutral REST router.

This crate is intended to be called from a consumer’s `build.rs`. It is the
codegen half of the gRPC→REST transcoding system; the runtime half lives in
`rest_over_grpc`. It is deliberately *not* a proc-macro: codegen is driven from
external descriptors (`.proto` / `FileDescriptorSet` + HTTP annotations),
which a `build.rs` is far better suited to read than a macro.

## Pipeline

1. Describe each RPC’s HTTP binding with an [`HttpRule`][__link0] (mirroring
   [`google.api.HttpRule`][__link1]).
1. [`HttpRule::lower`][__link2] turns a rule (plus its `additional_bindings`) into one
   or more [`Route`][__link3]s, each pairing an HTTP method + parsed
   [`PathTemplate`][__link4] with its body / response-body
   configuration.
1. [`Router::new`][__link5] collects the routes for a service and
   [`Router::generate`][__link6] emits the static dispatch code as a
   [`TokenStream`][__link7].

The emitted router performs no runtime trie/regex construction: it is
straight-line generated Rust that matches the HTTP method and path segments
and reports the resolved RPC plus its captured path-variable bindings.

## Scope

Codegen handles unary RPCs: the HTTP-rule model, lowering (with
`additional_bindings`, `response_body`, and custom verbs), the static
router, the async service trait, and the request/response dispatcher, with
path/query/body binding wired through `rest_over_grpc::transcode`. Streaming
RPCs are rejected at codegen time; the streaming response encodings live in
`rest_over_grpc::stream`.

## Examples

Build an HTTP rule, lower it into routes, and generate a static router:

```rust
use rest_over_grpc_build::{HttpMethod, HttpRule, Router};

let routes = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}")
    .lower()
    .expect("the path template is valid");
let tokens = Router::new(routes).generate();

assert!(tokens.to_string().contains("pub fn resolve"));
```

To inspect a larger generated service,
`examples/generate_service.rs` builds [`HttpRule`][__link8]s, lowers them, and
pretty-prints the generated service trait + dispatcher:

```text
cargo run -p rest_over_grpc_build --example generate_service
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc_build">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQb5huSPz2LvVQbD3uC7_ss2G8bj4-sv-hzLK8bWP4jq-rf6LlhZIOCcmh0dHBfcGF0aF90ZW1wbGF0ZWUwLjEuMIJrcHJvY19tYWNybzJnMS4wLjEwNoJ0cmVzdF9vdmVyX2dycGNfYnVpbGRlMC4xLjA
 [__link0]: https://docs.rs/rest_over_grpc_build/0.1.0/rest_over_grpc_build/?search=HttpRule
 [__link1]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
 [__link2]: https://docs.rs/rest_over_grpc_build/0.1.0/rest_over_grpc_build/?search=HttpRule::lower
 [__link3]: https://docs.rs/rest_over_grpc_build/0.1.0/rest_over_grpc_build/?search=Route
 [__link4]: https://docs.rs/http_path_template/0.1.0/http_path_template/?search=PathTemplate
 [__link5]: https://docs.rs/rest_over_grpc_build/0.1.0/rest_over_grpc_build/?search=Router::new
 [__link6]: https://docs.rs/rest_over_grpc_build/0.1.0/rest_over_grpc_build/?search=Router::generate
 [__link7]: https://docs.rs/proc_macro2/1.0.106/proc_macro2/?search=TokenStream
 [__link8]: https://docs.rs/rest_over_grpc_build/0.1.0/rest_over_grpc_build/?search=HttpRule
