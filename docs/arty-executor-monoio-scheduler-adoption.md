# Arty Executor Monoio-Style Scheduler Adoption

## Summary

This change replaces `arty_executor`'s mutex-backed wake queue, detached
`WakerState`, inactive-task hash set, and shared owner state with a
thread-per-core scheduler model inspired by Monoio and adapted to Arty's public
contracts.

The final design:

- permits one live executor per thread and allows replacement after drop;
- gives owner-only executor operations mutable access;
- stores owner-only cycle state directly instead of behind `RefCell`;
- keeps only genuinely reentrant submission state behind `Rc`/`Weak` and
  `RefCell`;
- stores each pinned task in `plurality::MultiPool` with `plurality::Arc`;
- makes raw wakers retain the pooled task allocation directly;
- schedules local wakes through a synchronization-free thread-local FIFO;
- transports remote wakes through `performables::sync::channel`;
- deduplicates local and remote notifications through one inline atomic state;
- keeps pending tasks in an allocation-free intrusive owner list;
- retires non-`Send` payloads on the owner thread before remote final release;
- skips context propagation and lifecycle reporting when the configured
  telemetry sink is provably no-op.

The implementation follows Monoio's concepts rather than copying its source.
Monoio revision `8675e9406ea7943c441d969e38c3f5fc198ef6d1` was used as the
design reference.

## Baseline

The previous executor used:

- `Rc<ExecutorCore>` with both reentrant and owner-only state in `RefCell`;
- a mutex-protected `VecDeque<TaskRef>` for wake notifications;
- detached pooled `WakerState` allocations for cloned wakers;
- separate embedded wake flags plus an O(n) inactive-task probe fallback;
- `HashSet<TaskRef>` as both pending-task ownership and scheduling state;
- `ThreadId` checks to classify local and remote wakes;
- pool occupancy as the definition of live tasks;
- telemetry context capture/application even for `Sink::noop()`.

These choices preserved correctness, but placed synchronization, allocation,
hashing, and context manipulation on elementary task and wake paths.

## Final Architecture

### Executor ownership

`Executor` directly owns `ExecutorCore`.

`ExecutorCore` directly owns its owner-only fields:

- the active task queue;
- the intrusive pending-task inventory;
- cycle clock and metrics state.

There is no `ExclusiveState` wrapper. `execute_cycle`, `begin_shutdown`, and
owner-drop cleanup use mutable core access directly.

`ExecutorShared` lives in its own module as a cloneable handle around internal
`Rc` state. `ExecutorCore` owns the primary handle and `TaskSet` holds a strong
clone; the intrusive `TaskList` does not own `ExecutorShared`. Tasks retain only
the narrower `WakeShared` needed by wakers. `ExecutorShared` exposes
operation-level `&self` APIs rather than `Ref`/`RefMut`.
Its fields are independently synchronized according to their actual needs:

- only the newly registered task queue uses `RefCell` for reentrant mutation;
- `MultiPool` and `JoinEventLake` use their existing shared-reference APIs;
- logical task count and shutdown state use their dedicated field operations;
- wake context and telemetry sink are immutable shared values.

`WakeShared` is itself a cheaply cloneable handle around one internal `Arc`
allocation. Callers clone the handle directly instead of wrapping it in
`Arc<WakeShared>`, and `WakeSignal` has the same shape in tests and production.
Test-only fake task ownership stays in test locals rather than adding a
conditional owner field to every test `WakeSignal`.

Owner drop marks the shared component shutting down before cancelling tasks, so
stale `TaskSet` clones reject submissions even though they may retain shared
pool/channel storage. This removes a `Weak::upgrade` from every spawn and keeps
`Rc` manipulation inside the shared module.

### Per-thread registration

`executor_context.rs` owns TLS identity, the local FIFO, cycle-active state, and
the cycle guard. `ExecutorBuilder` accepts a `thread_aware_core::Thread`,
validates that it identifies the current OS thread, and retains only its
`ThreadId`; standalone executors default directly to the current `ThreadId`.
Executor construction registers that value in TLS, and `Executor::drop` uses an
unwind-safe guard to unregister it.

