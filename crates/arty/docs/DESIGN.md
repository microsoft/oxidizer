# Design

Arty runs async tasks on dedicated workers, with separate pools for blocking
work. Tasks receive `Builtins` with their worker's scheduler, clock, and
telemetry sink.

This document describes the application-facing design. Maintainers should also
read the [runtime implementation map](INTERNALS.md) for bootstrap, ownership,
readiness, shutdown, and scoped-borrowing invariants.

## Workers and isolation

Each runtime owns its workers and shutdown. There is no process-global runtime.
Stopping one does not request shutdown of another, but application data can
still be shared. This is scheduling isolation, not a security boundary.

An async task stays on the worker that creates its future, including across
awaits. Arty does not move running tasks to balance load. This lets futures
keep thread-local and non-`Send` state.

Tasks on a worker take turns. Blocking that worker, or running without giving
control back, holds up its other tasks and timers. Other workers can continue
independently.

## Placing new work

`Runtime::scheduler()` lets the runtime place new work.
`Builtins::scheduler()` submits ordinary child tasks to its worker, even when
cloned or used from another thread.

Ordinary task factories and results must be `Send`, but their futures need not
be. Create worker-local, non-`Send` state inside the task future after it reaches
the destination worker.

`spawn_anywhere` lets the runtime choose a worker;
`Scheduler::spawn_everywhere` starts one task per worker. Submission order
does not guarantee completion order. See the
[scheduling guide](../src/documentation/scheduling.rs) for examples.

## Thread-aware values

`ThreadAware` lets a value adapt its worker-bound state during relocation.
It includes `Send`, so moving the value must be safe even without relocation.
Moving or cloning alone does not change its worker association.

`spawn_anywhere` and `spawn_everywhere` relocate their explicit input to the
destination worker before starting the task. On a worker's `Scheduler`,
their results must also be `ThreadAware`. Runtime-wide `spawn_anywhere` accepts
any `Send` result. Joining does not relocate results; ordinary `spawn` and
`block_on` do not relocate captures or results either.

Relocation does not transfer Arty's worker-bound handles to another runtime.
See the
[thread-awareness guide](../src/documentation/thread_awareness.rs).

## Blocking work and I/O

`spawn_blocking` runs a synchronous callback away from async workers. Captures
and results must be `Send`. Workers share one blocking pool by default; the
per-worker policy separates their blocking workloads into one pool per worker,
potentially using more threads.

Arty provides timers, but not async network or file I/O drivers. An async I/O
library still needs its own runtime support. See [I/O](IO.md).

## Runtime lifetime and shutdown

Schedulers, joins, and `Builtins` do not keep the runtime running. Dropping a
join handle neither cancels its task nor waits for it.

`RuntimeOperations::request_stop` requests shutdown without waiting.
`Runtime::stop` consumes the owner, normally waits, and reports shutdown errors.
Dropping the owner also requests shutdown and normally waits, but cannot return
errors. Neither waits when called from an async Arty worker or the runtime's own
blocking callback; `stop` returns an error in those contexts.

Shutdown rejects new submissions and cancels unfinished async tasks on their
worker. Rejected factories and queued blocking callbacks do not run.
Already-running callbacks finish before shutdown completes; one that never
returns can prevent it.

Await required work before requesting shutdown or returning from an
`#[arty::main]` or `#[arty::test]` body. See the
[shutdown guide](../src/documentation/shutdown.rs) and
[critical-task guidance](CRITICAL_TASKS.md).

## Failure handling

Application errors remain task results. A caught panic becomes `JoinError`;
that alone does not stop the runtime or repair shared state. Worker isolation
does not make every panic recoverable: some failures must stop the process.

Runtime telemetry uses a no-op sink by default. Configured telemetry processors
must not panic. See [panic handling](PANICS.md) for how task, shutdown, and
telemetry failures are handled.
