# Requirements

`arty_io_core` defines the agreement between drivers with independent versions,
native completion adapters, and a runtime that coordinates their progress.
The core implements the shared vocabulary and ownership handles, not a native
backend, scheduler, driver registry, or placement policy.

## `R1`: Shared vocabulary

- Runtimes and their drivers use the same core contract. Distinct versions of a
  driver can coexist through different context types; incompatible copies of the
  core itself do not become interoperable.
- Standard-library types are preferred. `thread_aware_core` is the only external
  type dependency in public signatures and is not re-exported.
- Operations, buffers, native completion records, registration tables, and
  routing identifiers stay private to drivers and native adapters.
- Core contains no unsafe implementation or caller-checked memory-safety
  protocol.

## `R2`: Lazy and independent registration

- The requested `IoContext` type selects its provider. Context acquisition needs
  no caller-supplied provider value.
- Successful registration is keyed by context type identity, not a package name
  or physical completion queue.
- The first request succeeds only after every active worker has initialized its
  driver. Later requests return cached contexts without registering again.
- The runtime owns synchronization, cancellation policy, publication, rollback,
  and caching. Failed attempts are not cached as successful registrations.
- Abandoning an acquisition result does not justify abandoning cleanup of an
  installation already in progress.

## `R3`: Negotiation and owner-thread initialization

- `ProviderContext` advertises the client capability types of one proposed
  completion configuration, without carrying native handles.
- The provider selects one supported strategy and declares its
  `CompletionRequirements`. Every capability in that set is required; unrelated
  alternative strategies are not combined into the set.
- The runtime chooses final driver-owning threads and native domains before
  creation. It validates requirements against each actual `DriverContext`.
- A provider clone is relocated and consumed once per worker creation attempt.
  The provider decides whether instances share an engine, resources, or nothing.
- `DriverContext` carries thread coordinates, system work, a domain identity, a
  readiness waker, and typed domain-scoped clients. It stays on its owning thread.
- Providers return `LocalDriver`. This handle cannot be sent or shared across
  threads even when the concrete implementation happens to be thread-safe.
- Creation is prompt and does not wait for runtime workers to make progress.
  Routing and notification are established before a context becomes usable.

## `R4`: Non-blocking service and separate native collection

- `Driver::service` never waits for new activity. It processes completions and
  performs any submission or kernel progress its implementation requires.
- Service charges `CompletionBudget` before each bounded progress step. Budgets
  cannot be copied or cloned to duplicate an allowance.
- Each newly installed driver and newly initiated drain starts runnable and
  receives an initial service turn without requiring a native notification.
- `ServiceStatus` distinguishes immediate continuation from idle and timed
  service. Work left after budget exhaustion remains runnable without requiring
  another native notification.
- Native collection and the worker's blocking wait belong to a separate
  `CompletionWaiter`. Collection is budgeted too and does not run driver service
  or application futures recursively.
- A waiter routes native records to their owning registrations before private
  decoding, or reports readiness for drivers that retain their own queues.
- A zero-duration collection never blocks. An exhausted collection budget never
  enters a wait. A normal timeout is idle, not a failure.
- Finite native waits may round up but never become infinite. Only
  `Duration::MAX` permits an unbounded wait.
- The runtime gives participants fair turns and accounts for driver, drain,
  waiter, task, and shutdown deadlines before sleeping.
- Extra collection domains, private observers, and dedicated hosts are explicit
  configuration choices. Registering another type does not itself create a
  thread.

## `R5`: Reliable readiness and interruption

- A driver's runtime-provided readiness waker signals that its service
  participant needs another turn. It does not transport operation records.
- The runtime publishes and latches source readiness before interrupting the
  domain waiter. A notification arriving during service or preparation cannot
  be cleared as though it belonged to an older turn.
- The waiter's separate waker interrupts the current or next blocking
  collection. Same-thread and remote wakes are honored and redundant wakes may
  coalesce.
- Non-blocking collection preserves pending interruption.
- `prepare_wait` arms notification and then rechecks private work. `Armed` means
  no immediate work was found; later activity signals readiness. `WorkReady`
  requires another service turn before sleeping.
