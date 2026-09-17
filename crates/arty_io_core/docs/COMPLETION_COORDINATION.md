# Completion coordination across I/O drivers

The coordination contracts described here are implemented by `arty_io_core`.
The reference runtime uses in-memory adapters; production IOCP, registered I/O,
and `io_uring` backends are not included. [Requirements](REQUIREMENTS.md) define
the obligations, and [Design](DESIGN.md) describes the ownership model.

## Separate servicing from waiting

Independently registered drivers do not need to share operation types or physical
completion queues. They need a compatible way to participate in one collection
and waiting point.

`Driver` performs non-blocking service, reports continuation or deadlines, and
arms its notification mechanism. `CompletionWaiter` separately collects native
activity and provides the blocking wait. The runtime schedules both sides and
their cooperative drains.

This separation avoids the blind spot of a combined wait/process method:

```text
service A without blocking
service B without blocking
block indefinitely inside A
B receives a completion, but needs owner-thread service to publish its task wake
```

An interruption handle for B does not cause B's native queue to interrupt A.
Returning a future does not create that missing notification connection either.
The native adapter must connect its source to the collection domain.

## The implemented boundary

| Role | Responsibility |
| --- | --- |
| Consumer context | Typed operations and independent driver-specific state. |
| Local driver | Bounded service and private completion processing on its owner thread. |
| Native waiter | Bounded native collection, routing, and latched blocking wait. |
| Runtime coordinator | Negotiation, publication, fair turns, readiness, deadlines, and failure policy. |
| Owned drain | Budgeted cleanup using the same source identity and notification path. |

`ProviderContext` advertises one proposed configuration's client types. The
provider chooses a strategy and reports its `CompletionRequirements`. The
runtime validates those requirements against each actual `DriverContext` before
creation.

Native adapters tag client handles through `CompletionDomain::service`.
`DriverContext::with_completion_service` rejects wrong-domain and duplicate
clients, while typed lookup reports unsupported clients explicitly. The map
exists only during construction; it is not a type-erased completion transport.

The domain tag identifies a collection arrangement. It does not own OS resources,
replace source-registration identities, or prove that an adapter paired an opaque
client with the correct native object. Those remain native adapter obligations.

`LocalDriver` prevents sending or sharing an installed driver across threads,
even if its concrete fields implement those traits. A provider may still share
an engine across instances, and mobile contexts can submit remotely without
moving the underlying native binding.

## Windows mapping: aggregate producers before waiting

[`CreateIoCompletionPort`][iocp-create] associates many overlapped file or socket
handles with one completion port. [RIO notifications][rio-notify] can direct
several private completion queues to that same port.

```text
driver A: overlapped operations ---+
driver B: overlapped operations ---+--> shared IOCP --> native waiter
driver C: RIO CQ notifications ----+
runtime wake packets -------------+
```

A Windows native adapter can expose scoped client handles for association and
registration through `DriverContext`. Its waiter owns collection and identifies
the destination source before a driver interprets an operation pointer.

Sharing a port does not make unchanged private readers interchangeable. One
reader could remove another driver's packet. The native adapter must preserve
completion status, byte count, and opaque operation identity and deliver them
to the intended private mailbox. A readiness waker then requests driver service.
There is no requirement to allocate a new envelope for each completion.

RIO queues can remain private. Their IOCP notification packets request service
of a queue rather than representing individual RIO results. The adapter and
driver preserve notification-arm state, drain continuation, and any required
submission flushing.

Existing independent ports are a different configuration.
[`GetQueuedCompletionStatusEx`][iocp-wait] accepts one port, and the documented
object list for [`WaitForMultipleObjects`][windows-wait] does not include
completion ports. A handle also remains associated with its chosen port until
it closes. Cooperative sharing must be negotiated before resources are bound;
core does not promise to merge arbitrary existing ports.

## Linux mapping: aggregate notifications without merging rings

