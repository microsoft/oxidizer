# Design

## What this proposal achieves

An application may use several I/O libraries, each with its own way of talking
to the operating system. We want those libraries to work on the same runtime
without requiring one common I/O implementation or a separate runtime for
each library.

The difficult part is not giving every library the same `read` or `write`
method. It is making them cooperate: when may their worker thread sleep, what
wakes it when any library has work, and how can everything stop safely?

`arty_io_core` defines that agreement. It provides neither a runtime nor an I/O
implementation. A driver author keeps control of native I/O; a runtime author
keeps control of scheduling. Both take responsibility for their side of the
agreement.

This document explains the approach through a hypothetical network driver and
file driver sharing a worker. [Requirements](REQUIREMENTS.md) is the reference
for the exact obligations; the walkthrough is not an additional API.

## The mental model

The runtime owns the worker's time. Each driver owns one source of I/O.
Application code uses a handle rather than calling the worker's driver directly.

| Part | What it is for |
| --- | --- |
| `IoContext` | The application's handle to the I/O library's operations. |
| `DriverProvider` | The setup recipe for creating that library's driver and context on each worker. |
| `Driver` | The worker-local part that processes submissions and completions. |
| Runtime | Connects these parts, gives drivers turns, and coordinates when the worker can wait or continue. |

The handle and driver have different lifetimes. Application code can retain a
context while the runtime calls the driver with exclusive access on its owning
worker. The driver need not be `Send` or `Sync`. Its provider decides whether
workers share queues, memory, or background threads; the contract does not
prescribe that layout. Relocating a context may improve placement, but cannot
be necessary for correctness.

## A walkthrough: two libraries, one worker

### First, make each library ready to use

When application code first requests a context, the runtime asks its provider
to create a driver/context pair on every active worker. Later requests reuse
that registration and clone existing contexts. Registration is keyed by the
concrete context type, so different driver versions can coexist without being
mistaken for the same library.

The application must not receive a handle to a half-initialized driver. For
each worker, the runtime clones and relocates the provider, assigns the driver's
role, and supplies earlier peers through `DriverOptions`. It creates the pair
without publishing the context, then runs a separate non-blocking, zero-wait
cycle. This gives the driver a chance to establish native notifications and
recheck work queued during construction.

Only then does the runtime store the pair and notify earlier drivers, in
registration order, about their new peer. Those notifications finish before
publication and acknowledgment; the first request returns only after every
active worker is ready.

Peer discovery lets compatible drivers cooperate without the runtime having to
understand their native resources. A driver may inspect a peer's borrowed,
worker-local handle and clone independently owned state from it. It cannot
retain the borrow. Whether the network and file drivers can share a native
waiting mechanism remains their decision, not a promise made by core.

### Give every driver a turn before sleeping

One logical cycle is a coordinated pass across a worker's drivers, not one
driver method call. The runtime starts coordination once, before checking work,
and supplies every driver with the same time snapshot and wait bound.

At registration, the runtime may choose at most one willing driver as the
**primary**. Only providers with `CAN_BE_PRIMARY = true` are eligible, and the
role stays fixed. Other drivers are **secondaries**. Secondaries run first;
the primary runs last, after the others have had an opportunity to make progress.

The primary is a possible place to wait, not permission to block whenever it
likes. The runtime grants that permission per invocation through `can_block`.
Secondaries cannot block the worker, though they may arrange background waits.
If there is no primary, the runtime is responsible for parking the worker.

Each driver processes a bounded batch so that it does not monopolize the
worker. If work can still be serviced immediately, it requests another cycle
by completing a pending-work handle. Merely having an unfinished I/O operation
does not request another cycle: that would turn waiting for I/O into a busy loop.

### Let either library wake the worker

Suppose there are no runnable application tasks. The file driver arranges a
background wait and returns. The network driver, acting as primary, waits on
the worker. A file completion must be able to end that network wait; otherwise
file I/O would depend on an unrelated network event to make progress.

The connection is an exchange of two responsibilities:

- Before entering or scheduling a native wait, the driver gives
  `Cycle::start_work` a waker that can interrupt that wait.
- The runtime registers the work and returns a `PendingWork` handle. The driver
  keeps it until the work has ended, publishes any results, then completes or
  drops it to notify the runtime.

In this example, when the file observer finishes its registered work, its
notification interrupts other waits, including the primary's. An interrupted
driver still processes pending completions. After the primary returns, the
runtime interrupts any remaining waits and waits for **all** registered work
to end before starting another cycle.

That last step matters: asking a wait to stop is not proof that it has stopped.
The pending-work handles form a completion barrier, keeping one cycle's work
from leaking into the next.

