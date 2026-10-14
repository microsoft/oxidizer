# Panic policy

Arty catches panics from code it explicitly runs for the user:

- async task polling;
- task factories;
- blocking callbacks.

When unwinding is enabled, Arty reports these panics through `JoinError` when
possible. With `panic = "abort"`, the process aborts instead.

Arty does not promise recovery from panics in destructors, custom wakers, join
notifications, telemetry processors, or other cleanup and notification hooks.
These panics may unwind the worker thread or abort if another panic is already
in progress. Values moved into tasks must not panic from `Drop`, and custom
wakers and telemetry processors must not panic.

Arty catches cleanup panics only when needed to protect memory safety or release
runtime-owned task storage exactly once. These catches must be narrow. A panic
that escapes while the runtime still owns live task storage stops the process
rather than risking memory corruption.

Failed runtime invariants, poisoned runtime state, and unsafe-contract
violations are bugs, not task failures. Arty does not convert them into
`JoinError`.

After a contained task panic, shutdown is best effort. Panic handling must not
introduce a deadlock, wait for the current callback to finish itself, or
continue with corrupted state.

Async and blocking tasks share one guarantee: a panic in the task or callback
body becomes a task or join failure. Their destructor and waker behavior may
differ when their execution models cannot support the same containment safely.

Add a new `catch_unwind` only when there is a realistic safe-Rust path, a sound
state to continue from, and useful behavior the caller can observe. Tests should
cover public guarantees and catches required for storage safety, not every
technically possible panicking `Drop` or `Waker`.
