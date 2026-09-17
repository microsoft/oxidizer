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
//! scheduling and a separate [`CompletionWaiter`] that collects native activity and provides
//! the blocking wait. This crate supplies the shared contracts, not a production native backend,
//! a driver registry, or a thread-placement policy.
//!
//! # Negotiation and registration
//!
//! A requested [`IoContext`] type selects its [`DriverProvider`]. The runtime supplies a
//! [`ProviderContext`] advertising the native client capability types of a proposed completion
//! configuration. The provider chooses one strategy and declares its [`CompletionRequirements`].
//! Alternative strategies are selected explicitly, not combined into one set of required clients.
//!
//! Before creation, the runtime chooses the final owning threads and configures their waiters.
//! Each waiter and the client services backed by it share a [`CompletionDomain`] identity.
//! [`CompletionService`] tags prevent accidentally combining clients from different domains.
//! Native adapters remain responsible for associating those clients with the correct native
//! resources and for their registration/retirement rules.
//!
//! On each owning thread, the runtime assembles [`DriverContext`] with thread coordinates,
//! [`SystemTasks`], a source readiness waker, and typed client capabilities. It validates the
//! selected requirements, relocates the provider clone, and consumes it to create a
//! [`LocalDriver`]. Creation must return promptly and establish routing and notification before
//! publishing a usable context.
//!
//! Provider and driver creation return [`DriverError`]. The first context request succeeds only
//! after every active worker has initialized the driver. Failure requires explicit rollback or
//! retirement of partial registrations; it is not success for the surviving workers. Later
//! requests reuse the successfully registered provider/context family. Drivers with independent
//! versions coexist through distinct context type identities while using the same core contract.
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
//! service lookup or through a `Waker`.
//!
//! The [`DriverContext::readiness_waker`] identifies a driver service participant. The runtime
//! latches readiness for that participant before waking the collection domain. The separate
//! [`CompletionWaiter::waker`] interrupts the current or next blocking collection. Both handles
//! remain memory-safe after their original participant disappears; a late signal must not
//! target a replacement registration.
//!
//! The coordinator alternates bounded collection, task/control work, and driver service. A
//! [`CompletionBudget`] limits one participant during each turn. [`ServiceStatus`] reports remaining work
//! or a service deadline. A runnable result schedules another turn without requiring a new
//! notification. Every newly installed driver and newly initiated drain starts runnable,
//! before the runtime can park, even if no native notification has arrived.
//!
//! Before a positive wait, the runtime services due work, asks every participating driver or
//! drain to prepare notifications, and rechecks task, control, and source readiness. Only
//! [`WaitStatus::Armed`] permits that participant to sleep. The waiter must preserve an
//! interruption racing the final check and actual wait. Deadlines and remaining runnable work
//! also constrain whether and how long the runtime waits.
//!
//! Drivers and [`Shutdown`] handles stay on their owning thread, enforced by local ownership
//! wrappers even when the concrete implementation has thread-safe fields. Native
//! clients may be thread-local; consumer contexts remain mobile through [`IoContext`].
//! A runtime with no native drivers can use an ordinary latched parking waiter. Unsupported
//! native configurations fail explicitly or use another explicitly configured domain; core
//! does not silently create helper threads.
//!
//! # Cooperative shutdown and safe ownership
//!
//! [`LocalDriver::shutdown`] consumes the running driver and closes admission before returning
//! a [`Shutdown`] handle. Its [`Drain`] continues service and notification preparation under the
//! same budget protocol. Every turn receives a budget; shutdown is not a blocking call or a future.
//!
//! The runtime initiates all relevant shutdowns, keeps collecting native activity and executing
//! required system work, and services drains fairly. [`DrainStatus::Complete`] marks graceful
//! retirement; pending status retains its notification or deadline obligation. Failures remain
//! visible through [`DriverError`], and the runtime applies an overall shutdown deadline.
//!
//! Context clones remain valid but closed and do not themselves delay drain completion.
//! Admitted operations, callbacks, and native registrations independently retain their storage.
//! Neither cancellation nor an expired deadline permits invalidating memory still reachable
//! by native code. Dropping a driver or abandoning a drain is always memory-safe, even when
//! graceful cleanup cannot complete. Destruction does not wait for I/O or other participants;
//! any necessary independent cleanup retains its own resource ownership.
//!
//! # Example and design
//!
//! The [two-thread reference runtime](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs)
//! demonstrates coordinated in-memory completion sources, not production IOCP or `io_uring`
//! implementations. Registries, native routing, placement, and timeout policy belong to that
//! runtime/native layer rather than this crate.
//! Its control thread uses blocking result handles; it is not an application-future executor.
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)
//! - [Completion coordination](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/COMPLETION_COORDINATION.md)

mod completion_budget;
mod completion_domain;
mod completion_requirements;
mod completion_waiter;
mod driver;
mod driver_context;
mod driver_error;
mod io_context;
mod local_driver;
mod provider;
mod provider_context;
mod service_status;
mod shutdown;
mod system_tasks;

pub use completion_budget::CompletionBudget;
pub use completion_domain::{CompletionDomain, CompletionService};
pub use completion_requirements::CompletionRequirements;
pub use completion_waiter::CompletionWaiter;
pub use driver::Driver;
pub use driver_context::DriverContext;
pub use driver_error::DriverError;
pub use io_context::IoContext;
pub use local_driver::LocalDriver;
pub use provider::DriverProvider;
pub use provider_context::ProviderContext;
pub use service_status::{ServiceStatus, WaitStatus};
pub use shutdown::{Drain, DrainStatus, Shutdown};
pub use system_tasks::{SystemTask, SystemTasks};
