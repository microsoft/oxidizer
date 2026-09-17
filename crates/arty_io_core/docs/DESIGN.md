# Design

## Purpose

Arty hosts independently supplied I/O drivers without selecting one universal
operation format or native queue implementation. `arty_io_core` defines how those
drivers negotiate facilities, make bounded progress, and retire safely while a
runtime coordinates their waiting point.

The design follows four rules:

1. Keep consumer operations and native representations private.
2. Separate driver service from native collection and waiting.
3. Make progress, notification, and failure obligations explicit.
4. Make graceful shutdown observable without making it a safety protocol.

## Ownership and data flow

```text
consumer context type
        |
        v
provider creates shared configuration
        |
        | clone and relocate per owner thread
        v
create selects actual clients <----- waiter supplies clients + readiness waker
        |
        v
boxed local driver
        |
        | non-blocking service under a budget
        v
owned cooperative drain

native activity --> separate waiter --> private records or source readiness
                           ^
                           |
                 runtime interruption
```

Consumer contexts are mobile, independently retained handles. Their relocation
can improve locality, but correctness does not depend on relocation being called.
An existing native binding continues to obey its own owner and lifetime rules.

Drivers stay on their final owning thread. Providers return
`Box<dyn Driver<Context = Self::Context>>` without `Send` or `Sync`, rather than a
movable concrete driver. Runtime-only adapters may erase unrelated context types
without adding those auto traits or changing where service and destruction run.

A provider can share an engine or resources across its instances. The per-worker
driver adapter does not imply a per-worker physical queue. Context type identity
selects registration; native adapters separately identify physical sources,
callback routes, and collection domains.

## Negotiation before publication

The runtime configures native waiters and chooses final owner threads before
creating drivers. It builds the basic `DriverContext`, then asks the waiter it
actually drives to attach that collector's client capabilities. This operation
is object-safe, so the runtime need not name platform-specific client types.

During `create`, a provider selects a strategy from the clients actually present,
before creating native bindings. Missing clients and duplicate insertion are
errors. There is no separate advertised capability list to repeat or keep in sync.

The provider factory creates shared configuration. Strategy-specific shared
resources can be initialized once actual clients are known, with compatible
subsequent worker instances reusing them. Creation stays prompt and cannot
wait for async work or a cross-worker initialization handshake. The runtime
supplies a coherent configuration for shared strategies; core does not solve
automatic capability intersections across heterogeneous workers.

Native adapter factories pair clients with their real backing resources and
retain registration ownership. The assembly seam is not a proof that an arbitrary
adapter implementation supplied the correct native object. Typed client lookup
also requires agreement on the adapter interface/version, beyond agreement on
the core contract itself.

Registration is transactional runtime work. The first context request succeeds
only after every active worker has created its instance. Routing and notification
must be ready before publication. Errors require cleanup or safe retirement of
partial instances rather than success for whichever workers happened to finish.
Later requests reuse cached contexts.

## Separate wake directions

Native completion activity makes a driver service participant ready. The runtime
latches that readiness and interrupts the domain waiter. A native collector may
first place records in a private mailbox, or simply report that a private queue
needs attention. A readiness waker carries neither operation data nor permission
to invoke thread-local driver code on the notifying thread.

The waiter's waker has another purpose: interrupt the current or next blocking
collection for task, command, or source activity. Its latch closes the race
between the last work check and entering the native wait.

Both paths remain safe for late callers. Native registrations use retained
identity or generations so old packets and wakers cannot target newly installed
drivers. Removing an entry from a routing table is not, by itself, a resource
retirement protocol.

## Bounded turns and safe parking

Collection, running-driver service, and draining each receive finite budgets.
Charge before a bounded progress step and report continuation when work remains.
The budget is cooperative accounting: implementations must not reset it or hide
an unbounded amount of work inside one charged operation.
New drivers and drains start runnable and receive an initial turn without a
native notification.
The core does not assume that a consumed batch produces another notification.
It also does not assume that inspecting a queue performs required submission
flushing or kernel task work.

