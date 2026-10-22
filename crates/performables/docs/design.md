---
title: Performables synchronization design
uid: performables_sync_design
---

# Synchronization design

Performables combines ownership and synchronization primitives with optional,
live-configurable telemetry. Its performance target includes builds with
Seismograph compiled in while recording is disabled. Enabling recording must not
change ownership, notification, poisoning, or timeout behavior.

This guide covers the synchronization extension. The
[implementation guide](implementation.md) explains its storage and platform
choices; the [crate overview](../src/lib.rs) and API documentation remain the
source of public contracts.

## Notifications and condition variables

[Notify](../src/sync/notify.rs) retains at most one unassigned permit.
Notifications without waiters coalesce; it is not an event counter. Once a wait
reserves or receives a notification, cancellation transfers that notification to
another waiter or retains it for a later wait.

Multiple consumers register with `NotifyWait::enable` before checking a
separately synchronized work queue. Future construction alone does not register.
Waking invokes caller-provided code, so registration and cancellation must remain
valid when waker cloning, destruction, or delivery re-enters the primitive.
Notification state is committed before calling that code. This does not promise
recovery from every panicking callback.

[Condvar](../src/sync/condition.rs) instead waits while releasing and reacquiring
a mutex. It does not retain notifications. Callers must recheck their protected
predicate after a wake, including a spurious wake.

## Locks and ownership

[Mutex](../src/sync/mutex.rs) and
[RwLock](../src/sync/lock.rs) preserve poisoning, explicit recovery, constant
construction, and native guard thread affinity. Internal backend choices are not
public layout guarantees. A guard must release its raw ownership exactly once,
including when telemetry unwinds.

Internal channel locks do not recursively emit synchronization telemetry.
Watch value borrows remain exclusive: supporting a `Send` payload does not
introduce a requirement that the payload also be `Sync`.

## Channels and cancellation

[Queue, oneshot, and watch channels](../src/sync/channel.rs) try a ready
operation before constructing a blocking adapter. A fallback must retain the
original timeout budget and contention-recording state rather than restarting
either.

Watch channels retain the latest value rather than a queue of every value.
Publication and closure share one version snapshot so a final unobserved value
remains observable before closure. Each receiver maintains its own observed
version; concurrent waits sharing one receiver claim a version only once.

Wait cancellation releases task and waker references promptly. Reusable capacity
can remain allocated until the primitive is destroyed. Allocation guarantees
exclude payload-owned storage, caller wakers, and telemetry recording unless an
API explicitly says otherwise.

## Validation boundary

Public contracts are exercised by the integration tests under
[tests](../tests). The [protocol models](../tests/loom_sync_primitives.rs)
explore registration/publication and ownership handoffs. Miri retains direct
ownership and synchronization paths; native tests and `cargo careful` retain
platform and stress breadth.

The edge-case fixtures and synchronization registration checks disable progress
watchdogs during mutation testing while retaining ordinary-run bounds.
Functional timeout tests remain active: an actual timeout contract is distinct
from a last-chance progress watchdog. See the repository's
[runtime-analysis policy](https://github.com/microsoft/oxidizer/blob/main/docs/runtime-analysis.md) for the complete
division of responsibility.
