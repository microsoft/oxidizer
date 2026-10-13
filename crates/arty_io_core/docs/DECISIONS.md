# Driver contract decisions

The following alternatives were considered and rejected for this contract.

## Keep context and driver creation together

`IoContext` remains both the application-facing I/O handle and the factory for
worker-local drivers. A separate public `DriverProvider` would add another type,
construction path, and relocation contract without adding a capability required
by the current design.

## Keep shutdown fallible

`Driver::shutdown` continues to return `Result<(), DriverError>`. The runtime
still attempts every driver after an error, but the result lets it emit uniform
telemetry when cleanup is incomplete. Drivers may also report deeper,
implementation-specific diagnostics.

## Keep shutdown concrete

`Driver::shutdown(self)` remains unavailable through `dyn Driver`. A runtime
that stores heterogeneous drivers can use a private owning adapter that erases
the concrete shutdown call together with its storage. The core contract does not
standardize runtime storage or type erasure.

## Keep recovery inside the driver

`DriverError` does not implement recovery classification. Drivers handle any
implementation-specific retries before returning an error. Once an
infrastructure error reaches the runtime, the driver is considered unusable and
the runtime begins shutdown.