Construction panics when another executor is live on the same thread. Drop
clears the registration through an unwind-safe guard. A later executor may then
register on the same thread.

Local/remote classification compares the `ThreadId` carried by the cloned wake
handle with the registered TLS value. This avoids both a raw identity pointer
and repeated calls to `std::thread::current`. `WakeShared` delegates local queue
operations to the executor context and contains no registration lifecycle
logic.

### Pooled task ownership

Each concrete task is allocated and pinned through `MultiPool::alloc_arc_pin`.
`TaskRef` holds a pinned `plurality::Arc<dyn TypeErasedTask>`.

`plurality::Arc` gained raw strong-reference operations modeled on
`std::sync::Arc`:

- `into_raw`;
- `from_raw`;
- `increment_strong_count`;
- `decrement_strong_count`.

These operations keep allocator/slot arithmetic inside `plurality`, where the
layout is defined. `arty_executor` handles only typed/non-owning task pointers
and explicit raw strong ownership transitions.

### Inline raw wakers

`WakeSignal` is embedded in the pinned task allocation and contains the atomic
scheduling state. A cloned raw waker increments the pooled task Arc directly.
There is no detached `WakerState`, side allocation, side pool, or side-pool
mutex.

An owned raw waker may be dropped on any thread. This is safe because the
executor's intrusive inventory retains an owner-thread strong reference until
the task payload has completed or been cancelled. Remote final release can
therefore reclaim only an empty task allocation.

### Atomic notification state

One atomic byte tracks:

- active;
- locally notified;
- remotely notified.

A wake claims a notification only when the task is active and not already
notified. Duplicate wakes do not enqueue duplicate work.

Retirement clears active state atomically. In-flight remote notifications then
fail promotion and release their task reference without touching the retired
payload.

### Local wakes

Local wakes push a temporary strong `TaskRef` into the registered executor's
TLS `VecDeque`.

The executor appends that queue directly to the active queue once per cycle,
records the appended range, and releases the TLS borrow before clearing each
task's notification state. No second owner scratch queue is required.

Completed tasks are released immediately after their inline wake state and
payload retire. Shutdown reuses the active queue as its temporary cancellation
batch after releasing reentrant submission state, so no completed-task scratch
queue is required either.

TLS also tracks whether the owner is currently executing a cycle. A self-wake
while already polling queues work but does not redundantly wake `WorkerSignal`.
A local wake outside a cycle still latches the owner signal before the worker
can park.

### Remote wakes

A remote wake:

1. atomically transitions from active to remotely notified;
2. creates a `RemoteWake` holding one strong task reference;
3. sends it through an unbounded `performables::sync::channel`;
4. sets the remote-pending latch;
5. wakes the executor owner.

The owner drains remote notifications at the beginning of a cycle. Each
notification promotes remote state to local state and enters the TLS FIFO. The
task future is never accessed remotely.

### Pending-task ownership

Pending tasks are stored in an intrusive `TaskList`.

Each pinned task supplies previous/next pointers. The list owns one raw strong
Arc reference for each linked task:

- first `Pending` links the task;
- runnable notifications carry temporary strong references;
- completion unlinks exactly once;
- shutdown drains and cancels the complete list on the owner thread.

This separates task ownership from scheduling state without allocating another
registry or hashing on every wake.

### Logical task count

Pool occupancy is not equivalent to live tasks because retained inert wakers
may keep an empty task allocation alive.

The shared state therefore tracks logical live-task count separately. Natural
completion and owner-thread cancellation decrement it. `has_tasks` and shutdown
completion use this logical count rather than `MultiPool::is_empty()`.

### No-op telemetry fast path

The default runtime sink is `Sink::noop()`.

When the sink is no-op, Arty now skips:

- enrichment `Transfer` capture;
- applying/restoring enrichment on every poll;
- lifecycle reporter construction and sink cloning;
- task-created, succeeded, and panicked emission machinery.

