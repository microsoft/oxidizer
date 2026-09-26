# Design

## Purpose

`arty_io_core` defines shared contracts between independently versioned I/O
drivers and thread-aware runtimes; it provides neither implementation.
Runtimes own registration, scheduling, and cycle coordination; drivers own
native I/O and queues. Each owns its synchronization.
[Requirements](REQUIREMENTS.md) lists their obligations.

## Public model

```text
IoContext::provider(ProviderOptions)
    -> DriverProvider: clone and relocate to each worker
    -> DriverProvider::create(DriverOptions)
    -> worker-owned Driver + consumer-held IoContext
```

Consumers access I/O through `IoContext`; runtimes call worker-local drivers
to process submissions and completions with exclusive access. Context relocation
may optimize placement but cannot affect correctness. Providers choose whether
instances share state.

`Driver` is dyn-compatible; consuming `shutdown` is not callable through
`dyn Driver`. A private owning shim may store heterogeneous pairs and dispatch
shutdown.

## Registration stays in the runtime

The runtime keys registration by concrete context type. The first request
initializes every active worker's pair; later requests clone existing contexts.
Different driver versions may coexist using compatible `arty_io_core` types.

For each worker, the runtime:

1. Clones and relocates the provider.
2. Assigns a fixed role and supplies earlier peers through `DriverOptions`.
3. Creates the pair without publishing its context.
4. Completes a separate zero-wait cycle with `can_block = false`.
5. Stores the pair, notifies earlier drivers in registration order, then
   acknowledges registration.

Peer handles are borrowed and worker-local. Drivers may downcast them and clone
independently owned state, but cannot retain the borrow.

Each worker has at most one primary, selected only from providers with
`CAN_BE_PRIMARY = true`; other drivers are secondaries. Without a primary,
the runtime owns worker parking.

## Execution and waiting

The runtime begins coordination once per logical cycle, before checking work,
never between driver calls. Secondaries run before the primary. All receive
the same `started_at` snapshot and `max_wait` bound.

Only `can_block = true` permits a worker wait, even for the primary.
Secondaries may arm background waits but must return promptly without joining
them.

`Cycle` mutably borrows the runtime's `PendingWorkTracker`. Before entering or
scheduling a native wait, the driver calls `Cycle::start_work(interrupt)`.
The tracker enrolls work in the completion barrier and installs its interruption
waker before returning. If already interrupted, it invokes the waker before
returning.

Native signals remain latched across wait entry.
Wakers may run inline on any thread; they must signal promptly without
panicking, joining work, or acquiring locks held by completing work.

Each `PendingWork` owns one barrier participation. Keep it until work ends;
publish results before completing or dropping it.

Completing and dropping are equivalent: either invokes the runtime's notification
waker exactly once. The notification retires this work's interruption
registration, interrupts other waits, then releases only this participation.
It must not affect later cycles. Waker cloning and dropping do not complete work.

After the primary returns, the runtime interrupts remaining waits and waits for
every handle before advancing.

Drivers process bounded batches. Immediately serviceable work remaining requests
another cycle; in-flight operations alone do not.

The runtime implements barrier counters, interruption state, and parking.

## Shutdown

`Driver::shutdown` consumes the driver, closes admission, and drains operations,
callbacks, and observers within a bounded wait, or returns `ShutdownError`.

- Contexts may outlive the driver as closed handles that reject new operations;
  they do not delay draining.
- Native operation storage stays alive until native access has ended.
  Cancellation alone is not proof that it has ended.
- Normal cycles have stopped. Progress must be local or on independent threads,
  not through another driver serialized on the same worker.
- The runtime keeps `SystemTaskSpawner` available until all shutdown calls
  return and attempts remaining drivers after an error.
- Dropping always remains memory-safe, including after failure, and closes
  admission if necessary. Raw pointers retained by native code require storage
  owned independently of the driver.

## Creation failure

Creation or initial cycle failure returns `DriverError`; the runtime rolls back
the unpublished pair. It reports normal cycle failures and shuts down the
worker's drivers.

Peer integration failure panics: partially connected registration cannot continue.

Drivers with conditional availability expose a capability check before
consumers request their context.

## System tasks

`SystemTaskSpawner` accepts blocking `FnOnce` work on runtime-owned system
threads. Submission returns after acceptance, not completion.
Indefinite observers need independent execution capacity; a bounded pool must
accommodate all simultaneously blocked observers.

## Compatibility

Runtimes and drivers must use compatible copies of the shared contract.
Placement uses `thread_aware_core` types directly, without re-exporting them;
`arty_io_core` must not stabilize before that dependency.

Optional construction facilities belong in `ProviderOptions` or
`DriverOptions`. Mandatory constructor arguments and trait methods are
compatibility commitments. Registries, type erasure, and placement policy
remain private to the runtime.

## Example

The [single-thread example](../examples/single_thread_runtime/main.rs) demonstrates
registration and peer discovery. Its drivers perform no I/O and its tracker
is a no-op, not a reference implementation of completion coordination.

## Deferred decisions

The contract does not choose thread pinning, cross-worker registration
atomicity, shutdown ordering, memory pools, clocks, or telemetry.
[Completion coordination](COMPLETION_COORDINATION.md) explores native wait
sharing and routing beyond this contract.
