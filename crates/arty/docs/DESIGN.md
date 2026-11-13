# Design

This document describes Arty's parts and the rules they follow.
The crate and module documentation describe how to use the public API.

## Separate runtimes

Each `Runtime` owns its workers and shutdown state. There is no single runtime
shared by the whole process. Stopping one runtime does not stop another runtime.

Schedulers, `JoinHandle`, and `Context` do not keep the runtime alive.
The `Runtime` value waits for workers to stop and reports runtime failures.

## Worker model

Each async worker runs tasks on one operating-system thread.
A task stays on the worker that creates its future. Arty does not move a running
task to balance load.

A future on that worker can keep thread-local and non-`Send` state.
Such state does not need to be sent between threads.
Tasks take turns when they await operations that are not ready.
A task that blocks the thread or does not give up control also delays other
tasks and timers on that worker. Other workers can continue.

Keeping tasks on separate workers controls where they run.
It does not protect one worker's data from another worker.
Workers can share application data.

## Choosing workers and updating state

`RuntimeScheduler` lets the runtime choose a worker for new tasks.
`Scheduler` sends tasks to one worker.
A task factory is a function that creates a future.
Factories and values sent between threads must implement the traits that the
scheduling method requires.

Relocation updates a value's state for a selected worker.
`ThreadAware` provides the method that makes this update.
Relocation does not move a running task or change which runtime an Arty scheduler
or context belongs to.
Waiting for a task's result outside an Arty async worker does not relocate that result.

A waker tells a worker that a task is ready to run again.
A cloned waker sends this request to the task's assigned worker.
It does not move the task or run it on the calling thread.
Keeping a clone does not keep the task or runtime alive.
After the task completes, panics, or is cancelled, you can call or clone the
waker safely. The operation has no effect. The same rule applies after runtime
shutdown.

## Supporting services

The runtime has three supporting service groups:

- Blocking pools run blocking functions outside async workers. The runtime can
  use one shared pool or one pool per worker. A running function can delay
  shutdown until it returns.
- I/O contexts provide access to I/O libraries. Their drivers process I/O
  requests. Arty creates contexts once per runtime and drivers once per worker.
  The [I/O guide](IO.md) describes driver roles, access rules, failures, and
  shutdown.
- Worker clocks wake tasks when their timers expire.

## Shutdown and failure

The first shutdown request records why the runtime stops. Later requests do not
replace that reason. Shutdown rejects new tasks and cancels pending async tasks.
It also discards queued blocking functions.

Worker code requests shutdown without waiting.
Calling `Runtime::stop` or dropping the `Runtime` value waits for workers to stop.
If a runtime failure caused shutdown, runtime and task errors retain it.
Use `Error::source` to read that underlying failure.

Arty catches panics only from the user operations listed in its documentation.
Other panics can stop progress or abort the process. See [panic handling](PANICS.md)
for the full policy.
