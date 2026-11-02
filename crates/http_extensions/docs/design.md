# HTTP body memory

A memory provider allocates buffers for body data.
Callers select the provider when they create an `HttpBodyBuilder`.
The constructor takes `bytesbuf::mem::OpaqueMemory`, which wraps any `MemoryShared` provider.
`new_fake` uses the global pool and a frozen clock.

The builder uses the provider for copied slices, text, JSON, and explicit memory reservations.
Bodies built from existing byte views keep their existing storage.
The provider can return more capacity than requested without changing the body's data length.
Streaming bodies keep their configured buffer limits and timeouts.

Cloning a builder clones its provider.
[Relocation](https://docs.rs/thread_aware/latest/thread_aware/#theory-of-operation) notifies the provider on its destination thread.
Each provider controls whether its clones share state.