Native collection must itself preserve fair discovery or delivery across active
sources. Repeatedly scanning from one busy source can starve another before the
runtime has any chance to schedule its driver. Source continuation and ordering
belong to the native adapter; participant rotation belongs to the coordinator.

An idle service result is only a scheduling observation. Before sleeping, each
participant arms its notification mechanism and rechecks private state. It either
reports work ready or confirms that notification is armed. The runtime separately
rechecks commands, tasks, source signals, and deadlines.

While other work is runnable, the runtime continues non-blocking native collection.
A positive wait is permitted only after the full preparation protocol. Finite
waits do not become infinite during native unit conversion, and collection never
blocks with an exhausted budget or waits again merely to fill a partially
available batch.

Deadlines use the standard monotonic `Instant` domain. The runtime accounts for
every relevant participant and its own task and shutdown deadlines. A deadline
already reached requires service rather than sleep.

## Cooperative shutdown

Consuming the boxed driver closes admission synchronously and returns a boxed
`Drain`. There is no repeatable borrowed initiation operation. Admission
closure is synchronized with accepting operations, so work racing shutdown is
either tracked in the drain or rejected.

A drain follows the same budget and notification preparation protocol as a
running driver. It retains explicit service budgets rather than a future poll
without this protocol's budget.
The runtime initiates all relevant drains, keeps native collection and system
work available, and gives each participant progress while enforcing an overall
deadline.

The coordinator removes and drops a drain after completion or an error from
service or preparation. It never calls a terminal drain again. The runtime reports
failure and continues cleanup of independent participants; errors cannot turn
into an idle or successful result on a subsequent turn.

Contexts remain closed handles and are not drain participants. Operations,
callbacks, and native registrations retain the storage they can still access.
When native code holds raw pointers, the driver keeps a backing owner alive
independently of its public handle. Cancellation, timeout, and early drop never
authorize invalidating that storage.

This makes destruction safe during failed initialization, unwinding, or abandoned
shutdown without requiring an unsafe driver trait or an inertness query.
Destruction does not wait for I/O, other participants, or callbacks; independent
cleanup retains its own resource ownership when needed.

## Error and policy boundaries

`DriverError` is the common failure boundary for negotiation, creation, progress,
and shutdown. Native error causes remain accessible. Classified unsupported
configuration, duplicate capability, and shutdown timeout do not
require parsing messages.
Attaching a cause preserves the classification and descriptive context rather
than forcing a choice between fallback information and native diagnostics.

Runtime policy chooses the response: reject a configuration, try another
explicitly configured domain, retire a failed source, or stop a failed domain.
Individual I/O errors are operation results and do not themselves fail a driver
or its collection domain.
Core does not silently add threads, busy-poll a supposedly idle source, or mask
partial registration.

System work uses the runtime-owned synchronous offload facility. It remains
available while running drivers and pending drains require it, including
registration rollback. It is not an async task scheduler or a thread-per-driver
implementation.
Submission returns an acceptance result: success does not imply completion,
and rejection promises no task execution or callback. A cleanup submission
failure is reported by draining rather than hidden behind an eventual timeout.
Retained owner threads and transferred cleanup also retain execution authority
when a controller times out. Accepted work cannot be discarded behind a pool stop
marker. Execution obligations, not inert handles or retained context clones,
determine when the existing facility can retire.

## Native scope and reference runtime

The native mechanisms and their constraints are described in
[Completion coordination](COMPLETION_COORDINATION.md). The core defines the
shared control protocol and typed client boundary; production native adapters
are separate components.

The two-thread reference runtime exercises that protocol through in-memory
record delivery and readiness sources. Its own registries, queue implementation,
source identity, fairness, and timeout policy are examples of runtime/native
responsibilities, not additional public core APIs.
The control thread uses blocking result handles. The example is not an
application-future executor or a production native backend.
