# Critical tasks

Arty does not currently have a critical-task mode. Calling work critical does
not protect it from shutdown: unfinished async tasks can be cancelled, and new
submissions are rejected once shutdown starts. Child tasks have the same limits.

For required work, such as flushing telemetry, start it while the runtime still
accepts tasks and await it and its required children before requesting shutdown.
With `#[arty::main]` or `#[arty::test]`, finish that work before the body returns.
Coordinate with any other code that can request shutdown; keeping the runtime
owner alive does not prevent an explicit stop request.

Do not rely on submitting cleanup from a destructor during shutdown. That work
can be rejected. Already-running blocking callbacks are allowed to finish, but
queued callbacks are not guaranteed to start.

Cleanup after a panic is only appropriate when the state it uses is still safe.
See [panic handling](PANICS.md).
