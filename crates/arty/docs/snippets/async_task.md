# Asynchronous tasks

Async tasks are intended for short bursts of computation separated by asynchronous waits.
They must not block their worker thread. Use blocking tasks for blocking operating-system calls.
