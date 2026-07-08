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

Automatically transcode gRPC services to REST/JSON endpoints.

Services that expose a gRPC API often need to also expose a parallel REST API
either for compatibility or to enable REST-centric use cases. Implementing the same
API for both gRPC and REST is tedious, error-prone work, or requires additional machinery such
as a transcoding gateway.

The `rest_over_grpc` crate makes it possible to start with a service that implements
a gRPC API and have it also expose a REST version of the same API with minimal work.
The resulting REST endpoint is located in the same process as the gRPC endpoint and is built on the
same set of handlers, so there isn’t any extra network hop involved.

The way this works is that you start with the `.proto` file that describes your
gRPC API and add annotations to individual gRPC methods to indicate how these methods
should be mapped into a REST API surface. The annotations are an industry standard
defined by [`google.api.http`][__link0].
Once the annotations are in place, you use this crate to generate a transcoder that
converts from an incoming HTTP request to a call to a gRPC method handler function.

Given the generated transcoder, you need to wire it in to a web server that handles the
network communication. `rest_over_grpc` makes this easy by integrating with both the
[`tower`][__link1] and [`layered`][__link2]
crates, as well as having first class support for [`axum`][__link3].

## Architecture

A `rest_over_grpc` endpoint is a stack of three layers. An HTTP request flows
down through them to a handler, and the response flows back up:

```text
                         HTTP request │   ▲ HTTP response
                                      ▼   │
  ┌────────────────────────────────────────────────────────────┐
  │  Serving      bytes on and off the wire                    │
  │               tower · layered · axum · raw `http` · none   │
  └────────────────────────────────────────────────────────────┘
                      (method, target, │   ▲ TranscodeResponse
                       headers, body)  ▼   │
  ┌────────────────────────────────────────────────────────────┐
  │  Transcoding  the generated `Transcoder`: route match +    │
  │               JSON/protobuf conversion                     │
  └────────────────────────────────────────────────────────────┘
                    request message +  │   ▲ response message
                           `Context`   ▼   │ or `Status`
  ┌────────────────────────────────────────────────────────────┐
  │  Handling     your gRPC method handlers                    │
  │               tonic bridge · direct impl · other stack     │
  └────────────────────────────────────────────────────────────┘
```

* **Serving** turns wire bytes into the neutral call the transcoder takes
  — `(method, target, headers, body)` — and turns the
  [`TranscodeResponse`][__link4] it returns back into a
  wire response. You choose how much of this the crate
  does for you, from a full [`tower_service::Service`][__link5] down to invoking the
  transcoder yourself.

* **Transcoding** is done by the generated `Transcoder` — the piece this crate
  builds from your annotated `.proto`. It resolves the route, decodes the JSON
  request into the gRPC API’s protobuf message, invokes the gRPC method
  handler, and encodes the reply (or a [`Status`][__link6]) back to JSON.

* **Handling** is your service logic, reached through the generated per-API
  trait — implemented directly, or bridged from an existing gRPC service.

The three layers are composed independently to form a fully cohesive REST/gRPC transcoding stack. Read
below for more details on each layer.

## Usage

Let’s consider the three architectural layers mentioned above and the various options that exist
within each.

### Serving layer

The serving layer is responsible for bridging from the network to/from the transcoder. You
have the following options for this layer:

* **`tower`**: enable the `tower` feature and
  wrap the generated `Transcoder` with
  [`RestService::new`][__link7], yielding a
  [`RestService`][__link8] that implements
  [`tower_service::Service`][__link9]. Mount it like any other tower service; the
  service reads the body and headers, transcodes, and returns an
  [`http::Response`][__link10]. A single [`RestService`][__link11] serves
  both unary (buffered) and server-streaming (frames forwarded to the wire)
  RPCs. See `examples/tower_service.rs`.

* **`layered`**: enable the `layered` feature and use the same
  [`RestService::new`][__link12] /
  [`RestService`][__link13], which also implements
  [`layered::Service`][__link14]. See `examples/layered_service.rs`.

