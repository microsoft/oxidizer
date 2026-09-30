# Panics

Remote and local task polling catches panics and transports the original payload
to the join handle. Awaiting a join, or calling `JoinHandle::wait`, resumes
that panic. Remote future-factory invocation is inside the same boundary.

A local future factory runs synchronously on its calling worker. A panic while
creating that future propagates to the calling task. Blocking-task panics are also
transported to their joins.

Dropping a join does not rethrow its task's panic elsewhere. Remote/local task
panics emit an `observed` event when a sink is configured, even if their result is
not observed. There is no separate configurable runtime panic-handler API.

Runtime capabilities are not universally `UnwindSafe` or `RefUnwindSafe`.
Catching a task panic does not repair application state or poisoned locks.
Applications remain responsible for deciding whether to stop after a panic.

Blocking runtime methods, including owner destruction, must not run on an
asynchronous worker. A blocking task may wait for asynchronous work, but cannot
wait for shutdown of its own runtime. Owner destruction from a blocking task
initiates shutdown without self-joining.

Worker startup can still panic when OS thread creation fails. The current
blocking-pool dependency does not provide fallible construction.
Scoped execution retains borrowed storage until destruction, including when
propagating a panic.