Configured sinks retain the existing context propagation and lifecycle event
behavior.

## Safety Invariants

### Owner-thread payload access

Task futures and results may be non-`Send`.

Only the owner thread may:

- poll a task;
- inspect or mutate task payload;
- cancel/drop a live future;
- publish or dispose the result target;
- manipulate intrusive task links.

Remote threads may only:

- increment/decrement task strong references;
- update atomic scheduling state;
- send/drop `RemoteWake`;
- call the owner waker.

### Final remote release

The intrusive list keeps a strong owner reference while payload is live.
Completion/cancellation retires wake state and empties payload before unlinking
the owner reference. A remote final Arc release therefore cannot drop a live
non-`Send` future or result.

### Raw ownership

`TaskPointer` is non-owning unless a caller explicitly documents possession of
one raw strong reference.

Operations that require a live allocation or raw ownership are unsafe:

- task dereference;
- strong-count increment/release;
- reconstruction from an owned raw pointer;
- retention from a non-owning pointer.

The intrusive list is the only long-lived conceptual owner represented solely
as a raw strong count. Its insert/remove operations transfer that ownership
exactly once.

### Shutdown

Shutdown:

1. closes task submission logically;
2. clears local runnable notifications;
3. gathers active, new, and intrusive-list tasks without duplication;
4. retires every payload on the owner thread;
5. disconnects join senders;
6. releases owner references;
7. allows retained inert wakers to outlive the executor safely.

## Public and Behavioral Changes

- `Executor::execute_cycle` now requires `&mut self`.
- `Executor::begin_shutdown` now requires `&mut self`.
- At most one executor may be alive per thread.
- A new executor may be created after the old one is dropped.
- The public `arty` scheduler, task, join, placement, panic, and shutdown
  contracts remain source- and behavior-compatible.
- Low-level tests that depended on shared owner access or dropping an executor
  from inside its own mutable cycle were removed or adapted because those
  states are no longer expressible through the safe API.

## Accepted Decisions

### Strong task references in wakers

Chosen because they:

- match Monoio's task-lifetime model;
- require no side allocation;
- allow retained wakers to remain valid;
- make channel notifications own their target explicitly.

Owner-thread inventory prevents remote final release from dropping live payload.

### `performables::sync::channel` for remote wakes

Chosen to reuse the workspace synchronization primitive and its telemetry,
closure, and ownership behavior. The channel carries owned `RemoteWake`
notifications; local wakes never use it.

### TLS pointer identity

Chosen instead of `ThreadId` because it is cheaper, identifies the exact
executor generation, and prevents a stale waker from targeting a replacement
executor on the same thread.

### Intrusive pending-task list

Chosen because it:

- provides O(1) arbitrary unlink;
- allocates no registry storage;
- avoids hash work per wake;
- preserves shutdown enumeration;
- naturally uses stable pooled task addresses.

### Mutable owner state

Chosen because executor cycles and shutdown are owner-exclusive. Retaining
`RefCell` there added runtime checks without enabling a supported behavior.
`RefCell` remains only where telemetry/future conversion may reentrantly submit
tasks.

## Rejected Decisions

### `plurality::Weak` wakers

Rejected because `plurality` has no weak-control representation. Its slot
refcount becomes the free-list link after strong count reaches zero. Supporting
weak handles would require a second count/control state and a pool-wide layout
change. Weak upgrade would also still need to create a strong channel
notification and preserve owner-thread payload destruction.

### `plurality::Box` task ownership

Rejected because tasks have multiple simultaneous owners: inventory, runnable
queues, cloned wakers, and remote notifications. A unique Box would require a
separate refcounted waker sidecar or dangling non-owning wakers. Box and Arc also
use different slot-refcount initialization, so in-place conversion is not a
sound shortcut.

### Detached pooled `WakerState`

Removed after measurement. It added a first-wake allocation/pool lock and
duplicated lifetime state. Inline task wakers reduced self-wake and clone costs
materially.

### Stable `HashSet<TaskRef>` registry

