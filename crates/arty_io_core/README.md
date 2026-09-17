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

Contracts for coordinating independent I/O drivers with the Arty runtime.

Drivers retain their operations, buffers, and completion decoding. The runtime owns their
scheduling and a separate [`CompletionWaiter`][__link0] that collects native activity and provides
the blocking wait. This crate supplies the shared contracts, not a production native backend,
a driver registry, or a thread-placement policy.

## Negotiation and registration

A requested [`IoContext`][__link1] type selects its [`DriverProvider`][__link2]. The runtime supplies a
[`ProviderContext`][__link3] advertising the native client capability types of a proposed completion
configuration. The provider chooses one strategy and declares its [`CompletionRequirements`][__link4].
Alternative strategies are selected explicitly, not combined into one set of required clients.

Before creation, the runtime chooses the final owning threads and configures their waiters.
Each waiter and the client services backed by it share a [`CompletionDomain`][__link5] identity.
[`CompletionService`][__link6] tags prevent accidentally combining clients from different domains.
Native adapters remain responsible for associating those clients with the correct native
resources and for their registration/retirement rules.

On each owning thread, the runtime assembles [`DriverContext`][__link7] with thread coordinates,
[`SystemTasks`][__link8], a source readiness waker, and typed client capabilities. It validates the
selected requirements, relocates the provider clone, and consumes it to create a
[`LocalDriver`][__link9]. Creation must return promptly and establish routing and notification before
publishing a usable context.

Provider and driver creation return [`DriverError`][__link10]. The first context request succeeds only
after every active worker has initialized the driver. Failure requires explicit rollback or
retirement of partial registrations; it is not success for the surviving workers. Later
requests reuse the successfully registered provider/context family. Drivers with independent
versions coexist through distinct context type identities while using the same core contract.

## One wait, multiple service participants

```text
native activity -> CompletionWaiter -> records or readiness -> driver service
                          ^
                          |
            runtime task/control/source interruption
```

A native adapter may route already-collected records into private driver mailboxes or report
that a driver must drain its own queue. Records never travel through the construction-time
service lookup or through a `Waker`.

The [`DriverContext::readiness_waker`][__link11] identifies a driver service participant. The runtime
latches readiness for that participant before waking the collection domain. The separate
[`CompletionWaiter::waker`][__link12] interrupts the current or next blocking collection. Both handles
remain memory-safe after their original participant disappears; a late signal must not
target a replacement registration.

The coordinator alternates bounded collection, task/control work, and driver service. A
[`CompletionBudget`][__link13] limits one participant during each turn. [`ServiceStatus`][__link14] reports remaining work
or a service deadline. A runnable result schedules another turn without requiring a new
notification. Every newly installed driver and newly initiated drain starts runnable,
before the runtime can park, even if no native notification has arrived.

Before a positive wait, the runtime services due work, asks every participating driver or
drain to prepare notifications, and rechecks task, control, and source readiness. Only
[`WaitStatus::Armed`][__link15] permits that participant to sleep. The waiter must preserve an
interruption racing the final check and actual wait. Deadlines and remaining runnable work
also constrain whether and how long the runtime waits.

Drivers and [`Shutdown`][__link16] handles stay on their owning thread, enforced by local ownership
wrappers even when the concrete implementation has thread-safe fields. Native
clients may be thread-local; consumer contexts remain mobile through [`IoContext`][__link17].
A runtime with no native drivers can use an ordinary latched parking waiter. Unsupported
native configurations fail explicitly or use another explicitly configured domain; core
does not silently create helper threads.

## Cooperative shutdown and safe ownership

[`LocalDriver::shutdown`][__link18] consumes the running driver and closes admission before returning
a [`Shutdown`][__link19] handle. Its [`Drain`][__link20] continues service and notification preparation under the
same budget protocol. Every turn receives a budget; shutdown is not a blocking call or a future.

The runtime initiates all relevant shutdowns, keeps collecting native activity and executing
required system work, and services drains fairly. [`DrainStatus::Complete`][__link21] marks graceful
retirement; pending status retains its notification or deadline obligation. Failures remain
visible through [`DriverError`][__link22], and the runtime applies an overall shutdown deadline.

Context clones remain valid but closed and do not themselves delay drain completion.
Admitted operations, callbacks, and native registrations independently retain their storage.
Neither cancellation nor an expired deadline permits invalidating memory still reachable
by native code. Dropping a driver or abandoning a drain is always memory-safe, even when
graceful cleanup cannot complete. Destruction does not wait for I/O or other participants;
any necessary independent cleanup retains its own resource ownership.

## Example and design

The [two-thread reference runtime][__link23]
demonstrates coordinated in-memory completion sources, not production IOCP or `io_uring`
implementations. Registries, native routing, placement, and timeout policy belong to that
runtime/native layer rather than this crate.
Its control thread uses blocking result handles; it is not an application-future executor.

* [Requirements][__link24]
* [Design][__link25]
* [Completion coordination][__link26]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbTdTRM3Ter5obfHbFD5kvsw8bol88Neh2pwkblZk1sRA_Fp1hZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionWaiter
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext::readiness_waker
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionWaiter::waker
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionBudget
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ServiceStatus
 [__link15]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=WaitStatus::Armed
 [__link16]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Shutdown
 [__link17]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link18]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDriver::shutdown
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Shutdown
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link20]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Drain
 [__link21]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DrainStatus::Complete
 [__link22]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
 [__link23]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs
 [__link24]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link25]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link26]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/COMPLETION_COORDINATION.md
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ProviderContext
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionRequirements
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionDomain
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionService
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDriver
