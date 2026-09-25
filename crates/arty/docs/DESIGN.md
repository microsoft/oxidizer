# Design

Arty is a single-threaded, thread-aware runtime. Stable external I/O integration contracts live
in `arty_io_core`.

Runtime shutdown is fail-fast for ordinary application work. The first task panic starts
shutdown, after which non-critical tasks stop being polled. Critical tasks continue until they
finish so specialized shutdown work, such as telemetry flushing, can complete.

Criticality is inherited through task spawning. A task spawned within a critical task's context
is also critical, including further descendants.
