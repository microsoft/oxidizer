# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `rest_over_grpc`, the framework-neutral runtime for transcoding
  gRPC services into REST/JSON endpoints.
- `Code` plus `map_code_to_http` / `map_http_to_code` for translating between
  gRPC status codes and HTTP status codes.
- Generated-router support types: `split_path`, `Segments`, and `Binding`.
- `Status` and `HttpResponse`: neutral request/response value types returned by
  generated dispatchers.
- `transcode` module: serde-based JSON⇄message request/response transcoding
  (`decode_request`, `encode_response`, `BodyKind`, `ResponseBodyKind`,
  `status_response`, `not_found_response`), composing with `pbjson`-generated
  proto3-canonical serde.
- Query-string helpers `split_query` and `parse_query`.
- Web-stack adapters: `HttpResponse::into_http`, `adapter::transcode_http`, and
  `adapter::RestService`, bridging the neutral dispatcher to the `http`/`http-body`
  ecosystem. `RestService` implements both `tower::Service` (feature `tower`) and
  `layered::Service` (feature `layered`) over the same dispatcher.
- Examples: `tower_service` and `layered_service` demonstrate serving requests
  through each adapter.
