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

- The requested `IoContext` type identifies its provider and driver.
- The runtime supplies a `ProviderContext` when creating the provider.
- `ProviderContext` is empty in the initial contract and can gain optional
  runtime facilities later without changing the provider factory signature.
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
- `DriverContext::thread` identifies that worker and its runtime owner.
- A relocated provider clone is consumed exactly once.
- The provider decides whether instances share queues, memory, threads, or
  nothing.

## R4: Driver-owned execution strategy

The runtime does not dictate how an I/O subsystem distributes work.

- Completion processing takes `&mut self`, reflecting the runtime's exclusive
  ownership of a thread-local driver without forcing implementations to add
  interior mutability.
- `Driver` remains dyn-compatible when its `Context` associated type is
  specified. A runtime may use a private owning shim to erase context types and
  adapt consuming shutdown to boxed storage.
- The runtime chooses the driver-owning thread before creation and invokes
  `process_completions` only from that thread.
- A driver may delegate work through the runtime-owned `SystemTasks` handle.
- A driver or provider may create any number of private threads.
- Primary, satellite, and thread-pinning policy are runtime implementation
  details and are not public driver roles.

## R5: Reliable interruption

A driver interrupt has the following semantics:

- An interrupt raised before a blocking wait is latched for the next blocking wait.
- A non-blocking completion pass does not consume a pending interrupt.
- An interrupt does not prevent pending completions from being processed.
- An interrupt raised by the driver's own thread is honored.
- Redundant interrupts may be coalesced.
- An interrupt is never dropped.
- An interruptor remains memory-safe after its driver is gone.

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
  message or an underlying cause.
- The driver bounds its own shutdown wait and returns `ShutdownError` rather
  than blocking indefinitely.
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
- `DriverContext` exposes a crate-owned cloneable handle rather than the runtime's
  shared-ownership implementation type.

## R9: Scope of the initial API

The initial contract deliberately excludes:

- a runtime driver registry or `get_or_init` API;
- primary-driver selection and satellite threads;
- memory pools, clocks, telemetry, and ecosystem-specific error types;
- batching and wake-coalescing optimizations;
- `no_std` support;
- a default I/O implementation.
