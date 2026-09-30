**Posted by an AI agent**

**All nine review areas completed using Review Lens 0.19.1.** Four issues should be addressed before merging: cancellation cleanup can abort the process, teardown can deadlock on a system task, thread-creation errors cause panics, and completion events lose task context.

There are **four blocking findings, eleven non-blocking comments, one nit, and two design questions** below. These concern the new runtime, not demonstrated regressions in an older runtime.

## Scope and limits

- Reviewed [microsoft/oxidizer#785](https://github.com/microsoft/oxidizer/pull/785) at [`45f3881`](https://github.com/microsoft/oxidizer/commit/45f388139374495e58de77922fed2bc4312c0c54), against [`baeabbf5`](https://github.com/microsoft/oxidizer/commit/baeabbf54c40cc5fd90770c63a8ab2a6237c98eb). Both revisions were rechecked; the PR remains a draft.
- Focused tests, controlled reproductions, macro compilation probes, and API comparisons ran on **macOS with Rust 1.100.0-nightly**. Existing facade tests passed at both revisions, and the requested API documentation was retrieved successfully.
- Miri was unavailable. No local Windows, Linux, or minimum-Rust-version verification is claimed. The benchmark report was checked, but benchmarks were not rerun.
- **CI is still red:** the required Anvil check, manifest sorting, coverage, and mutation checks fail. These failures are separate from the reproduced runtime defects below.

Nothing was posted to GitHub.

## Important issues

**Posted by an AI agent**

**Cancellation cleanup that spawns a local task can abort the process**

Location: [`crates/arty/src/rt/task/local.rs:132-142`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/local.rs#L132-L142)

**Problem**
Local submission stays enabled while shutdown cancels futures. A pending task's destructor can call `LocalTaskScheduler::spawn`, invoking its factory and entering `TaskSet::add` while shutdown holds the executor's mutable borrow. Debug and release probes aborted with `RefCell already borrowed` and SIGABRT; the same cleanup at normal completion, or through the remote scheduler, passed.

**Why this matters**
Cancellation cleanup can terminate the application even when the destructor does not itself panic.

**Suggested fix**
Close local admission before cancelling or dropping task storage. During shutdown, retained valid local schedulers should return disconnected joins without invoking factories or entering the executor. Preserve stale-token checks and add a process-isolated cancellation test.

**Posted by an AI agent**

**Dropping the runtime from its own system task deadlocks shutdown**

Location: [`crates/arty/src/rt/runtime/handle.rs:137-139`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/handle.rs#L137-L139), [184-187](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/handle.rs#L184-L187)

**Problem**
`spawn_system(move || drop(runtime))` passes the async-worker blocking guard. Destruction waits for async workers, while a worker joins the system pool and waits for that same task. With one async worker, both `isolated()` and `shared(1)` hung in debug and release; identical teardown on an ordinary thread completed.

**Why this matters**
A system task dropping the runtime, including its final `Arc`, can prevent shutdown permanently.

**Suggested fix**
Detect same-runtime system tasks before synchronously waiting for shutdown, or transfer teardown to a non-owned thread. Preserve legitimate blocking operations on system threads and add bounded tests for both pool policies.

**Posted by an AI agent**

**Startup and shutdown treat OS thread-creation failures as invariant violations**

Location: [`crates/arty/src/rt/runtime/thread/lifecycle.rs:36-40`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/thread/lifecycle.rs#L36-L40)
Related: [`crates/arty/src/rt/runtime/thread/waiter.rs:58-89`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/thread/waiter.rs#L58-L89)

**Problem**
Worker startup unwraps `std::thread::Builder::spawn`, although OS resource exhaustion is an ordinary API error. Shutdown also spawns and unwraps a new waiter thread. Both bypass typed error handling and the repository's rule that `expect` requires a programming error or established invariant. This is source-proven; resource exhaustion was not fault-injected.

**Why this matters**
Applications cannot handle resource-limited startup through `BuildError`. Releasing a runtime also needs spare thread capacity; a second panic during unwinding can abort the process.

**Suggested fix**
Return startup failures through the crate's error type, preserving the `io::Error` and cleaning up partially started workers. Make blocking shutdown work without another thread, or provide a failure-safe fallback when waiter creation fails.

**Posted by an AI agent**

**Task completion events lose the task's inherited context**

Location: [`crates/arty/src/rt/task/execution/remote.rs:66-69`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/execution/remote.rs#L66-L69)
Related: [`crates/arty/src/rt/task/execution/local.rs:66-69`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/execution/local.rs#L66-L69)

**Problem**
Both task futures drop the applied enrichment guard before emitting `TaskSucceeded` or `TaskPanicked`. Native remote/local success/panic probes kept context on task-body and spawn events, but all six completion events lost their attributes, including a log correlation ID and an opted-in bounded metric dimension.

**Why this matters**
Panic logs lose the caller's correlation data when operators most need it. Spawn and outcome counters can no longer be compared by the same workload group.

**Suggested fix**
Keep captured context applied through both completion branches, outside the inner `catch_unwind`, and restore it before returning from `poll`. Test emitted completion events for successful and panicking remote and local tasks.

## Other comments

**Posted by an AI agent · Non-blocking**

**Entry-point macros discard qualified argument types**

Location: [`crates/arty_macros_impl/src/lib.rs:62-69`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty_macros_impl/src/lib.rs#L62-L69)

**Problem**
The attributes keep only `TypePath.path`, discarding `qself`. An argument `cx: <App as HasContext>::Context` whose associated type is `Builtins` becomes `HasContext::Context` in the generated closure. A head compilation probe passed with the original qualified argument and failed with E0223 after adding the attribute.

**Why this matters**
Consumers using an associated context type cannot apply either entry-point macro despite supplying the required concrete type.

**Suggested fix**
Preserve and emit the complete `TypePath`, including `qself`. Add consumer compilation tests for both macros.

**Posted by an AI agent · Non-blocking**

**`stash_scheduler` discards the handle carrying its thread-local assertion**

Location: [`crates/arty/tests/scheduler.rs:85-86`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/tests/scheduler.rs#L85-L86)

**Problem**
The final task contains the only assertion exercising `THREAD_LOCAL_STASH`, but its join handle is discarded. Task panics are transported through the join rather than rethrown when unobserved.

**Why this matters**
The test can pass without observing that assertion, and shutdown can discard the task before it finishes.

**Suggested fix**
Wait on the final spawn before leaving the watchdog closure, or return its result and assert it on the test thread.

**Posted by an AI agent · Non-blocking**

**The test-macro checks disappear when the attribute emits nothing**

Location: [`crates/arty/tests/test_macro.rs:14-25`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/tests/test_macro.rs#L14-L25)

**Problem**
The consumer tests are created by the attribute being tested; no ordinary test references the generated functions. Head CI confirms that replacing `arty_macros::test` with empty tokens survived mutation testing. Implementation snapshots bypass this wrapper and do not cover that failure independently.

**Why this matters**
Deleting generated tests can leave a successful test binary with no tests instead of detecting that the attribute stopped registering them.

**Suggested fix**
Add an ordinary test that calls a generated function and checks its result or body effect. Ensure mutation testing executes that consumer target.

**Posted by an AI agent · Non-blocking**

**Thread events use a string for the integer `thread.id` attribute**

Location: [`crates/arty/src/rt/telemetry/events.rs:188-189`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/telemetry/events.rs#L188-L189)
Related: [199-200](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/telemetry/events.rs#L199-L200), [211-212](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/telemetry/events.rs#L211-L212), [223-224](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/telemetry/events.rs#L223-L224)

**Problem**
Four events use `thread.id` for a classified Rust debug string. A native probe emitted `Value::String("ThreadId(2)")`; OpenTelemetry defines `thread.id` as an integer managed-thread identity, and the repository's OTel mapper preserves the string type.

**Why this matters**
Consumers using the standard typed attribute cannot match these records as integer thread identities.

**Suggested fix**
Use an application-specific attribute name for the opaque string, or a conforming integer representation with an explicit privacy policy. Do not simply bypass classification or substitute an OS thread ID.

**Posted by an AI agent · Non-blocking**

**Repeated waits duplicate the runtime-stopped event**

Location: [`crates/arty/src/rt/runtime/dispatch/dispatcher_core.rs:206-212`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/dispatch/dispatcher_core.rs#L206-L212)

**Problem**
Every completed `DispatcherCore::join` emits `RuntimeStopped`, even after shutdown has finished. Two explicit waits followed by owner drop emitted one started event, one stopping event, and three stopped events.

**Why this matters**
Lifecycle event counts and start/stop pairing depend on the number of waiters, not the number of runtime shutdowns.

**Suggested fix**
Emit once at the shared completed transition, or guard emission with shared once-only state. Test repeated waits and dropping the owner after a wait.

**Posted by an AI agent · Non-blocking**

**Telemetry benchmarks charge handle-vector allocation to task submission**

Location: [`crates/arty/benches/arty_telemetry.rs:83-87`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/benches/arty_telemetry.rs#L83-L87)

**Problem**
`Case::spawn` creates and destroys a handle vector inside each measured call. Both timing and allocation measurements include that benchmark-owned storage, unlike the scheduling benchmark.

**Why this matters**
Absolute allocation counts include the harness allocation, and its common timing overhead dilutes the active/noop ratio. This is a measurement-boundary issue, not a measured runtime regression.

**Suggested fix**
Reuse handle storage in `Case`, following `ArtyCase`, and rerun the affected telemetry cases. Keep runtime construction outside these spawn measurements.

**Posted by an AI agent · Non-blocking**

**Hot `Builtins` accessors omit the repository's required inlining hints**

Location: [`crates/arty/src/rt/runtime/context/mod.rs:53`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/context/mod.rs#L53), [70](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/context/mod.rs#L70), [80](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/context/mod.rs#L80), [117](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/context/mod.rs#L117)

**Problem**
The public, non-generic `scheduler`, `clock`, `sink`, and `local_scheduler` methods lack the `#[inline]` required for their hot-path role by baseline `docs/performance.md`. Neighboring accessors already have it.

**Why this matters**
Ordinary downstream builds lack the requested inlining hints. The retained fat-LTO benchmarks do not establish performance in those builds; no slowdown is claimed.

**Suggested fix**
Add ordinary `#[inline]` to these four accessors without removing validation or affinity checks.

**Posted by an AI agent · Non-blocking**

**`JoinHandle::wait` documentation excludes supported system-thread calls**

Location: [`crates/arty/src/rt/task/join/remote.rs:54-55`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/join/remote.rs#L54-L55)

**Problem**
The method promises a panic on any thread owned by Arty, but the authored panic policy and guard restrict asynchronous workers. A native probe waited for an async task from an `oxidizer-sys` pool thread and returned `42`.

**Why this matters**
The documentation incorrectly rules out a supported bridge from system tasks to asynchronous work.

**Suggested fix**
Say "an asynchronous Arty worker" in the panic documentation, matching the guard. Do not broaden the runtime prohibition.

**Posted by an AI agent · Non-blocking**

**Telemetry documentation points to a missing design guide**

Location: [`crates/arty/src/rt/telemetry/events.rs:6`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/telemetry/events.rs#L6)

**Problem**
The module points readers to `docs/observability.md`, which does not exist in the pinned repository. The authored overview is in `crates/arty/src/rt/mod.rs`.

**Why this matters**
Maintainers cannot follow the promised explanation of telemetry and classification decisions.

**Suggested fix**
Point to the existing overview, or add the intended guide if it contains necessary rationale.

**Posted by an AI agent · Non-blocking**

**`RuntimeThreadState` requires initialization its generic consumer never calls**

Location: [`crates/arty/src/rt/runtime/context/init.rs:27-42`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/context/init.rs#L27-L42)

**Problem**
`AsyncWorker::new` receives a constructor factory and fixes `TS::Error` to `Infallible`; it never calls `TS::sync_init`. Bootstrap calls the concrete `Builtins` initializer, while tests implement the unused method.

**Why this matters**
Implementations carry a redundant initialization contract and impossible error plumbing. The worker needs cloneable state and its injected constructor.

**Suggested fix**
Keep the generic worker and async constructor, use `TS: Clone + 'static` and `Future<Output = TS>`, and make `Builtins::sync_init` inherent. Remove the redundant trait and result plumbing.

**Posted by an AI agent · Non-blocking**

**`CapturedContext` duplicates the existing enrichment-transfer type**

Location: [`crates/arty/src/rt/telemetry/enrichment.rs:20`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/telemetry/enrichment.rs#L20)

**Problem**
The wrapper forwards capture and application to `observed::context::Transfer`, which already provides the same `Clone`, opaque `Debug`, and per-poll guard behavior used by its sibling `Transferred`.

**Why this matters**
The adapter adds a type and vocabulary without an additional invariant or propagation behavior.

**Suggested fix**
Store `Transfer` directly and use its existing capture/application methods, preserving the current capture timing and per-poll guard scope.

**Posted by an AI agent · Nit**

**Task-future panic messages use inconsistent capitalization**

Location: [`crates/arty/src/rt/task/execution/remote.rs:76`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/execution/remote.rs#L76), [88](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/execution/remote.rs#L88)
Related: [`crates/arty/src/rt/task/execution/local.rs:76`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/execution/local.rs#L76), [88](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/task/execution/local.rs#L88)

**Problem**
Four checks say `"Future polled after completion"` instead of following the required lowercase convention.

**Why this matters**
The new internal diagnostics diverge from the repository's error wording.

**Suggested fix**
Use `"future polled after completion"`.

## Design questions

**Posted by an AI agent · Design note, no change requested**

**`Runtime::block_on` allows borrowed inputs but requires `'static` results**

Public path: `arty::rt::Runtime::block_on`

**Problem**
The emitted signature allows the factory and future to borrow for `'a`, but requires `R: core::marker::Send + 'static`. The complete requested rustdoc and governing context do not explain that choice. Is the result restriction intentional, or should scoped borrowed results be supported while retaining `Send`?

**Why this matters**
Callers can borrow their data inside the future but cannot return a reference into that same caller-owned data as the result. This is a contract-design question, not a runtime defect.

**Posted by an AI agent · Design note, no change requested**

**The intended scope of `RuntimeBuilder::stack_size` is unclear**

Location: [`crates/arty/src/rt/runtime/builder.rs:48-51`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/builder.rs#L48-L51)
Related: [`crates/arty/src/rt/runtime/system_worker/mod.rs:91-97`](https://github.com/microsoft/oxidizer/blob/45f388139374495e58de77922fed2bc4312c0c54/crates/arty/src/rt/runtime/system_worker/mod.rs#L91-L97)

**Problem**
The docs say "each worker thread", but system pools do not receive the setting. With `stack_size(8 MiB)`, a macOS probe measured asynchronous/system stacks of 8,400,896 and 2,109,440 bytes. Is asynchronous-only scope intended?

**Why this matters**
Callers configuring stacks for blocking work need to know which threads this setting covers. Whether the documentation or propagation should change depends on that intent.
