# I/O

Arty can run I/O drivers supplied by other crates. It does not include a
built-in network or file I/O implementation.

Use `Context::io()` to obtain the worker's `arty::io::Io` handle, then call
`Io::context` with the context type provided by the I/O library.

Arty creates each context type once per runtime. Each worker creates its own
driver lazily, the first time that worker requests the context. Requesting a
context on one worker does not initialize it on every worker.

The first context type registered with the runtime is the primary driver.
Later context types are secondary drivers. A worker runs its secondary drivers
before its primary driver.

A cloned `Context` or `Io` remains associated with its original worker. It can
be used from an ordinary thread or from another runtime such as Tokio. Arty
dispatches the request back to the associated worker. Arty rejects calls from
all other Arty workers to prevent reciprocal synchronous-dispatch deadlocks.
This rule applies to workers in the same runtime and in different runtimes.

Failure to initialize or run a driver stops the runtime. `Io::context` reports
an initialization failure by panicking.

The [primary driver example](../examples/arty_io.rs) accesses one context type
on every worker. It uses `RuntimeControl::workers` with `Scheduler::spawn_at`.
Each task uses its worker-local driver.

During shutdown Arty cancels worker tasks before shutting down their drivers.
It attempts every initialized driver even if one shutdown fails. `Runtime::stop`
does not return driver shutdown errors.

For synchronous I/O without an Arty driver, use `Scheduler::spawn_blocking`.
The callback runs in a blocking pool without blocking an async worker.
