# I/O

Arty hosts external I/O drivers that implement the `arty_io_core` contracts.
It does not provide a built-in network or file I/O implementation.

Access the worker-bound `arty::io::Io` handle through `Builtins::io()`. The
first `Io::context` call for a concrete `IoContext` registers that context with
the runtime. Before returning the context, Arty clones and relocates it to every
worker and creates one driver on each worker. Concurrent registrations remain
serialized per context type, and the context is not exposed until every driver
has been created successfully.

A cloned `Builtins` or `Io` remains associated with its runtime worker and can
be used from an ordinary thread or a thread owned by a non-Arty runtime such as
Tokio. I/O access is synchronously dispatched to the associated Arty worker,
preserving correctness at higher cost than worker-local access. Context options
are created before the call and passed by reference. Arty clones and relocates
them to the associated worker if first-time registration needs them, so an
Arty-integrated context's options must implement `Clone` and `ThreadAware`.
Calls from a different Arty worker are rejected because synchronously
cross-calling workers can deadlock.

`Io::context` panics when immediate driver initialization fails. Initialization
failures, including failures discovered later on another worker, request
runtime shutdown. A normal `Driver::execute_cycle` failure does the same.

One driver context may select the primary role. Its role is fixed across the
runtime: every worker must initialize that context as primary. Other driver
contexts are secondary. Secondary drivers execute before the primary in each
worker cycle.

The [primary driver example](../examples/arty_io.rs) uses
`Scheduler::spawn_everywhere` to access one context type on every worker and
execute work through each worker-local primary driver.

During shutdown Arty attempts every initialized driver. It reports each shutdown
error through runtime telemetry, continues shutting down the remaining drivers,
and returns the first driver error from `Runtime::stop`.

Use `Scheduler::spawn_blocking` for synchronous I/O that has no Arty driver. It
runs the callback in a blocking pool and returns a join that can be awaited
without blocking an async worker.
