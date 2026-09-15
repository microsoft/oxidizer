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
* [`ProviderContext`][__link2] supplies runtime facilities when that provider is created.
* [`DriverProvider`][__link3] creates and connects the per-worker adapters for a driver.
* [`DriverContext`][__link4] describes the worker and runtime facilities available during driver
  creation.
* [`ShutdownError`][__link5] reports unsuccessful graceful shutdown.
* [`SystemTasks`][__link6] lets a driver delegate blocking system work to the runtime.

Registration and driver placement are runtime behavior, not part of this crate. Keeping those
policies outside the contract allows the runtime and drivers to evolve independently.

## Runtime and driver lifecycle

The contract separates one provider per registered driver type, one driver per runtime worker,
and contexts that consumers may move and retain independently:

```text
context request
      |
      v
IoContext::provider(ProviderContext)
      |
      | clone and relocate once per worker
      v
DriverProvider::create(DriverContext)
      |
      +-- Driver: owned and driven by that worker
      +-- Context: obtained from the driver, then cached for consumers
```

### Registration and initialization

A consumer asks the runtime for a concrete [`IoContext`][__link7] type. On the first request for that
type, the runtime creates a [`ProviderContext`][__link8], calls [`IoContext::provider`][__link9], and registers
the resulting [`DriverProvider`][__link10]. Registration, synchronization, rollback, and caching remain
runtime concerns.

The runtime clones the provider for each active worker, relocates each clone to that worker,
and invokes [`DriverProvider::create`][__link11] on the worker thread. [`DriverContext`][__link12] identifies the
worker and supplies runtime facilities such as [`SystemTasks`][__link13]. The returned [`Driver`][__link14] stays
on that thread for its entire lifetime; it is deliberately not required to be [`Send`][__link15] or
[`Sync`][__link16]. Creation runs inline and must return promptly; waiting there for another worker to
make progress can deadlock registration.

After creation, the runtime obtains the worker’s context through [`Driver::context`][__link17] and may
cache both that context and the driver’s [interruptor][__link18]. The interruptor
honors interrupts raised by the driver’s own thread and remains safe to invoke after the driver
is gone. The first context request completes only after every active worker has created its
driver instance. Later requests reuse the registration and return the context cached for the
calling worker. A runtime may retain the provider to initialize workers created later.

Driver creation is infallible at the type level. If [`DriverProvider::create`][__link19] panics, the
runtime does not continue with a driver registered on only part of its worker set. A driver
with conditional platform or permission requirements therefore exposes its own capability
check for consumers to call before requesting its context.

### Driving I/O

Consumers start operations through contexts. Contexts may be cloned, relocated between
workers, and retained after their original driver is gone. Relocation may improve locality, but
correctness must not depend on it.

The runtime exclusively owns each driver and calls every [`Driver`][__link20] method only on its owning
thread. A zero wait to [`Driver::process_completions`][__link21] performs a non-blocking completion pass
without consuming a pending interrupt; a bounded or unbounded wait lets the same call provide
the worker’s idle point. The driver’s interruptor is latched, so it ends either the current
blocking wait or the next one without preventing pending completions from being processed.
Runtime policy decides which driver supplies a worker’s waiting point and how additional
drivers are scheduled.

Operations may be submitted from other threads while the driver waits. A driver therefore
separates its state into two parts:

* State reached by contexts, interruptors, background threads, or operating-system callbacks
  is shared independently of the driver and uses appropriate reference counting and
  synchronization. Each in-flight operation owns every resource it uses through a reference
  count, pool lease, or equivalent handle; contexts themselves hold no per-operation state and
  therefore do not delay shutdown.
* Completion buffers, queue-reader state, batching state, and lifecycle state used only on the
  owning thread remain ordinary driver fields accessed through `&mut self`.

In particular, [`Driver::process_completions`][__link22] must not hold anything across a blocking wait
that a submitter needs to make a completion possible, such as a lock, queue slot, or pool
capacity.

### Shutdown

Shutdown is cooperative, but it is not a memory-safety protocol:

1. The runtime removes the driver from its normal completion loop and calls
   [`Driver::shutdown`][__link23], transferring ownership of the driver.
1. `shutdown` closes admission, blocks while active operations and operating-system callbacks
   drain, and performs graceful cleanup. It owns the liveness policy for that wait and returns
   an error rather than blocking indefinitely. Contexts remain valid but reject new operations.
1. [`SystemTasks`][__link24] remains available until `shutdown` returns.
1. `shutdown` returns [`ShutdownError`][__link25] when graceful cleanup cannot be completed. The runtime
   records or reports the error and continues shutting down its remaining drivers.

A driver must nevertheless be safe to drop at any point, including during unwinding or after a
shutdown error. Dropping closes admission if necessary. Storage that an operating system can
reach only by raw pointer must have an independent owner that is retained rather than
invalidated on a premature drop. A successful shutdown determines whether cleanup was
graceful, never whether destruction is sound.

## Example

The [fixed two-thread runtime example][__link26]
starts both worker threads before `get_context::<SampleContext>()` uses the context type to
inject its associated driver.

## Project documents

* [Requirements][__link27]
* [Design][__link28]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbXHvW0KP2pNsbpAluTL3rKBwb7CkYGkSYEf0byc-sL65ysCdhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link15]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link16]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link17]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::context
 [__link18]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::interruptor
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ProviderContext
 [__link20]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link21]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link22]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link23]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link24]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link25]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link26]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs
 [__link27]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link28]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ShutdownError
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ProviderContext
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext::provider
