# Design

This document describes the component boundaries and invariants behind Arty.
The crate and module documentation describe how to use the public API.

## Runtime boundary

Each `Runtime` owns its workers and shutdown state. Arty has no process-global
runtime. Stopping one runtime does not stop another runtime.

Schedulers, join handles, and `Context` do not own the runtime. The runtime
owner waits for worker shutdown and reports runtime failures.

## Worker model

Each async worker runs a cooperative executor on one operating-system thread.
A task stays on the worker that creates its future. Arty does not move a running
task to balance load.

The worker-local future can keep thread-local and non-`Send` state. A task that
blocks the thread or does not yield also delays other tasks and timers on that
worker. Other workers can continue.

Worker isolation controls scheduling. It is not a security boundary. Workers
can share application data.

## Routing and relocation

`RuntimeScheduler` submits work without a worker association. `Scheduler` stays
bound to one worker. Task factories and values that cross a thread boundary use
the bounds on the selected scheduling method.

`ThreadAware` provides explicit relocation for worker-bound state. Relocation
does not move a running task or transfer an Arty handle to another runtime.
Joining outside an Arty worker does not relocate a result.

A cloned task waker routes another poll to the assigned worker. It does not move
the task or poll it on the calling thread. A retained clone does not keep the
task or runtime alive. After task completion, panic, cancellation, or runtime
shutdown, calling or cloning the retained waker is safe and has no effect.

## Supporting services

Blocking callbacks run outside async workers. The runtime can use one shared
pool or one pool per worker. A running callback can delay shutdown until it
returns.

Arty hosts I/O contexts once per runtime and drivers once per worker. The
[I/O guide](IO.md) describes driver roles, access rules, failures, and shutdown.
Worker clocks drive timers for tasks assigned to that worker.

## Shutdown and failure

The first shutdown request sets the shutdown cause. Later requests do not
replace it. Shutdown rejects new work, cancels pending async tasks, and discards
queued blocking callbacks.

Worker code requests shutdown without waiting. The external runtime owner waits
for workers. A runtime failure remains available through runtime and task error
chains.

Arty catches panics only at documented user-code boundaries. Other panic
sources can stop progress or abort the process. See [panic handling](PANICS.md)
for the full policy.
