# HTTP body memory implementation

`HttpBodyBuilder::new` accepts `impl Into<OpaqueMemory>` and converts it once.
`HttpBodyBuilder` stores the resulting `OpaqueMemory` directly.
This keeps its public type independent of the caller's provider type.
The builder's `Memory` implementation forwards reservations to that stored provider.
Its `HasMemory` implementation returns a clone of the provider.

`OpaqueMemory` owns the provider and forwards cloning and thread-aware relocation.
The builder must not replace those operations with a shared wrapper.
Providers can keep independent state in each clone.

`new_fake` passes a `GlobalPool` directly to `new`, which converts it to `OpaqueMemory`.
It also supplies a frozen clock.
The body representations, streaming policies, and timeout wrapper do not depend on the provider's concrete type.
