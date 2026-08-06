# Stabilization Notes

These notes capture the pending decisions for stabilizing `bytesbuf`. They
describe proposals under review, not a finalized stable API.

## Proposed stable boundary

Stabilize the core owned byte-sequence types:

- `BytesBuf` for assembling and owning writable byte sequences.
- `BytesView` for sharing and consuming immutable byte sequences.
- The iterator and cursor types returned by their stable operations:
  `BytesBufRemaining`, `BytesBufVectoredWrite`, and `BytesViewSlices`.
- `MemoryGuard` for keeping memory alive while native or unsafe I/O uses it.

The stable operation groups should cover:

- Creating empty buffers and views, reserving capacity, and querying length,
  capacity, and emptiness.
- Appending copied bytes or existing views, then consuming or peeking buffered
  data as immutable views.
- Slicing, ranging, concatenating, advancing, and iterating over view contents.
- Accessing unfilled buffer memory, including vectored access, and committing
  the number of bytes initialized by an owned-buffer read operation.
- Standard I/O and `bytes` crate adapters where their Cargo features are
  enabled.

## Memory-provider boundary

Stabilize the provider contracts needed by consumers and I/O implementations:

- `Memory` for reserving owned writable capacity.
- `MemoryShared` and `HasMemory` for sharing an endpoint-compatible provider.
- `GlobalPool` as the standard pooled provider.
- `OpaqueMemory` for storing a provider without exposing its concrete type.

The provider surface must preserve ownership, reuse, and cross-thread release
semantics needed by asynchronous operating-system I/O.

## Deferred surface

Keep convenience providers, test utilities, constants, and low-level block and
reference-counting machinery outside the stable surface unless the provider
implementation review proves they are required. This includes
`CallbackMemory`, `Block`, `BlockRef`, their metadata and vtable APIs, and
`MAX_INLINE_SPANS`.

If stable custom memory providers require any of these types, expose only the
smallest reviewed construction boundary instead of stabilizing implementation
details by default.

## Pending review

- [ ] Explicitly list and review every method included for `BytesBuf` and
      `BytesView`.
- [ ] Confirm the writable-slice and vectored-write APIs support the approved
      owned-buffer Read/Write design without exposing uninitialized memory
      unsafely.
- [ ] Validate ownership transfer, reuse, cloning, range, and conversion
      semantics for asynchronous I/O.
- [ ] Decide whether `BytesBufWriter` belongs in the stable standard-library
      adapter surface.
- [ ] Decide which `bytes` compatibility implementations are stable.
- [ ] Determine the minimal stable API for implementing custom memory providers.
- [ ] Confirm stable downstream crates expose no deferred `bytesbuf` types.
- [ ] Identify all remaining APIs as stable, unstable, or unnecessary for 1.0.
