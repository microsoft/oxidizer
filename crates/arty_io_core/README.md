<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Io Core Logo" width="96">

# Arty Io Core

[![crate.io](https://img.shields.io/crates/v/arty_io_core.svg)](https://crates.io/crates/arty_io_core)
[![docs.rs](https://docs.rs/arty_io_core/badge.svg)](https://docs.rs/arty_io_core)
[![MSRV](https://img.shields.io/crates/msrv/arty_io_core)](https://crates.io/crates/arty_io_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Stable contracts for integrating external I/O drivers with the Arty runtime.

The runtime hosts drivers supplied by libraries and applications rather than depending on one
I/O implementation. This crate contains the small vocabulary both sides share:

* [`Driver`][__link0] is the adapter between one worker and an I/O subsystem.
* [`IoContext`][__link1] is the consumer handle that selects its provider.
* [`ProviderOptions`][__link2] supplies runtime facilities when that provider is created.
* [`DriverProvider`][__link3] creates and connects the per-worker adapters for a driver.
* [`DriverOptions`][__link4] describes the worker and runtime facilities available during driver
  creation.
* [`DriverHandle`][__link5] connects drivers during same-worker registration.
* [`ShutdownError`][__link6] reports unsuccessful graceful shutdown.
* [`SystemTaskSpawner`][__link7] lets a driver delegate blocking system work to the runtime.

Registration and driver placement are runtime behavior, not part of this crate. Keeping those
policies outside the contract allows the runtime and drivers to evolve independently.

## Runtime and driver lifecycle

The contract separates one provider per registered driver type, one driver per runtime worker,
and contexts that consumers may move and retain independently:

```text
context request
      |
      v
IoContext::provider(ProviderOptions)
      |
      | clone and relocate once per worker
      v
DriverProvider::create(DriverOptions)
      |
      +-- Driver: owned and driven by that worker
      +-- Context: obtained from the driver for consumers
```

### Registration and initialization

A consumer asks the runtime for a concrete [`IoContext`][__link8] type. On the first request for that
type, the runtime creates a [`ProviderOptions`][__link9], calls [`IoContext::provider`][__link10], and registers
the resulting [`DriverProvider`][__link11]. Registration, synchronization, and rollback remain runtime
concerns.

The runtime clones the provider for each active worker, relocates each clone to that worker,
and invokes [`DriverProvider::create`][__link12] on the worker thread. [`DriverOptions`][__link13] identifies the
worker, supplies runtime facilities such as [`SystemTaskSpawner`][__link14], and provides construction-time
[`DriverHandle`][__link15] values for drivers registered earlier on that worker. The returned [`Driver`][__link16]
stays on that thread for its entire lifetime; it is deliberately not required to be [`Send`][__link17]
or [`Sync`][__link18]. Creation runs inline and must return promptly; waiting there for another worker to
make progress can deadlock registration.

After storing the new driver, the runtime calls [`Driver::on_peer_registered`][__link19] on every
driver registered earlier on that worker, in registration order. The callback receives the new
driver’s [`DriverHandle`][__link20] and completes before the worker acknowledges registration.

After creation, the runtime may cache the driver’s [waker][__link21]. The waker honors
interrupts raised by the driver’s own thread and remains safe to invoke after the driver is
gone. The first context request completes only after every active worker has created its driver
instance. Later requests reuse the registration and obtain a context from the calling worker’s
driver through [`Driver::context`][__link22]. A runtime may retain the provider to initialize workers
created later.

Driver registration is infallible at the type level. If [`DriverProvider::create`][__link23] or
[`Driver::on_peer_registered`][__link24] panics, the runtime does not continue with a driver registered
or connected on only part of its worker set. A driver with conditional platform or permission
requirements therefore exposes its own capability check for consumers to call before
requesting its context.

### Driving I/O

Consumers start operations through contexts. Contexts may be cloned, relocated between
workers, and retained after their original driver is gone. Relocation may improve locality, but
correctness must not depend on it.

The runtime exclusively owns each driver and calls every [`Driver`][__link25] method only on its owning
thread. A zero wait to [`Driver::process_completions`][__link26] performs a non-blocking completion pass
without consuming a pending interrupt; a bounded or unbounded wait lets the same call provide
the worker’s idle point. Before processing a cycle, the runtime captures one
[`Instant`][__link27] and passes it unchanged to every driver visited in that cycle.
The driver’s waker is latched, so it ends either the current blocking wait or the next
one without preventing pending completions from being processed. Runtime policy decides which
driver supplies a worker’s waiting point and how additional drivers are scheduled.

Operations may be submitted from other threads while the driver waits. A driver therefore
separates its state into two parts:

* State reached by contexts, wakers, background threads, or operating-system callbacks
  is shared independently of the driver and uses appropriate reference counting and
  synchronization. Each in-flight operation owns every resource it uses through a reference
  count, pool lease, or equivalent handle; contexts themselves hold no per-operation state and
  therefore do not delay shutdown.
* Completion buffers, queue-reader state, batching state, and lifecycle state used only on the
  owning thread remain ordinary driver fields accessed through `&mut self`.

In particular, [`Driver::process_completions`][__link28] must not hold anything across a blocking wait
that a submitter needs to make a completion possible, such as a lock, queue slot, or pool
capacity.

### Shutdown

Shutdown is cooperative, but it is not a memory-safety protocol:

1. The runtime removes the driver from its normal completion loop and calls
   [`Driver::shutdown`][__link29], transferring ownership of the driver.
1. `shutdown` closes admission, blocks while active operations and operating-system callbacks
   drain, and performs graceful cleanup. It owns the liveness policy for that wait and returns
   an error rather than blocking indefinitely. Contexts remain valid but reject new operations.
1. [`SystemTaskSpawner`][__link30] remains available until `shutdown` returns.
1. `shutdown` returns [`ShutdownError`][__link31] when graceful cleanup cannot be completed. The runtime
   records or reports the error and continues shutting down its remaining drivers.

A driver must nevertheless be safe to drop at any point, including during unwinding or after a
shutdown error. Dropping closes admission if necessary. Storage that an operating system can
reach only by raw pointer must have an independent owner that is retained rather than
invalidated on a premature drop. A successful shutdown determines whether cleanup was
graceful, never whether destruction is sound.

## Example

The [fixed single-thread runtime example][__link32]
starts both worker threads before `get_context::<SampleContext>()` uses the context type to
inject its associated driver.

## Project documents

* [Requirements][__link33]
* [Design][__link34]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbYyE8VhlgtNsbi7u0nqVmoxAbMVpz_S_AhIkbm-eb86lCA5NhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext::provider
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link15]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverHandle
 [__link16]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link17]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link18]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::on_peer_registered
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ProviderOptions
 [__link20]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverHandle
 [__link21]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::waker
 [__link22]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::context
 [__link23]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link24]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::on_peer_registered
 [__link25]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link26]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link27]: https://doc.rust-lang.org/stable/std/?search=time::Instant
 [__link28]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link29]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link30]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link31]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link32]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/single_thread_runtime/main.rs
 [__link33]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link34]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverOptions
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverHandle
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTaskSpawner
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ProviderOptions