* **`axum`**: because `axum` is built on `tower`, the `tower`-feature
  [`RestService`][__link15] mounts directly — e.g.
  as a `fallback_service` or `nest_service` — with no `axum` feature needed
  (its response body is an [`http_body::Body`][__link16] type, so `axum` accepts it
  as-is). See `rest_over_grpc_examples`’s `examples/serving/axum_app.rs`. The
  separate `axum` feature covers the other direction: it implements
  [`IntoResponse`][__link17]
  for the neutral [`HttpResponse`][__link18] /
  [`StreamingResponse`][__link19] /
  [`TranscodeResponse`][__link20] value types, so a custom
  `async fn` handler that constructs those responses itself (mixing transcoded
  and hand-built replies) can `return` them directly rather than converting by
  hand. (The orphan rule means only this crate can add those impls.)

* **Another `http` / `http-body` server directly**: skip the `Service` wrapper
  and call [`serve_http`][__link21]
  (the `serving` feature, on by default) with your
  [`http::Request`][__link22] and the
  generated `Transcoder`; it collects the body and transcodes — useful in a
  raw `hyper` `service_fn`, one arm of an ad-hoc router, or a test. Its
  closure-taking sibling [`serve_http_fn`][__link23] suits a
  hand-written transcoder. See `examples/serve_http_fn.rs`.

* **No web stack, or custom body handling:** with the `serving` feature
  disabled (`default-features = false`), call the generated `transcode` /
  `try_transcode` yourself with
  `(method, target, headers, body)`. Do this when your input isn’t an
  [`http::Request`][__link24] (a `FaaS` event, a custom transport), or when you need to
  own the body step — size caps, header inspection, custom response headers —
  that the adapters bake in. See `rest_over_grpc_examples`’s
  `examples/serving/custom_body_handling.rs`.

### Transcoding layer

The `Transcoder` itself is generated for you from the annotated `.proto`;
what you choose at this layer is how it delivers responses and how it composes
with routes it does not own.

#### Server-streaming responses

A streaming handler is two-phase — an `async fn` taking
the request and [`Context`][__link25] and returning a [`Result`][__link26] of a
[`ResponseStream`][__link27] or a [`Status`][__link28] — so initiation
can fail with a status before any item is produced, and the yielded stream is
`'static` (build it with [`Box::pin`][__link29]). The response encoding is negotiated
from the request’s `Accept` header — a JSON array (the default for
`application/json`, `*/*`, or an absent header), newline-delimited JSON
(`application/x-ndjson`), or Server-Sent Events (`text/event-stream`).

The generated `transcode` / `try_transcode` return a
[`TranscodeResponse`][__link30]: a **unary** RPC yields a
buffered [`HttpResponse`][__link31], and a **server-streaming**
RPC yields a [`StreamingResponse`][__link32] that the
serving adapters ([`serve_http`][__link33] /
[`RestService`][__link34]) forward to the wire frame by frame as the
handler produces it. A failure observed after the headers are sent truncates
the body.

#### Custom routes and fallbacks

`transcode` defaults an unmatched route to a `404`. To compose the generated
routes with hand-written ones — a health check, a bespoke 404 page, a
catch-all — use `try_transcode` instead: it returns an [`Option`][__link35], where
[`None`][__link36] means *no generated route matched*, as opposed to a handler
returning [`Status::not_found`][__link37], which still yields `Some(404)`. Match on the
`None` case to fall through to your own handling; `transcode` is just the
convenience wrapper that turns `None` into a `404`. See
`rest_over_grpc_examples`’s `examples/transcoding/custom_fallback.rs`.

#### Error responses with details

A handler reports a domain failure by returning a [`Status`][__link38]; the transcoding
layer renders it into the JSON error body. Beyond a [`Code`][__link39] and message, a
status can carry a `google.rpc.Status`-style `details` array (arbitrary JSON
values), which the transcoder renders as
`{"code": …, "message": …, "details": [ … ]}`, matching the shape the
reference gateways emit.

### Handling layer

The generated `<Service>` trait has one method per RPC; how you implement it
depends on your gRPC stack:

