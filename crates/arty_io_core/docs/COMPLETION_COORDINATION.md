# Completion coordination across I/O drivers

**Design proposal, not the current contract.** [DESIGN.md](DESIGN.md) and
[REQUIREMENTS.md](REQUIREMENTS.md) describe the existing API. This document
explores an alternative completion boundary; its conceptual operations are not
APIs implemented by `arty_io_core`.

## Recommendation

Keep independently registered drivers, but separate servicing I/O from owning
the runtime worker's blocking wait. Introduce a runtime-owned **completion
coordinator** that connects native notifications to the drivers that need
service.

Compatible Windows drivers can participate in a shared I/O completion port
(IOCP). Linux drivers can retain independent `io_uring` instances while their
notification sources participate in one wait. Dedicated completion threads
remain an explicit fallback for incompatible blocking sources, rather than an
automatic consequence of registering another driver.

Unify notification, scheduling, and lifetime rules, not drivers' operation
types, buffer layouts, or private completion queues. Sharing the coordination
contract still requires compatible versions of `arty_io_core`; it does not
provide interoperability between incompatible copies of the contract itself.

## Why the current boundary is insufficient

The current [driver interface](../src/driver.rs) combines completion processing
and waiting in `process_completions(max_wait, cycle_start)`. Its `waker` returns a
handle for ending that driver's current or next wait. It supplies one direction:

```text
runtime task, command, or shutdown activity -> interrupt the driver's wait
```

It does not establish the other direction needed for coordinated hosting:

```text
native source needs service -> notify the coordinator -> service its driver
```

Consider two drivers whose completions are published to tasks only when their
owner thread processes them:

```text
1. Service A without blocking.
2. Service B without blocking.
3. Block indefinitely in A.
4. B's native queue receives a completion.
5. B cannot publish the task wake until the worker services B.
```

B's waker targets B, not A. Nothing in the contract connects B's native
readiness to A's wait. Returning a future or combining wakers does not
create that connection; some implementation still has to observe the native
source.

Finite waits can bound this delay, but introduce polling and latency.
Zero-duration scans avoid blocking on the wrong driver but consume CPU while
idle. A dedicated observer solves progress at the cost of threads and possible
cross-thread completion delivery.

The [single-thread example](../examples/single_thread_runtime/runtime.rs)
demonstrates lazy registration and peer discovery, not a native completion loop.
Its worker receives control commands; the example drivers do not perform I/O or
service native sources. Discovering peers alone therefore does not establish
coordinated completion progress.

## Native constraints

### Windows: aggregate producers before waiting

[`CreateIoCompletionPort`][iocp-create] can associate many overlapped file or socket
handles with one port. [RIO notifications][rio-notify] can also direct several
RIO completion queues to that port, distinguishing them through completion keys
and notification `OVERLAPPED` objects.

This makes the following arrangement possible without a dedicated thread for
each logical driver:

```text
driver A: overlapped operations ---+
driver B: overlapped operations ---+--> shared IOCP --> coordinator
driver C: RIO CQ notifications ----+
runtime wake packets -------------+
```

There must be one coherent owner of collection and routing. A completion key
selects the destination registration before any driver interprets an operation pointer.
Drivers keep their own operation storage and decoding. RIO queues remain private;
their IOCP packets request a service pass rather than representing each RIO
operation's result.

Supplying the same port to several unchanged private readers is not sufficient:
one reader could consume another driver's packet. The coordinator must preserve
the native completion status, byte count, and opaque operation identity until
the intended driver receives them.

Already-associated, independently owned ports are a different case.
[`GetQueuedCompletionStatusEx`][iocp-wait] accepts one port, and the documented
object list for [`WaitForMultipleObjects`][windows-wait] does not include
completion ports.
A handle also remains associated with its chosen port until it closes.
Cooperative sharing must therefore be negotiated before native resources are
bound; arbitrary existing ports cannot simply be added to a portable wait set.

### Linux: aggregate notifications without merging rings

