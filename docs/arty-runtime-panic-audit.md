# Arty runtime panic audit

**Status:** Decision record and implementation backlog; no runtime behavior is
changed by this document. Review the findings before choosing which contracts
to change. Implement only after the Arty runtime migration is merged, and
revalidate the findings against the then-current code before changing it.

**Audited ref:** `martintmk-arty-runtime-migration` at
`615b7b65232ebdb5dc3a477aa0ec5a81f319461d`. This report reconciles an
earlier exhaustive audit at `9106a1daffab67b2948783fb179ae81d355d4a3b`
with the complete intervening three-commit delta. Source references and line
numbers in the inventory refer to **615b7b6**, not necessarily to the branch
head or to `main`. The audit covered production `arty` runtime, task, bootstrap,
dispatch and blocking-worker modules; the `arty_executor` task, wake and
ownership machinery; and `tick` clock/timer paths reachable through Arty.
`arty_io_core` does not implement an Arty I/O driver. `arty_macros_impl` runs
at compile time, but its generated entry wrappers run in applications.

**Decision in brief:** Invalid configuration and safely rolled-back external
failures are candidates for typed errors. Admitted task panics already become
task-scoped `JoinError`s under unwinding; application state is not repaired.
Programmer misuse, impossible internal state and unsafe teardown must not be
converted to successful outcomes. Keep aborts where releasing live storage
would be unsound. Separate reported panics from deadlocks and from
`panic = "abort"` behavior.

## Reconciliation from the earlier audit

The public synchronous `JoinHandle::wait` was **removed** in `615b7b6`.
The `join` method in `crates/arty/src/task/join/remote.rs:82-85` is
`#[cfg(test)]`, and the integration-test `JoinHandleExt::join` in
`crates/arty/tests/support/mod.rs:5-21` is also private test support. Delete
old **P05**, the public-wait panic and proposed `try_wait` overload. Keep
**P06** only for *direct* polling of a blocking join from a callback in its
own pool; `remote.rs:94-100` checks the pool before polling the receiver,
including when its result is already ready. `BlockingWaitContext`, its
ancestor/descendant checks, and strong-parent ownership chain were removed.
Delete old **I12** and **N01**: the Windows default-test-stack overflow from
the removed 4,098-node fixture was observed at `9106a1da`, not at the
audited SHA. Do not classify descendant cycles as caught task panics.

A blocking callback can call `RuntimeScheduler::block_on`; if its async work
or descendants await a callback queued to that saturated pool, both sides
wait indefinitely. This is a **liveness failure, not a panic**, documented
at `crates/arty/src/task/runtime_scheduler.rs:102-110`,
`task/scheduler.rs:338-346`, and
`documentation/scheduling.rs:115-121`. Generic external
`futures::executor::block_on(join)` can also synchronously stall an Arty
worker. Removing Arty's synchronous method does not make either dependency
safe. A direct same-pool polling test remains at
`crates/arty/tests/blocking_wait_cycles.rs:19-46`; the previous transitive
rejection and expired-provenance tests were removed.

The old worker-entry TLS flag was removed too. `block_on` and `Runtime::stop`
now check the registered `CURRENT_WORKER` in
`crates/arty/src/task/scheduler.rs:76-89`, while registration occurs in the
startup initialization task (`runtime/bootstrap/startup.rs:236-242`).
`LocalTaskScope::close` clears it before final timer callbacks
(`task/local.rs:47-51`). Classify callbacks before registration or after
clearing as an **open context-window question**, not a proved new panic:
`runtime/thread/waiter.rs:66-99` separately detects an actual self-join
and returns a typed `SelfJoin` error.

The configuration API is `RuntimeBuilder::workers(WorkersPolicy)` and
`.blocking_pool(BlockingPoolPolicy)`; selection invariants now reside in
`runtime/config/workers_policy.rs`. A zero worker count still fails during
`build`; `.stack_size(0)` still panics at its setter.

## Inventory conventions

One row groups sites implementing the same behavior; its trigger and call path
distinguish a user callback from an internal invariant. "Integrity" describes
the runtime after a trigger, **not** the application's state. Confidence
H/M/L describes the cited source path and reachability, not measured
frequency. All paths below are repository-relative at the audited SHA.

### Production and user-reachable behavior

