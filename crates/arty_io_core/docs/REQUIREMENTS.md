# Requirements

`arty_io_core` is the shared contract between the Arty runtime and independently
versioned I/O drivers. It describes interoperability requirements, not runtime
registration policy or a specific operating-system completion mechanism.

## R1: Stable shared vocabulary

The crate is the semver chokepoint for the driver ecosystem.

- A runtime and every driver it hosts name the same `arty_io_core` types.
- Public signatures prefer standard-library types.
- Thread placement uses the exact `thread_aware_core` types re-exported by this
  crate.
- Runtime-only helpers, registries, and scheduling policy stay outside this
  crate.

## R2: Lazy and independent registration

The contract supports registration after runtime startup.

- A provider remains sufficient to create every per-worker driver instance.
- Registration is keyed by Rust type identity.
- Semver-incompatible versions of one driver crate can be registered together
  because their provider and driver types have distinct identities.
- The runtime owns synchronization, cancellation, rollback, and caching for
  registration.

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

- A driver may expose a `Parker` and create no thread of its own.
- A driver may use runtime-owned blocking workers.
- A driver or provider may create any number of private threads.
- Primary, satellite, and thread-pinning policy are runtime implementation
  details and are not public driver roles.

## R5: Reliable wakeup

A `Parker` wakeup has the following semantics:

- A wake raised before a wait is latched for the next wait.
- A wake raised by the parker's own thread is honored.
- Redundant wakes may be coalesced.
- A wake is never dropped.
- A waker remains memory-safe after its driver is gone.

## R6: Safe and cooperative shutdown

Shutdown must not rely on an unsafe trait or a caller-checked inertness flag.

- A driver is memory-safe to drop at every point in its lifecycle.
- `begin_shutdown` prevents new operations and is idempotent.
- `poll_shutdown` reports graceful drain progress and wakes the supplied waker
  when progress becomes possible.
- Contexts may outlive drivers; later operations fail safely.
- The runtime bounds shutdown and reports or terminates on a liveness failure.
  Shutdown completion is not a memory-safety precondition.

## R7: Explicit initialization failure

Driver creation returns a typed error.

- Drivers with conditional platform or permission requirements do not need to
  panic during lazy registration.
- The runtime decides how a partial multi-worker registration is rolled back.
- Infallible providers use `std::convert::Infallible`.

## R8: Blocking work is named honestly

The runtime facility for synchronous work is named around blocking, not around
system or async tasks.

- Submitted work may block.
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
