// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![deny(missing_docs)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty_io_core/favicon.ico")]

//! Stable contracts for integrating external I/O drivers with the Arty runtime.
//!
//! The runtime hosts drivers supplied by libraries and applications rather than depending on one
//! I/O implementation. This crate contains the small vocabulary both sides share:
//!
//! - [`Driver`] is the adapter between one worker and an I/O subsystem.
//! - [`IoContext`] is the consumer handle that selects its provider.
//! - [`ProviderContext`] supplies runtime facilities when that provider is created.
//! - [`DriverProvider`] creates and connects the per-worker adapters for a driver.
//! - [`DriverContext`] describes the worker and runtime facilities available during driver
//!   creation.
//! - [`ShutdownError`] reports unsuccessful graceful shutdown.
//! - [`SystemTasks`] lets a driver delegate blocking system work to the runtime.
//!
//! Registration and driver placement are runtime behavior, not part of this crate. Keeping those
//! policies outside the contract allows the runtime and drivers to evolve independently.
//!
//! # Runtime and driver lifecycle
//!
//! The contract separates one provider per registered driver type, one driver per runtime worker,
//! and contexts that consumers may move and retain independently:
//!
//! ```text
//! context request
//!       |
//!       v
//! IoContext::provider(ProviderContext)
//!       |
//!       | clone and relocate once per worker
//!       v
//! DriverProvider::create(DriverContext)
//!       |
//!       +-- Driver: owned and driven by that worker
//!       +-- Context: obtained from the driver, then cached for consumers
//! ```
//!
//! ## Registration and initialization
//!
//! A consumer asks the runtime for a concrete [`IoContext`] type. On the first request for that
//! type, the runtime creates a [`ProviderContext`], calls [`IoContext::provider`], and registers
//! the resulting [`DriverProvider`]. Registration, synchronization, rollback, and caching remain
//! runtime concerns.
//!
//! The runtime clones the provider for each active worker, relocates each clone to that worker,
//! and invokes [`DriverProvider::create`] on the worker thread. [`DriverContext`] identifies the
//! worker and supplies runtime facilities such as [`SystemTasks`]. The returned [`Driver`] stays
//! on that thread for its entire lifetime; it is deliberately not required to be [`Send`] or
//! [`Sync`]. Creation runs inline and must return promptly; waiting there for another worker to
//! make progress can deadlock registration.
//!
//! After creation, the runtime obtains the worker's context through [`Driver::context`] and may
//! cache both that context and the driver's [interruptor][Driver::interruptor]. The interruptor
//! honors interrupts raised by the driver's own thread and remains safe to invoke after the driver
//! is gone. The first context request completes only after every active worker has created its
//! driver instance. Later requests reuse the registration and return the context cached for the
//! calling worker. A runtime may retain the provider to initialize workers created later.
//!
//! Driver creation is infallible at the type level. If [`DriverProvider::create`] panics, the
//! runtime does not continue with a driver registered on only part of its worker set. A driver
//! with conditional platform or permission requirements therefore exposes its own capability
//! check for consumers to call before requesting its context.
//!
//! ## Driving I/O
//!
//! Consumers start operations through contexts. Contexts may be cloned, relocated between
//! workers, and retained after their original driver is gone. Relocation may improve locality, but
//! correctness must not depend on it.
//!
//! The runtime exclusively owns each driver and calls every [`Driver`] method only on its owning
//! thread. A zero wait to [`Driver::process_completions`] performs a non-blocking completion pass
//! without consuming a pending interrupt; a bounded or unbounded wait lets the same call provide
//! the worker's idle point. The driver's interruptor is latched, so it ends either the current
//! blocking wait or the next one without preventing pending completions from being processed.
//! Runtime policy decides which driver supplies a worker's waiting point and how additional
//! drivers are scheduled.
//!
//! Operations may be submitted from other threads while the driver waits. A driver therefore
//! separates its state into two parts:
//!
//! - State reached by contexts, interruptors, background threads, or operating-system callbacks
//!   is shared independently of the driver and uses appropriate reference counting and
//!   synchronization. Each in-flight operation owns every resource it uses through a reference
//!   count, pool lease, or equivalent handle; contexts themselves hold no per-operation state and
//!   therefore do not delay shutdown.
//! - Completion buffers, queue-reader state, batching state, and lifecycle state used only on the
//!   owning thread remain ordinary driver fields accessed through `&mut self`.
//!
//! In particular, [`Driver::process_completions`] must not hold anything across a blocking wait
//! that a submitter needs to make a completion possible, such as a lock, queue slot, or pool
//! capacity.
//!
//! ## Shutdown
//!
//! Shutdown is cooperative, but it is not a memory-safety protocol:
//!
//! 1. The runtime removes the driver from its normal completion loop and calls
//!    [`Driver::shutdown`], transferring ownership of the driver.
//! 2. `shutdown` closes admission, blocks while active operations and operating-system callbacks
//!    drain, and performs graceful cleanup. It owns the liveness policy for that wait and returns
//!    an error rather than blocking indefinitely. Contexts remain valid but reject new operations.
//! 3. [`SystemTasks`] remains available until `shutdown` returns.
//! 4. `shutdown` returns [`ShutdownError`] when graceful cleanup cannot be completed. The runtime
//!    records or reports the error and continues shutting down its remaining drivers.
//!
//! A driver must nevertheless be safe to drop at any point, including during unwinding or after a
//! shutdown error. Dropping closes admission if necessary. Storage that an operating system can
//! reach only by raw pointer must have an independent owner that is retained rather than
//! invalidated on a premature drop. A successful shutdown determines whether cleanup was
//! graceful, never whether destruction is sound.
//!
//! # Example
//!
//! The [fixed two-thread runtime example](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/examples/two_thread_runtime/main.rs)
//! starts both worker threads before `get_context::<SampleContext>()` uses the context type to
//! inject its associated driver.
//!
//! # Project documents
//!
//! - [Requirements](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/REQUIREMENTS.md)
//! - [Design](https://github.com/microsoft/oxidizer/blob/main/crates/arty_io_core/docs/DESIGN.md)

mod driver;
mod driver_context;
mod io_context;
mod provider;
mod provider_context;
mod shutdown_error;
mod system_tasks;

pub use driver::Driver;
pub use driver_context::DriverContext;
pub use io_context::IoContext;
pub use provider::DriverProvider;
pub use provider_context::ProviderContext;
pub use shutdown_error::ShutdownError;
pub use system_tasks::{SystemTask, SystemTasks};
