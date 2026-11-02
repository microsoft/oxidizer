# I/O

Arty can run I/O drivers supplied by other crates. It does not include a
built-in network or file I/O implementation.
A driver processes I/O requests for a worker.
An I/O context gives your code access to the I/O library.

Use `Context::io()` to access the worker's I/O contexts through `arty::io::Io`.
Then call `Io::context` with the context type provided by the I/O library.

Arty creates each context type once per runtime. Each worker creates its own
driver the first time that worker requests the context. Requesting a
context on one worker does not create drivers on every worker.
The first request that creates a context determines its options.
Later requests ignore different options for that context type.

The driver for the first context type registered in the runtime is primary.
It can wait for I/O.
Drivers for later context types are secondary and must not wait for I/O.
A worker runs its secondary drivers before its primary driver.

Cloning a `Context` or `Io` value keeps its worker assignment.
When Arty passes the value to another worker in the same runtime,
`ThreadAware` updates its state for that worker.
You can call `Io::context` from a thread outside Arty.
This includes tasks on another runtime such as Tokio.
Arty sends the request to the assigned worker and waits for the response.

Arty rejects context access from a different Arty async worker.
Otherwise, two workers can wait for each other and never finish.
This restriction applies to async workers in the same runtime and in different runtimes.

If creating or running a driver fails, Arty stops the runtime.
If creating a driver fails, `Io::context` panics.

The [primary driver example](../examples/arty_io.rs) accesses one context type
on every worker. It uses `RuntimeControl::workers` with `Scheduler::spawn_at`.
Each task uses the driver on its own worker.

During shutdown, Arty cancels worker tasks before shutting down their drivers.
It attempts every driver that started, even if one shutdown fails. `Runtime::stop`
does not return driver shutdown errors.

For synchronous I/O without an Arty driver, use `Scheduler::spawn_blocking`.
The function runs on a separate thread without blocking an async worker.
