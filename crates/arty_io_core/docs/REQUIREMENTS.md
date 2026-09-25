# Requirements

`arty_io_core` is the shared contract between the Arty runtime and independently
versioned I/O drivers. It specifies interoperability, not runtime registration
policy or a particular operating-system completion mechanism.

## R1: Stable shared vocabulary

The crate is the shared, versioned API for the driver ecosystem.

- A runtime and every driver it hosts name the same `arty_io_core` types.
- Public signatures prefer standard-library types.
- Thread placement uses `thread_aware_core` directly; this crate does not
  re-export its types.
- Runtime-only helpers, registries, and scheduling policy stay outside this
  crate.

## R2: Lazy and independent registration

The contract supports registration after runtime startup.

- The requested `IoContext` type identifies its provider and driver.
- The runtime supplies a `ProviderOptions` when creating the provider.
- `ProviderOptions` is empty in the initial contract and can gain optional
  runtime facilities later without changing the provider factory signature.
- `get_context::<MyContext>()` needs no provider value or runtime configuration.
- The first lookup returns only after every active worker has initialized the
  associated driver and context.
- A provider remains sufficient to create every per-worker driver and context
  pair.
- Registration is keyed by the context's Rust type identity.
- Semver-incompatible versions of one driver crate can be registered together
  because their context types have distinct identities.
- The runtime owns registration coordination, cancellation, and rollback.
- Later lookups clone the worker-local context stored alongside the registered
  driver without creating more driver instances.

## R3: Per-worker initialization

The runtime explicitly initializes a driver adapter and context for each async
worker it serves.

- A provider clone is relocated to the worker before creation.
- `DriverOptions::thread` identifies that worker and its runtime owner.
- A worker that hosts drivers has exactly one primary. Roles are fixed for the
  lifetime of their drivers.
- `DriverOptions::role` identifies the primary or a secondary before creation.
- `DriverOptions::drivers` exposes type-erased handles for drivers whose
  registration previously completed on that worker.
- Existing driver handles are immutable, remain on their owning worker, and are
  available only during creation of the new driver.
- After storing a new driver, the runtime calls `Driver::on_peer_registered`
  on every driver registered earlier on that worker, in registration order.
- Registration is acknowledged only after every earlier driver has received
  the new driver's type-erased handle.
- A relocated provider clone is consumed exactly once.
- Before publication, the runtime invokes one zero-wait initialization cycle in
  the current interruption round; it does not reset the interruptor again.
- The provider decides whether instances share queues, memory, threads, or
  nothing.

## R4: Driver-owned execution strategy

The runtime does not dictate how an I/O subsystem distributes work.

- Completion processing takes `&mut self`, reflecting the runtime's exclusive
  ownership of a thread-local driver without forcing implementations to add
  interior mutability.
- `Driver` remains dyn-compatible. A runtime may use a private owning shim to
  store and erase the associated context type and adapt consuming shutdown to
  boxed storage.
- The runtime invokes every secondary driver before the primary.
- Every driver receives the same `Cycle::max_wait`.
- The primary may wait directly on the worker. Secondary callbacks do not block;
  they may schedule a wait using `max_wait` on a background thread.
- A secondary registers the waker for its current background wait each cycle and
  uses driver-private synchronization to arm or replace that wait.
- The runtime captures one `Instant` when it starts a completion-processing
  cycle and passes that value unchanged to every driver visited in the cycle.
- Drivers use the shared `Interruptor` to wake native waits and request another
  runtime cycle.
- If an immediately serviceable batch remains after a bounded pass, the driver
  requests another cycle. In-flight operations alone do not require a request.
- A driver may delegate work through the runtime-owned `SystemTaskSpawner` handle.
- A driver or provider may create any number of private threads.
- Thread pinning and native observer topology remain implementation details.

## R5: Reliable wake-ups

The shared interruptor has the following semantics:

- A request raised before a blocking wait is latched for that cycle.
- Secondary background observers can interrupt the primary worker wait.
- A wake-up does not prevent pending completions from being processed.
- A wake-up raised by the driver's own thread is honored.
- Redundant wake-ups may be coalesced.
- A wake-up is never dropped.
- Registered wakers remain memory-safe after their driver is gone.
- Only the runtime resets the request latch, before checking cycle work.
- The runtime resets exactly once per logical cycle, never between drivers.
- Once a driver is dropped or its shutdown returns, retained clones stop
  requesting new runtime cycles.

## R6: Safe and blocking shutdown

Shutdown must not rely on an unsafe trait or a caller-checked inertness flag.

- A driver is memory-safe to drop at every point in its lifecycle.
- Dropping a driver closes admission if shutdown has not already started.
- Contexts remain valid in a closed state and do not by themselves prevent
  shutdown completion.
- In-flight operations retain ownership of the state they access through
  reference counts, pool leases, or equivalent safe handles.
- If an operating system retains only a raw pointer into pooled storage, the
  storage owner remains alive independently of the driver.
- `Driver::shutdown` consumes the driver and blocks until graceful cleanup
  completes or fails.
- Shutdown closes admission before waiting for active operations and
  operating-system callbacks to drain.
- Shutdown may return `ShutdownError`, constructed from either a descriptive
  message or an underlying source.
- The driver bounds its own shutdown wait and returns `ShutdownError` rather
  than blocking indefinitely.
- Normal cycle interruption is no longer driven during shutdown; drivers use
  their own synchronization for shutdown progress.
- A driver does not wait for work that can run only after its shutdown returns,
  including another driver serialized on the same runtime thread.
- Returning an error does not relax the requirement that consuming and dropping
  the driver is memory-safe.
- Contexts may outlive drivers; later operations fail safely.
- The stable contract has no `unsafe Driver` implementation requirement and no
  `is_inert` query.
- Platform-specific unsafe code remains private to the driver implementation.
- The runtime reports shutdown failures and continues shutting down its
  remaining drivers. Shutdown completion is not a memory-safety precondition.
- `Driver::shutdown` is not callable through `dyn Driver`; runtimes that erase
  drivers use a private owning shim for graceful shutdown.

## R7: Registration failure

Native initialization and the initial cycle can return `DriverError`.

- The runtime rolls back an unpublished driver/context pair on error.
- A steady-state `execute_cycle` error is reported and initiates driver shutdown.
- An existing driver panics when it cannot integrate a newly registered driver.
- The runtime does not continue a partially connected peer registration.
- A driver with conditional availability exposes a capability check that a
  consumer calls before requesting its context.

## R8: System work is named explicitly

The runtime facility for synchronous I/O work uses `SystemTask` terminology.

- Submitted work may block.
- It is system work owned by an I/O driver, not an async application task.
- It does not run on an async worker.
- Submission returns before the work completes.
- The facility remains available through driver shutdown.
- `DriverOptions` exposes a crate-owned cloneable handle rather than the runtime's
  shared-ownership implementation type.

## R9: Scope of the initial API

The initial contract deliberately excludes:

- a runtime driver registry or `get_or_init` API;
- native observer placement beyond the primary/secondary role contract;
- memory pools, configurable clock services, telemetry, and ecosystem-specific
  error types;
- batching and wake-coalescing optimizations;
- `no_std` support;
- a default I/O implementation.
