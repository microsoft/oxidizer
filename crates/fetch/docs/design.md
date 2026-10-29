# HTTP client memory

A memory provider allocates buffers for HTTP body data.
Callers select one through `custom::CustomDeps::memory` or `tokio::TokioDeps::memory`.
These fields take `bytesbuf::mem::OpaqueMemory`, which wraps any `MemoryShared` provider.
Client request builders and custom transport factories use the selected provider.
Transports can also accept request bodies that already own storage from another provider.

`TokioDeps::default`, `TokioDeps::with_clock`, and fake clients use the global pool.
These convenience defaults do not restrict clients built with explicit dependencies.
The bundled Hyper transport also accepts a custom body builder.

Cloning client dependencies clones their provider.
[Relocation](https://docs.rs/thread_aware/latest/thread_aware/#theory-of-operation) notifies the provider on its destination thread.
The provider decides whether its clones share state.
Transport isolation controls whether a thread gets a new handler or shares an existing handler.
Memory selection does not change response buffer limits, timeouts, cancellation, or connection management.
