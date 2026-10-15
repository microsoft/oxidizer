# Panic policy

When unwinding is enabled, Arty catches panics raised while it:

- creates an async task from its task factory;
- polls an async task;
- runs a blocking callback.

Arty reports these panics through `JoinError` when possible. Receiving that
error does not repair application state changed before the panic. A contained
task panic does not by itself shut down the runtime.

With `panic = "abort"`, any panic aborts the process instead.

User-provided destructors, custom wakers, join notifications, telemetry
processors, and other cleanup or notification hooks must not panic. Arty does
not promise continued operation or complete cleanup if they do. The process may
abort, resources may leak, or affected tasks and runtime operations may stop
making progress. Arty must remain memory-safe.

A second panic while unwinding may abort the process.

Failed runtime invariants, poisoned runtime state, and unsafe-contract
violations are bugs. Arty provides no dedicated recovery guarantee for them. If
such a panic occurs inside an existing task or callback boundary, it may still
be reported through that boundary's `JoinError`.
