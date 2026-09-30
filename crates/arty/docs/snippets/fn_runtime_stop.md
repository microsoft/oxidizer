Requests shutdown without blocking the calling thread.

Repeated requests are allowed. Shutdown closes admission immediately, cancels
pending asynchronous/local tasks, and prevents queued blocking callbacks from
starting. Already-running blocking calls cannot be forcibly interrupted.

Cancelled or rejected joins return [`JoinError`](crate::task::JoinError) with
`is_shutdown() == true`. Use
[`Runtime::wait`](crate::runtime::Runtime::wait) from an allowed blocking context
to wait for worker shutdown and running blocking work to finish.
