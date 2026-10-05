# Requirements

These contracts govern independently versioned I/O drivers and thread-aware
runtimes. `arty_io_core` provides neither implementation. [Design](DESIGN.md)
describes the lifecycle; the no-op example wakers do not implement coordination.

## R1: Stable shared vocabulary

- Runtimes and drivers use compatible `arty_io_core` types.
- Providers return a non-exhaustive `DriverInstance` containing the public
  `driver`, `context`, and selected `role` fields.
- Public signatures prefer standard-library types. Placement uses
  `thread_aware_core` directly, without re-exports.
- Registries, scheduling policy, and native coordination stay outside core.

## R2: Lazy and independent registration

- The runtime keys registration by concrete `IoContext` type, which selects its
  provider, and supplies `ProviderOptions`.
- The first request returns only after every active worker has an initialized
  driver/context pair. Later requests clone existing contexts.
- Different driver versions can coexist through distinct context types.
- Registration coordination, cancellation, and rollback belong to the runtime.

## R3: Per-worker initialization

- The runtime clones and relocates each provider before consuming it on the
  owning worker. `DriverOptions` supplies that worker, a role permission, and
  the system task spawner.
- `DriverOptions::allowed_roles()` is the runtime's permission set, not the
  final role.
  The provider returns either the `Primary` or `Secondary` variant of
  `DriverInstance`. A worker has at most one primary, and the runtime enforces
  the permission and primary-capacity rules for that selection.
- Before publishing a primary context, the runtime completes a separate cycle
  with `max_wait = Duration::ZERO`. Secondary drivers do not receive cycle
  callbacks.
- Providers choose whether instances share queues, memory, or threads.

## R4: Driver-owned execution strategy

- Driver methods run on the owning worker. Completion processing takes
  `&mut self`; drivers need not be `Send` or `Sync`.
- `Driver` remains dyn-compatible; consuming `shutdown` is not callable through
  `dyn Driver`. A private owning shim may adapt it for erased storage.
- Only primary drivers are polled. A primary may wait on the worker, for up to
  `max_wait`; a zero wait bound means no waiting.
- `Cycle` exposes only `max_wait` through `Cycle::new(max_wait)` and
  `max_wait()`. It is passed as `&mut Cycle`, is not `Copy` or `Clone`, and is
  deliberately neither `Send` nor `Sync`.
- `PrimaryDriver::waker()` supplies the primary driver's notification path to
  the runtime. Completion processing uses it to wake a worker that may be
  waiting.
- Secondary drivers return promptly. They either coordinate completion
  processing with another driver and its wait/notification path, or continuously
  process completions on independent driver-owned background execution.
- Bounded batches with serviceable work remaining request another cycle.
  In-flight operations alone do not indicate serviceable work.
- Drivers may use `SystemTaskSpawner` or provider-owned threads.

## R5: Reliable wake-ups

- A driver's waker remains usable while its driver-owned completion machinery
  can notify the runtime. It signals promptly without panicking, joining work,
  or waiting on locks held by completing work.
- A completion notification racing the start or final check of a wait must
  remain observable by the driver's coordination mechanism or wake the runtime.
  Redundant signals may coalesce, but a notification cannot be lost.
- Waking the runtime is not completion processing. A driver must retain
  ownership of the state needed to drain completions until that processing is
  complete.
- The runtime invokes the primary at most once per logical cycle. A secondary
  cannot rely on a later invocation to process completions while the primary is
  waiting; it must use coordination or independent background processing as
  specified in R4.
- After a driver is dropped or shut down, its retained notification state must
  not access released driver resources.

## R6: Safe and blocking shutdown

- Dropping a driver is always memory-safe and closes admission if necessary.
  Contexts may outlive it as closed handles without delaying draining.
- Operations and native callbacks retain their accessible state. Storage
  referenced by native raw pointers has an owner independent of the driver.
- `shutdown` consumes the driver, closes admission, and drains within a bounded
  wait, or returns `ShutdownError` without relaxing drop safety.
- Shutdown uses driver-owned progress mechanisms: normal cycle coordination
  has stopped, and no other driver on the same worker may be required to run.
- The runtime reports failures, attempts remaining drivers, and keeps the system
  task spawner available until all shutdown calls return.
- Safety does not depend on successful shutdown or a caller-checked inertness
  flag. Platform-specific unsafe code stays private to driver implementations.

## R7: Registration failure

- Creation and the initial cycle may return `DriverError`; the runtime rolls
  back the unpublished pair.
- The runtime reports normal cycle errors and shuts down the worker's drivers.
- Drivers with conditional availability expose a capability check before a
  consumer requests the context.

## R8: System work is named explicitly

- `SystemTask` is blocking driver work on system threads, not an async task.
- Submission returns after acceptance, without waiting for completion.
- `SystemTaskSpawner` remains available through shutdown and hides the runtime's
  shared-ownership mechanism.

## R9: Scope of the initial API

Core does not provide peer-driver discovery, a driver registry, native observer
placement, memory pools, configurable clocks, telemetry, ecosystem-specific
errors, batching optimizations, `no_std` support, or a default I/O implementation.