| ID | Crate/module; site | Mechanism and exact trigger/call path | User reach?; integrity afterward | Why it exists; disposition and alternatives | Compatibility; confidence |
|---|---|---|---|---|---|
| P01 | arty config; `crates/arty/src/runtime/builder.rs:80-82` | `assert!` when `Runtime::builder().stack_size(0)` is called, before `build` | Direct; intact, no worker started | Reject invalid OS stack size. Add `try_stack_size`/`NonZeroUsize` or validate in fallible `build`; debug-only checking admits zero in release, and task failure/abort is the wrong stage. | Additive low; changing setter breaks callers. H |
| P02 | arty startup; `crates/arty/src/runtime/thread/lifecycle.rs:25-40` | `expect` if `build -> worker spawn` gets an OS error, possibly after prior workers started | OS/resource influenced; partial startup uncertain | Construction currently panics on thread creation failure. Return typed error **only after** stopping and joining every started worker; bare `?` or silently fewer workers hides partial startup. | `build` already returns Result; rollback medium. H path/M cleanup |
| P03 | arty bootstrap; `crates/arty/src/runtime/bootstrap/startup.rs:111,125,137,141,221,230` | Endpoint/start/success channel `expect` or nonempty selection assumption fails during build/start handshake | Indirect worker or OS failure, not ordinary zero-worker input; uncertain, possibly live-worker abort | Bootstrap requires every selected worker and endpoint. Add explicit failed-start state and rollback if possible; otherwise retain fatal failure. Never fabricate missing workers or report successful construction. | Medium; M |
| P04 | entry macros; `crates/arty_macros_impl/src/lib.rs:216,249,279,308,334,369`; `crates/arty/src/runtime/mod.rs:69-74` | Generated `expect`/`resume_unwind` on failed construction, cancelled/panicked root task, or failed shutdown; original root payload resumes after stopping | Direct; intact only if shutdown succeeds | Generic entry return types and root-failure precedence. Keep default panic; explicit `build -> block_on -> stop` or opt-in Result-compatible entry is suitable. Returning the body value on failed stop is not. | Opt-in low; changing default signature/precedence high. H |
| P06 | arty blocking join; `crates/arty/src/task/join/remote.rs:94-100`; `runtime/blocking_worker/mod.rs:125-127` | `assert!` when a blocking callback directly polls a blocking join from its own pool, **ready or pending** | Direct; intact if caught, but a caught callback is labelled as a task panic | Prevent direct pool starvation. Keep guard; consider distinct structural join error. Debug-only guard allows hangs; `try_wait` cannot overload a removed API. | New join classification medium; H |
| P07 | arty join; `crates/arty/src/task/join/remote.rs:94-105` | `assert!` when the caller polls an already-completed, consumed one-shot join again | Direct; intact | Return value cannot be produced twice. Keep programmer-error panic; repeatable joins require retention/cloning, while a task-scoped failure would blame the wrong party. | Redesign high; H |
| P08 | arty local tasks; `crates/arty/src/task/local.rs:132-140` | `expect`/`assert!` in `LocalScheduler::spawn` outside its live owning worker or on the wrong worker | Direct misuse; intact if caught | Protect `!Send` local executor affinity. Keep existing guard; optional `try_spawn` can reject before invoking the factory. A stronger scoped capability costs more; debug-only guard is inadequate. | Additive low/replacement high; H |
| P09 | arty task execution; `crates/arty/src/task/execution/{local,remote,preparation,storage}.rs` (e.g. `local.rs:97`, `remote.rs:125`) | Caught admitted factory, poll, relocation, blocking callback or cancellation destructor unwind | Direct callbacks; executor intact after ordinary single unwind, application state not repaired | Task failure becomes `JoinError::is_panic()`; cancellation becomes shutdown. **Keep task-scoped failure.** Re-throwing an ordinary join or returning success on cancellation is wrong; repeated destructor failure is P19. | Changing join output high; H |
| P10 | arty fan-out; `crates/arty/src/task/scheduler.rs:309-325`; `runtime/dispatch/dispatcher_core.rs` | User `Clone`/`Drop` can panic for a later `spawn_everywhere` worker after earlier work is already submitted | Direct; runtime intact, but admission/side effects partial | Clone then enqueue per worker; no atomicity promised. Document, or preclone before enqueue if atomic *admission* becomes required; preserve handles on error. Catch-and-report-all-succeeded is invalid. | Behavioral medium/high; H ordering/M reproduction |
| P11 | arty telemetry; `crates/arty/src/runtime/bootstrap/startup.rs:259-265` and builder/dispatcher emission | User `observed` processor panics synchronously during worker/runtime startup or stopping | Direct contract violation; partial start/stop uncertain, possibly live-worker abort | Sink processors must not panic. Keep contract until transitions/rollback are redesigned; a fallible or isolated sink is suitable only if shutdown remains honest. Blanket catch-and-continue can hide incomplete shutdown. | High; H path/M effect |
| P12 | tick timer/Arty worker; `crates/tick/src/state.rs:114-128`, `timers.rs:101-120`; `crates/arty/src/runtime/worker/async_worker.rs:190-196` | Custom user `Waker::wake` panics when manually polled Arty-clock delay fires **under the timer mutex** | Conditional safe callback; clock lock poisoned, worker may abort; reentry could deadlock (not reproduced) | Timer callback currently runs under lock. Drain ready wakers under lock and wake after unlocking; catching while retaining the lock does not restore invariants. | Tick-internal with observable behavior; H path/M reentry |
| P13 | tick controlled time; `crates/tick/src/clock_control.rs:402-415,448,552-565` | Opt-in `test-util` advances controlled `Instant`/`SystemTime` beyond representable range; checked arithmetic `expect` can fail after first assignment | Direct with test-util only; poisoned lock and possibly divergent clock fields | Virtual-time range. Add transactional checked operation computing both values before either assignment; saturating time silently changes the requested value. | Additive low; H source/M extreme |
| P14 | raw arty_executor; `crates/arty_executor/src/task_set.rs:40-47`, `executor_core.rs:204-218` | Disconnected `expect` or shutdown `assert!` when direct raw `TaskSet::add` occurs after owner drop or shutdown | Direct raw API; uncertain if unsafe owner dropped prematurely | Admission must close. Keep integrity guard, perhaps checked `try_add` after proving result storage safe; do not mislabel rejected work as an admitted task failure. | Additive medium/replacement high; H |
| P15 | raw arty_executor; `crates/arty_executor/src/executor_core.rs:211,261,271,280,524-529` | `RefCell` borrow panic on reentrant cycle/shutdown, or assert on repeated shutdown | Direct raw owner misuse; uncertain after interrupted cycle | Executor is single-owner/nonreentrant. Keep guard until rollback is proved; debug-only validation would be insufficient. | High; H |
| P16 | raw arty_executor; `crates/arty_executor/src/join_handle.rs:46` | Receiver disconnects before bare join can return its promised `R`, triggering `expect` | Conditional raw API; uncertain | Bare join returns `R` rather than Arty's `Result`. Keep until an output-breaking fallible raw join is designed; cannot fabricate `R`. | Breaking; H mechanism/M reach |
| P17 | raw arty_executor; `crates/arty_executor/src/executor_core.rs:527-536` | `Instant::checked_add(...).expect` at `begin_shutdown` for unrepresentable configured timeout such as `Duration::MAX` | Direct raw builder; safe shutdown not reached | Deadline must fit `Instant`. Validate policy before unsafe teardown; simply returning `Err` then dropping live owner may be unsound. | Additive check medium/start API high; H |
| P18 | raw arty_executor; `crates/arty_executor/src/executor_core.rs:570-617` | **Production abort**, not catchable unwind, when shutdown deadline passes with referenced task/waker/result storage | Conditional retained work; intentionally terminal | Cannot free pinned live references. Keep abort; cooperative cancellation or process isolation are alternatives, but ordinary `Err` followed by unsafe drop is not. | None; H |
| P19 | arty disposal; `crates/arty/src/task/execution/storage.rs:82-94,142-163` | **Abort** after repeated panics disposing user payload or escaping fallback destructor | Crafted user `Drop`; intentionally terminal | Bounded disposal avoids double-panic/unsafe release. Keep abort; isolate process if continued service is required, not a success-shaped fallback. | None; H |
| P20 | executor waker; `crates/arty_executor/src/wake.rs:393,403-411,443` | **Abort** on extremely high owned/inline waker refcount before overflow | Theoretical huge clone count; intentionally terminal | Avoid refcount wrap/UB. Keep abort; standard `Waker::clone` has no fallible result. | None; H source/L incidence |
| P21 | arty final timer; `crates/arty/src/runtime/worker/async_worker.rs:190-205`; worker wrapper and `runtime/thread/waiter.rs:66-100` | Custom timer waker panics at final timer advancement **after** executor storage retires | Conditional callback; worker failed, task storage retired, `stop` reports worker error | Worker wrapper reports terminal failure. Keep terminal error and fix callback origin/lock boundary; do not report task success or pretend this is a task join. | Medium; H |
| P22 | raw executor admission; `crates/arty_executor/src/executor_core.rs:204-218` | User `IntoFuture::into_future` panics during direct raw `TaskSet::add`, before task poll containment | Direct raw API; uncertain if owner skips required shutdown | Conversion precedes registration. Retain caller panic or construct outside borrowed core/introduce task-scoped factory; blanket catching lacks teardown proof. | Medium/high; M |

