// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Understanding worker associations, cloning, and explicit relocation.
//!
//! Once a task starts, its future stays on the same worker thread. The owned
//! [`Builtins`](crate::task::Builtins) passed to the task contains capabilities
//! associated with that worker: its scheduler, clock, and telemetry sink.
//!
//! # A coordinate is not a thread handle
//!
//! [`core::Thread`](crate::core::Thread) records a runtime
//! [`Owner`](crate::core::Owner), an OS thread identifier, and a
//! [`NumaNode`](crate::core::NumaNode). It neither owns the thread nor keeps a
//! runtime running. Reading or cloning a coordinate does not change which OS
//! thread is executing your code.
//!
//! [`RuntimeOperations::pin_to`](crate::runtime::RuntimeOperations::pin_to)
//! changes the current OS thread's affinity using an explicit worker coordinate.
//! Operations can be created from `&Runtime` or `&Builtins`; they have no worker
//! association and are not `ThreadAware`. Pinning does not turn the calling thread
//! into an Arty worker or rebind a scheduler. Worker affinity information remains
//! usable after the runtime stops; retaining it
//! does not keep task execution or worker timer drivers running.
//!
//! # Cloning preserves the association
//!
//! A cloned scheduler targets the same worker as its source, even when used
//! elsewhere. This example sends a clone to a blocking thread, which submits
//! work back to the original asynchronous worker:
//!
//! ```
//! use arty::task::Builtins;
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), arty::task::JoinError> {
//!     let home = cx.thread().id();
//!     let cloned = cx.scheduler().clone();
//!     let executed_on = cx
//!         .scheduler()
//!         .spawn_blocking(move || cloned.spawn(async |child| child.thread().id()).wait())
//!         .await??;
//!     assert_eq!(executed_on, home);
//!     Ok(())
//! }
//! ```
//!
//! Keeping scheduler handles does not keep the runtime alive. Cloning
//! `Builtins` also preserves its association; moving the clone with ordinary
//! Rust moves does not notify it of a new destination.
//!
//! # Relocate an explicit payload
//!
//! [`TaskScheduler::spawn_anywhere`](crate::task::TaskScheduler::spawn_anywhere)
//! selects a worker round-robin and invokes
//! [`ThreadAware::relocate`](crate::core::ThreadAware::relocate) on the payload
//! there, before calling the factory. The factory is a function pointer, so its
//! input must be supplied explicitly rather than hidden in captures.
//!
//! Prefer the `Builtins` supplied to a new task's factory when you only need
//! that task's services. Relocation is useful when carrying an existing value
//! that must adapt to its destination.
//!
//! ```
//! use arty::task::{Builtins, JoinError};
//!
//! #[arty::main]
//! async fn main(cx: Builtins) -> Result<(), JoinError> {
//!     cx.scheduler()
//!         .spawn_anywhere(cx.clone(), |moved| async move {
//!             assert_eq!(moved.thread().id(), std::thread::current().id());
//!             assert!(moved.local_scheduler().is_some());
//!             let child = moved
//!                 .scheduler()
//!                 .spawn(async |child| child.thread().id())
//!                 .await?;
//!             assert_eq!(child, moved.thread().id());
//!             Ok::<(), JoinError>(())
//!         })
//!         .await??;
//!     Ok(())
//! }
//! ```
//!
//! The selected worker can be the source worker, especially in a one-worker
//! runtime. This example checks that capabilities match the selected worker,
//! not that a different worker was chosen.
//!
//! For runtime capabilities, relocation has these boundaries:
//!
//! | Destination | Effect |
//! | --- | --- |
//! | The existing associated worker | The association is unchanged |
//! | An initialized, registered worker of the same runtime | Capabilities rebind coherently to that worker |
//! | Another runtime, or a coordinate with no initialized worker services in the owner | The original association is retained |
//!
//! [`local_scheduler()`](crate::task::Builtins::local_scheduler) returns a
//! local scheduler only when called on the value's associated worker. Merely
//! carrying `Builtins` to another thread does not grant local scheduling access.
//!
//! # Relocation is not automatic task migration
//!
//! `spawn_anywhere` places a new task; it does not move an existing task.
//! Ordinary `spawn`, `run`, and `block_on` do not relocate captured values or
//! results. Receiving a `Builtins` value as a result does not rebind it to the
//! receiver.
//!
//! `Send` permits a value to cross threads. `ThreadAware` describes how a
//! transferable value adapts after an explicit move; it does not make a
//! non-`Send` value transferable. Implementations must preserve their real data
//! and remain correct even without a relocation notification. Read the
//! [`ThreadAware` contract](crate::core::ThreadAware) before implementing it.
