# Design

Arty runs async tasks on dedicated workers, with separate pools for blocking
work. Tasks receive `Context` with their worker's scheduler, clock, I/O handle,
and telemetry sink.

This document describes the application-facing design. Maintainers should also
read the [runtime implementation map](INTERNALS.md) for bootstrap, ownership,
readiness, shutdown, and scoped-borrowing invariants.

## Workers and isolation

Each runtime owns its workers and shutdown. There is no process-global runtime.
Stopping one does not request shutdown of another, but application data can
still be shared. This is scheduling isolation, not a security boundary.

Each worker owns a separate OS thread. An exact worker count can exceed the
available processor count. In that case, multiple worker threads share each
processor, and the operating system schedules their execution.

An async task stays on the worker that creates its future, including across
awaits. Arty does not move running tasks to balance load. This lets futures
keep thread-local and non-`Send` state.

Tasks on a worker take turns. Blocking that worker, or running without giving
control back, holds up its other tasks and timers. Other workers can continue
independently.

## Placing new work

`Runtime::scheduler()` lets the runtime place new work.
`Context::scheduler()` submits ordinary child tasks to its worker, even when
cloned or used from another thread.

Ordinary task factories and results must be `Send`, but their futures need not
be. Create worker-local, non-`Send` state inside the task future after it reaches
the destination worker.

`spawn_anywhere` lets the runtime choose a worker.
`Scheduler::spawn_at` targets a worker coordinate obtained from
`Context::control().workers()`. Submission order does not guarantee
completion order. See the
[scheduling guide](../src/documentation/scheduling.rs) for examples.

## Thread-aware values

`ThreadAware` lets a value adapt its worker-bound state during relocation.
It includes `Send`, so moving the value must be safe even without relocation.
Moving or cloning alone does not change its worker association.

`spawn_anywhere` and `spawn_at` relocate their explicit input to the destination
worker before starting the task. On a worker's `Scheduler`, their results must
also be `ThreadAware`. Runtime-wide `spawn_anywhere` accepts any `Send` result.
Joining a worker-bound `spawn_anywhere` or `spawn_at` on an Arty worker relocates
the result to that worker. Joining outside Arty does not relocate the result.
Runtime-wide `spawn_anywhere`, ordinary `spawn`, and `block_on` do not relocate
results.

Relocation does not transfer Arty's worker-bound handles to another runtime.
See the
[thread-awareness guide](../src/documentation/thread_awareness.rs).

## Blocking work and I/O

`spawn_blocking` runs a synchronous callback away from async workers. Captures
and results must be `Send`. Workers share one blocking pool by default; the
per-worker policy separates their blocking workloads into one pool per worker,
potentially using more threads.

Arty provides timers and hosts drivers supplied by I/O libraries built on
`arty_io_core`, but it does not include a built-in network or file I/O
implementation. Access compatible contexts through `Context::io()`; see
[I/O](IO.md). Use `spawn_blocking` for synchronous I/O.

## Runtime lifetime and shutdown

Schedulers, joins, and `Context` do not keep the runtime running. Dropping a
join handle neither cancels its task nor waits for it.

`Context::control()` is the only source of `RuntimeControl`.
`RuntimeControl::request_stop` requests shutdown without waiting.
`Runtime::stop` consumes the owner, normally waits, and reports shutdown errors.
Dropping the owner also requests shutdown and normally waits, but cannot return
errors. Neither waits when called from an async Arty worker or the runtime's own
blocking callback. `stop` returns an error in those contexts.

Shutdown rejects new submissions and cancels unfinished async tasks on their
worker. Rejected factories and queued blocking callbacks do not run.
Already-running callbacks finish before shutdown completes. One that never
returns can prevent it.

The first shutdown request sets the shutdown cause. Later requests cannot
replace it. If a runtime failure starts shutdown, cancelled task joins retain
the failure in their error source chain.

Await required work before requesting shutdown or returning from an
`#[arty::main]` or `#[arty::test]` body. See the
[shutdown guide](../src/documentation/shutdown.rs) and
[critical-task guidance](CRITICAL_TASKS.md).

### Cloned task wakers

A task remains assigned to one worker. Calling a clone of its waker from another
thread asks that worker to poll the task again; it never moves or polls the task
on the calling thread.

A cloned waker may outlive both its task and the runtime. Completion and
cancellation disconnect every clone from the task before the task's resources
are reclaimed. Later calls are safe no-ops, and keeping a clone does not delay
shutdown. This lifetime separation is both a memory-safety invariant and part of
the runtime's shutdown contract.

## Failure handling

Application errors remain task results. A caught panic becomes `JoinError`.
By default, it also starts coordinated runtime shutdown. `PanicPolicy::ignore()`
keeps the runtime running, but it does not repair shared state. Worker isolation
does not make every panic recoverable: some failures must stop the process.
Failure to initialize or execute an I/O driver stops the runtime.

Runtime telemetry uses a no-op sink by default. Configured telemetry processors
must not panic. See [panic handling](PANICS.md) for how task, shutdown, and
telemetry failures are handled.
