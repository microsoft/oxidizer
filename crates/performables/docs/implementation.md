---
title: Performables synchronization implementation
uid: performables_sync_implementation
---

# Synchronization implementation

These are implementation choices, not additional public guarantees. The
[design guide](design.md) describes observable behavior and links to the API
contracts.

## Owned platform locks

The [mutex HAL](../src/sync/mutex/hal/mod.rs) uses an atomic ownership word and
address-based parking on Windows and Linux. The
[reader-writer HAL](../src/sync/lock/hal/mod.rs) similarly counts readers and
represents exclusive ownership and potential sleepers in one word.

Wrapping `std` locks would retain another owning guard and poison snapshot.
The owned backends avoid that guard and result-copying overhead while retaining
the existing safe platform adapters. Other targets keep the standard-library
fallback. Neither choice is a claim that Performables always beats `std`.

Raw ownership is retained across telemetry callbacks by cleanup guards. Their
destructors release ownership if a callback unwinds; successful handoffs disarm
the cleanup. [RawMutex](../src/sync/raw_mutex.rs) uses the same HAL without
recording internal channel operations recursively. Its exclusive guard is the
access boundary around its `UnsafeCell`.

## Reusable waiter slots

[SlotList](../src/sync/slot_list.rs) uses indexed links and a free list.
Unlike removing a known waiter from `VecDeque`, unlinking does not scan or shift
the remaining queue. Registration keys include generations so ordinary slot
reuse does not let cancellation remove a different registration.

The tradeoff is retained peak capacity. Removal drops the stored task or waker
reference, but does not shrink the slot vector. Registration, repolling,
cancellation, and wake delivery must preserve that distinction.

## Watch registration and publication

[Broadcast](../src/sync/broadcast.rs) partitions watch waiters across eight
cache-separated shards. Watch has no global FIFO delivery requirement, allowing
independent registration/cancellation locks. A thread-local selector is only a
load-balancing hint; a moved future retains its token's original shard.

Registration installs or refreshes a waker under its shard lock, publishes the
pending hint, then rechecks readiness. Its sequentially consistent fence pairs
with the producer's publication fence: either the receiver observes publication
or the producer observes a pending registration. Waker cloning and destruction
occur outside the lock because caller code can re-enter the channel.

A broadcast drains waiter slots, but a future can retain its registration token
until repolling or destruction. Generation checks reject ordinary reused slots;
repolling can create a replacement registration. Sharding and padding increase
idle storage, justified by the high-core cancellation workload. The shard count
and measured object sizes are not stable layout promises.

The [watch metadata](../src/sync/channel.rs) is separate from the value mutex.
Release version updates publish replacements, and acquire reads observe that
publication. Final-sender release joins preceding sender drops before setting
closure in the same status word. Relaxed endpoint increments account for
already-owned endpoints; they do not publish values. Observed-version
compare-exchange operations serialize claims on a shared receiver. Borrowing or
modifying the generic value still acquires the value mutex.

## Notification storage

[Notify](../src/sync/notify.rs) keeps an empty, retained-permit, or waiting state
in its header. A lazily initialized queue keeps the ready path compact without a
primitive-owned waiter allocation. Queue initialization precedes publication of
the waiting state.

A notifier selects a waiter under the queue lock, marks its notification
assigned, releases the lock, and only then invokes its thread or task wake
target. Cancellation removes a queued registration by identity and forwards an
already assigned notification. The notified flag is distinct from queue
membership because selection removes the slot before the consumer runs.

## Time budgets and measurements

Blocking adapters carry elapsed-time budgets rather than overflowing absolute
deadlines. Ready results are checked before expiry. Individual parks stay below
Windows' infinite-timeout sentinel, without resetting the original budget after
a spurious wake.

The [ready benchmarks](../benches/performables_ready.rs) separate setup and
teardown from measured operations. The
[watch benchmarks](../benches/performables_watch.rs) retain workers between
samples and include dispatch barriers equally for every control.
`std::time::Instant` measures that interval. The standard watch control is a
mutex-backed fixture, not a watch primitive provided by `std`. Its semantics and
cancellation cost must remain explicit when interpreting comparisons.

## Mutation watchdogs

Progress deadlines are test scaffolding, not behavioral timeout contracts.
They are disabled with `testing_aids::is_mutation_testing`; test-only clock setup
inside a production adapter belongs in a `cfg(test)` helper rather than in its
mutation-selected body. The mutation runner still fails on non-termination.
Reproduced progress-breaking variants are excluded by exact mutation identity,
not reported as caught because a test-local clock panicked.
