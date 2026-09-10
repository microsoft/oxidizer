# Panics

Arty types must be unwind-safe unless documented otherwise. The runtime catches task panics and
re-raises them when the task result is awaited. Unobserved task panics are reported to a runtime
panic handler. Foundational I/O contracts in `arty_io_core` must also be panic-safe.
