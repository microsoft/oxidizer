# Requirements

`arty_io_core` is the shared contract between the Arty runtime and independently
versioned I/O drivers. It describes interoperability requirements, not runtime
registration policy or a specific operating-system completion mechanism.

## R1: Stable shared vocabulary

The crate is the semver chokepoint for the driver ecosystem.

- A runtime and every driver it hosts name the same `arty_io_core` types.
- Public signatures prefer standard-library types.
- Thread placement uses `thread_aware_core` directly; this crate does not
  re-export its types.
- Runtime-only helpers, registries, and scheduling policy stay outside this
  crate.

## R2: Lazy and independent registration

The contract supports registration after runtime startup.

- The requested context type identifies its provider and driver.
- `get_context::<MyContext>()` needs no provider value or runtime configuration.
- The first lookup returns only after every active worker has initialized the
  associated driver.
- A provider remains sufficient to create every per-worker driver instance.
- Registration is keyed by the context's Rust type identity.
- Semver-incompatible versions of one driver crate can be registered together
  because their context types have distinct identities.
- The runtime owns synchronization, cancellation, rollback, and caching for
  registration.
- Later lookups return the cached worker-local context without creating more
  driver instances.

## R3: Per-worker initialization

The runtime explicitly initializes a driver adapter for each async worker it
serves.

- A provider clone is relocated to the worker before creation.
- `DriverInit::thread` identifies that worker and its runtime owner.
- A relocated provider clone is consumed exactly once.
- The provider decides whether instances share queues, memory, threads, or
  nothing.

## R4: Driver-owned execution strategy

The runtime does not dictate how an I/O subsystem distributes work.

- Every `Driver` method takes `&self`; runtime invocation requires neither
  mutable storage access nor a synchronization wrapper.
- `Driver` remains dyn-compatible when its `Context` associated type is
  specified, so it can be stored as `Box<dyn Driver<Context = C>>`.
- Drivers use thread-local interior mutability when callbacks change state.
- Every driver exposes a `Parker` through a shared reference.
- The runtime chooses the driver-owning thread before creation and drives the
  `Parker` only from that thread.
- A driver may delegate `SystemTask` work to runtime-owned workers.
- A driver or provider may create any number of private threads.
- Primary, satellite, and thread-pinning policy are runtime implementation
  details and are not public driver roles.

## R5: Reliable wake-up

A `Parker` wake-up has the following semantics:

- A wake raised before a wait is latched for the next wait.
- A wake raised by the `Parker`'s own thread is honored.
- Redundant wakes may be coalesced.
- A wake is never dropped.
- A waker remains memory-safe after its driver is gone.

## R6: Safe and cooperative shutdown

Shutdown must not rely on an unsafe trait or a caller-checked inertness flag.

- A driver is memory-safe to drop at every point in its lifecycle.
- Contexts and in-flight operations retain ownership of the state they access
  through reference counts, pool leases, or equivalent safe handles.
- The runtime calls `begin_shutdown` exactly once per driver; implementations
  do not need to handle a second call.
- Calling `begin_shutdown` closes admission and returns a future that reports
  graceful drain progress.
- The shutdown future is boxed so `begin_shutdown` remains object-safe.
- The shutdown future wakes its task when progress becomes possible and may be
  polled repeatedly until ready.
- Contexts may outlive drivers; later operations fail safely.
- The stable contract has no `unsafe Driver` implementation requirement and no
  `is_inert` query.
- Platform-specific unsafe code remains private to the driver implementation.
- The runtime bounds shutdown and reports or terminates on a liveness failure.
  Shutdown completion is not a memory-safety precondition.

## R7: Initialization failure is fatal

Driver creation is infallible at the type level.

- A provider panics when its driver cannot be initialized.
- The runtime does not continue after a worker fails to initialize a registered
  driver.
- A driver with conditional availability exposes a capability check that a
  consumer calls before requesting its context.

## R8: System work is named explicitly

The runtime facility for synchronous I/O work uses `SystemTask` terminology.

- Submitted work may block.
- It is system work owned by an I/O driver, not an async application task.
- It does not run on an async worker.
- Submission returns before the work completes.
- The facility remains available through driver shutdown.

## R9: Scope of the initial API

The initial contract deliberately excludes:

- a runtime driver registry or `get_or_init` API;
- primary-driver selection and satellite threads;
- memory pools, clocks, telemetry, and ecosystem-specific error types;
- batching and wake-coalescing optimizations;
- `no_std` support;
- a default I/O implementation.
