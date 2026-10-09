# I/O

Arty provides task scheduling and timers, but no async network or file
I/O driver. Libraries that depend on another runtime's I/O driver still need
that runtime; awaiting their futures on Arty does not supply the missing driver.

Use `Scheduler::spawn_blocking` for synchronous I/O. It runs the callback in
a blocking pool and returns a join that can be awaited without blocking an
async worker.

`arty_io_core` supports future I/O integrations; it is not an I/O driver.
