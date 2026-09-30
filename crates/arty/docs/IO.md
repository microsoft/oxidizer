# I/O

The initial Arty runtime has no asynchronous I/O subsystem, driver injection API,
or built-in memory pool. Neither worker execution nor `Builtins` uses `oxidizer_io`
or `bytesbuf`.

Task wakeups, incoming commands, timer advancement, and shutdown operate without an
I/O driver. Blocking tasks remain available for synchronous operating-system
calls; they are not an asynchronous I/O integration.

External I/O integration through `arty_io_core` is planned separately. The existence
of that contract crate does not enable I/O in this release.
