# Changelog

## [0.4.0] - 2026-11-02

- Breaking: Give each driver a waker that differs from its owner's waker. A waker is a callback that
  requests execution.
- Do not forward a driver wake directly to its owner in the same call.
- Define how each worker initializes an I/O context: clone it, relocate it with no source
  coordinate, then create its driver.
- Create at most one driver for each context registration on each worker.
- Update the requirement for local package `thread_aware_core` from `0.1.2` to `0.1.3`.

## [0.3.0] - 2026-10-20

- Replace the I/O provider contract with direct `IoContext::driver` creation, `DriverInstance`, owner wakers, and consuming shutdown.
- Update local dependency requirements: thread_aware_core ^0.1.1 -> 0.1.2.
