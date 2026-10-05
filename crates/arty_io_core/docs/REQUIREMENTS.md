# Requirements

These contracts govern independently versioned I/O drivers and thread-aware
runtimes. `arty_io_core` provides neither implementation. [Design](DESIGN.md)
describes the lifecycle; the sample drivers do not implement native I/O.

## R1: Stable shared vocabulary

- Runtimes and drivers use compatible `arty_io_core` types.
- Public signatures prefer standard-library types. Placement uses
  `thread_aware_core` directly, without re-exports.
- Registries, scheduling policy, and native coordination stay outside core.

## R2: Lazy and independent registration

- The runtime keys registration by concrete `IoContext` type, which selects its
  provider, and supplies `ProviderOptions`.
- The first request returns only after every active worker has an initialized
  driver/context/role result. Later requests clone existing contexts.
- Different driver versions can coexist through distinct context types.
- Registration coordination, cancellation, and rollback belong to the runtime.

## R3: Per-worker initialization

- The runtime clones and relocates each provider before consuming it on the
  owning worker. `DriverOptions` supplies that worker, the most permissive
  role, and the system task spawner.
- The provider returns a `DriverInstance` containing the context, driver, and
  role selected by that driver. A driver may select secondary when primary is
  permitted, but may not select primary without permission. Each worker has
  at most one primary; without one, the runtime owns worker parking.
- Before publishing a context, the runtime completes a separate cycle with
  `max_wait = Duration::ZERO`.
- Providers choose whether instances share queues, memory, or threads.

## R4: Driver-owned execution strategy

- Driver methods run on the owning worker. Completion processing takes
  `&mut self`; drivers need not be `Send` or `Sync`.
- `Driver` remains dyn-compatible; consuming `shutdown` is not callable through
  `dyn Driver`. A private owning shim may adapt it for erased storage.
- Secondaries run before the primary. Every invocation receives a mutable
  `Cycle` carrying the same `max_wait`.
- Only the primary may wait on the worker, for up to `max_wait`. A zero wait
  bound means no waiting. Secondary drivers may coordinate with other drivers
  or continuously process completions on a driver-owned background thread.
- `Cycle` is worker-local and does not implement `Send` or `Sync`.
- `Driver::waker` interrupts a driver's pending native wait. Wakes are latched
  and remain safe after the driver is dropped.
- Bounded batches with serviceable work remaining request another cycle.
  In-flight operations alone do not indicate serviceable work.
- Drivers may use `SystemTaskSpawner` or provider-owned threads.

## R5: Reliable wake-ups

- Interruption is latched, including signals raised before a wait or by its
  own thread. Redundant signals may coalesce but cannot be lost.
- Wakers remain memory-safe independently of the driver, signaling promptly
  without panicking, joining work, or waiting on locks held by the completion
  path.
- A wake interrupts only the wait; the driver still processes pending
  completions.
- Drivers own coordination for work that continues beyond their worker-local
  call. The runtime does not track or join pending driver work.
- A retained waker may continue to interrupt the driver's wait after the driver
  is dropped or shut down, without accessing driver-owned storage.

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
