# Arty runtime panic audit disposition

**Revalidated baseline:** `cb3ef36dc0942dc93017df934a3fcacfc7f451ad`
(`main` immediately after #785 merged).

This disposition applies the decision-ready evidence preserved in #800 to the
merged code. It preserves the report's distinction between user-reachable
panics, deadlocks, internal invariants, debug-only checks, terminal aborts, and
non-production findings. It does not treat a panic, deadlock, or abort as a
successful task outcome.

The independent-waker decision is tracked by Azure DevOps
[User Story 8026956](https://dev.azure.com/O365Exchange/O365%20Core/_workitems/edit/8026956)
under
[Feature 7852437](https://dev.azure.com/O365Exchange/O365%20Core/_workitems/edit/7852437).
No duplicate work item is required.

## Implemented hardening

- Cloned task wakers have independently pooled lifecycle state. They route to
  the owning worker while pending and become inert after completion,
  cancellation, or shutdown without retaining task storage.
- Consuming wake ownership uses RAII, so a panicking configured owner waker
  cannot leak the raw waker's counted ownership.
- The four deferred retained-waker integration scenarios are enabled. They
  cover success, task panic, cancellation, final-timer failure, cross-thread
  wake, and shutdown.
- Macro-generated runtime paths resolve Cargo dependency aliases.
- User-facing package and crate descriptions distinguish a multi-worker runtime
  from thread-affine tasks.

## Production and user-reachable findings

| ID | Merged-state disposition |
|---|---|
| P01 | **Keep deterministic programmer/configuration panic.** `stack_size(0)` still rejects immediately. A checked setter remains an additive API option, not required for this compatibility-preserving hardening. |
| P02 | **Deferred typed rollback.** Thread-spawn failure still cannot become a returned construction error until every previously started worker is stopped and joined. A bare `?` would be unsafe partial-start handling. |
| P03 | **Keep fatal bootstrap invariants.** Missing endpoint/start/success signals mean the published worker topology is incomplete. Fabricating a worker or reporting successful construction remains invalid. |
| P04 | **Keep macro panic/error precedence.** Generated entry points still stop the runtime before returning or resuming the root failure. Explicit runtime construction remains the fallible alternative. |
| P06 | **Keep direct same-pool polling panic.** This is a deliberate deadlock guard and is covered by the public `blocking_wait_cycles` integration boundary. Transitive pool dependency cycles remain documented deadlocks, not panics. |
| P07 | **Keep one-shot repoll panic.** Polling a consumed join is an obvious programming error and is not duplicated as a new integration contract. |
| P08 | **Not applicable.** The separate `LocalScheduler` subsystem was removed before merge. Worker-affine scheduling remains covered through the public scheduler integration tests. |
| P09 | **Keep task-scoped `JoinError`.** Factory, poll, relocation, blocking-callback, and cancellation disposal behavior is covered by public Arty panic integration tests. |
| P10 | **Keep documented partial fan-out admission.** Panicking user `Clone`/`Drop` can leave earlier submissions admitted; atomic fan-out is not promised. |
| P11 | **Keep telemetry non-panicking contract.** Catch-and-continue could report incomplete startup or shutdown as successful. Existing public observability tests cover supported processors; a panicking processor remains contract violation. |
| P12 | **Confirmed separate Tick integrity work.** Custom timer wakers still run through Tick's timer path; lock-bound callback redesign is not hidden as an Arty task failure in this change. The final-timer integration test preserves worker-terminal reporting. |
| P13 | **Test-util only; deferred transactional Tick change.** Saturating virtual time would silently change the requested value. |
| P14 | **Keep raw executor admission invariant.** Post-shutdown `TaskSet::add` is raw API misuse, not an admitted task failure. |
| P15 | **Keep raw executor non-reentrancy invariants.** Recovering from interrupted `RefCell` state without rollback proof would be unsafe. |
| P16 | **Keep raw join disconnect invariant.** The bare raw join cannot synthesize its promised `R`; a fallible redesign would be breaking. |
| P17 | **Deferred pre-shutdown timeout validation.** Returning after entering unsafe teardown state is not safe; validation must precede the transition. |
| P18 | **Keep terminal abort.** Shutdown timeout with live referenced executor storage must not free that storage. Retained task wakers no longer create this condition. |
| P19 | **Keep terminal abort.** Repeated panics while disposing user payloads cannot become a success-shaped fallback. |
| P20 | **Keep overflow abort.** Pooled independent waker counts retain the conservative no-wrap rule. |
| P21 | **Keep worker-terminal error.** Final timer callback panic after task retirement is reported by `Runtime::stop`; the retained-waker variant is now enabled. |
| P22 | **Keep caller panic for raw `IntoFuture` conversion.** Conversion occurs before task admission; catching it cannot honestly produce an admitted task result. |

The saturated blocking-pool dependency described by #800 remains a **deadlock**
risk, not a panic. This change does not resurrect an unbounded strong-parent
ownership chain to detect transitive cycles.

## Internal, debug-only, and non-production findings

| IDs | Merged-state disposition |
|---|---|
| I01-I03 | Keep nonempty-topology, routing, and worker one-shot transition invariants fail-closed. Empty-topology or partial-start errors require proven rollback before becoming typed errors. |
| I04 | Local-scheduler portions are removed; remaining worker registration/publication invariants stay fail-closed. |
| I05-I07 | Keep scoped-borrow, one-shot task-part, and pinned raw-task invariants. Early return or catch-and-continue could release borrowed or referenced storage. |
| I08-I11 | Keep checked accounting, poisoned-lock, raw-waker pointer, and unsafe teardown guards. Saturation, lock reset, or ordinary `Err` would hide corruption. |
| I13 | Keep thread-affinity and unsafe lifetime guards; no concrete safe caller path was established that permits weakening them. |
| D01-D05 | Retain diagnostics. Debug assertions do not replace release-mode memory-safety or deadlock guards. |
| N02-N08 | Test, example, benchmark, Miri, parser, and compile-time findings remain non-production evidence. They are not reported as runtime user panics. |

The withdrawn I12 ancestry-chain and N01 stack-overflow claims remain
non-findings because their implementation and fixture were removed before the
merged baseline.

## Independent-waker contract and alternatives

The supported contract is constrained support:

1. While pending, cloned wakers may be retained and invoked from any thread.
   Wake routing returns to the task's owning worker and never relocates the task.
2. Completion and cancellation mark the pooled state inactive before task
   storage is released.
3. Wake-by-reference and consuming wake after retirement or shutdown are valid
   inert no-ops.
4. Retained wakers own only pooled wake state. They own neither the task, the
   worker, nor the runtime, and therefore cannot delay shutdown.

Keeping wake state inline was rejected because escaped clones retain pinned task
storage and can drive shutdown to the terminal timeout. A fresh
`std::sync::Arc` state was rejected as the default because it adds a heap
allocation to task registration. `plurality::Pool<WakerState>` preserves stable
addresses and cross-thread final release while reusing slots after warm-up. The
remaining atomic clone/drop cost is measured against both the merged baseline
and a fresh-`Arc` allocation benchmark.