An `io_uring` descriptor supports polling. The kernel reports readable state when
completion entries or relevant pending work are present, so a coordinator can
monitor suitable ring descriptors through `epoll`. Alternatively, a ring can
register a completion `eventfd`. A coordinating ring can itself poll other rings'
descriptors. [Kernel readiness implementation][uring-poll]

These arrangements preserve independent submission queues, completion queues,
registered buffers, and operation identifiers.

[`eventfd` notifications][uring-eventfd] are hints to inspect a ring, not a count
of completed operations. Notifications may be coalesced or spurious. A shared
`eventfd` can reduce notification resources but loses source identity, requiring
a scan of participating rings. Each ring permits only one `eventfd` registration;
an adapter must not replace another owner's registration or race another
consumer when clearing it.

Ring configuration also carries progress requirements:

- `IORING_SETUP_DEFER_TASKRUN` requires the submitting thread to enter the kernel
  appropriately to run outstanding work. A readable source need not already
  contain visible completion entries; servicing it may require `io_uring_get_events`.
- `IORING_SETUP_IOPOLL` requires active completion polling. An adapter must not
  promise that passive notification alone can make such a ring progress.
- `IORING_SETUP_SQPOLL` may create kernel submission threads.
  `IORING_SETUP_ATTACH_WQ` shares kernel worker resources, not completion queues
  or a user-space wait.

The coordinator must respect these requirements rather than treating every ring
as an interchangeable readable descriptor. See the [setup flags][uring-setup]
and [outstanding-work API][uring-events].

### Prior art and its limits

[The Glommio reactor][glommio] services main, latency, and polling rings on one
reactor thread. Before sleeping in one ring, it can submit a poll request against
another ring. Its wait path also flushes submissions, checks whether all rings
can sleep, and rechecks remote work. This demonstrates multiple rings sharing a
waiting point, not a ready-made contract for arbitrary third-party drivers.

The [`mio` Windows selector][mio-selector] separates native collection from event
dispatch, and its [waker][mio-waker] posts through the same port. This is useful
structural precedent for shared native collection. It is not a reason to replace
completion-based I/O with a readiness-only API.

## Proposed division of responsibilities

| Participant | Owns |
| --- | --- |
| Consumer context | Typed operations and driver-specific resource access. |
| Driver | Submission policy, private completion state, operation decoding, and task completion. |
| Completion coordinator | Source attachment, native collection or readiness, routing, service scheduling, and the worker's blocking wait. |

Runtime ownership means ownership of lifetime and scheduling. The native wait
implementation can live in a platform-specific component; the runtime need not
depend on a particular transport or concrete I/O driver.

Per-worker coordination preserves locality, but a provider can still share an
engine or native sources across instances. A driver registration, a per-worker
adapter, and a physical completion source are distinct identities. The contract
must describe source ownership and service affinity rather than require one
native queue per worker or per driver type.

### Negotiate participation before construction

The provider describes the completion facilities it can use. Runtime policy
selects a supported arrangement before constructing the thread-local driver.
The existing absence of a `Send` requirement is preserved.

| Participation | Coordination |
| --- | --- |
| Shared completion domain | Supply a native endpoint lease and routing registrations; deliver collected records to their owner. |
| Pollable source | Wait for readiness, then let the driver drain its private queue. |
| Externally driven source | Connect an existing callback or completion service to runtime notification. |
| Exclusive blocking source | Use an explicit dedicated host or reject the arrangement under the runtime's policy. |

Active polling and required service deadlines are additional requirements, not
capabilities that can be silently approximated by a passive wait.

An existing private IOCP cannot be converted into externally driven I/O merely
by wrapping it in a future. That mode requires a real notification mechanism.
Similarly, a failed cooperative attachment must not silently allocate arbitrary
threads. Thread budgets and fallback policy belong to the runtime.

### Source registration is not context registration

Keep context type identity for lazy acquisition and caching. A separate source
registration carries an opaque routing identity, service affinity, notification
connection, and lifetime lease. One driver may attach several sources, and a
provider-shared source must not acquire competing consumers accidentally.

