# `rest_over_grpc` worked examples

These examples use one annotated `library.proto` service and cover the three
layers of `rest_over_grpc`: generating handlers, transcoding requests, and
serving HTTP.

The build script demonstrates both supported handler models:

- **tonic bridge**: generate tonic messages and its server trait, add pbjson
  serde implementations, then let `rest_over_grpc` emit the REST trait,
  transcoder, and default tonic bridge.
- **direct handler**: generate prost messages and pbjson serde implementations,
  disable the tonic bridge, and implement the generated REST trait directly.

See [`build.rs`](build.rs) for generation and
[`src/tonic_bridge.rs`](src/tonic_bridge.rs) /
[`src/custom.rs`](src/custom.rs) for the generated `include!` layout.

## Examples

| Task | Example |
|---|---|
| Call a generated transcoder directly | [`basic_transcode`](examples/transcoding/basic_transcode.rs) |
| Add custom routes and fallback behavior | [`custom_fallback`](examples/transcoding/custom_fallback.rs) |
| Mount a generated service in Tower | [`tower_service`](examples/serving/tower_service.rs) |
| Mount it in Axum and return neutral responses from handlers | [`axum_app`](examples/serving/axum_app.rs) |
| Observe server-streamed response frames | [`streaming_response`](examples/serving/streaming_response.rs) |
| Enforce custom content-type and body-size policy | [`custom_body_handling`](examples/serving/custom_body_handling.rs) |
| Implement the generated REST trait directly | [`direct_service`](examples/handling/direct_service.rs) |
| Bridge another gRPC framework | [`volo_bridge`](examples/handling/volo_bridge.rs) |
| Route client-streaming uploads outside the JSON transcoder | [`client_streaming_upload`](examples/handling/client_streaming_upload.rs) |
| Inspect the generated OpenAPI document | [`openapi_document`](examples/build/openapi_document.rs) |

Run any example from the workspace root:

```console
cargo run -p rest_over_grpc_examples --example basic_transcode
cargo run -p rest_over_grpc_examples --example axum_app
cargo run -p rest_over_grpc_examples --example openapi_document
```

The examples execute requests in-process so they remain deterministic; the Axum
example also shows the two lines needed to replace `oneshot` with a TCP listener.
