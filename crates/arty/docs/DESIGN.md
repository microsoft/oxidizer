# Design

Arty uses single-threaded execution for each asynchronous task. A runtime can
have several worker threads, but a task stays on the worker that creates its
future. It can keep thread-local and non-`Send` state across asynchronous waits.

A cross-thread submission sends a factory to a worker, where the factory creates
the future. Local submissions also support non-`Send` captures and results.
Blocking callbacks run in a separate pool so they do not stall asynchronous work.

Thread-aware values carry worker coordinates. Explicit relocation can update
those coordinates when submitting new work elsewhere; it does not move a
running task or its returned value.

The runtime owner controls shutdown. Scheduler and capability handles do not
keep it alive. Shutdown rejects new submissions, cancels pending asynchronous
work, and waits for already-running blocking callbacks.