Windows adapters use these registrations to route native packets before private
pointer decoding. Linux adapters can instead mark a source ready while leaving
completion queue ownership with its driver. Native record delivery and readiness
delivery are different inputs to the same service policy.

Attach routing and establish reliable notification before publishing a usable
context or acknowledging initialization. Native creation and attachment failures need an
explicit error and partial-registration cleanup policy. A preliminary capability
check cannot eliminate resource exhaustion or a later permission failure.

### Service and parking are separate operations

The following describes protocol concepts, not final Rust signatures:

| Operation | Required result or guarantee |
| --- | --- |
| Service with a budget | Nonblocking progress, explicit failures, whether immediate work remains, and any required service deadline. |
| Prepare to wait | Notifications armed and work rechecked, or an indication that the source still needs service. |
| Collect and route | Deliver readiness or preserved native records to the correct source owner. |
| Interrupt coordinator | Latch task, control, submission, or lifecycle activity across the transition into a wait. |

Service includes the backend's required submission progress, completion
processing, and task wakes. Deferred work must have an identified owner
responsible for flushing it before sleeping. Completion processing must not be
assumed to flush submissions unless its contract says so.

Budgets bound work per source and per worker cycle. Exhausting a budget keeps
the source runnable even when no further kernel notification will arrive.
Separate control packets from operation completions, retain native errors, and
avoid allocating a new object for every completion merely to cross this
boundary.

An existing `process_completions(Duration::ZERO, cycle_start) -> ()` does not prove that its
queue is drained. Legacy drivers remain on their declared hosting path until
they implement the stronger progress and notification guarantees.

### The parking invariant

Before blocking, the coordinator must establish all of the following:

1. Required submissions and kernel work have been serviced.
2. No source with immediate work has been forgotten because its budget expired.
3. Native notifications and runtime interruption are armed.
4. Sleeping intent is published, and task, source, and control state is rechecked.
5. The chosen deadline accounts for timers and sources that require later service.

This is one no-lost-wakeup protocol, not a collection of unrelated empty checks.
A notification racing the final check must remain latched or reach the native
wait. Edge-triggered and one-shot notifications require correct draining and
rearming; notification counts cannot substitute for examining source state.

Do not hold a lock across the wait that a submitter or notifier needs. Invoke
driver service on its supported owner thread, not directly from an arbitrary
notification callback. A runtime with no attached native sources retains a
native-I/O-free parking path.

## Cooperative shutdown without a safety protocol

The current consuming, blocking `shutdown` makes each driver responsible for its
own progress. A coordinator cannot generally keep servicing other drivers while
one opaque shutdown call owns its thread. The current contract explicitly
forbids depending on work that can run only after that call returns.

For coordinated participants, use an owned transition into a runtime-drivable
draining state. Consuming the running state preserves exactly-once initiation;
it does not require restoring borrowed shutdown futures that can be initiated
independently.
The coordinator closes admission across participants, continues service and
required system work, and finalizes each participant after draining or an
explicit failure policy.

Preserve the existing safety requirements:

- Admission closure synchronizes with acquiring active-operation ownership.
  A flag check followed by work without active-operation ownership is not a drain
  barrier.
- Retained contexts remain valid but closed; merely keeping a context alive does
  not keep graceful shutdown pending.
- Operations, callbacks, and operating-system-visible storage retain independent
  ownership. Cancellation is a request, not permission to free native storage.
- Source leases cover queued and executing dispatches as well as active I/O.
  Stale wake packets and retained wakers cannot target freed or reused
  registrations.
- Routing generations or tombstones prevent stale dispatch, but do not replace
  ownership of buffers still accessible to the operating system.
- Required system work and native domains remain available during drain.
  Shutdown failure is reported without making safe destruction conditional on
  successful cleanup.

An empty operation count is not proof that no old control packet remains in a
shared native queue. Registration retirement needs an explicit quiescence
protocol. If a transition to drained state produces no native notification, it
must produce a runtime notification or carry a bounded recheck deadline.

