# Design

## What this proposal achieves

Independently developed I/O libraries should work on one runtime without
requiring one common I/O implementation or a separate runtime per library.

The shared problem is cooperation: when may the worker sleep, what wakes it
when any library has work, and how can everything stop safely?

`arty_io_core` defines that agreement, not an implementation. Driver authors
keep control of native I/O; runtime authors keep control of scheduling.
[Requirements](REQUIREMENTS.md) specifies the obligations; this document
explains the approach and its rationale.

## The mental model

The runtime owns the worker's time. Each driver owns one source of I/O.
Application code uses a handle rather than calling the worker's driver directly.

| Part | What it is for |
| --- | --- |
| `IoContext` | The application's handle to the I/O library's operations. |
| `DriverProvider` | Creates the library's driver and context on each worker. |
| `Driver` | The worker-local part that processes submissions and completions. |
| Runtime | Gives drivers turns and coordinates when the worker may wait or continue. |

Contexts can outlive drivers. The runtime calls each driver with exclusive
access on its owning worker; providers decide whether workers share queues, memory, or
threads. Context relocation is an optimization, not a correctness requirement.

## A walkthrough: two libraries, one worker

Imagine a network driver and a file driver sharing a worker.

### Make each library ready before handing it to the application

The first context request creates a driver/context pair on every active worker.
Before publishing it, the runtime runs a non-blocking initialization cycle.
The request returns only when all workers are ready, so application code cannot
see a half-initialized driver.

Later requests reuse the registration. Concrete context types distinguish
registrations, allowing different driver versions to coexist.

### Give every driver a turn before sleeping

A logical cycle is one coordinated pass across the drivers, with a shared time
snapshot and wait bound. At registration, the runtime may select one eligible
driver as **primary**. **Secondaries** run first without blocking the worker;
the primary runs last and may wait only when the runtime permits it. Without a
primary, parking remains the runtime's responsibility.

Bounded batches keep one driver from monopolizing the worker. Immediately
serviceable work requests another cycle; unfinished I/O alone does not, avoiding
a busy loop while waiting for the operating system.

### Let either library wake the worker

Suppose the file driver has a background wait while the primary network driver
waits on the worker. A file completion must end that network wait, rather than
depend on an unrelated network event.

Before waiting, each driver registers an interruption waker through
`Cycle::start_work` and receives a `PendingWork` handle. It keeps the handle
until the work ends, publishes results, then completes or drops the handle.
That notification interrupts other waits, including the primary's.

After the primary returns, the runtime interrupts remaining waits and waits
for all registered work to end before advancing. Asking a wait to stop is not
proof that it has stopped; the handles provide that completion barrier.

Signals also stay latched across wait entry. A completion arriving just before
another driver sleeps must not be lost. Coordination starts before checking
work and is never reset between driver calls. The exact wake-up protocol is
specified in [R5](REQUIREMENTS.md#r5-reliable-wake-ups).

### Keep scheduling decisions in the runtime

The runtime implements `PendingWorkTracker` because it knows when the worker
has other work and how it should park. Drivers supply native interruption;
each side owns its synchronization.

Blocking observers can use `SystemTaskSpawner` or provider-owned threads.
These are not async application tasks: indefinitely blocked observers need
independent execution capacity, or an undersized pool can prevent progress.

## Stop safely, even when cleanup fails

Shutdown stops new operations and drains existing operations, callbacks, and
observers within a bounded wait. Contexts may survive as closed handles;
users need not drop every context before draining can finish.

Cancellation is not permission to free memory still accessible to native code.
That memory needs ownership independent of the driver, and dropping the driver
must remain safe even when shutdown fails.

Normal cycles have stopped, so shutdown must progress without another driver
on the same worker. The runtime keeps system tasks available until all shutdown
calls return and continues cleanup after an error.

## Make failures explicit

Initialization errors roll back the unpublished pair. Infrastructure errors
during normal cycles shut down the worker's drivers. Shutdown failure returns
`ShutdownError` without relaxing memory safety.

These are infrastructure failures, distinct from individual I/O errors.

## Deliberate boundaries and trade-offs

Independence preserves native resource choices, but requires runtimes and drivers
to implement the protocol correctly. It does not make arbitrary native waits
shareable without background threads; the
[coordination survey](COMPLETION_COORDINATION.md) explains those constraints.

All parties need compatible contract types. Placement uses `thread_aware_core`
directly, so core must not stabilize before that dependency. New mandatory
methods or constructor arguments are compatibility commitments.

Registries, type erasure, thread placement, cross-worker registration atomicity,
shutdown ordering, memory pools, clocks, and telemetry remain outside this
initial agreement.

## Potential improvement: driver awareness

A future extension could let drivers discover peers on the same worker and
negotiate shared native resources, such as a completion port. Borrowed peer
handles during creation and notifications after registration could reduce
duplicate resources or observer threads without putting native details in the
runtime. This would need clear lifetime, publication, and failure rules.
Peer discovery, peer handles, and registration callbacks are not part of the
current API.

## What the example demonstrates

The [single-thread example](../examples/single_thread_runtime/main.rs) demonstrates
registration and driver roles only. Its drivers perform no I/O and its tracker
is a no-op, not a reference implementation of completion coordination.