There is also a race to close before sleeping. Work might finish just before
another driver enters its native wait. Registration therefore installs the
interruption waker before returning and invokes it immediately if the cycle
is already interrupted. The native signal stays latched until the wait observes
it. Coordination is never reset between driver calls, so a later driver cannot
erase an earlier notification.

`PendingWork` is non-cloneable. Completing and dropping it send the same
notification exactly once; cloning or dropping a waker does not complete work.
The runtime's notification first retires that work's interruption registration,
then interrupts other waits, then releases only that work's barrier participation.
Old notifications must not affect later cycles.

### Keep scheduling decisions in the runtime

The runtime knows when the worker has other work to do and how it should park.
It therefore implements `PendingWorkTracker`, including interruption state,
counters, and parking. `Cycle` borrows that tracker mutably on the owning worker;
core does not impose a particular synchronization algorithm.

Drivers supply the native interruption mechanism. Wakers may run inline on any
thread, so they must signal promptly without panicking, joining work, or taking
locks held by the completing work. Each side owns the synchronization needed
to uphold its part of the contract.

When a driver needs blocking background work, `SystemTaskSpawner` supplies
runtime-owned system threads. These are not async application tasks: submission
returns after acceptance, not after completion. An indefinitely blocked observer
needs independent execution capacity; putting several such observers into an
undersized pool would prevent some of them from ever running. Drivers may also
use provider-owned threads.

## Stop safely, even when cleanup fails

Shutdown has two separate goals: stop accepting new operations, and finish
access to resources already in use. `Driver::shutdown` consumes the driver,
closes admission, and drains operations, callbacks, and observers within a
bounded wait, or returns `ShutdownError`.

Application contexts may remain alive as closed handles that reject new work;
users do not have to drop every context before draining can finish. Conversely,
cancellation is not permission to free a buffer that native code can still
access. Such storage needs ownership independent of the driver. Dropping a
driver must always be memory-safe and close admission if necessary, including
when graceful cleanup fails.

Normal cycles have stopped at this point. Each driver must make shutdown
progress locally or on independent threads, rather than waiting for another
driver on the same worker to run. The runtime keeps the system task spawner
available until all shutdown calls return and attempts the remaining drivers
after an error. This avoids making one driver's failure a reason to skip
everyone else's cleanup.

## Make failures explicit

Not every failure means the same thing:

| Failure | What happens and why |
| --- | --- |
| Creation or the initial cycle fails | Return `DriverError` and roll back the unpublished pair; application code must not see a partially initialized driver. |
| Driver infrastructure fails during normal cycles | Report the error and shut down that worker's drivers; continuing normal coordination is no longer reliable. |
| A driver cannot integrate a peer | Panic; this contract does not support continuing a partially connected registration. |
| Graceful shutdown cannot finish | Return `ShutdownError`, keep drop safety, and continue attempts to shut down other drivers. |

These infrastructure failures are distinct from an individual I/O operation
failing. Drivers that are only conditionally available expose a capability check
before an application requests their context.

## Deliberate boundaries and trade-offs

This is a shared contract, not a shared I/O engine. It leaves native operation
formats and resource choices with drivers, at the cost of requiring each
runtime and driver to implement the coordination protocol correctly. It does
not guarantee that arbitrary native waits can be combined without background
threads. The [completion coordination survey](COMPLETION_COORDINATION.md)
discusses those platform constraints independently of this proposal.

Runtimes and drivers must use compatible `arty_io_core` types. Placement uses
`thread_aware_core` directly, without re-exports, so core must not stabilize
before that dependency. Optional construction facilities belong in
`ProviderOptions` or `DriverOptions`; mandatory constructor arguments and trait
methods are compatibility commitments.

The runtime's registry, type erasure, and placement policy remain private.
`Driver` supports dynamic dispatch, but its consuming `shutdown` cannot be
called through `dyn Driver`; a runtime may use a private owning shim for
heterogeneous storage and shutdown.

The contract deliberately does not choose thread pinning, cross-worker
registration atomicity, shutdown ordering, memory pools, clocks, or telemetry.
Those decisions belong to concrete implementations or later proposals, not to
this initial agreement.

## What the example demonstrates

The [single-thread example](../examples/single_thread_runtime/main.rs) shows how
registration and peer discovery fit together. Its drivers perform no I/O and
its tracker is a no-op. It is not a reference implementation of completion
coordination, nor evidence that native waiting and shutdown work.

For the detailed contract, use [Requirements](REQUIREMENTS.md): R2-R3 cover
joining the runtime, R4-R5 cover execution and wake-ups, and R6-R8 cover shutdown,
failures, and system work.