### Internal invariant and integrity paths

| ID | Crate/module; site | Mechanism and exact trigger/call path | User reach?; integrity afterward | Why it exists; disposition and alternatives | Compatibility; confidence |
|---|---|---|---|---|---|
| I01 | arty selection; `crates/arty/src/runtime/config/workers_policy.rs:117,120,124` | `expect` if available processor set is empty or validated nonzero selection unexpectedly fails | Not proved for ordinary input; startup uncertain | Nonempty available topology is assumed; zero requested workers already return error. Keep guard pending dependency proof; map genuine empty topology to typed build error, never invent a processor. | Medium; M |
| I02 | arty routing; `crates/arty/src/runtime/dispatch/dispatcher_core.rs:89-102,136,186-191,211-217` | Owner/ID assert, `.get().expect`, or modulo failure for mixed owners, duplicate IDs, zero/unregistered endpoints | Not from valid constructor; routing compromised | `NonEmpty` registered endpoint invariant. Keep fail-closed; prevalidate malformed topology before publication, never select a default worker. | Internal low; H site/L trigger |
| I03 | arty worker; `crates/arty/src/runtime/worker/async_worker.rs:153,187,230,241,253,281,292-297` | `expect`/Drop abort after duplicate initialization, missing live executor/channel, twice-consumed factory or unexpected disconnect | No valid public trigger; compromised, abort with live storage | Paired one-shot transitions and pinned references. Keep fail-closed abort; explicit terminal state only after safe retirement, not catch-and-continue. | Internal high; H site/L trigger |
| I04 | arty TLS/local; `crates/arty/src/task/scheduler.rs:81-99`, `task/local.rs:35-73`, `task/builtins.rs:250,280,282` | `expect`/assert for duplicate scope/registration, missing worker slot, double publication | Not from ordinary submissions; uncertain, may abort worker | Unique initialized worker identity. Keep; state-machine construction or checked preflight is preferable to success with absent service. Wrong-context public use is P08. | Internal; H site/L trigger |
| I05 | arty scoped borrowing; `crates/arty/src/task/runtime_scheduler.rs:20-23,136-147` | ScopedJoin destructor `expect_err` if private sender sends rather than disconnects; unsafe lifetime extension depends on destroying borrowed work first | No exposed sender; borrowed lifetime uncertain | Preserve caller borrows until factory/future destroyed. Keep invariant; an explicit ownership protocol is a possible redesign, but timeout/early return would dangle borrows. | Internal high; H site/L trigger |
| I06 | arty one-shot tasks; `crates/arty/src/task/execution/{local,remote,storage,preparation}.rs` (e.g. `local.rs:97`, `remote.rs:125`, `storage.rs:66,112-133`) | `expect`/assert on retired future repoll or twice-consumed sender/body/metadata | Not valid lifecycle; uncertain, may abort | Parts consumed once. Keep guard; bundle state if redesigning, only return task failure when disposal is demonstrably safe. | Internal medium/high; H site/L trigger |
| I07 | executor raw task; `crates/arty_executor/src/task.rs:90-109,149,173-221` | Null/phase `expect`, `unreachable!`, unsafe pointer assumption on uninitialized signal/sender or poll after payload drop | No valid safe path; compromised/possible UB | Pinned initialized-once layout. Keep fatal guard; stronger checked state before raw dereference may improve safety, but no valid fallback `R` exists. | Internal high; H site/L trigger |
| I08 | executor accounting; `crates/arty_executor/src/executor_core.rs:435-444,492-494` | `checked_sub.expect` if completed/inactive/drop counts decrease against phase expectations | No proved safe trigger; compromised accounting | Detect impossible mutation instead of wrapping. Keep; terminal error only if storage can retire, never saturate to hide corruption. | Internal; H site/L trigger |
| I09 | arty/executor locks; `crates/arty/src/runtime/blocking_worker/mod.rs:177,190,201,224,229,248`; `runtime/thread/waiter.rs:89,97,108`; `crates/arty_executor/src/executor_core.rs:343,518`, `wake.rs:174-177` | Poisoned mutex `expect` or dependency guard after earlier unwind while pool/wait/wake lock held | Not ordinary caught poll; uncertain, possibly worker abort | Lock poison may indicate broken protected state. Keep fail-closed until each lock's invariant is proven; resetting to empty loses queued work. Old ancestry locks no longer exist. | Internal; M |
| I10 | executor raw waker/hash; `crates/arty_executor/src/wake.rs:384-389,480`; `ptr_hash.rs:41` | `unreachable!`/`expect` for consumed borrowed waker, null raw pointer or unsupported pointer width | No valid safe wrapper path; compromised/UB possible | RawWaker vtable and pointer-width contract. Keep fatal invariant; repair construction/target support instead of user Result after dangling pointer. | Internal/target; H site/L trigger |
| I11 | raw executor teardown; `crates/arty_executor/src/executor_core.rs:690-701`; `crates/arty/src/runtime/worker/async_worker.rs:290-297` | Drop assert/abort with unsafe-built executor not shut down or pooled references live | Raw unsafe caller may violate; compromised | Never free referenced storage. Keep abort; safe ownership redesign is possible, not returning `Err` while dropping live storage. | Breaking redesign; H |
| I13 | TLS/unsafe lifetime; `crates/arty/src/task/{scheduler.rs:77-93,local.rs:35-57,runtime_scheduler.rs:143-146}`; `crates/arty_executor/src/{task_ref.rs:56-82,wake.rs:263,405-477}` | TLS access during teardown or owner-thread/live-ticket/borrowed-storage precondition failure; may be **UB, not a catchable panic** | No concrete safe trigger established; uncertain/compromised | Preserve affinity and pinned tickets. Keep guards, use bounded Miri/interleavings and redesign ownership if safe callers can break it. | High internal; L trigger |

