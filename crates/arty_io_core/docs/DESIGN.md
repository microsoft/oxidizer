# Design

## What this proposal achieves

Independently developed I/O libraries should work on one runtime without
requiring one common I/O implementation or a separate runtime per library.

The shared problem is cooperation: when may the worker sleep, how do drivers
share its execution time, and how can everything stop safely?

`arty_io_core` defines that agreement, not an implementation. Driver authors
keep control of native I/O; runtime authors keep control of scheduling. This
document explains the approach and its rationale.

## The mental model

The runtime owns the worker's time. Each driver owns one source of I/O.
Application code uses a handle rather than calling the worker's driver directly.

| Part | What it is for |
| --- | --- |
| `IoContext` | The application's handle to the I/O library's operations and factory for worker-local drivers. |
| `Driver` | The worker-local part that processes submissions and completions. |
| Runtime | Gives drivers turns and coordinates when the worker may wait or continue. |

Contexts can outlive drivers. The runtime calls each driver with exclusive
access on its owning worker; contexts decide whether workers share queues, memory, or
threads. Context relocation is an optimization, not a correctness requirement.
`IoContext` also creates worker-local drivers. Keeping those responsibilities on
one type avoids a separate provider construction and relocation contract.

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

A logical cycle is one coordinated pass across the drivers with a wait bound.
At registration, the runtime tells the context which
roles remain available, and the returned driver instance selects one. A worker
has at most one **primary**. **Secondaries** run first without blocking the
worker; the primary runs last and may wait up to the runtime's wait bound. A
zero bound means no waiting. Without a primary, parking remains the runtime's
responsibility.

Bounded batches keep one driver from monopolizing the worker. Immediately
serviceable work uses the owner waker to request another cycle; unfinished I/O
alone does not, avoiding a busy loop while waiting for the operating system.
Each driver also exposes a waker that lets the runtime interrupt a pending
completion wait.

### Keep scheduling decisions in the runtime

Blocking observers use context-owned threads or another implementation-owned mechanism.
These are not async application tasks: indefinitely blocked observers need independent
execution capacity.

## Stop safely, even when cleanup fails

Shutdown stops new operations and drains existing operations, callbacks, and
observers within a bounded wait. Contexts may survive as closed handles;
users need not drop every context before draining can finish.

Cancellation is not permission to free memory still accessible to native code.
That memory needs ownership independent of the driver, and dropping the driver
must remain safe even when shutdown fails.

Normal cycles have stopped, so shutdown must progress without another driver
on the same worker. The runtime continues cleanup after an error.
The fallible result lets the runtime report incomplete cleanup consistently,
even though it does not stop cleanup of the remaining drivers.

## Make failures explicit

Initialization errors roll back the unpublished pair. Infrastructure errors
during normal cycles shut down the worker's drivers. Shutdown failure returns
`DriverError` without relaxing memory safety.

These are infrastructure failures, distinct from individual I/O errors.
Drivers perform any implementation-specific retries before returning an error;
the runtime treats a returned error as terminal and does not classify it for
recovery.

## Deliberate boundaries and trade-offs

Independence preserves native resource choices, but requires runtimes and drivers
to implement the protocol correctly. It does not make arbitrary native waits
shareable without background threads.

All parties need compatible contract types. Placement uses `thread_aware_core`
directly, so core must not stabilize before that dependency. New mandatory
methods or constructor arguments are compatibility commitments.

Registries, type erasure, thread placement, cross-worker registration atomicity,
shutdown ordering, memory pools, clocks, and telemetry remain outside this
initial agreement.

In particular, consuming shutdown is not available through `dyn Driver`.
Runtimes that erase driver types can use a private owning adapter rather than
making runtime storage part of this public contract.

## Potential improvement: driver awareness

A future extension could let drivers discover peers on the same worker and
negotiate shared native resources, such as a completion port. Borrowed peer
handles during creation and notifications after registration could reduce
duplicate resources or observer threads without putting native details in the
runtime. This would need clear lifetime, publication, and failure rules.
Peer discovery, peer handles, and registration callbacks are not part of the
current API.
