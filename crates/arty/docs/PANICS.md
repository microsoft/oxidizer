# Panics

When unwinding is enabled, Arty reports task-factory, task-body, and blocking
callback panics through `JoinError`. Receiving that error does not resume the
panic or repair the task's application state. With `panic = "abort"`, a panic
aborts the process instead.

Async result delivery is also covered when Arty discards a result or panic
payload because its join was dropped. A panicking join-notification callback
does not discard an already-delivered result. Destroying a value or error
already owned by the caller is the caller's responsibility.

During shutdown, Arty contains sole destructor panics in cancelled async
futures and accepted queued factories, then continues cleanup. Their joins
still report cancellation. This does not repair shared state or make a panic
during another unwind recoverable.

Discarding a panic payload can itself panic. Repeated disposal failures are a
last-resort termination case, not a guarantee of recovery from arbitrary
programming errors.

A panic that escapes task handling while the runtime still owns live task
storage stops the process rather than risking memory corruption. This safety
boundary is not permission to continue using panic-damaged state.

Dropping a join neither resumes a panic nor cancels the task. Runtime shutdown
cancels pending tasks; their joins return a shutdown error, not a task panic.

`RuntimeScheduler::block_on` reports task failure through `runtime::Error`,
with the `JoinError` retained as its source. It returns an error, rather than
panicking, if called from an asynchronous Arty worker or inside an
already-running `futures` executor.

`Runtime::stop` consumes the owner and returns `Result<(), runtime::Error>`.
Worker failures are returned after all workers have been joined. An asynchronous
worker or the runtime's own blocking callback cannot wait for its shutdown;
calling `stop` there requests shutdown but returns an error without waiting.

Dropping the runtime owner requests shutdown and normally waits for it. It
cannot return a shutdown error; worker failures remain available through their
diagnostics. On any asynchronous Arty worker or the runtime's own blocking
callback, dropping the owner requests shutdown without waiting or panicking.
Workers finish their cleanup independently, so `drop` in those contexts does
not guarantee that shutdown has completed.

Polling a blocking join from a callback running in the same blocking pool
panics instead of allowing a direct pool-starvation cycle. Polling any join
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

Configured `observed` processors must not panic. Event delivery is synchronous,
and Arty does not recover from panics in its telemetry delivery.
