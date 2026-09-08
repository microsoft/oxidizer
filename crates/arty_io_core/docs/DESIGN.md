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
        +-- completion processing and wake-up
        +-- optional SystemTasks use
        +-- optional provider-owned threads
```

`DriverProvider` is the cross-worker object. It can hold shared state used to
connect the instances it creates. The runtime only knows that each relocated
clone creates one `Driver`.

`Driver` is thread-local. Its `Context` is the mobile handle that consumers
hold. `ThreadAware` relocation lets the context optimize for the destination
worker, but correctness cannot depend on relocation being called.

The runtime owns each driver exclusively, so completion processing and shutdown
receive `&mut self`. This lets a driver mutate thread-local state directly
without adding synchronization or dynamic borrow checks solely to satisfy the
contract.

`Driver` is dyn-compatible once its `Context` associated type is specified, so
a runtime can store `Box<dyn Driver<Context = C>>` and invoke every lifecycle
method through the trait object.

Every context implements `DriverContext`, whose associated `Provider` and
`provider()` function are the complete registration recipe. A runtime method
such as `get_context::<MyContext>()` therefore needs only the context type. The
first request constructs the provider, initializes its driver on every active
worker, and returns only after all workers acknowledge completion. Later
requests reuse the registered driver and return the cached context for the
calling worker.

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

An I/O subsystem chooses its own execution strategy. Before calling
`DriverProvider::create`, the runtime chooses the thread that will own the
driver. Completion processing and shutdown polling run only on that thread.
Internally, the driver may process completions there, delegate system work, or
coordinate with threads managed by its provider.

This avoids exposing primary or satellite roles as public API. Those are
placement choices the runtime may change later.

The driver's waker follows a strict latched contract. Without latching, a wake
between the runtime's final work check and the actual wait can be lost and the
worker can sleep forever.

## Shutdown

The reference design used an unsafe driver trait and an `is_inert` query to
prevent the runtime from dropping a driver while external code still referenced
its memory. This contract moves soundness back to the owning type:

- `Driver::Drop` is always safe.
- `begin_shutdown` idempotently stops new operations.
- `poll_shutdown` reports graceful cleanup progress.

The runtime records the lifecycle transition before invoking the driver, but
idempotence keeps duplicate calls from becoming an implicit safety precondition
on runtime control flow. Polling shutdown also begins it when necessary, so call
ordering is not a safety or liveness precondition either.

Contexts remain usable as closed handles after shutdown and therefore do not
participate in the drain count. In-flight operations and operating-system
callbacks own the state they can access through reference-counted handles, pool
leases, or equivalent safe ownership tokens. Shutdown closes admission and
waits for those active owners to drain.

An operating system may retain only a raw pointer rather than an ownership
token. A pooled implementation therefore keeps the pool's owner alive
independently of `Driver::Drop`, releasing it after a clean drain and retaining
it on premature drop. Dropping a driver closes admission before releasing or
retaining this owner.

This model supports high-performance implementations without putting unsafe
lifecycle obligations in the stable API. Drivers can build it from standard
reference counting or ecosystem storage such as `multitude`, `plurality`,
`infinity_pool`, and `rallocator`. The contract does not depend on those crates,
so implementations can evolve independently. Platform-specific unsafe code, if
needed, remains isolated behind the driver's private ownership types.

The runtime begins shutdown for every driver before polling any one of them.
While a driver reports `Pending`, the runtime continues bounded completion
processing. A shutdown timeout may terminate graceful cleanup, but it never
changes whether dropping the driver is memory-safe.

## Creation failure

Creation is infallible at the type level. A provider that cannot initialize its
driver panics because the runtime cannot continue coherently with a driver
registered on only part of its worker set.

A driver with conditional platform or permission requirements exposes a
capability check. The consumer calls that check before
`get_context::<MyContext>()`, while it can still choose another context type.

## System tasks

Some I/O mechanisms need synchronous calls that cannot run on an async worker.
`SystemTasks` is intentionally narrower than an async scheduler:

- it accepts only synchronous `FnOnce` work;
- work is explicitly allowed to block;
- it returns after acceptance, not completion;
- it stays alive through driver shutdown.

The cloneable handle hides the runtime's shared-ownership mechanism instead of
exposing `Arc<dyn ...>` in `DriverInit`. Its generic `spawn` method boxes only
at the internal callback boundary.

## Compatibility

Every public non-standard type increases the chance that two otherwise
compatible drivers resolve to different copies of the contract. The initial API
therefore depends only on `thread_aware_core`, whose `Thread` and `ThreadAware`
types are essential for per-worker placement.

`arty_io_core` references those types directly but does not re-export them.
Driver authors depend on `thread_aware_core` when implementing relocation.
`arty_io_core` must not stabilize before `thread_aware_core`.

Future additions follow these rules:

- Add optional runtime facilities through private `DriverInit` fields, defaults
  in the existing constructor, and new accessors.
- Adding a new mandatory constructor input is a breaking change and requires
  explicit stabilization review.
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
