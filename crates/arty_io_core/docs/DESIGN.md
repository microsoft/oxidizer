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
| `DriverProvider` | Creates a `DriverInstance` on each worker. |
| `DriverInstance` | Packages the worker-local driver, context, and driver's selected role. |
| `Driver` | The worker-local part that processes submissions and completions. |
| Runtime | Gives drivers turns and coordinates when the worker may wait or continue. |

Contexts can outlive drivers. The runtime calls each driver with exclusive
access on its owning worker; providers decide whether workers share queues, memory, or
threads. Context relocation is an optimization, not a correctness requirement.

## A walkthrough: two libraries, one worker

Imagine a network driver and a file driver sharing a worker.

### Make each library ready before handing it to the application

The first context request creates a `DriverInstance` on every active worker.
Before publishing it, the runtime runs a non-blocking initialization cycle.
The request returns only when all workers are ready, so application code cannot
see a half-initialized driver.

The runtime passes `DriverOptions::allowed_roles()` as a role permission set to
the provider.
The returned `DriverInstance::role` is the driver's selected role; it is not a
second runtime assignment. The runtime accepts only a selection permitted by
the worker's capacity and the provider's permission. Later requests reuse the
registration. Concrete context types distinguish registrations, allowing
different driver versions to coexist.

### Give every driver a turn before sleeping

A logical cycle is one coordinated pass across the drivers. The runtime passes
each driver a mutable `Cycle` containing its wait bound. At registration, the
runtime permits at most one driver to select **primary**. **Secondaries** run
first and return promptly; the primary runs last and may wait up to the
runtime's wait bound. A zero bound means no waiting. Without a primary,
parking remains the runtime's responsibility. `Cycle` is deliberately
non-`Send` and non-`Sync` so the mutable cycle stays on the owning worker.

Bounded batches keep one driver from monopolizing the worker. Immediately
serviceable work requests another cycle; unfinished I/O alone does not, avoiding
a busy loop while waiting for the operating system.

### Let either library wake the worker

Suppose the file driver has a completion while the primary network driver
waits on the worker. The file driver must make that completion visible without
depending on an unrelated network event.

The runtime obtains the driver's notification path from `Driver::waker()`.
A secondary can coordinate its completion source with the primary's native wait
and use that path to wake the worker. Alternatively, it can continuously drain
completions on independent driver-owned background execution and use its waker
to notify the runtime. A secondary must return promptly from its worker-local
`execute_cycle`; merely leaving a completion for the next cycle is not enough
when the primary can block.

Waking the worker does not itself drain a native completion queue. The driver
retains ownership of the state needed to process completions, and its next
worker-local invocation performs bounded completion processing. A notification
racing wait entry or the final pre-wait check must remain observable rather than
being discarded. The exact wake-up obligations are specified in
[R5](REQUIREMENTS.md#r5-reliable-wake-ups).

### Keep scheduling decisions in the runtime

The runtime owns the worker's parking policy. Drivers supply their notification
paths and own synchronization for native completion state.

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
registration and driver roles only. Its drivers perform no I/O and return no-op
wakers, not a reference implementation of completion coordination.
