# Driver contract follow-ups

These items should be resolved before the contract is stabilized. They do not
block evaluation of the current API.

## Separate context from driver creation

`IoContext` is both the application-facing I/O handle and the factory for
worker-local drivers. Consider moving driver creation to a `DriverProvider` so
endpoint authors do not need to implement runtime lifecycle methods.

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

## Stabilize `thread_aware_core`

`Thread` and `ThreadAware` are part of this crate's public API. Stabilize those
types and their relocation contract before stabilizing `arty_io_core`.

## Review unwind-safety contracts

Decide which public value types should promise `UnwindSafe` and
`RefUnwindSafe` before stabilization. Add contract assertions for the intended
markers.

`DriverError` currently accepts any `Error + Send + Sync` source, so it does not
promise unwind-safety markers. Keep broad source acceptance unless there is a
concrete need to narrow it.

## Move shutdown to `Drop`

Consider removing `Driver::shutdown` and requiring each driver to drain its
resources from `Drop`. An infallible shutdown method would otherwise duplicate
`Drop`.

This removes the runtime's ability to emit one uniform event when a driver
cannot finish draining. Each driver would need to report that condition itself.
Resolve that telemetry tradeoff before removing the current fallible method.