* **`tonic`:** the [`build`][__link40] module emits a blanket
  `impl <Service> for T where T: <tonic server trait>` by default, so a
  service written once against tonic’s generated server trait also serves
  REST with no extra code — statuses, request/response metadata, and
  server-streaming responses are bridged for you. See the
  `rest_over_grpc_examples` crate’s `tonic_bridge` module for this path end to
  end.

* **Another framework (`volo`, `grpcio`, …) or none:** implement the
  generated trait directly, or — to reuse an existing framework service —
  write a small bridge that forwards each RPC and converts the framework’s
  request/response/status types. See `rest_over_grpc_examples`’s
  `examples/handling/volo_bridge.rs`.

Each generated method takes the decoded request message plus a mutable
[`Context`][__link41] reference: read the request headers (request-side metadata) from
it, and set custom response headers (`Location`, `ETag`, `Set-Cookie`, …) on
it. The transcoding layer merges those response headers into the returned
[`HttpResponse`][__link42] once the handler completes.

## Generated code

Let’s assume you’ve annotated your `.proto` file with the following:

```protobuf
service Greeter {
  // Unary: bind the `{name}` path segment into the request message.
  rpc SayHello(HelloRequest) returns (HelloReply) {
    option (google.api.http) = { get: "/v1/greet/{name}" };
  }

  // Server-streaming: a sequence of replies, encoded as a JSON array,
  // NDJSON, or Server-Sent Events per the request's `Accept` header.
  rpc StreamGreetings(HelloRequest) returns (stream HelloReply) {
    option (google.api.http) = { get: "/v1/greet/{name}:stream" };
  }
}
```

From that, `rest_over_grpc::build` (run from a `build.rs`; see the [`build`][__link43]
module) emits code shaped roughly like this — one service trait, a
`Transcoder`, and by default, a `tonic` bridge:

```rust
// 1. Handling layer — a service trait with one `async fn` per gRPC method, taking the
//    decoded request and a `&mut Context` and returning `Result<Reply, Status>`
//    (a server-streaming RPC returns a `ResponseStream`). You implement this
//    (or reach it through the `tonic` bridge below).
pub trait Greeter: Send + Sync {
    async fn say_hello(&self, request: HelloRequest, cx: &mut Context)
        -> Result<HelloReply, Status>;

    async fn stream_greetings(&self, request: HelloRequest, cx: &mut Context)
        -> Result<ResponseStream<HelloReply>, Status>;
}

// 2. Transcoding layer — a `Transcoder` generic over your handler(s). It owns
//    the router built from the annotations and implements `Transcode`.
pub struct Transcoder<S> { /* ... */ }

impl<S> Transcoder<S> {
    pub fn new(service: S) -> Self { /* ... */ }
}

impl<S: Greeter> rest_over_grpc::transcoding::Transcode for Transcoder<S> {
    // Routes `GET /v1/greet/{name}` to `Greeter::say_hello`, decoding the JSON
    // request and encoding the reply. Returns a `TranscodeResponse` (a buffered
    // unary reply or a server-streaming frame stream); `None` when no route
    // matches.
    async fn try_transcode(&self, method: &str, target: &str,
        headers: HeaderMap, body: &[u8]) -> Option<TranscodeResponse> { /* ... */ }
}

// 3. By default, a blanket `tonic` bridge: any `tonic`-generated server for
//    this service is a `Greeter` too, so one implementation serves gRPC and
//    REST with no extra code.
impl<T: greeter_server::Greeter> Greeter for T { /* forwards each RPC */ }
```

## Limitations

The transcoder targets **unary and server-streaming** RPCs with JSON-shaped
payloads. Two limits follow from that, worth knowing before you design an
API around it:

* **Client-streaming and bidirectional RPCs are unsupported.** They have no
  `google.api.http` mapping (an HTTP request is one message), so the
  [`build`][__link44] module rejects them at build time. Handle such an API —
  e.g. a chunked upload — with a dedicated HTTP handler that reads the body
  incrementally and bridges it to your native gRPC client-streaming call,
  composed alongside the transcoded JSON routes. See
  `rest_over_grpc_examples`’s `examples/handling/client_streaming_upload.rs`.

