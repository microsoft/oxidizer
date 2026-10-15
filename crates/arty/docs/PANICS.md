# Panic policy

When unwinding is enabled, Arty catches panics raised while it:

- creates an async task from its task factory;
- polls an async task;
- runs a blocking callback.

Arty reports these panics through `JoinError` when possible. Receiving that
error does not by itself shut down the runtime.

Arty does not catch panics from destructors, custom wakers, join notifications,
telemetry processors, or other cleanup and notification hooks unless a catch is
required to maintain memory safety. If one of these panics, Arty does not
guarantee continued operation or complete cleanup. The process may abort,
resources may leak, or affected tasks and runtime operations may stop making
progress. Arty remains memory-safe under all panic conditions.