Implemented and reverted. It avoided remove/reinsert on wake but added an Arc
clone and hashed immediately-ready tasks. `arty_contention` regressed,
especially under multi-worker load.

### Existing inactive `HashSet<TaskRef>`

Removed. It combined ownership and scheduling state and performed hash-table
insert/remove on every pending/wake transition.

### `std::collections::LinkedList`

Rejected because stable Rust cannot remove an arbitrary completed node in O(1)
from a task pointer.

### `Vec<TaskRef>` plus task index

Implemented and reverted. It was simpler, but short-lived executors allocated
vector storage and benchmarks regressed:

- pending cycle: 70.19 ns to 76.90 ns;
- clone during poll: 74.68 ns to 81.90 ns;
- one additional measured allocation in affected cases.

### Pointer-only runnable queues

Implemented and reverted. The intrusive list remained the sole strong owner
while active and TLS queues stored non-owning task pointers. This removed
logical task counting, but forced a retain from the intrusive owner whenever a
task became runnable. Representative elementary benchmarks regressed:

- pending cycle: 72.72 ns to 81.85 ns;
- clone during poll: 73.83 ns to 82.88 ns;
- one additional measured allocation in affected cases.

The final model therefore retains `TaskRef` in runnable queues, keeps the
intrusive list as pending-task ownership, and tracks logical task count
separately from pool occupancy. This is required because retained inert wakers
can keep empty pooled allocations alive after the logical task has completed.

### `UnsafeCell` TLS queue

Implemented and reverted. Removing `RefCell` borrow checks did not produce a
reliable improvement and regressed measured single-worker cases.

### Test-only mutex wake queue

Removed as unnecessary test-only architecture. Standalone wake tests now use
the same TLS FIFO as production, with a small test constructor retaining the
fake task allocation. This removes the alternate `AwakenedQueue` type, mutex,
constructor shape, and fallback branches from production-adjacent code.

### Owner scratch queues

The TLS wake scratch queue and completed-task queue were removed after review.
TLS wakes append directly to the active queue, and shutdown reuses the active
queue for cancellation. Elementary benchmarks remained stable, while
self-waking cases removed one measured allocation after completed tasks began
releasing immediately.

### Shared-state grouping

The former `ReentrantState` aggregate and borrow accessors were removed.
`ExecutorShared` now provides named operations such as accepting/draining new
tasks, task-count retirement, shutdown requests, wake access, and completion
queries. No `Ref` or `RefMut` escapes the shared component. `MultiPool` and
`JoinEventLake` are direct fields, task count and shutdown use `Cell`, and only
the reentrant new-task queue uses `RefCell`.

`ExecutorCore` no longer has an `ExclusiveState` wrapper. Its active queue,
intrusive inventory, clock, and last-cycle timestamp are direct fields, and
cycle behavior is expressed as focused instance methods (`start_cycle`,
`accept_new_tasks`, `activate_awakened_tasks`, `poll_active_tasks`,
`cancel_if_shutting_down`, `cycle_outcome`, and `finish_cycle`).

The TLS `CurrentExecutorState` and `WakeCycle` guard were also moved out of
`wake.rs` into `executor_context.rs`, leaving `wake.rs` focused on inline task
notification state, raw-waker ownership, and remote channel transport.

### Separate `Cell<bool>` shutdown flag

Implemented and reverted after significant single-worker regressions. Keeping
the flag in reentrant shared state produced better code generation in measured
workloads.

### Redundant local owner wakes

Removed only while an executor cycle is active. Removing owner wakes
unconditionally would create a send-before-park race for same-thread wakes
outside polling.

### Monoio benchmark dependency

Deferred. Native one-worker Monoio would be comparable. An eight-worker case
would require a custom group of eight single-thread runtimes and a custom
external dispatcher, which must be labeled separately from a native
multi-thread runtime.

## Performance Evidence

### Elementary executor operations

The Monoio-style inline waker improved the detached-state implementation:

