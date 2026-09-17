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
scheduling and a separate [`CompletionWaiter`][__link0] that collects native activity and provides the
blocking wait. This crate supplies the shared contracts, not a production native backend, a
driver registry, or a thread-placement policy.

## Registration and owner-thread creation

A requested [`IoContext`][__link1] type selects its [`DriverProvider`][__link2]. [`IoContext::provider`][__link3]
establishes shared provider state only; it receives no native capabilities and makes no
preliminary compatibility declaration.

Before creation, the runtime chooses the final owning threads and their native arrangement.
On each owning thread it assembles a [`DriverContext`][__link4] with thread coordinates,
[`SystemTasks`][__link5], and a source readiness waker, and asks the collector on that worker to attach its
own typed clients through [`CompletionWaiter::attach_clients`][__link6]. It then relocates the provider
clone and consumes it through [`DriverProvider::create`][__link7], which returns a consumer context
for that worker together with its concrete driver in an allocation-free [`LocalDriver`][__link8].
The provider must pair each context with the instance it actually belongs to.

The provider selects its native strategy from the clients the context actually supplies,
before performing native side effects, and reports an unsupported configuration when no
supported strategy is available. Strategy-specific shared native initialization may be
deferred into provider state, but creation must return promptly without awaiting async work
or a cross-worker initialization handshake. It establishes routing and notification before
publishing a usable context.
The runtime supplies coherent worker configurations; the core performs no automatic
intersection discovery across differently configured workers.

A client type is matched by exact type identity. Sharing this crate is not sufficient for
native interoperability: a driver and a native adapter agree on the actual client type and
interface. Independently compiled client-interface versions may need an explicit bridge;
matching field layouts do not make different types interchangeable.

Provider and driver creation return [`DriverError`][__link9]. The first context request succeeds only
after every active worker has initialized the driver. Failure requires explicit rollback or
retirement of partial registrations; it is not success for the surviving workers. Later
requests reuse the successfully registered provider and context family. Drivers with
independent versions coexist through distinct context type identities while using the same
core contract.

## One wait, multiple service participants

```text
native activity -> CompletionWaiter -> records or readiness -> driver service
                          ^
                          |
            runtime task/control/source interruption
```

A native adapter may route already-collected records into private driver mailboxes or report
that a driver must drain its own queue. Records never travel through the construction-time
client lookup or through a `Waker`. Collection is fair across native sources: a continuously
actionable source is eventually delivered or signaled despite a hot peer.

The [`DriverContext::readiness_waker`][__link10] identifies a driver service participant. The runtime
latches readiness for that participant before waking the collector. The separate
[`CompletionWaiter::waker`][__link11] interrupts the current or next blocking collection. Both handles
remain memory-safe after their original participant disappears; a late signal must not target
a replacement registration.

The coordinator alternates bounded collection, task and control work, and driver service. A
[`CompletionBudget`][__link12] limits one participant during each turn. It is cooperative residual
accounting shared with the participant, not automatic enforcement. [`ServiceStatus`][__link13] reports
remaining work or a service deadline. [`ServiceStatus::Runnable`][__link14] schedules another turn
without requiring a new notification. Every newly installed driver and newly initiated drain
starts runnable, before the runtime can park, even if no native notification has arrived.

Before a positive wait, the runtime services due work, asks every participating driver or
drain to prepare notifications, and rechecks task, control, and source readiness. Only
[`WaitStatus::Armed`][__link15] permits that participant to sleep. The collector must preserve an
interruption racing the final check and actual wait. Deadlines and remaining runnable work
also constrain whether and how long the runtime waits.

[`LocalDriver`][__link16] and [`LocalDrain`][__link17] hold concrete state inline and dispatch statically. They
stay on their owning thread even when the implementation has thread-safe fields. A runtime
may choose private erasure for heterogeneous storage; the public contracts require no driver
boxing. Thread confinement is not pinning: callback-visible storage must remain independently
stable, pinned, or otherwise safely owned. Native clients may be
thread-local; consumer contexts remain mobile through [`IoContext`][__link18]. A runtime with no native
drivers can use an ordinary latched parking collector. Unsupported native configurations fail
explicitly or use another explicitly configured arrangement; core does not silently create
helper threads.