* **The request body is buffered as one JSON `&[u8]`.** The generated
  `transcode` decodes the whole body with `serde_json`, so there is no
  incremental request path, and binary payloads must be base64-encoded in
  JSON. This is fine for ordinary request messages but unsuitable for large
  or binary uploads — route those around the transcoder (see the upload
  example above).

## Crate features

The core (always available) provides status mapping, the neutral message
types, and — in [`codegen_helpers`][__link45] — the generated-router transcode
primitives and JSON⇄protobuf message coding (via `pbjson`). Feature-gated
modules add the rest:

* `serving` (default): the [`serve_http`][__link46] /
  [`serve_http_fn`][__link47] one-shot helpers that bridge an
  [`http::Request`][__link48] to a transcoder and return an [`http::Response`][__link49] whose body
  ([`RestBody`][__link50]) serves both unary (buffered) and
  server-streaming (frames forwarded to the wire) replies. Implied by
  `tower` / `layered`.

* `tower`: a [`tower_service::Service`][__link51] adapter
  ([`RestService`][__link52]); implies `serving`.

* `layered`: a [`layered::Service`][__link53] adapter (the repository’s `async fn`-based
  service trait), on the same [`RestService`][__link54]; implies `serving`.

* `axum`: an [`IntoResponse`][__link55]
  impl for [`HttpResponse`][__link56], [`StreamingResponse`][__link57], and
  [`TranscodeResponse`][__link58], so the neutral response types can be returned directly
  from an `axum` handler.

* `build`: provides the build-time code generator as the [`build`][__link59]
  module, for use from a `build.rs`.

Server-streaming response encodings — JSON array, NDJSON, and Server-Sent
Events — are always available (negotiated from the request’s `Accept` header),
alongside the [`ResponseStream`][__link60] handler return type;
the [`StreamingResponse`][__link61] /
[`TranscodeResponse`][__link62] path forwards each frame
to the wire as it is produced.

## Examples

Many runnable examples are provided in the source repository at the links below.

This crate’s own [`examples/`][__link63]
show the adapters end to end, driving a stand-in transcoder:

* [`tower_service.rs`][__link64]
  — wrap a neutral transcoder as a [`RestService`][__link65] for the `tower` ecosystem.
* [`layered_service.rs`][__link66]
  — the same over the repository’s `layered` `async fn` service trait.
* [`serve_http_fn.rs`][__link67]
  — call [`serve_http_fn`][__link68] directly, without the [`RestService`][__link69] wrapper.

The [`rest_over_grpc_examples`][__link70]
crate runs against a real generated transcoder, with examples grouped by the
architectural layer each exercises. The **serving** layer:

* [`tower_service.rs`][__link71]
  — wiring the transcoder into a `tower` / `hyper` / `axum` stack.
* [`axum_app.rs`][__link72]
  — mounting the generated transcoder in an `axum::Router` as a
  `fallback_service`, no handler glue required.
* [`streaming_response.rs`][__link73]
  — real server-streaming to the wire via `transcode` and
  [`serve_http`][__link74].
* [`custom_body_handling.rs`][__link75]
  — owning the body step yourself (size caps, header inspection, custom
  response headers) around the generated `transcode`.

The **transcoding** layer:

* [`basic_transcode.rs`][__link76]
  — the core `transcode` loop over a `tonic`-bridged service.
* [`custom_fallback.rs`][__link77]
  — hand-written routes and a custom 404 via `try_transcode`.

The **handling** layer:

* [`direct_service.rs`][__link78]
  — implementing the generated service trait directly, using [`Context`][__link79]
  for request/response metadata and returning [`Status`][__link80] errors with details.
* [`volo_bridge.rs`][__link81]
  — a hand-written bridge for a non-`tonic` gRPC stack.
* [`client_streaming_upload.rs`][__link82]
  — handling a client-streaming upload outside the transcoder.

Generating the service code from `google.api.http` rules is shown by this
crate’s [`build`][__link83] example:

