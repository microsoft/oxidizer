# Tick design

## Purpose

The `tick` crate gives production code and tests one time API. Production
clocks read operating system time. Controlled clocks let tests set or advance
time without a real wait.

This guide explains the stable contracts and the internal timer design. The
API documentation remains the source for public behavior.

## Clock types

`SimpleClock` reads absolute and monotonic time. It does not own timers and does
not need a runtime driver.

`Clock` adds delays, periodic timers, and timeouts. A system clock uses a
`ClockDriver` to advance registered timers. A Tokio clock uses one background
task for this work.

`ClockControl` owns controlled test time. Its clones share one state, including
all registered timers. Its fluent configuration methods update this shared
state.

`InactiveClock` lets a runtime select isolated or shared timer storage before
activation. Activation returns a `Clock` and its `ClockDriver`.

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
out of the public dependency contract. An application can adapt a timer to its
chosen stream library.

A pending wait keeps its registered deadline when its future is canceled. A
later poll uses the same deadline. If the task changes, the poll replaces the
registered waker.

The timer registration owns a waker until the deadline fires, the wait resumes,
or the timer object is dropped. This retention is part of keeping cancellation
safe without exposing a separate public wait-future type.

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

## Races

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
