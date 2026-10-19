# Design

`arty_io_core` lets independent I/O libraries run on the same runtime. The
runtime schedules work. Each I/O library keeps control of its operating-system
I/O implementation.

## Terms

This document uses the following terms:

- An **async worker** is a runtime-owned thread that executes application tasks
  and I/O drivers.
- An **I/O context** is the cloneable handle that application code uses to start
  I/O operations.
- A **driver** is the worker-local object that submits I/O and receives
  completions for one I/O context type.
- **Context retrieval** is a request to the runtime for a concrete
  `IoContext` type.
- A **registration** is the runtime state created by the first retrieval of one
  concrete `IoContext` type.
- The **maximum wait time** is the `Duration` in `Cycle::max_wait`.

## Context retrieval

The runtime identifies a registration by the concrete `IoContext` type. The
first retrieval performs these steps:

1. Call `IoContext::context` once.
2. Clone and relocate the context to every async worker.
3. Call `IoContext::driver` on each async worker.
4. Publish the context only after every worker has created its driver.

Later retrievals return a clone of the existing context. The runtime does not
register the same concrete context type twice. Libraries that need two
independent registrations use two distinct context types.

Cloning a registered context must not retrieve an I/O context. The runtime may
serialize context clones, so retrieving another context while cloning can create
a lock-order cycle between registrations on different workers.

`IoContext::driver` completes driver setup before the context is published. A
driver must not wait for another async worker to run during creation. If driver
creation fails, the runtime treats the failure as fatal and starts runtime
shutdown instead of retrying the registration.

## Driver roles

Each context registration uses one driver role on every async worker:

- A **primary** driver may wait inside `execute_cycle`.
- A **secondary** driver must return from `execute_cycle` without waiting.

The runtime assigns the first context type the primary role and later context
types the secondary role. It passes the assigned role through
`DriverOptions::role`.

An async worker invokes secondary drivers first with
`Cycle::max_wait() == Duration::ZERO`. A secondary either processes completions
that are ready now or returns immediately when another thread handles its
notifications. If the secondary needs another `execute_cycle` call after it
returns, it arranges for the owner waker to be called when work becomes ready.

The worker invokes the primary driver last. The primary may wait for I/O for up
to `Cycle::max_wait()`. If no primary driver exists, the runtime waits for work
without calling a driver.

## Waking the async worker

`DriverOptions::owner_waker` wakes the async worker that owns the driver. A
driver calls this waker before returning if more completed operations are
already ready. A secondary also arranges for the waker to be called when a
later notification requires another `execute_cycle` call. A primary does not
need a separate wake for later completions because its next `execute_cycle`
call performs the worker's I/O wait.

`Driver::waker` serves the opposite direction. The runtime calls it to make a
waiting `execute_cycle` return promptly. Any wake before or during a wait applies
to that wait. If no wait is active, the wake remains latched for the next wait,
regardless of how much time passes before the wait starts. The driver may process
ready completions after waking; the requirement is that it stop waiting promptly.

## Processing completions

`execute_cycle` processes completed I/O operations. It must return rather than
loop indefinitely, so other drivers and application tasks get time on the async
worker. The contract does not require a fixed number of completions per call.

Driver cycle callbacks, including formatting an error returned by
`execute_cycle`, must not retrieve an I/O context from the same worker. The host
keeps its driver collection borrowed during cycle execution to avoid allocation
and reference-counting on this hot path.

An error from `execute_cycle` means the driver cannot continue. The runtime
starts shutdown on every async worker. Errors for individual application I/O
operations belong to those operations and do not use `DriverError`.

## Runtime shutdown

When the runtime shuts down an async worker, it stops calling
`execute_cycle` and synchronously calls `Driver::shutdown` for every driver
owned by that worker.

Each driver closes its context to new operations and may cancel or disrupt
operations still in flight. Cleanup must make progress independently and use a
finite limit. The runtime reports returned errors and attempts to shut down the
remaining drivers.

## Scope

This crate defines the types shared by runtimes and I/O libraries. It does not
define a runtime registry, type-erased driver storage, a telemetry format, or an
I/O implementation. `thread_aware_core` supplies the async-worker identity and
context relocation contract used by this API.
