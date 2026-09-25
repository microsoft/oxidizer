# Stabilization Notes

These notes capture the pending decisions for stabilizing `bytesbuf_io`. They
describe proposals under review, not a finalized stable API.

## Proposed stable boundary

- Keep owned buffers as the fundamental I/O abstraction. Native asynchronous I/O
  may retain memory for the entire operation, which cannot be represented safely by
  an API based only on borrowed slices.
- Stabilize the core `Read` and `Write` traits after their required methods and
  result types have been reviewed.
- Allow stable downstream crates to expose approved `bytesbuf_io` types in their
  public APIs.

Extension traits, test utilities, ecosystem adapters, and other convenience APIs
should remain outside the stable boundary until each surface is reviewed separately.

## Trait requirements

The current `Read` and `Write` traits unconditionally require `HasMemory`, `Memory`,
and `Debug`, and use `trait_variant` to require `Send` from implementations and
returned futures. Review each requirement instead of carrying it into the stable API
by default. Changing these requirements is a breaking API change that must be settled
before 1.0:

- Remove `HasMemory`, `Memory`, and `Debug` supertraits where they are not essential
  to the I/O contract. Memory providers can be exposed through separate capabilities
  or adapters when an implementation benefits from endpoint-specific allocation.
  Extension methods that currently rely on these inherited capabilities must be
  adjusted as part of the same decision.
- Decide whether `Send` is required by the traits, by each returned future, or only
  at usage sites. Removing the current requirement changes guarantees available to
  downstream callers. The stable contract should not impose cross-thread execution
  when an implementation and its caller are both thread-local.
- Select the concrete error boundary. The associated error type and the crate's
  general-purpose `Error` wrapper must provide predictable composition without
  exposing unstable implementation details.

## Read and write operations

The current read surface includes bounded reads, implementation-selected reads, and
allocation of a new buffer. Review which operations are fundamental and which can be
implemented as extensions without losing endpoint-specific efficiency.

The `(usize, BytesBuf)` read result communicates both progress and ownership but is
easy to use incorrectly. Consider an intentional result type that names the bytes
read and returned buffer while preserving efficient ownership transfer.

The write contract should continue to make full writes explicit. Partial-write,
flush, shutdown, cancellation, and end-of-stream semantics must be either defined by
the stable traits or explicitly left to adapters and endpoint-specific APIs.

## Ecosystem adapters

Borrowed-buffer APIs such as Tokio and `futures` I/O should be supported through
adapters rather than define the fundamental abstraction. Adapter documentation must
state when data is copied, when an allocation is required, and how cancellation
returns or releases an owned buffer.

Dynamic or type-erased adapters should also be evaluated. Stabilization must account
for their allocation, dispatch, pinning, and error-erasure costs, as well as whether
they compose with both thread-local and `Send` implementations.

## Pending review

- [ ] Review the trait shape against `arty_io`, TCP, and fetch use cases.
- [ ] Explicitly list every API included in the stable surface.
- [ ] Resolve the `HasMemory`, `Memory`, and `Debug` supertrait requirements.
- [ ] Decide where `Send` requirements belong.
- [ ] Decide the stable I/O error representation.
- [ ] Confirm which read variants are fundamental.
- [ ] Decide whether read operations return a named result type.
- [ ] Define cancellation, end-of-stream, and partial-write semantics.
- [ ] Document compatibility adapters and their allocation and composition costs.
- [ ] Confirm stable signatures expose only approved stable types.
