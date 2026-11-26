# HTTP client memory implementation

`CustomDeps` and the internal `Transport` store `OpaqueMemory`.
`create_body_builder` clones that provider into each `HttpBodyBuilder`.
The client uses these builders for request bodies and pipeline services.
Each transport factory receives a builder through `CustomContext`.
This path keeps one provider choice across client and transport construction without adding provider type parameters.

The Tokio constructor copies `TokioDeps::memory` into `CustomDeps`.
Its handler passes the context's body builder to `HyperTransportBuilder`.
The standalone Hyper builder requires a caller-supplied body builder.
Fake constructors copy `FakeDeps::memory`. The test-only default clones the
shared provider from `HttpBodyBuilder::new_fake`.

Thread-aware derives include the memory fields.
The isolated transport path relocates its stored dependencies before it creates a handler on a new thread.
The shared transport path retains its shared handler while the client's own body builder follows client relocation.
`OpaqueMemory` forwards cloning and relocation to the caller's provider.

`HttpClient::reserve` borrows its stored body builder directly.
It does not acquire an owned provider for each reservation.
`HttpClient::memory` returns an independent owned provider for callers that need one.
Callers can retain that result across allocations.