### Debug-only paths

| ID | Crate/module; site | Mechanism and trigger | User reach?; integrity afterward | Why it exists; disposition and alternatives | Compatibility; confidence |
|---|---|---|---|---|---|
| D01 | arty worker; `crates/arty/src/runtime/worker/async_worker.rs:213` | `debug_assert!` if thread state is absent but command receiver remains | Not established; live worker may abort in debug | Paired teardown diagnostic. Keep, add release guard if safety depends on it; prefer atomic state shape over silent fallback. | Internal; H site/L trigger |
| D02 | executor; `crates/arty_executor/src/executor_core.rs:323,512,580-582` | `debug_assert!` for unexpected active/new/inactive queues | Not established; worker may abort in debug | Detect phase bookkeeping errors; terminal release check if safety requires it, never silently change counts. | Internal; H site/L trigger |
| D03 | executor task; `crates/arty_executor/src/task.rs:221` | `debug_assert!` if wake signal initialized twice before raw write | Not established; possible UB in release | Initialize-once invariant. Demand release proof/check if this guards memory safety; typed state machine alternative. | Internal; H site/L trigger |
| D04 | executor wake; `crates/arty_executor/src/wake.rs:356` | `debug_assert!` when pinned wake signal drops with inline references alive | Not established; possible lifetime failure | Retirement invariant. Keep diagnostic plus prove production guard; refcount-aware teardown alternative, no Result from Drop. | Internal; H site/L trigger |
| D05 | executor diagnostic feature; `crates/arty_executor/src/wake_diagnostic.rs:64,81-82,129,152` | Poisoned/mismatched backtrace registry or invalid raw waker pointer | Only under diagnostic selection; uncertain | Track outstanding wakers. Keep diagnostic; recover poison only after registry-consistency proof; not a general production failure. | Internal; H site/M trigger |

