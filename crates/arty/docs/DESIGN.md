# Design

Arty runs a single-threaded executor on each selected processor. A task never migrates
after it starts. The `rt` Cargo feature exposes runtime APIs under `arty::rt` and
implies `time`; the default feature set remains empty.

`Runtime` owns startup and shutdown. `Runtime::task_scheduler()` returns a detached,
cloneable scheduler that distributes submissions round-robin. A scheduler obtained
from `Builtins` preserves its worker affinity. Relocation may rebind a capability
within its owning runtime, but foreign or unregistered destinations leave it unchanged.
Cloning a scheduler does not extend the runtime's lifetime.

Submission is available on `TaskScheduler`, not duplicated on `Runtime`. `run`,
`block_on`, and scheduler callbacks receive owned `Builtins`. A task may retain or
relocate its capabilities without borrowing another task or the worker's state.

Callbacks execute on their destination worker, so the futures they produce
need not be `Send`. Ordinary remote results require `Send`, not `ThreadAware`, and
are not automatically relocated. Local scheduler tokens accept non-`Send` work and
keep executor state confined to its worker, including during destruction.

Shutdown cancels asynchronous work and waits for accepted system work. A cancelled
join stays pending; callers must not rely on it completing after cancellation.
`block_on` supports borrowed captures and does not return or unwind before their
storage is destroyed.

There are no task metadata, fan-out, explicit placement, or runtime yield APIs.
Blocking system tasks use per-worker pools by default, or one shared pool when
configured. Time primitives remain available independently under `arty::time`.
