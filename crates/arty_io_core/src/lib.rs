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
//! - [`DriverContext`] associates a requested context type with its provider.
//! - [`DriverProvider`] creates and connects the per-worker adapters for a driver.
//! - [`DriverInit`] describes the worker and runtime facilities available during creation.
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
//! DriverContext::provider()
//!       |
//!       | clone and relocate once per worker
//!       v
//! DriverProvider::create(DriverInit)
//!       |
//!       +-- Driver: owned and driven by that worker
//!       +-- Context: obtained from the driver, then cached for consumers
//! ```
//!
//! ## Registration and initialization
//!
//! A consumer asks the runtime for a concrete [`DriverContext`] type. On the first request for
//! that type, the runtime calls [`DriverContext::provider`] and registers the resulting
//! [`DriverProvider`]. Registration, synchronization, rollback, and caching remain runtime
//! concerns.
//!
//! The runtime clones the provider for each active worker, relocates each clone to that worker,
//! and invokes [`DriverProvider::create`] on the worker thread. [`DriverInit`] identifies the
//! worker and supplies runtime facilities such as [`SystemTasks`]. The returned [`Driver`] stays
//! on that thread for its entire lifetime; it is deliberately not required to be [`Send`] or
//! [`Sync`]. Creation runs inline and must return promptly; waiting there for another worker to
//! make progress can deadlock registration.
//!
//! After creation, the runtime obtains the worker's context through [`Driver::context`] and
//! may cache both that context and the driver's [waker][Driver::waker]. The waker honors wakes
//! raised by the driver's own thread and remains safe to invoke after the driver is gone. The
//! first context request completes only after every active worker has created its driver instance.
//! Later requests reuse the registration and return the context cached for the calling worker. A
//! runtime may retain the provider to initialize workers created later.
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
//! thread. A zero wait to [`Driver::process_completions`] performs a non-blocking completion pass;
//! a bounded or unbounded wait lets the same call provide the worker's idle point. The driver's
//! waker is latched, so it ends either the current wait or the next one. Runtime policy decides
//! which driver supplies a worker's waiting point and how additional drivers are scheduled.
//!
//! Operations may be submitted from other threads while the driver waits. A driver therefore
//! separates its state into two parts:
//!
//! - State reached by contexts, wakers, background threads, or operating-system callbacks is
//!   shared independently of the driver and uses appropriate reference counting and
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
//! 1. The runtime calls [`Driver::begin_shutdown`] on every driver, closing admission before it
//!    polls any one driver for drain progress. The method is idempotent.
//! 2. Existing operations continue making progress through [`Driver::process_completions`].
//!    Contexts remain valid but reject new operations.
//! 3. The runtime calls [`Driver::poll_shutdown`] for each driver. A pending driver registers the
//!    supplied task waker, and the runtime continues processing completions with bounded waits.
//!    Calling `poll_shutdown` before `begin_shutdown` starts shutdown as part of the poll.
//! 4. [`Poll::Ready`][std::task::Poll::Ready] reports that active operations and
//!    operating-system callbacks have drained. Context handles do not themselves delay this
//!    transition, and later polls remain ready.
//! 5. [`SystemTasks`] remains available through the graceful drain. The runtime bounds the total
//!    drain duration and reports or terminates on a liveness failure. A driver's premature-drop
//!    soundness must not depend on system work submitted after that deadline.
//!
//! A driver must nevertheless be safe to drop at any point, including during unwinding or after a
//! shutdown timeout. Dropping closes admission if necessary. Storage that an operating system can
//! reach only by raw pointer must have an independent owner that is retained rather than
//! invalidated on a premature drop. Shutdown completion determines whether cleanup was graceful,
//! never whether destruction is sound.
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

mod context;
mod driver;
mod init;
mod provider;
mod system_tasks;

pub use context::DriverContext;
pub use driver::Driver;
pub use init::DriverInit;
pub use provider::DriverProvider;
pub use system_tasks::{SystemTask, SystemTasks};