- Preparation is bounded bookkeeping, not a hidden drain or cancellation loop.
- The runtime arms participants, publishes sleeping intent, and rechecks task,
  command, and source state before a positive wait. The native interruption
  latch covers the final check-to-sleep race.
- Do not hold resources needed to submit operations or signal readiness across a wait.
- Saved wakers remain memory-safe after rollback or destruction and cannot
  redirect late activity to a replacement registration.

## `R6`: Owned cooperative shutdown and safe destruction

- `LocalDriver::shutdown` consumes the running driver. The underlying boxed
  `Driver::shutdown` closes admission synchronously before returning `Shutdown`.
- Admission closure synchronizes with acquiring active-operation ownership:
  racing operations are either admitted and included in draining or rejected.
- The drain keeps the same native registrations and readiness identity.
  Shutdown initiation is not repeated on each service turn.
- `Shutdown` drives a local `Drain` with the same budget and arming protocol as
  running drivers. Every turn receives a budget; it is not a blocking call or a future.
- The runtime initiates relevant shutdowns, continues native collection and
  required system work, and services all drains fairly.
- Pending drains retain a notification, immediate-continuation, or timed-service
  obligation. The runtime applies an overall graceful-shutdown deadline.
- Completion or an error releases the drain. Calling it after a terminal result
  is a programming error, not a new shutdown attempt.
- Context clones remain valid but closed and do not themselves prevent graceful
  completion. Active operations and callbacks retain their own resources.
- Dropping a running driver or abandoning a drain always remains memory-safe.
  Raw-pointer-visible storage has an independent owner and is retained rather
  than invalidated on incomplete cleanup.
- Destruction does not wait for I/O, other participants, or callbacks.
  Independent cleanup retains or receives ownership when it is still needed.
- Cancellation or timeout is not permission to free native storage. Late control
  packets, callbacks, and wakers participate in registration retirement.
- Failure remains observable and does not stop cleanup of independent
  participants. Successful graceful shutdown is never a memory-safety precondition.

## `R7`: Explicit initialization and progress errors

- Both `IoContext::provider` and `DriverProvider::create` return `DriverError`.
  Environmental initialization failure is not required to panic.
- Missing capabilities, duplicate clients, wrong-domain clients, and shutdown
  timeout have classifications; callers do not parse diagnostic text to recover.
- Native causes remain available through the standard error source chain.
- Attaching a native cause preserves the selected classification, so an
  unsupported strategy can retain both fallback information and its native error.
- A preliminary capability advertisement does not eliminate resource,
  permission, or native registration failures.
- Partial installation is rolled back or safely retired before reporting a
  coherent registration result. Callback-visible state must remain owned during
  that process, and cleanup failures remain visible alongside the original error.
- Service and preparation failures are not converted into idle or successful
  drain results. A source failure and a collection-domain failure have different
  scopes; the runtime applies an explicit policy to each.
- Individual operation errors are delivered as operation results, not promoted
  into driver or collection-domain failure merely because the operation failed.

## `R8`: Runtime system work

- `SystemTasks` accepts synchronous work that may block and returns after
  acceptance rather than completion.
- Submitted work does not run on an async worker.
- The facility stays available during normal service, registration rollback, and
  cooperative draining while participants still require it.
- The core provides the cloneable handle, not an implicit thread per operation
  or driver. The runtime implements and owns the execution facility.

## `R9`: Native service boundaries and scope

- Each waiter identifies its `CompletionDomain`; client capabilities are tagged
  with the same identity before being supplied to drivers.
- Typed lookup is a construction-time operation. Native completion records do
  not pass through a type-erased per-operation envelope.
- Domain tags reject accidental mixing of client sets. They are identities, not
  native-resource owners or verification of an adapter's underlying OS handles.
- Native adapter packages own client interfaces, source registration, routing,
  and safe retirement. A domain, a registered driver type, and a physical queue
  are not interchangeable identities.
- The runtime may preserve a native-I/O-free parking path when no native sources
  are attached.
- Production IOCP, registered I/O, and `io_uring` adapters are outside this crate.
  The reference runtime uses safe in-memory adapters to exercise both record
  delivery and readiness-style coordination.
- No default I/O implementation, native dependency, public memory-pool type,
  `no_std` configuration, or automatic fallback thread policy is provided.
