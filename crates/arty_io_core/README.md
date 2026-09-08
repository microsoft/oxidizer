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
* [`DriverContext`][__link1] associates a requested context type with its provider.
* [`DriverProvider`][__link2] creates and connects the per-worker adapters for a driver.
* [`DriverInit`][__link3] describes the worker and runtime facilities available during creation.
* [`SystemTasks`][__link4] lets a driver delegate blocking system work to the runtime.

Registration and driver placement are runtime behavior, not part of this crate. Keeping those
policies outside the contract allows the runtime and drivers to evolve independently.

## Runtime and driver lifecycle

The contract separates one provider per registered driver type, one driver per runtime worker,
and contexts that consumers may move and retain independently:

```text
context request
      |
      v
DriverContext::provider()
      |
      | clone and relocate once per worker
      v
DriverProvider::create(DriverInit)
      |
      +-- Driver: owned and driven by that worker
      +-- Context: obtained from the driver, then cached for consumers
```

### Registration and initialization

A consumer asks the runtime for a concrete [`DriverContext`][__link5] type. On the first request for
that type, the runtime calls [`DriverContext::provider`][__link6] and registers the resulting
[`DriverProvider`][__link7]. Registration, synchronization, rollback, and caching remain runtime
concerns.

The runtime clones the provider for each active worker, relocates each clone to that worker,
and invokes [`DriverProvider::create`][__link8] on the worker thread. [`DriverInit`][__link9] identifies the
worker and supplies runtime facilities such as [`SystemTasks`][__link10]. The returned [`Driver`][__link11] stays
on that thread for its entire lifetime; it is deliberately not required to be [`Send`][__link12] or
[`Sync`][__link13]. Creation runs inline and must return promptly; waiting there for another worker to
make progress can deadlock registration.

After creation, the runtime obtains the worker’s context through [`Driver::context`][__link14] and
may cache both that context and the driver’s [waker][__link15]. The waker honors wakes
raised by the driver’s own thread and remains safe to invoke after the driver is gone. The
first context request completes only after every active worker has created its driver instance.
Later requests reuse the registration and return the context cached for the calling worker. A
runtime may retain the provider to initialize workers created later.

Driver creation is infallible at the type level. If [`DriverProvider::create`][__link16] panics, the
runtime does not continue with a driver registered on only part of its worker set. A driver
with conditional platform or permission requirements therefore exposes its own capability
check for consumers to call before requesting its context.

### Driving I/O

Consumers start operations through contexts. Contexts may be cloned, relocated between
workers, and retained after their original driver is gone. Relocation may improve locality, but
correctness must not depend on it.

The runtime exclusively owns each driver and calls every [`Driver`][__link17] method only on its owning
thread. A zero wait to [`Driver::process_completions`][__link18] performs a non-blocking completion pass;
a bounded or unbounded wait lets the same call provide the worker’s idle point. The driver’s
waker is latched, so it ends either the current wait or the next one. Runtime policy decides
which driver supplies a worker’s waiting point and how additional drivers are scheduled.

Operations may be submitted from other threads while the driver waits. A driver therefore
separates its state into two parts:

* State reached by contexts, wakers, background threads, or operating-system callbacks is
  shared independently of the driver and uses appropriate reference counting and
  synchronization. Each in-flight operation owns every resource it uses through a reference
  count, pool lease, or equivalent handle; contexts themselves hold no per-operation state and
  therefore do not delay shutdown.
* Completion buffers, queue-reader state, batching state, and lifecycle state used only on the
  owning thread remain ordinary driver fields accessed through `&mut self`.

In particular, [`Driver::process_completions`][__link19] must not hold anything across a blocking wait
that a submitter needs to make a completion possible, such as a lock, queue slot, or pool
capacity.

### Shutdown

Shutdown is cooperative, but it is not a memory-safety protocol:

1. The runtime calls [`Driver::begin_shutdown`][__link20] on every driver, closing admission before it
   polls any one driver for drain progress. The method is idempotent.
1. Existing operations continue making progress through [`Driver::process_completions`][__link21].
   Contexts remain valid but reject new operations.
1. The runtime calls [`Driver::poll_shutdown`][__link22] for each driver. A pending driver registers the
   supplied task waker, and the runtime continues processing completions with bounded waits.
   Calling `poll_shutdown` before `begin_shutdown` starts shutdown as part of the poll.
1. [`Poll::Ready`][__link23] reports that active operations and
   operating-system callbacks have drained. Context handles do not themselves delay this
   transition, and later polls remain ready.
1. [`SystemTasks`][__link24] remains available through the graceful drain. The runtime bounds the total
   drain duration and reports or terminates on a liveness failure. A driver’s premature-drop
   soundness must not depend on system work submitted after that deadline.

A driver must nevertheless be safe to drop at any point, including during unwinding or after a
shutdown timeout. Dropping closes admission if necessary. Storage that an operating system can
reach only by raw pointer must have an independent owner that is retained rather than
invalidated on a premature drop. Shutdown completion determines whether cleanup was graceful,
never whether destruction is sound.

## Example

The [fixed two-thread runtime example][__link25]
starts both worker threads before `get_context::<SampleContext>()` uses the context type to
inject its associated driver.

## Project documents

* [Requirements][__link26]
* [Design][__link27]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbnHo0pRW_MUcb7ZTXw1O_VRob8DVd7WClLfkbhbV0zDZwSwZhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link12]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link13]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::context
 [__link15]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::waker
 [__link16]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link17]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver
 [__link18]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link20]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::begin_shutdown
 [__link21]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::process_completions
 [__link22]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::poll_shutdown
 [__link23]: https://doc.rust-lang.org/stable/std/?search=task::Poll::Ready
 [__link24]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link25]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs
 [__link26]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link27]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInit
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext::provider
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverInit
