# Driver contract follow-ups

Resolve these items before the contract is stable. They do not
block evaluation of the current API.

## Separate context from driver creation

`IoContext` is the application I/O handle and the factory for worker-local
drivers. Consider moving driver creation to a `DriverProvider`. This change
keeps runtime lifecycle methods out of endpoint implementations.

The intended split is:

- `IoContext` is the application I/O handle.
- `DriverProvider` creates worker-local drivers for the runtime.
- The context identifies or creates its provider.
- The runtime clones and relocates the provider to each worker.

For provider-specific inputs, such as a GPU handle, a strongly typed provider
option is preferable to a shared empty option type.

Before changing the API, settle:

- How callers construct the context and provider.
- Whether the provider is cloned for each worker or relocated between workers.
- Which `Clone`, `Send`, and `ThreadAware` bounds each type needs.
- When the runtime publishes the context relative to driver initialization.

## Stabilize `thread_aware_core`

`Thread` and `ThreadAware` are part of this crate's public API. Make those types
and their relocation contract stable before you stabilize `arty_io_core`.

## Review unwind-safety contracts

Decide which public value types will implement `UnwindSafe` and `RefUnwindSafe`.
Add contract assertions for those markers before stabilization.

`DriverError` accepts any `Error + Send + Sync` source. It therefore does not
promise unwind-safety markers. Keep this broad source support unless a specific
requirement needs narrower bounds.

## Move shutdown to `Drop`

Consider removing `Driver::shutdown`. Each driver then drains its resources from
`Drop`. An infallible shutdown method duplicates `Drop`.

This change stops the runtime from emitting one uniform event when a driver
cannot finish draining. Each driver then reports that condition. Resolve this
telemetry tradeoff before you remove the current fallible method.