## Cooperative shutdown and safe ownership

[`LocalDriver::shutdown`][__link19] consumes the running owner and invokes [`Driver::shutdown`][__link20], which
closes admission and returns its concrete [`Drain`][__link21]. A [`LocalDrain`][__link22] keeps that value local.
The drain continues service and notification preparation under the same budget
protocol. Every turn receives a budget; shutdown is not a blocking call or a future.

The runtime initiates all relevant shutdowns, keeps collecting native activity and executing
required system work, and services drains fairly. [`DrainStatus::Complete`][__link23] marks graceful
retirement; pending status retains its notification or deadline obligation. The runtime owns
terminal removal: after completion or an error from either drain method it drops the drain and
calls neither method again. Failures remain visible through [`DriverError`][__link24], and the runtime
applies an overall shutdown deadline.

Context clones remain valid but closed and do not themselves delay drain completion. Admitted
operations, callbacks, and native registrations independently retain their storage. Neither
cancellation nor an expired deadline permits invalidating memory still reachable by native
code. Dropping a driver or abandoning a drain is always memory-safe, even when graceful
cleanup cannot complete. Destruction does not wait for I/O or other participants; any
necessary independent cleanup retains its own resource ownership.

[`SystemTasks::spawn`][__link25] reports admission, not completion. Success means execution ownership
was accepted; rejection returns [`DriverError`][__link26] and promises no task execution or callback.
Retained owners and pending cleanup keep execution authority on the existing facility even
when the controller reaches its shutdown deadline. Inert handles alone do not extend it.
Offload tasks still use private closure boxing and dispatch through [`SystemTask::run`][__link27];
[`SystemTasks`][__link28] uses an `Arc`-backed callback. Wakers, clients, and driver-owned resources may
also allocate. Only the inline ownership wrappers themselves introduce no allocation or
dynamic dispatch.

## Example and design

The [two-thread reference runtime][__link29]
demonstrates coordinated in-memory completion sources, not production IOCP or `io_uring`
implementations. Registries, native routing, placement, and timeout policy belong to that
runtime and native layer rather than this crate.
Its control thread uses blocking result handles; it is not an application-future executor.

* [Requirements][__link30]
* [Design][__link31]
* [Completion coordination][__link32]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbca6IE69-d0QbBBER8Sngb7kbS7EEJqfJgI8b_fJzepPHDqZhZIGCbGFydHlfaW9fY29yZWUwLjIuMA
 [__link0]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionWaiter
 [__link1]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link10]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext::readiness_waker
 [__link11]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionWaiter::waker
 [__link12]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionBudget
 [__link13]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ServiceStatus
 [__link14]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=ServiceStatus::Runnable
 [__link15]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=WaitStatus::Armed
 [__link16]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDriver
 [__link17]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDrain
 [__link18]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext
 [__link19]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDriver::shutdown
 [__link2]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider
 [__link20]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Driver::shutdown
 [__link21]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=Drain
 [__link22]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDrain
 [__link23]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DrainStatus::Complete
 [__link24]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
 [__link25]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks::spawn
 [__link26]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
 [__link27]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTask::run
 [__link28]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link29]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs
 [__link3]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=IoContext::provider
 [__link30]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md
 [__link31]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md
 [__link32]: https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/COMPLETION_COORDINATION.md
 [__link4]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverContext
 [__link5]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=SystemTasks
 [__link6]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=CompletionWaiter::attach_clients
 [__link7]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverProvider::create
 [__link8]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=LocalDriver
 [__link9]: https://docs.rs/arty_io_core/0.2.0/arty_io_core/?search=DriverError