* [`generate_service.rs`][__link84]
  — build the HTTP rules and emit the service trait + transcoder as a
  `TokenStream`.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbwn6pSfYHoLcbAEHeBM-he94bFEozNDrTho4bUhXmMID_F6NhZIWCZGh0dHBlMS40LjKCaWh0dHBfYm9keWUxLjAuMYJnbGF5ZXJlZGUwLjMuNYJucmVzdF9vdmVyX2dycGNlMC4xLjCCbXRvd2VyX3NlcnZpY2VlMC4zLjM
 [__link0]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
 [__link1]: https://crates.io/crates/tower
 [__link10]: https://docs.rs/http/1.4.2/http/?search=Response
 [__link11]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link12]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService::new
 [__link13]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link14]: https://docs.rs/layered/0.3.5/layered/?search=Service
 [__link15]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link16]: https://docs.rs/http_body/1.0.1/http_body/?search=Body
 [__link17]: https://docs.rs/axum-core/latest/axum_core/response/trait.IntoResponse.html
 [__link18]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::HttpResponse
 [__link19]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::StreamingResponse
 [__link2]: https://crates.io/crates/layered
 [__link20]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::TranscodeResponse
 [__link21]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http
 [__link22]: https://docs.rs/http/1.4.2/http/?search=Request
 [__link23]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http_fn
 [__link24]: https://docs.rs/http/1.4.2/http/?search=Request
 [__link25]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Context
 [__link26]: https://doc.rust-lang.org/stable/std/result/struct.Result.html
 [__link27]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::ResponseStream
 [__link28]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status
 [__link29]: https://doc.rust-lang.org/stable/std/?search=boxed::Box::pin
 [__link3]: https://crates.io/crates/axum
 [__link30]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::TranscodeResponse
 [__link31]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::HttpResponse
 [__link32]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::StreamingResponse
 [__link33]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http
 [__link34]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link35]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
 [__link36]: https://doc.rust-lang.org/stable/std/?search=option::Option::None
 [__link37]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status::not_found
 [__link38]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status
 [__link39]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Code
 [__link4]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::TranscodeResponse
 [__link40]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link41]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Context
 [__link42]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::HttpResponse
 [__link43]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link44]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link45]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/codegen_helpers/index.html
 [__link46]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http
 [__link47]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http_fn
 [__link48]: https://docs.rs/http/1.4.2/http/?search=Request
 [__link49]: https://docs.rs/http/1.4.2/http/?search=Response
 [__link5]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link50]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestBody
 [__link51]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link52]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link53]: https://docs.rs/layered/0.3.5/layered/?search=Service
 [__link54]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link55]: https://docs.rs/axum-core/latest/axum_core/response/trait.IntoResponse.html
 [__link56]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::HttpResponse
 [__link57]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::StreamingResponse
 [__link58]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::TranscodeResponse
 [__link59]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link6]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status
 [__link60]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::ResponseStream
 [__link61]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::StreamingResponse
 [__link62]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=transcoding::TranscodeResponse
 [__link63]: https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc/examples
 [__link64]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/tower_service.rs
 [__link65]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link66]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/layered_service.rs
 [__link67]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/serve_http_fn.rs
 [__link68]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http_fn
 [__link69]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link7]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService::new
 [__link70]: https://github.com/microsoft/oxidizer/tree/main/crates/rest_over_grpc_examples
 [__link71]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/tower_service.rs
 [__link72]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/axum_app.rs
 [__link73]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/streaming_response.rs
 [__link74]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::serve_http
 [__link75]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/serving/custom_body_handling.rs
 [__link76]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/transcoding/basic_transcode.rs
 [__link77]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/transcoding/custom_fallback.rs
 [__link78]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/handling/direct_service.rs
 [__link79]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Context
 [__link8]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=serving::RestService
 [__link80]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/?search=handling::Status
 [__link81]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/handling/volo_bridge.rs
 [__link82]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc_examples/examples/handling/client_streaming_upload.rs
 [__link83]: https://docs.rs/rest_over_grpc/0.1.0/rest_over_grpc/build/index.html
 [__link84]: https://github.com/microsoft/oxidizer/blob/main/crates/rest_over_grpc/examples/generate_service.rs
 [__link9]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
