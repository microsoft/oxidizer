Requests shutdown without blocking the calling thread.

Repeated requests are allowed. Shutdown cancels asynchronous work, closes admission
to new tasks, and waits for already accepted blocking work.

Cancelled or rejected tasks leave their joins pending indefinitely. Observe any
required asynchronous results before requesting shutdown. Use
[`Runtime::wait`](crate::runtime::Runtime::wait) from an allowed blocking context
to wait for worker shutdown; that does not complete cancelled joins.
