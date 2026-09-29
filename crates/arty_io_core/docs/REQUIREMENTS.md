# Requirements

These contracts govern independently versioned I/O drivers and thread-aware
runtimes. `arty_io_core` provides neither implementation. [Design](DESIGN.md)
describes the lifecycle; the no-op example tracker does not implement coordination.

## R1: Stable shared vocabulary

- Runtimes and drivers use compatible `arty_io_core` types.
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
  owning worker. `DriverOptions` supplies that worker, its role, and the system
  task spawner.
- Roles are fixed. Each worker has at most one primary, assigned only when
  `CAN_BE_PRIMARY` is true. Without one, the runtime owns worker parking.
- Before publishing a context, the runtime completes a separate zero-wait cycle
  with `can_block = false`.
- Providers choose whether instances share queues, memory, or threads.

## R4: Driver-owned execution strategy

- Driver methods run on the owning worker. Completion processing takes
  `&mut self`; drivers need not be `Send` or `Sync`.
- `Driver` remains dyn-compatible; consuming `shutdown` is not callable through
  `dyn Driver`. A private owning shim may adapt it for erased storage.
- Secondaries run before the primary. Every invocation receives the same
  `started_at` and `max_wait`.
- Only `can_block = true` permits a worker wait. Secondaries may register
  background waits but return without joining them.
- `Cycle` mutably borrows a runtime-provided `PendingWorkTracker`, which need
  not be `Send` or `Sync`.
- `Cycle::start_work` synchronously registers the native interruption waker
  and returns one `PendingWork` handle.
- Bounded batches with serviceable work remaining request another cycle.
  In-flight operations alone do not indicate serviceable work.
- Drivers may use `SystemTaskSpawner` or provider-owned threads.

## R5: Reliable wake-ups

- Interruption is latched for the cycle, including signals raised before a
  wait or by its own thread. Redundant signals may coalesce but cannot be lost.
- Registration enrolls work and installs its waker before returning. If already
  interrupted, it invokes the waker before returning.
- Wakers remain memory-safe independently of the driver, signaling promptly
  without panicking, joining work, or waiting on locks held by completing work.
- Drivers may hold multiple non-cloneable `PendingWork` handles.
- Completing and dropping are equivalent: either notifies the runtime exactly
  once to retire this registration, interrupt other waits, then release only
  this barrier participation. Publish results before either action.
- Notifications must not affect later cycles. Waker cloning and dropping neither
  complete work nor affect later cycles.
- After the primary returns, the runtime interrupts remaining waits and waits
  for every handle before advancing. Only the runtime begins a new logical
  cycle, once, before checking work, never between driver calls.
- Interrupted waits still process pending completions. After a driver is dropped
  or shut down, its retained wakers no longer interrupt runtime cycles.

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