An `io_uring` descriptor supports polling. The kernel reports readable state
for visible completion entries or relevant pending work. A Linux native adapter
can monitor suitable descriptors through `epoll`, use registered completion
`eventfd` handles, or collect through a coordinating ring that polls other
rings. [Kernel readiness implementation][uring-poll]

Each driver retains its own submission and completion queues, registered buffers,
and operation identifiers. The waiter can report readiness instead of transferring
completion ownership.

[`eventfd` notifications][uring-eventfd] are hints, not completion counts. They
may be coalesced or spurious. Sharing an event descriptor loses source identity
and requires scanning the relevant rings; separate descriptors permit targeted
service. A ring supports one event registration, which an adapter must not replace
or clear in conflict with another owner.

Ring configuration affects the service obligation:

- `IORING_SETUP_DEFER_TASKRUN` requires appropriate kernel entry on the submitting
  thread. A readable source can require work before completion entries become
  visible; service may need `io_uring_get_events`.
- `IORING_SETUP_IOPOLL` requires active completion progress. Such a participant
  reports runnable work or a suitable service deadline, rather than claiming a
  passive notification is sufficient.
- `IORING_SETUP_SQPOLL` can create kernel submission threads.
  `IORING_SETUP_ATTACH_WQ` shares kernel worker resources, not completion queues
  or a user-space waiting point.

These requirements are part of the native strategy selected during negotiation.
See the [setup flags][uring-setup] and [outstanding-work API][uring-events].

## Both notification directions matter

`DriverContext::readiness_waker` is supplied by the runtime for one service
participant. Native adapters publish private records or readiness first, then
signal it. The runtime preserves that source notification and interrupts its
collection domain.

`CompletionWaiter::waker` interrupts the domain's current or next blocking
collection. It also serves task and command activity unrelated to I/O.
Non-blocking collection does not consume the pending interruption.

The two paths do not carry operation records and do not run thread-local driver
code on the notifying thread. Saved handles remain memory-safe, and native routes
must prevent old signals from targeting replacement registrations.

## Service, preparation, and deadlines

Each collection, driver, and drain turn receives a finite `CompletionBudget`.
Charge before processing a completion or performing another bounded progress
step. A participant that runs out of allowance with work remaining returns
`ServiceStatus::runnable`, even when no new notification will arrive.
New drivers and drains start runnable and receive an initial service turn before
the coordinator may park.

Service owns any necessary submission flushing and native task work. Examining
a completion queue is not assumed to perform either. Scheduling continuation
does not mean starting an unbounded inner loop; the runtime returns to other
participants and control work between turns.

Before a positive wait, the runtime establishes all of these conditions:

1. Required submissions and native progress have been serviced.
2. No immediate continuation was forgotten because a budget expired.
3. Participants armed notifications and rechecked private state.
4. Sleeping intent is published and task, command, and source state is rechecked.
5. The wait is limited by every relevant service, task, and shutdown deadline.

`WaitStatus::WorkReady` requests another service turn. `Armed` means the
participant's notification mechanism and final private-work check are ready
for the shared wait. Preparation does bounded bookkeeping rather than hiding a
completion or cancellation loop outside the budget.

The waiter's latch covers notifications racing the final check and native wait.
Do not hold a resource needed by a submitter or notifier across that wait.
Finite native waits cannot become infinite through unit conversion, and a normal
timeout is not an error.

## Draining through the same coordinator

Consuming `LocalDriver::shutdown` closes admission before returning `Shutdown`.
The drain keeps the same registrations and readiness identity and receives the
same bounded service and preparation opportunities. Every turn receives a budget;
shutdown is not a blocking call or a future.

The runtime begins all relevant shutdowns, keeps native collection and required
system work available, and interleaves drains under an overall deadline.
Completion or an error is terminal; it does not permit restarting shutdown.

Retained contexts remain closed handles, not outstanding operations. Admission
closure must synchronize with accepting active work. Operations and callbacks
retain independent ownership, including backing storage when native code holds
raw pointers.

