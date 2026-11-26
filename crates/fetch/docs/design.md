# HTTP client memory

A memory provider allocates buffers for HTTP body data.
The application selects one and passes it through
`custom::CustomDeps::memory` or `tokio::TokioDeps::new`.
These fields take `bytesbuf::mem::OpaqueMemory`, which wraps any `MemoryShared` provider.
Client request builders and custom transport factories use the selected provider.
Transports can also accept request bodies that already own storage from another provider.

`TokioDeps` and the bundled Hyper transport require the application-owned
provider. They do not create a global pool.
Fake clients can receive a provider through `FakeDeps::memory`. Their default
test support reuses the test binary's shared provider.

Cloning client dependencies clones their provider.
[Relocation](https://docs.rs/thread_aware/latest/thread_aware/#theory-of-operation) notifies the provider on its destination thread.
The provider decides whether its clones share state.
Transport isolation controls whether a thread gets a new handler or shares an existing handler.
Memory selection does not change response buffer limits, timeouts, cancellation, or connection management.
