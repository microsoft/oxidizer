# Driver contract follow-ups

## TL;DR

- Add a `DriverProvider` between `IoContext` and `Driver`.
- Make `Driver::shutdown` infallible.
- Keep `Driver::execute_cycle` fallible.

## Separate context from driver creation

`IoContext` has two audiences today:

- endpoint authors use it to start I/O;
- the runtime uses it to create worker-local drivers.

This makes a consumer-facing type carry runtime lifecycle methods. Add a
`DriverProvider` to keep those roles separate.

The intended split is:

- `IoContext` is the application-facing I/O handle;
- `DriverProvider` creates worker-local drivers for the runtime;
- the context identifies or creates its provider;
- the runtime clones and relocates the provider to each worker.

Provider-specific inputs, such as a GPU handle, should use a strongly typed
provider option rather than a shared empty options type.

Before changing the API, settle:

- how callers construct the context and provider;
- whether the provider is cloned per worker or relocated between workers;
- which `Clone`, `Send`, and `ThreadAware` bounds each type needs;
- when the runtime publishes the context relative to driver initialization.

The extra type is intentional. It gives endpoint code and runtime code narrower
interfaces and makes their different responsibilities visible.

## Make shutdown infallible

Change:

```rust
fn shutdown(self) -> Result<(), DriverError>;
```

to:

```rust
fn shutdown(self);
```

The runtime continues shutting down the other drivers regardless of the result,
so the error does not change control flow. Drivers must still close admission,
drain for a bounded time, remain safe to drop, and never panic.

This change has a cost. The runtime can no longer emit one uniform telemetry
event when a driver reports an unclean shutdown. Each driver must report its
own shutdown anomaly, closer to the failing operating-system call. We should
explicitly accept that loss of runtime-level telemetry before making shutdown
infallible.

## Keep cycle execution fallible

Keep:

```rust
fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError>;
```

Operating-system failures can make a cycle fail. A driver may retry internally,
but an error returned to the runtime means recovery failed. The runtime should
treat the driver as poisoned, stop normal cycles, and begin shutdown.