Protect ownership during initialization and attachment failures too: invoking
driver-controlled hooks must not expose partially published state or release
native resources still in use. [Windows cancellation guidance][windows-cancel]
illustrates why requesting cancellation alone is insufficient.

## Alternatives and trade-offs

| Alternative | Benefit | Limitation |
| --- | --- | --- |
| One primary and dedicated hosts for later drivers | Accommodates opaque blocking implementations. | Thread count and completion delivery costs depend on registration count and order. |
| Round-robin finite waits | Works with the existing combined method. | Adds idle polling and service latency; several sequential waits accumulate delay. |
| A common raw wait-handle interface | Simple for suitable pollable sources. | Does not compose arbitrary IOCPs or define ownership of already-dequeued records. |
| One universal I/O implementation | Centralizes native ownership. | Couples unrelated drivers to operation formats, ring configuration, and resource choices. |
| Capability-based coordinator with explicit fallback | Preserves private driver state while sharing compatible waiting points. | Requires a stronger attachment, progress, and retirement protocol. |

Prefer the final option. Preserve the fused single-driver fast path where
possible. Sharing a completion domain should remove unnecessary thread handoff,
not introduce a new thread or per-operation allocation. Its actual performance
remains a question for concrete implementations, not a property proven by the
abstract interface.

## Relationship to the existing contract

Keep context-selected providers, lazy acquisition, context caching, and
thread-local driver ownership. `ProviderOptions` and `DriverOptions` are natural
places to supply optional coordination facilities without exposing a concrete
runtime type. Native adapters can provide platform-specific registration while
the scheduling contract stays independent of operation representations.

The proposal changes more than how a runtime picks a primary driver. Coordinated
participation affects the [execution model](REQUIREMENTS.md#r4-driver-owned-execution-strategy),
cooperative draining changes the [blocking shutdown model](REQUIREMENTS.md#r6-safe-and-blocking-shutdown),
and recoverable attachment needs a failure policy different from the current
[fatal registration policy](REQUIREMENTS.md#r7-registration-failure-is-fatal).
These require an explicit contract decision;
optional context accessors alone do not give existing drivers those guarantees.

Open choices include the precise capability and drain interfaces, default
fallback/thread budgets, and the Linux wait implementation. `epoll` provides a
straightforward composition point; a coordinating ring is an alternative with
different submission, cancellation, and registration-lifetime costs.

## Evidence needed before adoption

A concrete implementation should demonstrate independent native sources making
progress on one worker, not only two context types being registered. Important
cases include a hot source beside a sparse source, a completion arriving during
parking, same-thread and remote interruption, registration during operation,
budget exhaustion without another notification, and failed partial attachment.

Shutdown scenarios must retain contexts and wakers, race admission with
shutdown, include pending native operations, and account for late control
packets. Thread-local and provider-shared source arrangements both matter.

Compare zero, one, and several driver configurations for CPU use, thread count,
allocations, wake frequency, context switches, throughput, and tail latency.
Separate active-service cost from idle-wait cost, and include explicit dedicated
hosting as the compatibility baseline. Native ring configurations with required
kernel service need their own coverage.

## Public sources

- [`CreateIoCompletionPort`: handle association and completion keys][iocp-create].
- [`GetQueuedCompletionStatusEx`: native collection from one port][iocp-wait].
- [`WaitForMultipleObjects`: supported object types][windows-wait].
- [`RIONotify`: completion queue notifications through events or IOCP][rio-notify].
- [Linux `io_uring` readiness implementation][uring-poll].
- [`liburing` event registration and notification semantics][uring-eventfd].
- [`io_uring` setup flags and progress requirements][uring-setup].
- [`io_uring_get_events`: outstanding work and completion flushing][uring-events].
- [Glommio: one reactor coordinating multiple rings][glommio].
- [`mio`: Windows native collection and dispatch][mio-selector] and
  [shared-port wake-up][mio-waker].
- [Windows asynchronous cancellation and resource lifetime][windows-cancel].

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
