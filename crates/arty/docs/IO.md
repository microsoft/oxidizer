# I/O

Arty provides task scheduling and timers, but no asynchronous network or file
I/O driver. Libraries that depend on another runtime's I/O driver still need
that runtime; awaiting their futures on Arty does not supply the missing driver.

Use `TaskScheduler::spawn_blocking` for synchronous I/O. It runs the callback in
a blocking pool and returns a join that can be awaited without blocking an
asynchronous worker.

`arty_io_core` defines foundations for future I/O integrations. Those contracts
are not an I/O driver supplied by the Arty runtime.
