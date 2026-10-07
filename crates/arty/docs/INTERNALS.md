# Runtime implementation map

This document is for maintainers. It connects the cross-module ownership and
coordination invariants behind Arty's public runtime behavior.

## Bootstrap and readiness

[`runtime/bootstrap/startup.rs`](../src/runtime/bootstrap/startup.rs) starts
workers in stable processor order. Each worker first publishes a
`WorkerEndpoint`: its command sender, wake handle, thread coordinate, and
blocking worker. Bootstrap collects every endpoint before constructing the
dispatcher, then distributes a `DispatcherClient` back to every worker through
its start channel.

A worker publishes its immutable service bundle into the corresponding
`SharedState` slot during initialization. The slot ordering matches dispatcher
worker indexes; [`runtime/context.rs`](../src/runtime/context.rs) documents this
write-once relationship. Bootstrap waits for every worker's success signal
before returning the `Runtime`, so public scheduling starts with endpoints and
worker services ready.

OS thread-creation errors and cleanup after partial startup remain deferred to a
separate lifecycle/error design. The accepted scope decision is recorded in
[the startup review discussion](https://github.com/microsoft/oxidizer/pull/785#discussion_r4183811801).

## Dispatch and ownership

[`runtime/dispatch/dispatcher_core.rs`](../src/runtime/dispatch/dispatcher_core.rs)
owns the complete endpoint set and maps runtime thread IDs to worker indexes.
`DispatcherClient` is the cloneable capability used by schedulers and workers;
it does not own a separate runtime or worker set.

`Runtime` owns the dispatcher and the worker-shutdown waiter. Scheduler,
`Builtins`, join, clock, and operations handles can outlive individual tasks,
but they do not keep the runtime running. `Runtime::stop` and `Runtime::drop`
request shutdown through the dispatcher; the waiter joins async workers, and
each worker joins its blocking pool before its thread exits.

## Worker command lifecycle

[`runtime/worker/async_worker.rs`](../src/runtime/worker/async_worker.rs)
combines command admission, bounded command batches, executor polling, timer
advancement, and shutdown. The dispatcher publishes shutdown before queuing
stop commands so workers can reject or dispose queued factories without waiting
for FIFO delivery. The worker must complete executor retirement before its
unsafe executor storage is dropped.

## Scoped borrowed work

[`task/runtime_scheduler.rs`](../src/task/runtime_scheduler.rs) lets
`RuntimeScheduler::block_on` run a future that borrows synchronous caller data.
`ScopedStorage` owns the borrowing factory or future and a completion sender.
`ScopedJoin` cannot return or unwind until the receiver observes destruction of
that storage. The field order and the safety comment beside the lifetime
extension are part of the invariant: task completion alone is not sufficient;
the borrowing future must be destroyed before the caller's stack is released.
