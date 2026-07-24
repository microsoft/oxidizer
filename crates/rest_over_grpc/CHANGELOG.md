# Changelog

## [0.1.1] - 2026-07-24

### Added

- Initial release of `rest_over_grpc`, the framework-neutral runtime for transcoding
  gRPC services into REST/JSON endpoints.
- `Code::to_http_status` and `Code::from_http_status` for translating between
  gRPC and HTTP status codes.
- Build-time generation through `Generator`, `ServiceDefinition`, `HttpRule`,
  and `Binding`.
- Framework-neutral handling and response types: `Status`, `Context`,
  `HttpResponse`, `StreamingResponse`, and `TranscodeResponse`.
- Serde-based JSON⇄message transcoding through `decode_request` and
  `encode_response`, using `RequestBodyKind` and `ResponseBodyKind` and
  composing with `pbjson`-generated proto3-canonical serde.
- Query-string helpers `split_query` and `parse_query`.
- Serving adapters `serve_http`, `serve_http_fn`, and `RestService`, bridging
  generated transcoders to the `http`/`http-body` ecosystem. `RestService`
  implements both `tower::Service` (feature `tower`) and `layered::Service`
  (feature `layered`).
- Runnable examples covering Tower, Axum, direct transcoding, streaming,
  OpenAPI generation, custom handlers, and gRPC-stack bridges.

- 🔧 Maintenance

  - Now requires `0.3.6` of `layered`

- ✨ Features

  - add REST transcoding for gRPC services ([#600](https://github.com/microsoft/oxidizer/pull/600))

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

