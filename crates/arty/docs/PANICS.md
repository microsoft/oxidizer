# Panic policy

When unwinding is enabled, Arty catches panics from three operations. It catches
a panic while it creates an async task from its task factory. It also catches a
panic while it polls an async task or runs a blocking callback.

Arty reports these panics through `JoinError` when possible. By default, the
first contained async task or blocking callback panic shuts down the runtime.
`PanicPolicy::ignore()` keeps the runtime running.

Arty does not guarantee recovery from panics raised by destructors, custom
wakers, join notifications, or other cleanup and notification hooks. Arty also
does not recover poisoned state, failed invariants, or unsafe-contract
violations.

Arty can catch some of these panics to maintain memory safety. Callers cannot
rely on continued operation or complete cleanup. The process can abort,
resources can leak, or affected tasks and runtime operations can stop making
progress. Arty remains memory-safe under all panic conditions.
