# Panic policy

With `panic = "unwind"`, Rust cleans up the stack after a panic.
Arty can then catch panics from three user operations:

- Calling a task factory, which creates the async task's future.
- Polling an async task to let it make progress.
- Running a blocking function on a separate thread.

Arty reports these panics through `JoinError` when possible. By default, the
first panic that Arty catches from these operations shuts down the runtime.
`PanicPolicy::ignore()` keeps the runtime running.

A waker tells a worker that a task is ready to run again.
Do not rely on Arty to recover from panics in cleanup or notification code.
This includes `Drop` implementations, custom wakers, and code that announces task completion.
Arty also does not repair state marked unusable after a panic.
It does not recover from broken internal assumptions or violations of
requirements for unsafe code.

Arty can catch some of these panics to maintain memory safety. Callers cannot
rely on continued operation or complete cleanup. The process can terminate without finishing cleanup.
Resources can remain allocated, or affected tasks and runtime operations can stop making progress.
Arty remains memory-safe under all panic conditions.