| Operation | Before | After |
| --- | ---: | ---: |
| Borrowed self-wake, four polls | 451.58 ns | 318.54 ns |
| Cloned self-wake, four polls | 448.98 ns | 325.99 ns |
| Clone during poll | 153.00 ns | 80.68 ns |

The intrusive list further improved the hash-set implementation:

| Operation | Hash set | Intrusive list |
| --- | ---: | ---: |
| Pending cycle | 78.16 ns | 70.19 ns |
| Borrowed self-wake | 344.08 ns | 322.77 ns |
| Cloned self-wake | 353.29 ns | 323.05 ns |
| Clone during poll | 84.51 ns | 74.68 ns |

The affected intrusive-list cases also removed one measured allocation.

### `arty_contention` versus detached `main`

Detached `main` revision: `f56baeba80cbc3420eda917b5455067d447a2754`.

| Arty workload | `main` | Final current | Improvement |
| --- | ---: | ---: | ---: |
| Retained waker, one worker | 30.54 us | 15.10 us | 50.55% |
| Retained waker, eight workers | 83.67 us | 57.81 us | 30.90% |
| Relocated cache, one worker | 27.22 us | 15.14 us | 44.36% |
| Relocated cache, eight workers | 79.70 us | 62.73 us | 21.29% |

At one worker, final Arty was 6.97% faster than Tokio for retained wakers and
9.79% faster for relocated cache.

### Remaining multi-worker gap

Focused `arty_scheduling` results identify remote injection as the remaining
gap:

| Twenty tasks | Arty | Tokio |
| --- | ---: | ---: |
| Nested, one worker | 1.51 us | 1.66 us |
| Nested, four workers | 1.56 us | 3.93 us |
| External spawn, one worker | 8.25 us | 6.50 us |
| External spawn, four workers | 23.73 us | 10.13 us |

At that stage, Arty's local executor was competitive or faster. Multi-worker external
submission still pays for per-worker channel sends, explicit wakeups, boxed
factory transport, result-channel setup, and payload relocation. Tokio uses a
global injection queue plus stealing and more aggressive wake batching.

### Arty integration follow-up

The follow-up keeps worker affinity and inline pooled futures, and removes costs
above the executor rather than introducing stealing or a global injection queue:

- Remote factories use source-thread `plurality::MultiPool` storage instead of a
  fresh standard `Box` per submission. A pooled erased callback consumes the
  concrete `FnOnce` through `Cell<Option<_>>`, so captures need not be `Unpin`.
  Detached pool handles allow registration or disposal after the submitting
  thread exits. The existing queued-factory panic boundary remains intact.
- Local child submissions rent cross-thread result channels from a worker-local
  pool, rather than contending on the dispatcher's runtime-wide pool. The
  returned join handles remain `Send` and may outlive the worker.
- `Builtins` clones share one worker-local allocation. Relocation uses
  copy-on-write so moving a clone does not rebind the source's services.
  Deferred factories check shutdown through those retained services rather than
  cloning the runtime-wide shutdown flag for every child.
- `WorkerSignal` uses `Thread::park_timeout` and `Thread::unpark`, with an atomic
  notification latch and lazily registered owner handle. Registration precedes
  the latch check, retaining notifications before the first wait and across the
  check-to-park window. Repeated notifications coalesce; spurious unparks consume
  only the remaining timeout budget.

Three complete Criterion runs on 2026-10-18 used 100 samples per case, a
one-second warm-up and a five-second measurement window on Apple M5/macOS.
The benchmark source, default features, compiler and bench profile match the
earlier branch measurements. The table uses the median of the three slope
estimates, with times per batch of 10 outer tasks and 100 inner operations:

| Workload | Workers | Earlier branch | Follow-up | Same-run Tokio | Reduction vs earlier branch |
| --- | ---: | ---: | ---: | ---: | ---: |
| Relocated cache | 1 | 16.93 us | 10.77 us | 17.10 us | 36.4% |
| Relocated cache | 8 | 61.73 us | 36.33 us | 47.73 us | 41.2% |
| Retained waker | 1 | 15.96 us | 10.68 us | 16.95 us | 33.1% |
| Retained waker | 8 | 59.45 us | 34.68 us | 41.05 us | 41.7% |

