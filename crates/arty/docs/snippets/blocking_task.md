# Blocking tasks

Blocking tasks execute synchronous, potentially blocking work on a separate thread pool.
This keeps asynchronous workers responsive while a system call is in progress.
Pool sizing targets blocking calls rather than sustained CPU-bound computation.
