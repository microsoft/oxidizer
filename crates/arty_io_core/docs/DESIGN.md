# Design

## Purpose

Arty does not own an I/O implementation. Libraries and applications inject
drivers, and the runtime hosts them. `arty_io_core` is the small agreement that
lets both sides evolve independently.

The contract follows four design rules:

1. Keep the semver chokepoint small.
2. Let providers own topology and threading.
3. Make graceful shutdown observable without making it a safety protocol.
4. Keep runtime policy out of driver signatures.

## Public model

```text
one registered driver type
        |
        v
DriverProvider
        |
        | clone, relocate, consume once per worker
        v
Driver + Context
        |
        +-- Parker for completion progress and waiting
        +-- optional SystemTaskSpawner use
        +-- optional provider-owned threads
```

`DriverProvider` is the cross-worker object. It can hold shared state used to
connect the instances it creates. The runtime only knows that each relocated
clone creates one `Driver`.

`Driver` is thread-local. Its `Context` is the mobile handle that consumers
hold. `ThreadAware` relocation lets the context optimize for the destination
worker, but correctness cannot depend on relocation being called.

Every `Driver` method takes `&self`. This does not make a driver `Sync`: the
runtime still invokes it only from its owning thread. Implementations use
thread-local interior mutability for state changes, which lets the runtime store
and erase drivers without wrapping them in a mutex solely for method access.

Every context implements `DriverContext`, whose associated `Provider` and
`provider()` function are the complete registration recipe. A runtime method
such as `get_context::<MyContext>()` therefore needs only the context type. The
first request constructs and registers the provider; later requests reuse it.

## Registration stays in the runtime

Lazy registration requires a type-keyed registry, synchronization between
workers, caching, and rollback after failed creation. None of these mechanisms
need to be shared with a driver, so none belong in this crate.

The contract enables lazy registration by making the context select a
self-contained provider. The runtime can store that provider when the context
type is first requested and use it to create all current or future worker
instances.

Different major versions of one driver crate naturally have different Rust
context types and `TypeId` values. They coexist as long as both versions use
the same `arty_io_core` contract.

## Execution and waiting

An I/O subsystem chooses its own execution strategy. Every driver exposes a
`Parker` through a shared reference. The runtime decides whether to integrate a
parker into an async worker's idle wait or drive it from another runtime-owned
thread. Internally, the driver may process completions there, delegate system
work, or coordinate with threads managed by its provider.

This avoids exposing primary or satellite roles as public API. Those are
placement choices the runtime may change later.

The `Parker` waker follows a strict latched contract. Without latching, a wake
between the runtime's final work check and the actual wait can be lost and the
worker can sleep forever.

## Shutdown

The reference design used an unsafe driver trait and an `is_inert` query to
prevent the runtime from dropping a driver while external code still referenced
its memory. This contract moves soundness back to the owning type:

- `Driver::Drop` is always safe.
- `begin_shutdown` stops new operations.
- `poll_shutdown` tracks graceful cleanup.

Contexts and in-flight operations own the state they can access through
reference-counted handles, pool leases, or equivalent safe ownership tokens.
The driver keeps its own owner while running. Shutdown closes admission and
waits until the external owners have drained before releasing the final owner.
Dropping early releases only the driver's owner; outstanding handles keep their
state alive.

This model supports high-performance implementations without putting unsafe
lifecycle obligations in the stable API. Drivers can build it from standard
reference counting or ecosystem storage such as `multitude`, `plurality`,
`infinity_pool`, and `rallocator`. The contract does not depend on those crates,
so implementations can evolve independently. Platform-specific unsafe code, if
needed, remains isolated behind the driver's private ownership types.

`Driver::shutdown` adapts the explicit begin-and-poll protocol into a future.
The lower-level methods remain available because a runtime will usually erase
unrelated drivers behind an internal object-safe shim and poll them alongside
executor shutdown.

## Creation failure

Creation is infallible at the type level. A provider that cannot initialize its
driver panics because the runtime cannot continue coherently with a driver
registered on only part of its worker set.

A driver with conditional platform or permission requirements exposes a
capability check. The consumer calls that check before
`get_context::<MyContext>()`, while it can still choose another context type.

## System tasks

Some I/O mechanisms need synchronous calls that cannot run on an async worker.
`SystemTaskSpawner` is intentionally narrower than an async scheduler:

- it accepts only synchronous `FnOnce` work;
- work is explicitly allowed to block;
- it returns after acceptance, not completion;
- it stays alive through driver shutdown.

The boxed task allocates on each submission. Removing that allocation without
making the trait non-object-safe or expanding the compatibility surface is
deferred until a real driver demonstrates that it matters.

## Compatibility

Every public non-standard type increases the chance that two otherwise
compatible drivers resolve to different copies of the contract. The initial API
therefore depends only on `thread_aware_core`, whose `Thread` and `ThreadAware`
types are essential for per-worker placement.

`arty_io_core` references those types directly but does not re-export them.
Driver authors depend on `thread_aware_core` when implementing relocation.
`arty_io_core` must not stabilize before `thread_aware_core`.

Future additions follow these rules:

- Add runtime facilities through private `DriverInit` fields and new accessors.
- Add trait methods only with compatible defaults when possible.
- Prefer standard-library types over ecosystem dependencies.
- Keep erasure, registration, and placement helpers private to the runtime.

## Deferred decisions

The initial API does not decide:

- how a runtime selects the driver that provides its waiting point;
- whether workers or drivers are pinned to processors;
- whether registrations cover existing workers atomically;
- how runtime shutdown timeouts are configured;
- whether a reusable latched-waker implementation belongs in a later utility
  crate;
- which memory pool, clock, or telemetry facilities drivers may eventually
  receive.
