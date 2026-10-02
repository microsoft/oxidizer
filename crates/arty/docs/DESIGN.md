# Design

An Arty runtime can use several workers, but each async task stays on the
worker that creates its future. This lets the task keep thread-local and
non-`Send` state across awaits.

For new work, Arty creates the future on the worker where it will run.
Local tasks can also have non-`Send` captures and results. Blocking calls
run in a separate pool so they do not stall async tasks.

Thread-aware values can relocate worker-bound state when new work starts
elsewhere. Relocation does not move a running task or its result.

The runtime owner controls shutdown. Scheduler and capability handles do not
keep it alive. Shutdown rejects new submissions, cancels pending async
work, and waits for already-running blocking callbacks.
