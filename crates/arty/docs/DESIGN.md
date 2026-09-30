# Design

Arty runs a single-threaded executor on each selected processor. A task never migrates
after it starts. The `rt` Cargo feature exposes runtime APIs under `arty::runtime` and
implies `time`; the default feature set remains empty. Schedulers and join handles
live under `arty::task`. Configuration types are exported directly from
`arty::runtime`, alongside the single opaque `Error` type.

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

Shutdown cancels asynchronous work and waits for accepted blocking work. A cancelled
join stays pending; callers must not rely on it completing after cancellation.
Local admission closes before cancellation runs destructors. Dropping the owner
inside its own blocking task requests shutdown without waiting for that task to join
itself; explicitly waiting for its own runtime shutdown from a blocking task is rejected.
`block_on` supports borrowed captures and does not return or unwind before their
storage is destroyed.

There are no task metadata, fan-out, explicit placement, or runtime yield APIs.
Blocking tasks use per-worker pools by default, or one shared pool when
configured. Time primitives remain available independently under `arty::time`.

With `test-util`, Miri exercises scheduling, owned capabilities, macros, timers,
and cleanup against simulated processor data. This does not claim to test OS
affinity or real hardware discovery. Native tests retain their original workload.
