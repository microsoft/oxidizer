# Design

## Purpose

Arty does not provide an I/O implementation. Libraries and applications supply
drivers, and the runtime hosts them. `arty_io_core` defines the shared contract
that lets runtimes and drivers evolve independently.

The contract follows four design rules:

1. Keep the shared, versioned API small.
2. Let providers own topology and threading.
3. Make graceful shutdown observable without making it a safety protocol.
4. Keep runtime policy out of driver signatures.

## Public model

```text
one registered driver type
        |
        v
IoContext::provider(ProviderOptions)
        |
        | clone, relocate, consume once per worker
        v
DriverProvider::create(DriverOptions)
        |
        +-- Driver owned by this worker
        +-- IoContext returned to consumers
        +-- construction-time access to earlier drivers on this worker
        +-- completion processing and wake-up
        +-- optional SystemTaskSpawner use
        +-- optional provider-owned threads
```

`DriverProvider` is the cross-worker factory. It may hold state shared by the
drivers it creates. Each relocated provider clone creates one `(Driver,
IoContext)` pair.

`Driver` is thread-local. Its associated `IoContext` is the movable handle held
by consumers. `ThreadAware` relocation may optimize a context for its
destination worker, but correctness cannot depend on relocation.

The runtime owns each driver exclusively, so completion processing receives
`&mut self`. This lets a driver mutate thread-local state directly without
adding synchronization or dynamic borrow checks solely to satisfy the contract.

`Driver` is dyn-compatible. Runtimes that erase unrelated context types use a
private owning shim to store the context beside its driver, which is also where
they adapt the by-value `shutdown` method to boxed storage.

Every consumer context implements `IoContext`. Its associated `Provider` and
`provider(ProviderOptions)` function form the registration recipe. A runtime
method such as `get_context::<MyContext>()` therefore needs only the context
type. The first request creates the provider and initializes a driver and
context on every active worker. Later requests reuse that registration.

`ProviderOptions` is the runtime-to-provider extension point. It is empty in the
initial contract. `DriverOptions` is the separate per-worker extension point
passed to `DriverProvider::create`; it identifies the owning worker and exposes
runtime facilities needed by the driver. It also carries type-erased handles for
drivers registered earlier on that worker. This lets a new driver discover and
connect to compatible local drivers without moving thread-local driver state or
exposing the runtime's registry.

## Registration stays in the runtime

Lazy registration requires a type-keyed registry and coordination between
workers. These are runtime concerns and do not belong in the shared contract.

The contract enables lazy registration by making the context select a
self-contained provider. The runtime can store that provider when the context
type is first requested and use it to create all current or future worker
instances.

Before creating a driver on a worker, the runtime collects handles from drivers
whose registration has already completed on that worker. The current driver is
not included, and handle order is runtime-defined. These are immutable,
construction-time references: the returned `'static` driver cannot retain them,
but it can downcast a compatible handle and clone independently owned shared
state. Because a handle may refer to a thread-local driver, `DriverOptions`
remains on the worker that assembled it.

After creating and storing the new driver and context, the runtime calls
`Driver::on_peer_registered` on every earlier driver in registration order. Each
callback receives the new driver's type-erased handle and runs on the owning
worker before registration is acknowledged. This makes discovery bidirectional
without allowing either side to retain a borrowed driver reference.

Different major versions of a driver crate have distinct context types and
`TypeId` values. They can coexist when both versions use the same
`arty_io_core` contract.

## Execution and waiting

An I/O subsystem chooses its own execution strategy. Before calling
`DriverProvider::create`, the runtime chooses the thread that will own the
driver. Completion processing and blocking shutdown run only on that thread.
Internally, the driver may process completions there, delegate system work, or
coordinate with threads managed by its provider.

At the start of each completion-processing cycle, the runtime captures one
`Instant` and passes that same value to every driver it visits. A shared snapshot
lets drivers compare deadlines consistently without later drivers observing
time advanced merely because they were scheduled later in the cycle. It is not
a completion timestamp or a general runtime clock service.

Primary and satellite roles remain runtime placement choices rather than public
driver roles.

The driver's waker follows a latched contract. Without latching, a wake-up
between the runtime's final work check and the wait could be lost. A
non-blocking completion pass does not consume the latch; the next call that may
block observes it. A wake-up changes only the wait behavior, so pending
completions are still processed.

## Shutdown

The safety contract is:

- Dropping a driver is always safe.
- `Driver::shutdown` consumes the driver.
- Shutdown closes admission and blocks until cleanup completes or fails.
- Failure is reported through `ShutdownError`.

Contexts remain usable as closed handles after shutdown and therefore do not
participate in the drain count. In-flight operations and operating-system
callbacks own the state they can access through reference-counted handles, pool
leases, or equivalent safe ownership tokens. Shutdown closes admission and
waits for those active owners to drain.

An operating system may retain only a raw pointer rather than an ownership
token. A pooled implementation therefore keeps the pool's owner alive
independently of the driver, releasing it after a clean drain and retaining it
on premature drop. Dropping a driver closes admission before releasing or
retaining this owner.

This model supports high-performance implementations without putting unsafe
lifecycle obligations in the stable API. Drivers may use standard reference
counting, pool leases, or private ownership types. Platform-specific unsafe
code remains isolated inside the driver implementation.

The runtime removes a driver from its normal completion loop and transfers
ownership into `shutdown`. The call performs whatever completion processing,
waiting, cancellation, and cleanup the implementation requires. `SystemTaskSpawner`
remains available until the call returns. The driver owns the liveness policy
for this blocking phase and returns an error instead of waiting indefinitely.
It does not depend on work that can run only after its own shutdown returns,
including another driver serialized on the same runtime thread. A returned
error reports incomplete graceful cleanup but never changes whether dropping
the consumed driver is memory-safe.

`ShutdownError` accepts either a message or an underlying error. This keeps the
shared contract independent of driver-specific error types while preserving a
standard error source chain.

## Creation failure

Registration is infallible at the type level. A provider that cannot initialize
its driver and context, or an existing driver that cannot integrate a newly
registered driver, panics because the runtime cannot continue coherently with a
driver registered or connected on only part of its worker set.

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

The cloneable handle hides the runtime's ownership mechanism instead of exposing
it through `DriverOptions`. The generic `spawn` method boxes work only at the
callback boundary.

## Compatibility

Every public dependency type increases the chance that otherwise compatible
drivers resolve to different copies of the contract. The initial API therefore
depends only on `thread_aware_core`, whose `Thread` and `ThreadAware` types are
required for per-worker placement.

`arty_io_core` references those types directly but does not re-export them.
Driver authors depend on `thread_aware_core` when implementing relocation.
`arty_io_core` must not stabilize before `thread_aware_core`.

Future additions follow these rules:

- Add optional provider facilities through private `ProviderOptions` fields and
  new accessors.
- Add optional per-driver facilities through private `DriverOptions` fields and
  new accessors.
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
- how runtimes order independent driver shutdown calls;
- whether a reusable latched-waker implementation belongs in a later utility
  crate;
- which memory pool, clock service, or telemetry facilities drivers may
  eventually receive.

[Completion coordination across I/O drivers](COMPLETION_COORDINATION.md)
explores a separate proposal for shared waiting, native source routing, and
cooperative draining. It does not change the contract described in this document.
