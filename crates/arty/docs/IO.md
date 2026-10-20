# I/O

Arty hosts external I/O drivers that implement the `arty_io_core` contracts.
It does not provide a built-in network or file I/O implementation.

Access the worker-bound `arty::io::Io` handle through `Builtins::io()`. The
first `Io::context` call for a concrete `IoContext` creates and registers one
thread-neutral context for the runtime. The first context type registered is
assigned the primary driver role; later context types are assigned the
secondary role.

Each worker initializes contexts and drivers independently when it first
accesses a registered context type. Arty clones and relocates the registered
context to that worker, creates its worker-local driver, and returns the local
context only after initialization succeeds. Other workers remain
uninitialized until they access that context type. Concurrent first accesses
create the registered context once while allowing each worker to initialize
its own driver.

A cloned `Builtins` or `Io` remains associated with its runtime worker and can
be used from an ordinary thread or a thread owned by a non-Arty runtime such as
Tokio. I/O access is synchronously dispatched to the associated Arty worker,
preserving correctness at higher cost than worker-local access. Context options
are created before the call and passed by reference. Arty clones them when an
external-thread request must cross the worker boundary. Calls from a different
Arty worker are rejected because synchronously cross-calling workers can
deadlock.

`Io::context` panics when its worker-local driver initialization fails. Any
driver initialization failure, including one discovered later by another
worker, marks the runtime's I/O state failed and requests runtime shutdown. A
normal `Driver::execute_cycle` failure does the same.

The primary context type keeps that role on every worker that initializes it.
Other context types are secondary. Secondary drivers execute before the primary
in each worker cycle.

The [primary driver example](../examples/arty_io.rs) uses
`Scheduler::spawn_everywhere` to access one context type on every worker and
execute work through each worker-local primary driver.

During shutdown Arty cancels and drops worker tasks before dropping the worker's
driver host. It then attempts every initialized driver, reports each shutdown
error through runtime telemetry, and continues shutting down the remaining
drivers. Driver shutdown is best effort and does not add a shutdown result to
the public runtime API.

Use `Scheduler::spawn_blocking` for synchronous I/O that has no Arty driver. It
runs the callback in a blocking pool and returns a join that can be awaited
without blocking an async worker.
