# Panics

When unwinding is enabled, Arty reports task-factory, task-body, and blocking
callback panics through `JoinError`. Receiving that error does not resume the
panic or repair the task's application state. With `panic = "abort"`, a panic
aborts the process instead.

Dropping a join neither resumes a panic nor cancels the task. Runtime shutdown
cancels pending tasks; their joins return a shutdown error, not a task panic.

`RuntimeScheduler::block_on` reports task failure through `runtime::Error`,
with the `JoinError` retained as its source. It returns an error, rather than
panicking, if called from an asynchronous Arty worker.

`Runtime::stop` consumes the owner and returns `Result<(), runtime::Error>`.
Worker failures are returned after all workers have been joined. An asynchronous
worker or the runtime's own blocking callback cannot wait for its shutdown;
calling `stop` there requests shutdown but returns an error without waiting.

Dropping the runtime owner requests shutdown and normally waits for it. It
cannot return a shutdown error; worker failures remain available through their
diagnostics. Dropping it from its own blocking callback does not wait for that
callback. Dropping it on an asynchronous Arty worker panics.

`JoinHandle::wait` still panics on an asynchronous Arty worker. Polling a join
again after receiving its result, or using a local scheduler outside its
worker's local context, is also a programming error that can panic.

The `main` and `test` attributes stop their runtime before returning the body's
value or resuming its original panic on the calling thread. Construction
errors, root cancellation, and shutdown errors become panics; a root failure
takes precedence if shutdown also fails. An application error returned by the
body remains its return value.

Explicit construction returns configuration errors, although worker-thread
creation and initialization can still panic. Use explicit construction,
`block_on`, and `stop` when the caller needs to handle returned runtime errors.