Cancellation, timeout, or abandoning a drain never authorizes invalidating native
storage. A domain identity is not a retirement lease. Native adapters separately
retain routes and resources for queued and executing dispatches, late control
packets, and saved wakers. Generations prevent stale routing but do not replace
buffer ownership. [Windows cancellation guidance][windows-cancel]

Destruction cannot wait for I/O or another participant. Transfer or retain
resource ownership for independent cleanup rather than introducing a blocking
wait through a destructor.

The same ownership rules apply during failed creation or partial installation.
Failures remain visible, and unrelated participants continue cleanup.

## Configuration and alternatives

| Arrangement | Trade-off |
| --- | --- |
| Shared completion domain | One collector routes compatible native activity while drivers retain private state. |
| Polling-capable source | One waiter observes readiness while the driver keeps completion ownership. |
| Externally driven source | An explicitly configured observer or callback connects activity to the readiness path. |
| Separate domain or dedicated host | Supports an incompatible native wait at an explicit resource and placement cost. |
| Repeated finite waits on opaque drivers | Introduces polling and latency; it is not the coordinated core contract. |

Core does not automatically choose or start extra threads. Unsupported
configurations return classified errors. Domain configuration and provider
strategy selection must agree before construction, especially for thread-local
native state.

A universal operation format would couple independent drivers to shared buffers,
queue configuration, and decoding. A raw wait-handle interface alone would miss
both IOCP routing ownership and native service requirements. The implemented
control boundary instead exposes scoped clients, bounded progress, readiness,
and safe owned draining.

## Reference scope and public prior art

The reference runtime uses in-memory native-adapter simulations for record
delivery and readiness-style sources with distinct driver operations. It is
intended to make pending operations depend on collection and service, rather
than merely register multiple context types.
Its control-plane calls and result handles block outside the workers; it does
not implement an application-future executor.

Native implementation evaluation must distinguish active-service cost from
idle-wait cost, include zero/one/several driver configurations, and account for
CPU use, thread count, allocations, wake frequency, context switches, throughput,
and tail latency. No native performance result follows from the core interface.

[The Glommio reactor][glommio] demonstrates several rings sharing one reactor
thread, including polling another ring before sleeping. Its flush, service, and
sleep preparation are useful precedent, not a ready-made integration for
arbitrary third-party drivers.

The [`mio` Windows selector][mio-selector] separates native collection from
dispatch, and its [waker][mio-waker] posts to the same port. This illustrates shared
collection without requiring a readiness-only replacement for completion I/O.

[iocp-create]: https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-createiocompletionport
[iocp-wait]: https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getqueuedcompletionstatusex
[windows-wait]: https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitformultipleobjects
[rio-notify]: https://learn.microsoft.com/en-us/windows/win32/api/mswsock/nc-mswsock-lpfn_rionotify
[uring-poll]: https://github.com/torvalds/linux/blob/v6.17/io_uring/io_uring.c#L2866-L2899
[uring-eventfd]: https://man7.org/linux/man-pages/man3/io_uring_register_eventfd.3.html
[uring-setup]: https://man7.org/linux/man-pages/man2/io_uring_setup.2.html
[uring-events]: https://man7.org/linux/man-pages/man3/io_uring_get_events.3.html
[glommio]: https://github.com/DataDog/glommio/blob/8434815962ce0bc161ace1967137213dc2334e4b/glommio/src/sys/uring.rs#L1721-L1885
[mio-selector]: https://github.com/tokio-rs/mio/blob/da425f909dd6b86d887da9eaefcb158099b5b165/src/sys/windows/selector.rs
[mio-waker]: https://github.com/tokio-rs/mio/blob/da425f909dd6b86d887da9eaefcb158099b5b165/src/sys/windows/waker.rs
[windows-cancel]: https://learn.microsoft.com/en-us/windows/win32/fileio/canceling-pending-i-o-operations
