# Design

Arty runs async tasks on dedicated worker threads. It uses separate thread pools
for blocking work. Each task receives `Context` with its scheduler, clock, I/O handle, and runtime
control.

This document describes the application-facing design.

## Runtime and workers

Each `Runtime` owns its workers and shutdown state. Arty has no process-global
runtime. Stopping one runtime does not stop another runtime.

Each async worker runs on its own operating-system thread. An exact worker count
can exceed the available processor count. The operating system then schedules
multiple worker threads on each processor.

An async task stays on the worker that creates its future. Arty does not move a
running task to balance load. The future can keep thread-local and non-`Send`
state.

Tasks on one worker take turns. A task that blocks the thread or does not yield
also delays other tasks and timers on that worker. Other workers can continue.

Worker isolation controls scheduling. It is not a security boundary, and
workers can share application data.

## Scheduling work

`Runtime::scheduler()` returns a `RuntimeScheduler`. It lets the runtime choose
where to place work.

`Context::scheduler()` returns a worker-bound `Scheduler`. Its ordinary
`spawn` method keeps child tasks on that worker. A cloned `Scheduler` still
targets the same worker when another thread uses it.

Task factories, captured values, and results cross thread boundaries and must
implement `Send`. The future does not need to implement `Send`. Create
worker-local and non-`Send` state inside the future.

`RuntimeScheduler::spawn_anywhere` lets the runtime select a worker.
`Scheduler::spawn_at` targets a worker coordinate from
`Context::control().workers()`. Submission order does not set completion
order.

See the [scheduling guide](../src/documentation/scheduling.rs) for examples.

## Thread-aware values

`ThreadAware` extends `Send` with relocation for worker-bound state. Moving or
cloning a value without relocation does not change its worker association.

`Scheduler::spawn_anywhere` and `Scheduler::spawn_at` relocate their explicit
input before the task starts. Results from these methods must also implement
`ThreadAware`.

When an Arty worker joins one of these tasks, Arty relocates the result to that
worker. Joining from an external thread does not relocate the result.
`RuntimeScheduler::spawn_anywhere`, ordinary `spawn`, and `block_on` do not
relocate results.

Relocation does not transfer an Arty handle to another runtime. See the
[thread-awareness guide](../src/documentation/thread_awareness.rs).

## Blocking work

`spawn_blocking` runs a synchronous callback outside the async worker. Its
captured values and result must implement `Send`.

All workers share one blocking pool by default. The per-worker policy creates
one pool for each async worker. This policy separates blocking workloads but
can use more threads.

Already-running blocking callbacks finish during shutdown. Queued callbacks do
not have to start. A callback that never returns can prevent shutdown from
finishing.

## I/O

Arty hosts drivers from I/O libraries that use `arty_io_core`. It does not
include a network or file I/O implementation.

Use `Context::io()` to access a compatible I/O context. Arty creates each
context type once per runtime. Each worker creates its own driver when it first
requests that context.

The first registered context type provides the primary driver. Later context
types provide secondary drivers. A worker runs secondary drivers before its
primary driver.

A cloned `Context` or I/O handle remains bound to its original worker. An
ordinary thread can use it because Arty sends the request to that worker. Arty
rejects access from another async worker to prevent synchronous dispatch cycles.

Use `spawn_blocking` for synchronous I/O. See the [I/O guide](IO.md) for driver
and shutdown behavior.

## Runtime lifetime and shutdown

Schedulers, join handles, and `Context` do not keep the runtime running.
Dropping a join handle does not cancel its task or wait for it.

`Context::control()` provides `RuntimeControl`.
`RuntimeControl::request_stop` requests shutdown without waiting.
`Runtime::stop` consumes the owner, waits for shutdown, and returns shutdown
errors. Dropping the owner also requests shutdown and waits, but it cannot
return an error.

The runtime owner must stay on an external thread. `Runtime::stop` and
`Runtime::drop` panic on async and blocking workers. Worker code must use
`RuntimeControl::request_stop`.

Shutdown rejects new submissions and cancels unfinished async tasks. It also
discards rejected task factories and queued blocking callbacks.

The first shutdown request sets the shutdown cause. Later requests do not
replace it. If a runtime failure starts shutdown, cancelled task joins retain
that failure in their error source chain.

Complete required work before requesting shutdown. With `#[arty::main]` or
`#[arty::test]`, complete that work before the function returns. See the
[shutdown guide](../src/documentation/shutdown.rs).

## Task wakers

A task stays on one worker. A cloned task waker asks that worker to poll the task
again. It does not move the task or poll it on the calling thread.

A cloned waker can outlive its task and runtime. Completion and cancellation
disconnect each clone before Arty reclaims task resources. Later wake calls do
nothing, and a retained clone does not delay shutdown.

## Panics

Application errors remain task results. Arty reports a caught task or blocking
callback panic through `JoinError` when possible.

By default, the first caught panic requests coordinated runtime shutdown.
`PanicPolicy::ignore()` keeps the runtime running, but it cannot repair shared
application state.

Arty does not guarantee recovery from a panic in a destructor, custom waker,
join notification, or cleanup hook. Such a panic can abort the process or stop
runtime progress. Failure to initialize or run an I/O driver stops the runtime.

See [panic handling](PANICS.md) for the full policy.