### Test, example, benchmark and compile-time paths

| ID | Crate/module; site | Mechanism and trigger | User reach?; integrity afterward | Why it exists; disposition and alternatives | Compatibility; confidence |
|---|---|---|---|---|---|
| N02 | executor harness; `crates/arty_executor/src/executor_core.rs:616-617` | Test-selected timeout panic rather than production abort | Unit tests; harness survives | Observe terminal branch. Keep fixture; verify actual production abort separately in a child process, not as a normal Result. | None; H |
| N03 | arty runtime unit fixtures; `crates/arty/src/runtime/{bootstrap,config,dispatch,thread,worker,blocking_worker}` test modules | Assert/unwrap/intentional user panics in scripted setup | Test only; test binary | Verify startup, shutdown and pool behavior. Keep assertions; removed `runtime/thread/blocking.rs` and ancestry fixture are **not** current sites. | None; H |
| N04 | arty task unit fixtures; `crates/arty/src/task/{execution,join,local,runtime_scheduler,scheduler}` tests; `task/join/remote.rs:82-85` | Assert/unwrap; private `#[cfg(test)] join` drives future with `block_on` | Test only; test binary | Check joins/context. Keep fixture; private join is not a shipped synchronous API. | None; H |
| N05 | executor unit fixtures; `crates/arty_executor/src/{executor,executor_core,task,task_ref,task_set,wake,join_handle,builder,testing}.rs` | Assert/unwrap/panic in tests or optional `test-util` | Test/helper only; no normal Arty runtime | Verify raw storage. Keep; never project test-only timeout panic onto production abort. | None; H |
| N06 | arty integration tests; `crates/arty/tests/*.rs` (40 root Rust test files); `tests/blocking_wait_cycles.rs:19-46`, `tests/support/mod.rs:5-21` | Deliberate panics/asserts; direct-pool test timeout calls `process::exit(112)` on deadlock | Test process only; timeout exits its process | Preserve direct-pool regression. Restore subprocess isolation for future hangs; former transitive/provenance tests were removed and their assertions must not be described as current behavior. | Test-only; H |
| N07 | examples/benches/doctests; `crates/arty_executor/examples/`, `crates/arty/benches/`, `crates/arty_executor/benches/`, `crates/arty/src/documentation/` | Intentional demo panics, benchmark asserts and doctest checks | Non-production executable | Demonstrate and measure. Keep; generated crate READMEs are not authored by hand. | None; H |
| N08 | macro parser/Miri; `crates/arty_macros_impl/src/lib.rs` attribute parser; `crates/arty/src/runtime/bootstrap/startup.rs:299-305` | Compile-time `compile_error!` for invalid attributes or Miri fake-hardware literal `expect` | Compile-time/test configuration | Reject invalid syntax/test fixture. Keep; **generated runtime** `expect` belongs to P04. | None; H |

