# Panic policy

When unwinding is enabled, Arty catches panics raised while it:

- creates an async task from its task factory;
- polls an async task;
- runs a blocking callback.

Arty reports these panics through `JoinError` when possible. By default, Arty
shuts down the runtime after the first unhandled user-task panic.
`PanicPolicy::ignore()` keeps the runtime running.

Arty does not guarantee recovery from panics raised by destructors, custom
wakers, join notifications, telemetry processors, or other cleanup and
notification hooks. Arty also does not recover poisoned state, failed
invariants, or unsafe-contract violations.

Arty may catch some of these panics to maintain memory safety, but callers
cannot rely on continued operation or complete cleanup. The process may abort,
resources may leak, or affected tasks and runtime operations may stop making
progress. Arty remains memory-safe under all panic conditions.
