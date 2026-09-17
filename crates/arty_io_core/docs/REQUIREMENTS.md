# Requirements

`arty_io_core` defines the agreement between drivers with independent versions,
native completion adapters, and a runtime that coordinates their progress.
The core implements the shared service and lifecycle vocabulary, not a native
backend, scheduler, driver registry, or placement policy.

## `R1`: Shared vocabulary

- Runtimes and their drivers use the same core contract. Distinct versions of a
  driver can coexist through different context types; incompatible copies of the
  core itself do not become interoperable.
- Native composition also requires matching adapter client interfaces. Sharing
  the core does not discover compatible native APIs or bridge different client
  types from independently compiled adapter versions.
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

- `IoContext::provider` creates provider state without a separate capability
  advertisement. The runtime chooses final owner threads and configured native
  waiters before per-worker creation.
- The runtime asks the waiter it actually drives to `attach_clients` to the
  new `DriverContext`. Native clients retain their actual backing resources;
  this construction rule is not a generic proof of native provenance.
- `DriverProvider::create` selects a supported strategy from the clients actually
  supplied, before creating native bindings. Missing clients produce classified
  unsupported errors instead of partially usable drivers.
- A provider clone is relocated and consumed once per worker creation attempt.
  The provider decides whether instances share an engine, resources, or nothing.
- Strategy-specific shared state may be initialized when actual clients are
  known. The runtime supplies compatible worker configurations; core does not
  discover intersections across heterogeneous capability sets.
- `DriverContext` carries thread coordinates, system work, a readiness waker,
  and typed clients. It stays on its owning thread.
- Providers return `Box<dyn Driver<Context = Self::Context>>` without `Send` or
  `Sync`. The installed boundary cannot cross threads even when the concrete
  implementation happens to be thread-safe.
- Creation is prompt and does not wait for runtime workers to make progress.
  Routing and notification are established before a context becomes usable.

## `R4`: Non-blocking service and separate native collection

- `Driver::service` never waits for new activity. It processes completions and
  performs any submission or kernel progress its implementation requires.
- Service charges `CompletionBudget` before each bounded progress step. Budgets
  cannot be copied or cloned through the helper API. This is cooperative
  accounting, not preemption or enforcement against a faulty implementation:
  do not reset the allowance or hide variable-length work inside one charged step.
- Each newly installed driver and newly initiated drain starts runnable and
  receives an initial service turn without requiring a native notification.
- `ServiceStatus::Idle`, `Runnable`, and `Deadline(Instant)` distinguish idle,
  immediate continuation, and timed service. Work left after budget exhaustion
  remains runnable without requiring another native notification.
- Native collection and the worker's blocking wait belong to a separate
  `CompletionWaiter`. Collection is budgeted too and does not run driver service
  or application futures recursively.
- A waiter routes native records to their owning registrations before private
  decoding, or reports readiness for drivers that retain their own queues.
- Native collection itself must eventually deliver or signal each continuously
  actionable source despite another source remaining busy. Preserve source
  continuation or ordering across collection turns; fair scheduling of already
  notified drivers is not sufficient.
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

- `Driver::shutdown(self: Box<Self>)` consumes the running driver and closes
  admission synchronously before returning `Box<dyn Drain>`.
- Admission closure synchronizes with acquiring active-operation ownership:
  racing operations are either admitted and included in draining or rejected.
- The drain keeps the same native registrations and readiness identity.
  Shutdown initiation is not repeated on each service turn.
- The boxed `Drain` is local and uses the same budget and arming protocol as
  running drivers. Every turn receives a budget; it is not a blocking call or a future.
- The runtime initiates relevant shutdowns, continues native collection and
  required system work, and services all drains fairly.
- Pending drains retain a notification, immediate-continuation, or timed-service
  obligation. The runtime applies an overall graceful-shutdown deadline.
- The coordinator removes and drops a drain after `Complete`, a service error,
  or a preparation error. It never calls the drain again after that terminal
  result. Terminal retirement is runtime state, not another public owner wrapper.
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
- Missing capabilities, duplicate clients, and shutdown timeout have
  classifications; callers do not parse diagnostic text to recover.
- Native causes remain available through the standard error source chain.
- Attaching a native cause preserves the selected classification, so an
  unsupported strategy can retain both fallback information and its native error.
- Successful client lookup does not eliminate resource, permission, or native
  registration failures.
- Partial installation is rolled back or safely retired before reporting a
  coherent registration result. Callback-visible state must remain owned during
  that process, and cleanup failures remain visible alongside the original error.
- Service and preparation failures are not converted into idle or successful
  drain results. A source failure and a collection-domain failure have different
  scopes; the runtime applies an explicit policy to each.
- Individual operation errors are delivered as operation results, not promoted
  into driver or collection-domain failure merely because the operation failed.

## `R8`: Runtime system work

- `SystemTasks::spawn` returns `Result<(), DriverError>` for synchronous work
  that may block. `Ok(())` means execution ownership was accepted, not that the
  task completed. `Err` means the task was not accepted and will not start.
- Rejection may drop the task and its captures; no completion callback is
  promised. Classification and native causes remain observable to the caller.
- Submitted work does not run on an async worker.
- The facility stays available during normal service, registration rollback, and
  cooperative draining while participants still require it.
- A controller reaching its shutdown deadline does not revoke execution access
  from retained owner threads or pending cleanup. Those obligations retain
  execution authority on the existing facility.
- Accepted work is not discarded behind a stop marker. Pool retirement waits
  for execution obligations, not merely for the controller to request shutdown.
- Worker or queue failures can still prevent admission and must be returned
  explicitly. Cleanup reports rejection instead of waiting for a completion
  that cannot arrive.
- Retained consumer contexts or inert facility handles are not, by themselves,
  active operations or graceful-drain participants.
- The core provides the cloneable handle, not an implicit thread per operation
  or driver. The runtime implements and owns the execution facility.

## `R9`: Native service boundaries and scope

- `CompletionWaiter::attach_clients` supplies the client capabilities backed by
  the collector the runtime will drive. The default attaches no native clients.
- Typed lookup is a construction-time operation. Native completion records do
  not pass through a type-erased per-operation envelope.
- Typed lookup selects an agreed client interface; it does not verify native
  handles or ownership. Adapter factories and registrations establish and retain
  the connection to the correct collector.
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
