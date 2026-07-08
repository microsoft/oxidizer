<div align="center">
 <img src="./logo.png" alt="Rest Over Grpc Logo" width="96">

# Rest Over Grpc

[![crate.io](https://img.shields.io/crates/v/rest_over_grpc.svg)](https://crates.io/crates/rest_over_grpc)
[![docs.rs](https://docs.rs/rest_over_grpc/badge.svg)](https://docs.rs/rest_over_grpc)
[![MSRV](https://img.shields.io/crates/msrv/rest_over_grpc)](https://crates.io/crates/rest_over_grpc)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Automatically transcode gRPC services to REST/JSON endpoints.

`rest_over_grpc` generates REST routes from `google.api.http` annotations in
your `.proto` files. The generated REST surface runs in the same process as
the gRPC service, so you can reuse the same handlers without a separate
gateway hop.

The crate is organized around three layers:

* **Serving**: adapt network I/O to the transcoder and back.
* **Transcoding**: match routes, decode JSON into protobuf messages, and
  encode replies or [`Status`][__link0] errors.
* **Handling**: implement the generated service trait directly or bridge an
  existing gRPC stack into it.

### Quick start: bridge an existing `tonic` service

The normal setup generates protobuf messages, proto3-JSON serde
implementations, and the REST layer from the same descriptor set.

1. Annotate the service:

```text
syntax = "proto3";
package library;

import "google/api/annotations.proto";

service Library {
  rpc GetShelf(GetShelfRequest) returns (Shelf) {
    option (google.api.http) = {
      get: "/v1/shelves/{shelf}"
    };
  }
}

message GetShelfRequest {
  string shelf = 1;
}

message Shelf {
  string name = 1;
}
```

2. In `build.rs`, compile one descriptor set through `tonic-prost-build`,
   `pbjson-build`, and `rest_over_grpc`. The REST generator does not generate
   message types or serde implementations itself:

```text
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut compiler = protox::Compiler::new(["proto"])?;
    compiler.include_imports(true);
    compiler.include_source_info(true);
    compiler.open_file("library.proto")?;
    let descriptors = compiler.encode_file_descriptor_set();

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true)
        .build_transport(false)
        .compile_fds(compiler.file_descriptor_set())?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptors)?
        .build(&[".library"])?;
    rest_over_grpc::build::compile_fds(
        &descriptors,
        std::env::var("OUT_DIR")?,
    )?;
    Ok(())
}
```

Add `rest_over_grpc` with features `build,tower` plus `protox`,
`tonic-prost-build`, and `pbjson-build` as build dependencies. Generated
pbjson code also requires the corresponding serde/runtime dependencies.
The [worked example manifest][__link1] lists a complete set.

3. Include the generated files. Message, serde, and service-trait output
   belong in the proto package module; the top-level transcoder is included
   beside that module:

```text
pub mod library {
    include!(concat!(env!("OUT_DIR"), "/library.rs"));
    include!(concat!(env!("OUT_DIR"), "/library.serde.rs"));
    include!(concat!(env!("OUT_DIR"), "/library.rest.rs"));
}

mod rest {
    use super::library;
    include!(concat!(env!("OUT_DIR"), "/transcoder.rest.rs"));
}
```

4. Implement the generated `tonic` server trait as usual, then wrap that
   implementation in the generated transcoder:

```rust
#[derive(Clone)]
struct LibraryService;

#[tonic::async_trait]
impl library::library_server::Library for LibraryService {
    async fn get_shelf(
        &self,
        request: tonic::Request<library::GetShelfRequest>,
    ) -> Result<tonic::Response<library::Shelf>, tonic::Status> {
        let shelf = request.into_inner().shelf;
        Ok(tonic::Response::new(library::Shelf {
            name: format!("shelves/{shelf}"),
        }))
    }
}

let transcoder = rest::Transcoder::new(LibraryService);
let service = rest_over_grpc::serving::RestService::new(transcoder)
    .with_max_body_bytes(1 << 20);
```

The tonic bridge is emitted by default; call
[`Generator::builder`][__link2] with
[`emit_tonic_bridge(false)`][__link3] when
implementing the generated REST trait directly. See the [complete build
script][__link4], [generated includes][__link5], and [tonic handler][__link6] for versions that compile.

### Serving

Pick the integration that fits your stack:

* `tower`: wrap a generated transcoder with [`RestService::new`][__link7] to get a [`tower_service::Service`][__link8].
* `layered`: the same [`RestService`][__link9] also implements [`layered::Service`][__link10].
* `axum`: the `tower`-based [`RestService`][__link11] mounts directly in `axum`; with the `axum` feature, the neutral response types also implement [`IntoResponse`][__link12].
* direct HTTP: call [`serve_http`][__link13] or [`serve_http_fn`][__link14] yourself.
* custom transport: disable `serving` and call [`transcode`][__link15] / [`try_transcode`][__link16] with `(method, target, headers, body)`.

### Transcoding

Generated transcoders support unary and server-streaming RPCs. Unary calls
return a buffered [`HttpResponse`][__link17]; server-streaming
calls return a [`StreamingResponse`][__link18] whose
frames are forwarded as they are produced.

Server-streaming response encoding is negotiated from `Accept`: JSON array
(`application/json`, `*/*`, or absent), NDJSON (`application/x-ndjson`), or
Server-Sent Events (`text/event-stream`).

Use [`transcode`][__link19] when unmatched routes
should become `404`; use [`try_transcode`][__link20]
when you want to fall back to custom routing.

### Handling

The generated `<Service>` trait has one method per RPC, each taking the
decoded request plus a mutable [`Context`][__link21].

* `tonic`: the [`build`][__link22] module emits a blanket bridge so a `tonic`
  implementation can serve REST too.
* direct implementation: implement the generated trait yourself.
* other gRPC stacks: write a small bridge that forwards into the generated
  trait.

Server-streaming methods return a [`ResponseStream`][__link23],
and handlers report failures with [`Status`][__link24]. Use
`Context` for request metadata and to set response headers.

### Limitations

The crate supports unary and server-streaming RPCs only. Client-streaming
and bidirectional RPCs have no `google.api.http` mapping and are rejected by
[`build`][__link25].

Requests are buffered and parsed as JSON, so there is no incremental request
body path and binary payloads must fit JSON-friendly encoding.

### Features

* `serving` (default): [`serve_http`][__link26], [`serve_http_fn`][__link27], and [`RestBody`][__link28].
* `tower`: [`RestService`][__link29] as a [`tower_service::Service`][__link30].
* `layered`: [`RestService`][__link31] as a [`layered::Service`][__link32].
* `axum`: `IntoResponse` for [`HttpResponse`][__link33], [`StreamingResponse`][__link34], and [`TranscodeResponse`][__link35].
* `build`: the build-time code generator module.
* `build-openapi`: `build` plus OpenAPI 3.1 document generation.

`tower` and `layered` imply `serving`. The `axum` feature only adds
`IntoResponse`; enable `tower` as well to mount [`RestService`][__link36]
as an Axum fallback service.

### Examples

The [example index][__link37] maps common tasks to runnable examples. It covers
end-to-end generation, serving, direct transcoding, custom fallback,
streaming, OpenAPI, direct handlers, `tonic` bridging, and non-`tonic`
bridges. [`generate_service.rs`][__link38] demonstrates the lower-level manual
`HttpRule` API; annotation-driven generation is shown in the [complete build
script][__link39].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG6VKA6VrAhPqG09DdGLOEO1iG1kN_tWCLOWQGyLpr4ebASIAYWSDgmdsYXllcmVkZTAuMy41gm5yZXN0X292ZXJfZ3JwY2UwLjEuMIJtdG93ZXJfc2VydmljZWUwLjMuMw
 [__link0]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status
 [__link1]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/Cargo.toml
 [__link10]: https://docs.rs/layered/0.3.5/layered/?search=Service
 [__link11]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link12]: https://docs.rs/axum-core/latest/axum_core/response/trait.IntoResponse.html
 [__link13]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http
 [__link14]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http_fn
 [__link15]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::Transcode::transcode
 [__link16]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::Transcode::try_transcode
 [__link17]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::HttpResponse
 [__link18]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::StreamingResponse
 [__link19]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::Transcode::transcode
 [__link2]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=build::Generator::builder
 [__link20]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::Transcode::try_transcode
 [__link21]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Context
 [__link22]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link23]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::ResponseStream
 [__link24]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status
 [__link25]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link26]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http
 [__link27]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http_fn
 [__link28]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestBody
 [__link29]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link3]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=build::GeneratorBuilder::emit_tonic_bridge
 [__link30]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link31]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link32]: https://docs.rs/layered/0.3.5/layered/?search=Service
 [__link33]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::HttpResponse
 [__link34]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::StreamingResponse
 [__link35]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::TranscodeResponse
 [__link36]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link37]: https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc_examples#examples
 [__link38]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/generate_service.rs
 [__link39]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/build.rs
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/build.rs
 [__link5]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/src/tonic_bridge.rs
 [__link6]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/src/tonic_bridge.rs
 [__link7]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService::new
 [__link8]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link9]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
