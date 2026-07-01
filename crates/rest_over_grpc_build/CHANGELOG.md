# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of `rest_over_grpc_build`, the build-time code generator that
  lowers `google.api.http`-annotated gRPC services into a static REST router.
- `HttpRule` model and `HttpRule::lower`, turning a binding (plus its
  `additional_bindings`) into `Route`s pairing an HTTP method + parsed
  `PathTemplate` with their body / response-body configuration.
- `Router::generate`, emitting a static `resolve` dispatcher (no runtime
  trie/regex) as a `proc_macro2::TokenStream`, suitable for use from a
  consumer's `build.rs`.
- `Service` / `ServiceMethod`: generation of a framework-neutral async service
  trait plus an async `dispatch` function that resolves a request, transcodes
  path/query/body into the request message, invokes the trait, and transcodes
  the response back to JSON.
- `services_from_descriptor` (feature `descriptor`): reads `google.api.http`
  annotations from a compiled `FileDescriptorSet` via reflection and builds the
  `Service`s, removing the need to hand-write `HttpRule`s.
- Vendored `google.api` annotation protos (`HTTP_PROTO`, `ANNOTATIONS_PROTO`,
  `write_annotation_protos`) so consumers need not source them separately.
