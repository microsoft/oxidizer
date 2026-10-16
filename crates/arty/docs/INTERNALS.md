# Runtime implementation map

This document is for maintainers. It connects the cross-module ownership and
coordination invariants behind Arty's public runtime behavior.

## Bootstrap and readiness

[`runtime/bootstrap/startup.rs`](../src/runtime/bootstrap/startup.rs) starts
workers in stable processor order. Each worker first publishes a
`WorkerEndpoint`: its command sender, wake handle, and thread coordinate.
Bootstrap collects every endpoint before constructing the dispatcher, then
distributes a `DispatcherClient` back to every worker through its start channel.

Each worker constructs its `Builtins` after receiving the shared dispatcher.
Bootstrap waits for every worker's success signal before returning the
`Runtime`, so public scheduling starts with endpoints and worker services ready.

OS thread-creation errors and cleanup after partial startup remain deferred to a
separate lifecycle/error design. The accepted scope decision is recorded in
[the startup review discussion](https://github.com/microsoft/oxidizer/pull/785#discussion_r4183811801).

## Dispatch and ownership

[`runtime/dispatch/dispatcher_core.rs`](../src/runtime/dispatch/dispatcher_core.rs)
owns the complete endpoint set and maps runtime thread IDs to worker indexes.
`DispatcherClient` is the cloneable capability used by schedulers and workers;
it does not own a separate runtime or worker set.

`Runtime` owns the dispatcher, the worker-shutdown waiter, and the single
`vicinal::Pool` used for blocking work. Each async worker and the dispatcher
hold scheduler values for that pool. Scheduler, `Builtins`, join, clock, and
operations handles can outlive individual tasks, but they do not keep the
runtime running. `Runtime::stop` and `Runtime::drop` request shutdown through
the dispatcher; workers drop their scheduler values during shutdown, the
waiter joins async workers, and dropping the runtime-owned pool completes
blocking-pool shutdown. Pool destruction is handed to a helper thread only when
the runtime owner is dropped from an async Arty worker. A Vicinal blocking
worker does not carry that marker, so dropping the owner from its own blocking
callback is unsupported and can self-wait.

## Intentional blocking compatibility boundary

Arty guarantees that unwinding from the `spawn_blocking` callback body is
reported as `JoinError`. The Vicinal integration intentionally does not recreate
the former blocking-worker machinery for these uncommon edge cases:

- panics while destroying an abandoned blocking result;
- panics while destroying a cancelled blocking callback capture;
- panics from a blocking join's receiver waker;
- runtime-owner destruction from inside the runtime's own blocking callback.

Do not reintroduce those checks or compatibility shims only to restore old
integration tests. The simpler lifecycle is the current design. Add support
later only when a concrete caller requirement justifies the extra ownership,
panic-containment, and reentrancy machinery.

### Shutdown guard tests

Tests that consume or drop a runtime owner on an async worker must retain a
dispatcher on the test thread and call its `wait` method after the worker-side
operation. Joining a task or a different runtime does not join that runtime's
workers. Cancellation can also destroy captured runtime owners on the worker,
so each nested runtime needs its own completion barrier.

The consuming-owner cases live in `runtime/handle/worker_wait_guards_tests.rs`
because the public API does not expose a completion handle after consuming the owner.
The public cancellation test retains its runtime owner outside the cancelled
task and calls `stop` from the test thread. Both forms keep the worker wait
guards active and join every runtime before returning.

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
