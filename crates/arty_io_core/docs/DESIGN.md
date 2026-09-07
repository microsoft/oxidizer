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
        +-- optional Parker for the worker's wait
        +-- optional BlockingTaskSpawner use
        +-- optional provider-owned threads
```

`DriverProvider` is the cross-worker object. It can hold shared state used to
connect the instances it creates. The runtime only knows that each relocated
clone creates one `Driver`.

`Driver` is thread-local. Its `Context` is the mobile handle that consumers
hold. `ThreadAware` relocation lets the context optimize for the destination
worker, but correctness cannot depend on relocation being called.

## Registration stays in the runtime

Lazy registration requires a type-keyed registry, synchronization between
workers, caching, and rollback after failed creation. None of these mechanisms
need to be shared with a driver, so none belong in this crate.

The contract enables lazy registration by making the provider self-contained.
The runtime can store a provider when a type is first requested and use it to
create all current or future worker instances.

Different major versions of one driver crate naturally have different Rust
types and `TypeId` values. They coexist as long as both versions use the same
`arty_io_core` contract.

## Execution and waiting

An I/O subsystem chooses its own execution strategy.

A driver that can efficiently combine completion processing with the async
worker's idle wait may expose `Parker`. The runtime offers this opportunity
through `DriverInit::waiting_point`. If unavailable or declined, the driver
uses runtime blocking workers or threads managed by its provider.

This avoids making the runtime spawn one thread per additional driver. It also
avoids exposing primary or satellite roles as public API. Those are placement
choices the runtime may change later.

The parker's `Waker` follows a strict latched contract. Without latching, a wake
between the runtime's final work check and the actual wait can be lost and the
worker can sleep forever.

## Shutdown

The reference design used an unsafe driver trait and an `is_inert` query to
prevent the runtime from dropping a driver while external code still referenced
its memory. This contract moves soundness back to the owning type:

- `Driver::Drop` is always safe.
- `begin_shutdown` stops new operations.
- `poll_shutdown` tracks graceful cleanup.

If a driver cannot safely release a resource yet, dropping it retains that
resource instead of invalidating it. This can leak during abrupt termination,
but it cannot cause undefined behavior.

`Driver::shutdown` adapts the explicit begin-and-poll protocol into a future.
The lower-level methods remain available because a runtime will usually erase
unrelated drivers behind an internal object-safe shim and poll them alongside
executor shutdown.

## Creation errors

Creation is fallible because lazy registration may discover unavailable
platform features, missing permissions, or exhausted resources after the
runtime has already started. A typed `Result` lets the requesting library choose
another implementation or surface a useful error.

The runtime owns the harder transactional problem. If one worker fails after
others succeeded, it begins shutdown for the created instances and reports the
provider error only after rollback is safe.

## Blocking tasks

Some I/O mechanisms need synchronous calls that cannot run on an async worker.
`BlockingTaskSpawner` is intentionally narrower than an async scheduler:

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

Those types are re-exported from `arty_io_core`, along with `Owner` and
`NumaNode`, so driver authors do not need to name a second package in public
signatures. `arty_io_core` must not stabilize before `thread_aware_core`.

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
