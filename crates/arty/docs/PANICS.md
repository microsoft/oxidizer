# Panic hooks

With `panic = "unwind"`, Rust cleans up the stack after a panic.
Arty can then catch panics from these user operations:

- Calling a task factory, which creates the async task's future.
- Polling an async task to let it make progress.
- Running a blocking function on a separate thread.

Arty reports these panics through `JoinError` when possible. By default, an
async task panic shuts down the runtime. Register `Panic` and call
`Panic::ignore` in its callback to keep the runtime running:

```rust
use arty::runtime::Runtime;
use arty::runtime::hook::Panic;

let runtime = Runtime::builder()
    .on(Panic::IGNORE)
    .build()?;
# runtime.stop()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

A blocking function panic does not create a `Panic` event or stop the
runtime. Its join handle still returns the original panic payload.

`JoinError::is_panic()` identifies the task that panicked.
`JoinError::into_panic()` returns its original panic payload.
For tasks cancelled or rejected by shutdown, `JoinError::shutdown_cause()`
identifies two causes through `arty::runtime::ShutdownCause`.
`Requested` means that shutdown started without an error.
`Panic` means that a contained task panic started shutdown.

Other runtime failures return `None`. For these failures,
`JoinError::is_shutdown()` still returns `true`, and
`std::error::Error::source()` retains the failure.
The method also returns `None` for a direct task panic or an invalid destination.
The first shutdown cause remains authoritative. Later failures do not replace it.
Requested shutdown has no error source. For abnormal shutdown, inspect
`std::error::Error::source()` on the join error.
For tasks cancelled or rejected because another task panicked, the source's
`Display` output includes the original string panic payload.
Other panic payloads use the diagnostic `non-string panic payload`.

A waker tells a worker that a task is ready to run again.
Do not rely on Arty to recover from panics in cleanup or notification code.
This code includes `Drop` implementations, custom wakers, and task-completion
notifications.
Arty also does not repair state marked unusable after a panic.
It does not recover from broken internal assumptions or violations of
requirements for unsafe code.

Arty can catch some of these panics to maintain memory safety. Callers cannot
rely on continued operation or complete cleanup. The process can terminate
before cleanup finishes. Resources can remain allocated. Affected tasks and
runtime operations can stop making progress.
Arty remains memory-safe under all panic conditions.
