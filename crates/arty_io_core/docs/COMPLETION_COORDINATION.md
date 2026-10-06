# Completion coordination across I/O drivers

An overview of technologies for coordinating completion notifications from
independent I/O sources on Windows and Linux. This is a technology survey,
not a crate API or implementation proposal.

## The coordination problem

Independent drivers can own their operation formats, buffers, and completion
queues while sharing a thread that waits for work. The challenge is connecting
every source's notification to that waiting thread.

Consider two drivers whose completions are published to tasks only when their
owner thread processes them:

```text
1. Service A without blocking.
2. Service B without blocking.
3. Block indefinitely in A.
4. B's native queue receives a completion.
5. B cannot publish the task wake until the worker services B.
```

Without a notification path from B to the waiting thread, B cannot make
progress. Finite waits can bound this delay, but introduce polling and latency.
Zero-duration scans avoid blocking on the wrong driver but consume CPU while
idle. A dedicated observer solves progress at the cost of threads and possible
cross-thread completion delivery. Native sharing can avoid that handoff when
the sources are compatible.

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

## Coordination patterns and trade-offs

| Pattern | Benefit | Limitation |
| --- | --- | --- |
| Shared IOCP | Multiple compatible producers feed one collection point. | Requires coordinated handle association and routing before private pointer decoding. |
| Pollable rings or completion `eventfd`s | Independent Linux rings share a waiting point. | Readiness is a hint; ring-specific progress, draining, and rearming still matter. |
| Existing callback or completion service | Connects an already observed source to a waiting thread. | Requires a real notification path; wrapping a blocking API in a future does not create one. |
| Dedicated observer threads | Accommodates incompatible or opaque blocking sources. | Adds thread, synchronization, and possible cross-thread delivery costs. |
| Round-robin finite waits | Avoids requiring a shared native notification mechanism. | Adds polling and service latency; sequential waits accumulate delay. |
| One I/O implementation | Centralizes native ownership. | Couples consumers to common operation formats, queue configuration, and resource choices. |

These patterns can be combined. Native compatibility, thread budgets, locality,
and active-polling requirements determine which arrangements are practical.
Sharing a waiting point does not require sharing operation representations.

For `arty_io_core`, a secondary driver does not receive runtime cycle
callbacks. It therefore either coordinates its completion source with the
primary driver's wait and notification path, or continuously processes
completions on independent driver-owned background execution. A notification
only tells the runtime that service may be needed; it does not replace draining
the source or provide ownership of completion state.

## Progress and parking

Service includes the backend's required submission progress, completion
processing, and task wakes. Deferred work must have an identified owner
responsible for flushing it before sleeping. Completion processing must not be
assumed to flush submissions unless its contract says so.

Budgets bound work per source and per worker cycle. Exhausting a budget keeps
the source runnable even when no further kernel notification will arrive.
Separate control packets from operation completions, retain native errors, and
avoid allocating a new object for every completion merely to cross this
boundary.

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

## Lifetime and shutdown constraints

Shared collection introduces ownership beyond individual operations. Queued
notifications, executing callbacks, and registrations can outlive the source
that initiated them. An implementation needs to account for these lifetimes:

- Admission closure synchronizes with acquiring active-operation ownership.
  A flag check followed by work without active-operation ownership is not a drain
  barrier.
- Retained user handles must not access resources already released by shutdown.
- Operations, callbacks, and operating-system-visible storage retain independent
  ownership. Cancellation is a request, not permission to free native storage.
- Source leases cover queued and executing dispatches as well as active I/O.
  Stale wake packets and retained notification handles cannot target freed or reused
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

## Comparing implementations

A concrete implementation should demonstrate independent native sources making
progress on one worker, not only successful registration. Important
cases include a hot source beside a sparse source, a completion arriving during
parking, same-thread and remote interruption, registration during operation,
budget exhaustion without another notification, and failed partial attachment.

Shutdown scenarios must retain user and notification handles, race admission with
shutdown, include pending native operations, and account for late control
packets. Thread-local and shared source arrangements both matter.

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
