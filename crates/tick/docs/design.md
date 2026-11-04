# Runtime-neutral time design

## Purpose

The `tick` crate abstracts time for application code. Code uses one API for
clock reads, elapsed time, delays, periodic timers, and timeouts. It does not
need to depend on the time API of one async runtime.

The same application code can use a production clock or a controlled test
clock. Tests can set or advance time without a real wait.

`tick` can integrate with an async runtime that can:

- Store and invoke a `Waker`.
- Call a clock driver from its runtime loop.
- Park until a timer deadline or a driver notification.

The API documentation remains the source for public behavior. This guide
explains the runtime boundary and its internal design.

## Application boundary

Application code uses `SimpleClock` or `Clock`.

`SimpleClock` reads absolute and monotonic time. It does not own timers and does
not need a runtime driver.

`Clock` adds delays, periodic timers, and timeouts. `Delay`, `Timer`, and
`FutureExt` use the clock instead of a runtime-specific timer type.

This boundary lets a library accept a clock from its caller. The library does
not need to know which runtime drives that clock.

## Runtime integration boundary

The `runtime` module connects `tick` to an async runtime. The runtime completes
these steps:

1. Create an `InactiveClock`.
2. Activate it with the waker for the runtime driver.
3. Give the returned `Clock` to application tasks.
4. Call `ClockDriver::advance_timers` from the runtime loop.
5. Park until the returned deadline or a driver notification.

The first timer and each newly earlier timer invoke the driver waker. This
notification tells the runtime to calculate its next parking deadline again.

A notification can arrive after the driver returns and before the runtime
parks. The runtime must retain pending notifications or make this transition
race-free.

The driver waker must not own the related `Clock` or `ClockDriver`. Such
ownership creates a cycle that can prevent shutdown.

## Runtime storage models

`InactiveClock` supports two timer-storage models.

### Isolated storage

Isolated storage is for runtimes that keep work on stable worker threads. The
runtime clones and relocates an inactive clock before activation. Each worker
then gets separate timer storage and its own driver.

This model removes timer-lock contention between workers. A timer is visible
only to the driver for its storage.

### Shared storage

Shared storage is for work-stealing runtimes where tasks can move between
threads. The runtime activates one clock and shares it between tasks. One
driver advances the shared timer set.

The optional Tokio integration uses this model. Tokio is one adapter, not the
core runtime contract.

## Controlled time

`ClockControl` supplies time for tests. Its clones share one controlled time
state and one timer set.

Production and test code use the same `Clock` API. A test can replace real time
with controlled time without changing the code under test.

The owner of a controlled clock advances time and wakes ready timers. Its
driver has no operating system parking deadline. A runtime must stop its
controlled driver loop from its own shutdown state.

## Time conversion

`Clock::system_time_as` and `SimpleClock::system_time_as` are infallible
convenience methods. Production data is a current operating system clock value
without test-controlled offsets. Each supported `tick::fmt` type accepts that
range.

A controlled clock can hold a value outside a target format range. In that
case, the convenience method panics. Code that expects conversion failure must
call `system_time` and then call the target type's `TryFrom<SystemTime>`
implementation.

## Delay and timer contracts

`Delay` represents one deadline. `Timer` represents repeated waits with one
period. `Timer::next` returns `TimerTick`, which records the observed completion
instant.

`Timer` does not implement a public stream trait. This keeps stream libraries
out of the runtime-neutral dependency contract. An application can adapt a
timer to its chosen stream library.

A pending wait keeps its registered deadline when its future is canceled. A
later poll uses the same deadline. If the task changes, the poll replaces the
registered waker.

The timer registration owns a waker until the deadline fires, the wait resumes,
or the timer object is dropped. This retention keeps cancellation safe without
exposing a separate public wait-future type.

## Timer storage

Timer storage uses an ordered map. A timer key contains the deadline and a
discriminator. The discriminator keeps registrations distinct when they have
the same deadline.

The system driver removes ready wakers into caller-provided stack storage. It
releases the timer lock before it wakes a task or drops a displaced waker. This
rule lets user code re-enter clock operations without deadlocking.

The common idle and one-ready-timer paths use a one-slot probe. Larger ready
sets use an eight-slot stack batch and repeat until no ready timer remains. This
avoids heap allocation on the timer advancement path.

Controlled clocks collect ready wakers while their state is locked. They wake
the tasks after the state operation releases the lock.

## Timer races

A driver can remove and wake a timer after a waiter reads the clock but before
it replaces the waker. A missing registration during replacement therefore
means that the timer already fired. The wait completes instead of registering a
new deadline.

Registering an earlier timer wakes the driver. This wake makes the driver read
timer storage again, even if a previous advancement returned an older next
deadline.

## Errors

`tick::Error` owns the crate error contract. It provides timeout and range
classification, retains upstream causes, and captures a backtrace.

Display output includes the backtrace when capture is enabled. Callers must use
classification methods and typed sources for decisions. They must not match the
display text.

## Validation

Public behavior is tested through integration tests in `tests`. Private unit
tests cover raw timer ordering, lock-release boundaries, and storage races that
the public API cannot arrange directly.

Native tests cover standard backtrace formatting. Miri does not run that one
formatting case because its isolation mode cannot call the operating system
functions used by the standard library backtrace formatter.
