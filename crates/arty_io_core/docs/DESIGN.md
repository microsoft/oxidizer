# Design

`arty_io_core` lets independent I/O libraries run on the same runtime. The
runtime schedules work. Each I/O library controls its operating-system I/O.

## Terms

This document uses the following terms:

- An **async worker** is a runtime-owned thread. It runs application tasks and
  I/O drivers.
- An **I/O context** is the cloneable handle that application code uses to start
  I/O operations.
- A **driver** is a worker-local object. It submits I/O and receives completions
  for one I/O context type.
- **Context retrieval** is a request to the runtime for a concrete
  `IoContext` type.
- A **registration** is the runtime state created by the first retrieval of one
  concrete `IoContext` type.
- The **maximum wait time** is the `Duration` in `Cycle::max_wait`.

## Context retrieval

The concrete `IoContext` type identifies a context registration. When a worker
first retrieves a context, the runtime clones the registered context. It calls
`ThreadAware::relocate` with `None` as the source and the worker as the
destination. It then calls `IoContext::driver`. `DriverOptions::thread`
identifies the same worker. Driver creation finishes all local setup before the
runtime returns the initialized context clone.

The runtime does not register the same concrete context type twice. Libraries must use two distinct context types for two independent registrations.

For each registration, the runtime calls `driver` at most once per async
worker, not once per context clone. Implementations can reject repeated calls for the same registration and worker.
If driver creation returns an error, the runtime does not retry it. The runtime
starts shutdown on every async worker.

Application operations require successful initial relocation and driver
creation. Implementations can reject these operations on contexts that did not
finish the sequence. Clones of initialized contexts remain usable without
another `driver` call.

This requirement applies only to initial context and driver setup. After
initialization, contexts and their clones follow the usual `ThreadAware`
contract. They remain usable after movement without another `relocate` call.
Later relocation affects performance only.

## Driver roles

Each context registration uses one driver role on every async worker:

- A **primary** driver can wait inside `execute_cycle`.
- A **secondary** driver must return from `execute_cycle` without waiting.

The runtime gives the primary role to the first context type. It gives the
secondary role to later context types. It passes the role through
`DriverOptions::role`.

An async worker invokes secondary drivers first with
`Cycle::max_wait() == Duration::ZERO`. A secondary either processes ready
completions or returns immediately when another thread handles its
notifications. If it needs another `execute_cycle` call, it arranges an owner
wake when work becomes ready.

The worker invokes the primary driver last. The primary can wait for I/O for up
to `Cycle::max_wait()`. If no primary driver exists, the runtime waits for work
without calling a driver.

## Waking the async worker

`DriverOptions::owner_waker` wakes the worker that owns the driver. A driver
calls this waker before it returns if more completed operations are ready. A
secondary also arranges a wake when a later notification requires another
`execute_cycle` call. A primary does not need this wake. Its next
`execute_cycle` call performs the worker's I/O wait.

`Driver::waker` wakes a driver instead. The runtime calls it to make a waiting
`execute_cycle` return promptly. A wake before or during a wait applies to that
wait. If no wait is active, the wake remains latched for the next wait. Elapsed
time does not change this rule. The driver can process ready completions after
the wake, but it must stop waiting promptly.

## Processing completions

`execute_cycle` processes completed I/O operations. It must return and must not
loop indefinitely. This rule gives other drivers and application tasks time on
the worker. The contract does not set a fixed completion count for each call.

An error from `execute_cycle` means the driver cannot continue. The runtime
starts shutdown on every async worker. Errors for individual application I/O
operations belong to those operations and do not use `DriverError`.

## Runtime shutdown

When the runtime shuts down an async worker, it stops calling
`execute_cycle` and synchronously calls `Driver::shutdown` for every driver
owned by that worker.

Each driver closes its context to new operations. It can cancel or disrupt
in-flight operations. Cleanup must make progress independently and use a finite
limit. The runtime reports returned errors. It also attempts to shut down the
remaining drivers.

Contexts and their clones can outlive their driver and host. They must reject
new operations after the driver closes them. These live contexts must not
prevent runtime shutdown.

## Scope

This crate defines the types shared by runtimes and I/O libraries. It does not
define a runtime registry, type-erased driver storage, a telemetry format, or an
I/O implementation. `thread_aware_core` supplies the async-worker identity and
context relocation contract used by this API.