## Other implicit and dependency boundaries

The original static inventory and this delta found no direct production
Arty/executor `[]` collection indexing/slicing, unchecked division or shift
that established an additional panic. Dispatcher modulo uses a `NonEmpty`
endpoint set; `dispatcher_core.rs` `.get(...).expect` is I02. Blocking-pool
growth uses saturating counters; executor accounting uses checked subtraction
(I08). The blocking-pool thread cap does **not** bound its queued work;
allocator exhaustion is an OS/allocator failure boundary, not a demonstrated
catchable Rust panic.

`crates/tick/src/timers.rs:71-79` wraps its `u32` key discriminator. A still
live timer at the same instant after a full discriminator wrap could collide
with a new key and replace its waker. This is a source-inferred
**lost-notification/liveness** concern, not itself a panic; validate with a
bounded internal collision test rather than 2^32 registrations.
`many_cpus` topology, threadpool behavior, `performables` lock poisoning and
`observed` callback implementations were not exhaustively audited as
external dependencies. Unsafe precondition violations can be UB or abort,
not necessarily a catchable unwind.

## Documented contract discrepancies at the audited SHA

`crates/arty/docs/PANICS.md:46` still claimed removed `JoinHandle::wait`
panicked; this was corrected **after the audit** on the migration branch.
At the audited SHA the central document also omitted direct same-pool poll
and transitive deadlock distinctions. `PANICS.md:50-54` claims application
`Err(E)` always remains the macro return value without qualifying that
shutdown must succeed; generated wrappers can instead surface shutdown
failure. `PANICS.md:56-58` overgeneralizes construction `Result`: zero
stack size panics immediately at its setter. Public scheduling/block-on docs
already warn of the transitive dependency cycle. Updating these source docs
should be coordinated with, not silently bundled into, a chosen behavior
change.

