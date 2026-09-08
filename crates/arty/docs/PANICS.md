# Panics

Arty types must be unwind-safe unless documented otherwise. Foundational I/O contracts in
`arty_io_core` must also be panic-safe.

Every spawned task has a join handle whose result includes a `JoinError` when the task does not
complete normally. The error distinguishes a task panic from a task that stopped because the
runtime shut down.

The first task panic encountered by the runtime starts runtime shutdown. The panic remains
observable through the panicking task's join handle.

## Critical tasks

Most tasks are non-critical. Once shutdown starts, the runtime stops polling non-critical tasks
and their join handles complete with the shutdown form of `JoinError`.

Critical tasks are reserved for specialized, high-value operations that must finish before
shutdown completes, such as flushing telemetry. The runtime continues polling critical tasks
during shutdown. A task spawned from within a critical task's context is automatically critical,
so the entire operation can finish without each child task being marked separately.
