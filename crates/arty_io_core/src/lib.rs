// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![forbid(unsafe_code)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Contracts for coordinating independent I/O drivers with the Arty runtime.
//!
//! Drivers retain their operations, buffers, and completion decoding. The runtime owns their
//! scheduling and a separate [`CompletionWaiter`] that collects native activity and provides the
//! blocking wait. This crate supplies the shared contracts, not a production native backend, a
//! driver registry, or a thread-placement policy.
//!
//! # Registration and owner-thread creation
//!
//! A requested [`IoContext`] type selects its [`DriverProvider`]. [`IoContext::provider`]
//! establishes shared provider state only; it receives no native capabilities and makes no
//! preliminary compatibility declaration.
//!
//! Before creation, the runtime chooses the final owning threads and their native arrangement.
//! On each owning thread it assembles a [`DriverContext`] with thread coordinates,
//! [`SystemTasks`], and a source readiness waker, and asks the collector on that worker to attach its
//! own typed clients through [`CompletionWaiter::attach_clients`]. It then relocates the provider
//! clone and consumes it through [`DriverProvider::create`], which returns a consumer context
//! for that worker together with its concrete driver in an inline [`LocalDriver`] owner.
//! The provider must pair each context with the instance it actually belongs to.
//!
//! The provider selects its native strategy and takes the required clients through
//! [`DriverContext::take_completion_service`] before native side effects. Clients may be local
//! and move-only. When no supported strategy is available, creation reports an unsupported
//! configuration. Strategy-specific shared native initialization may be
//! deferred into provider state, but creation must return promptly without awaiting async work
//! or a cross-worker initialization handshake. It establishes routing and notification before
//! publishing a usable context.
//! The runtime supplies coherent worker configurations; the core performs no automatic
//! intersection discovery across differently configured workers.
//!
//! A client type is matched by exact type identity. Sharing this crate is not sufficient for
//! native interoperability: a driver and a native adapter agree on the actual client type and
//! interface. Independently compiled client-interface versions may need an explicit bridge;
//! matching field layouts do not make different types interchangeable.
//!
//! Provider and driver creation return [`DriverError`]. The first context request succeeds only
//! after every active worker has initialized the driver. Failure requires explicit rollback or
//! retirement of partial registrations; it is not success for the surviving workers. Later
//! requests reuse the successfully registered provider and context family. Drivers with
//! independent versions coexist through distinct context type identities while using the same
//! core contract.
//!
//! # One wait, multiple service participants
//!
//! ```text
//! native activity -> CompletionWaiter -> records or readiness -> driver service
//!                           ^
//!                           |
//!             runtime task/control/source interruption
//! ```
//!
//! A native adapter may route already-collected records into private driver mailboxes or report
//! that a driver must drain its own queue. Records never travel through the construction-time
//! client lookup or through a `Waker`. Collection is fair across native sources: a continuously
//! actionable source is eventually delivered or signaled despite a hot peer.
//!
//! The [`DriverContext::readiness_waker`] identifies a driver service participant. The runtime
//! latches readiness for that participant before waking the collector. The separate
//! [`CompletionWaiter::waker`] interrupts the current or next blocking collection. Both handles
//! remain memory-safe after their original participant disappears; a late signal must not target
//! a replacement registration.
//!
//! The coordinator alternates bounded collection, task and control work, and driver service. A
//! [`CompletionBudget`] limits one participant during each turn. It is cooperative residual
//! accounting shared with the participant, not automatic enforcement. [`ServiceStatus`] reports
//! remaining work or a service deadline. [`ServiceStatus::Runnable`] schedules another turn
//! without requiring a new notification. Every newly installed driver and newly initiated drain
//! starts runnable, before the runtime can park, even if no native notification has arrived.
//!
//! Before a positive wait, the runtime services due work, asks every participating driver or
//! drain to prepare notifications, and rechecks task, control, and source readiness. Only
//! [`WaitStatus::Armed`] permits that participant to sleep. The collector must preserve an
//! interruption racing the final check and actual wait. Deadlines and remaining runnable work
//! also constrain whether and how long the runtime waits.
//!
//! [`LocalDriver`] and [`LocalDrain`] hold concrete state inline and dispatch statically. They
//! stay on their owning thread even when the implementation has thread-safe fields. A runtime
//! may choose private erasure for heterogeneous storage; the public contracts require no driver
//! boxing. Thread confinement is not pinning: callback-visible storage must remain independently
//! stable, pinned, or otherwise safely owned. Native clients may be
//! thread-local; consumer contexts remain mobile through [`IoContext`]. A runtime with no native
//! drivers can use an ordinary latched parking collector. Unsupported native configurations fail
//! explicitly or use another explicitly configured arrangement; core does not silently create
//! helper threads.
//!
//! # Cooperative shutdown and safe ownership
//!
//! [`LocalDriver::shutdown`] consumes the running owner and invokes [`Driver::shutdown`], which
//! closes admission and returns its concrete [`Drain`]. A [`LocalDrain`] keeps that value local.
//! The drain continues service and notification preparation under the same budget
//! protocol. Every turn receives a budget; shutdown is not a blocking call or a future.
//!
//! The runtime initiates all relevant shutdowns, keeps collecting native activity and executing
//! required system work, and services drains fairly. [`DrainStatus::Complete`] marks graceful
//! retirement; pending status retains its notification or deadline obligation. The runtime owns
//! terminal removal: after completion or an error from either drain method it drops the drain and
//! calls neither method again. Failures remain visible through [`DriverError`], and the runtime
//! applies an overall shutdown deadline.
//!
//! Context clones remain valid but closed and do not themselves delay drain completion. Admitted
//! operations, callbacks, and native registrations independently retain their storage. Neither
//! cancellation nor an expired deadline permits invalidating memory still reachable by native
//! code. Dropping a driver or abandoning a drain is always memory-safe, even when graceful
//! cleanup cannot complete. Destruction does not wait for I/O or other participants; any
//! necessary independent cleanup retains its own resource ownership.
//!
//! [`SystemTasks::spawn`] reports admission, not completion. Success means execution ownership
//! was accepted; rejection returns [`DriverError`] and promises no task execution or callback.
//! Retained owners and pending cleanup keep execution authority on the existing facility even
//! when the controller reaches its shutdown deadline. Inert handles alone do not extend it.
//! Offload tasks still use private closure boxing and dispatch through [`SystemTask::run`];
//! [`SystemTasks`] uses an `Arc`-backed callback. Wakers, clients, and driver-owned resources may
//! also allocate. Only the inline ownership wrappers themselves introduce no allocation or
//! dynamic dispatch.
//!
//! # Example and design
//!
//! The [two-thread reference runtime](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs)
//! demonstrates coordinated in-memory completion sources, not production IOCP or `io_uring`
//! implementations. Registries, native routing, placement, and timeout policy belong to that
//! runtime and native layer rather than this crate.
//! Its control thread uses blocking result handles; it is not an application-future executor.
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)
//! - [Completion coordination](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/COMPLETION_COORDINATION.md)

mod completion_budget;
mod completion_waiter;
mod drain;
mod driver;
mod driver_context;
mod driver_error;
mod io_context;
mod local_owner;
mod provider;
mod service_status;
mod system_tasks;

pub use completion_budget::CompletionBudget;
pub use completion_waiter::CompletionWaiter;
pub use drain::{Drain, DrainStatus};
pub use driver::Driver;
pub use driver_context::DriverContext;
pub use driver_error::DriverError;
pub use io_context::IoContext;
pub use local_owner::{LocalDrain, LocalDriver};
pub use provider::DriverProvider;
pub use service_status::{ServiceStatus, WaitStatus};
pub use system_tasks::{SystemTask, SystemTasks};