**Post-audit branch changes (not a second exhaustive audit):** At report
creation, the migration branch was at `93adefe1391e6f891dac54fb3e52128644834cdc`.
The four intervening commits corrected the stale `JoinHandle::wait` paragraph,
moved P06's assertion into a helper in `task/join/remote.rs`, added
privately typed runtime validation causes, and used `NonZeroUsize` internally
for stack size and pool limits. They did **not** add a checked public stack
setter, a public error classification API, or restore transitive cycle
detection. Thus line anchors and resolved documentation issues must be
rechecked at merge time. Do not treat this short delta note as an audit of
all runtime changes after `615b7b6`.

## Decisions to make after the migration merges

**Keep:** one-shot repoll and wrong-worker local-use programming-error
panics; direct own-pool guard pending a safely specified alternative;
raw-executor disconnection/reentrancy/post-shutdown and unsafe-storage
invariants; default macro root-payload/error precedence; task panics only
as *caught task-scoped joins* under unwinding; terminal abort for live
executor, incomplete unsafe teardown, repeated panicking disposal and
waker-count overflow. A debug assertion can supplement but must not replace
a release safety/deadlock guard.

**Prefer user-facing typed Results when continuation preserves integrity:**
zero stack size via additive checked configuration; thread creation/startup
and genuinely empty topology **after full rollback**; publicly inspectable
categories for already-returned `runtime::Error` (configuration, affinity,
forbidden `block_on`, worker-terminal, task failure); optionally checked
local admission, structural own-pool join rejection, transactional controlled
time and opt-in checked entry macros. `try_wait` is not an option on a removed
public method. Task-scoped failure is unsuitable for telemetry outside task
catches, caller-side `Clone`, bootstrap, unsafe owner violations or
transitive deadlocks. Abort/process isolation is appropriate where
referenced storage cannot be safely retired, not for ordinary bad config.

**Order of implementation and measurable acceptance criteria:**

1. Freeze the merged contract in public tests: zero stack, own-pool
   ready/pending polling, repoll, wrong-worker use, root payload and
   body-`Err` plus stop-error precedence. Use child-process timeouts for
   saturated-pool transitive/external worker-blocking scenarios. Accept when
   tests distinguish direct panic from unguarded hang; feature combinations
   (`no-default`, `rt`, `time`, `test-util`) compile and source docs match.
2. Protect integrity *before* catching more failures. Exercise custom timer
   wakers/reentry, panicking telemetry, partial startup, wake retirement and
   scoped borrows with bounded fault injection/Miri/child processes; move
   timer callbacks out of the tick lock if the path is confirmed. Accept only
   if no worker is reported stopped while live, no scoped borrow escapes,
   no clock remains poisoned by a catch-and-continue, and unsafe teardown
   still terminates rather than freeing referenced storage.
3. Expose typed error inspection and compatible checked input. Accept when
   invalid size fails before worker startup and startup/worker/task/context
   failures remain distinguishable without breaking existing default
   signatures or losing `Error::source`.
4. Decide the liveness contract: unsupported/documented transitive
   blocking-callback dependencies, or dependency-aware admission with an
   explicit structural error. Accept when bounded tests cover direct,
   descendant, shared/isolated, cross-pool and ready-result cases without
   mislabelling a cycle as a user task panic; do not resurrect an unbounded
   strong-parent ownership chain.
5. Only after rollback proof, decide typed mid-start errors, optional
   checked macros, transactional test-clock operations, raw-executor
   timeout/admission validation and telemetry policy. Accept when injected
   partial startup joins every worker, timeout is validated *before*
   unsafe shutdown state transitions, and default root-payload and
   shutdown precedence stay stable unless explicitly changed.

## Verification and unresolved questions

The target commit and branch ref were confirmed during the refresh, pinned
Git-object source and the intervening diff were inspected, and
`git diff --check 9106a1da..615b7b6` was clean. **No test or build was run
at `615b7b6`**: the original read-only checkout remained at `9106a1da`.
Earlier passing Arty/executor/macro suites and an observed Windows
default-test-stack overflow belong **only** to `9106a1da`; the latter's
fixture and production ancestry chain were deleted by `615b7b6`. Neither
old outcome is verification of the audited target.

Open evidence includes a bounded reproduction of transitive deadlock and
worker-context windows; partial worker-start and sink failure injection;
tick custom-waker panic/reentry; controlled-time extremes; a bounded timer
key collision; raw shutdown timeout and executor-owner Drop in child
processes; Miri of unsafe ticket/borrow/waker lifetimes; and exact behavior
of the named external dependencies. Do not infer that these hypotheses were
observed as production failures. Re-audit the merged revision before
prioritizing implementation.