Both eight-worker workloads beat Tokio in each of the three follow-up runs.
Median batch time is 23.9% lower for relocated cache and 15.5% lower for retained
wakers. These measurements meet the eight-worker parity goal for these cases
on this host, not a general runtime-performance claim. The machine is shared and
has heterogeneous cores; exact percentages include host scheduling variability.

Native validation added regressions for factory-slot reuse, source-thread exit,
non-`Unpin` captures, worker-local result handles surviving shutdown, independent
relocation of shared `Builtins`, retained notifications and spurious unparks.
It also exposed shutdown-only executor assertions being evaluated during normal
pending-task cycles; the shutdown-completion check now gates those assertions on
shutdown actually having started.

Follow-up verification is partial, not a clean full-tier result:

- All 135 Arty unit tests pass, including the new factory and parking tests.
- Arty integration tests pass except three existing no-op-sink enrichment
  propagation failures, also reproduced on the unchanged branch in release.
- Two executor failures remain: cross-thread wake completion and the warmed
  debug allocation count. Both reproduce on pre-optimization code with only the
  shutdown-assertion guard applied.
- Strict package-only Arty Clippy, formatting, README generation, spelling, and
  no-default-features checks with and without `rt` pass. The generated workspace
  Clippy check remains blocked by existing executor lints.
- Miri, Loom, coverage and mutation testing were not run for this follow-up.

## Validation

Executed locally:

```text
cargo check --locked --all-targets \
  --package plurality \
  --package arty_executor \
  --package arty

CARGO_TARGET_DIR=... cargo bench --locked \
  --package arty_executor --bench ae_basic_operations

CARGO_TARGET_DIR=... cargo bench --locked \
  --package arty --bench arty_contention

CARGO_TARGET_DIR=... cargo bench --locked \
  --package arty --bench arty_scheduling \
  -- --criterion-arg 'spawn|nested'
```

All affected targets compile. Benchmark and allocation reports were generated.
The final elementary run measured 71.07 ns for a pending cycle and 73.80 ns for
cloning a waker during a pending poll, both with 19 measured allocations in the
full benchmark lifecycle.

Tests were compiled but not executed, following the requested minimal local
validation policy. CI remains responsible for comprehensive native tests,
Miri, Loom, careful, coverage, mutation, and repository-required checks.

`just anvil-fmt --fix` could not run because the installed `cargo-each` was
0.1.0 while Anvil requires 0.3.0. No tool was installed. The affected packages
were formatted with package-scoped `cargo fmt`.

## Remaining Risks and Follow-Up

- The change contains a concentrated unsafe core around pooled Arc raw
  ownership, raw waker vtables, and intrusive task links. The unsafe surface is
  documented and narrowed to 60 lexical unsafe blocks across `arty_executor`,
  but Miri and CI runtime-analysis results remain required.
- The integration follow-up removes the measured eight-worker contention gap on
  this host without changing routing. Broader workloads and other hosts still
  need measurement; parity in these coordination cases is not general parity.
  Worker notification now uses an atomic latch and native thread parking rather
  than taking a mutex for every command.
- A batch drain API on `performables::channel` could amortize queue-state
  inspection for remote wakes and worker commands. The executor currently
  avoids a final failed receive because that operation reports channel
  contention, so replacing the loop locally without a channel-level batch
  primitive would change telemetry.
- Remote factory transport is now pooled erased command storage. It retains the
  concrete future registration path instead of returning a boxed future.
- A shared injection queue or explicit command batches may reduce per-task
  channel sends, but they must preserve destination affinity, prompt submission
  without timer-based batching delay, shutdown rejection, and panic-contained
  destruction on the destination owner.
- The no-op telemetry fast path must remain behaviorally paired with the
  configured-sink path. Existing telemetry tests compile and CI should execute
  them.
- A native Monoio one-worker benchmark may be added later. Any custom
  multi-runtime Monoio harness must be labeled as such.
