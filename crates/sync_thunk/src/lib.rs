// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Efficiently handle blocking calls in async code.
//!
//! Mark any async method with [`#[thunk]`](thunk) and its body will execute on a
//! dedicated worker thread, freeing the async executor to do other work.
//!
//! ```ignore
//! use sync_thunk::{Thunker, thunk};
//!
//! struct MyService {
//!     thunker: Thunker,
//! }
//!
//! impl MyService {
//!     #[thunk(from = self.thunker)]
//!     async fn blocking_work(&self) -> String {
//!         // This body runs on a worker thread, not the async executor.
//!         std::fs::read_to_string("/etc/hostname").unwrap()
//!     }
//! }
//! ```
//!
//! # Why
//!
//! Async runtimes assume tasks yield quickly. Blocking operations — filesystem I/O,
//! DNS lookups, CPU-heavy computation — stall the executor and hurt throughput.
//! The traditional fix is `spawn_blocking`, but that allocates a closure, boxes the
//! return value, and may spawn an unbounded number of OS threads.
//!
//! `sync_thunk` solves this differently:
//!
//! - **Zero-allocation dispatch.** Arguments are packed into a stack-allocated struct
//!   and sent through a pre-allocated bounded channel. No `Box`, no `Arc`, no closure.
//!
//! - **Zero-copy design.** Arguments is moved to the worker thread without requiring any copying or funny ownership gymnastics.
//!
//! - **Auto-scaling thread pool.** The [`Thunker`] starts with a single worker thread
//!   and automatically scales up when the queue backs up — up to a configurable
//!   maximum. Idle workers exit after a configurable cool-down interval, but at least
//!   one worker is always kept alive.
//!
//! # Getting Started
//!
//! **1. Create a [`Thunker`]:**
//!
//! ```
//! use sync_thunk::Thunker;
//!
//! let thunker = Thunker::builder()
//!     .max_thread_count(4) // at most 4 workers
//!     .cool_down_interval(std::time::Duration::from_secs(10))
//!     .build();
//! ```
//!
//! **2. Annotate methods with [`#[thunk]`](thunk):**
//!
//! The `from` parameter tells the macro where to find the [`Thunker`]. It can be a
//! struct field, a method call, a function parameter, or a static — anything that returns a `&Thunker`.
//!
//! ```ignore
//! #[thunk(from = self.thunker)]
//! async fn do_io(&self) -> std::io::Result<Vec<u8>> {
//!     std::fs::read("/some/file")
//! }
//! ```
//!
//! **3. Call it like any other async method:**
//!
//! ```ignore
//! let data = service.do_io().await?;
//! ```
//!
//! # Where the Thunker Comes From
//!
//! The `from` parameter is flexible. Here are the four common patterns:
//!
//! ## From a struct field
//!
//! The most common pattern — the struct owns the thunker:
//!
//! ```ignore
//! struct MyService { thunker: Thunker }
//!
//! impl MyService {
//!     #[thunk(from = self.thunker)]
//!     async fn work(&self) -> u64 { /* ... */ }
//! }
//! ```
//!
//! ## From a method call
//!
//! Useful when the thunker is behind a getter or shared via an accessor:
//!
//! ```ignore
//! impl MyService {
//!     fn thunker(&self) -> &Thunker { &self.inner_thunker }
//!
//!     #[thunk(from = self.thunker())]
//!     async fn work(&self) -> u64 { /* ... */ }
//! }
//! ```
//!
//! ## From a function parameter
//!
//! Useful for associated functions with no `self` receiver:
//!
//! ```ignore
//! impl MyService {
//!     #[thunk(from = thunker)]
//!     async fn create(thunker: &Thunker, path: &Path) -> std::io::Result<Self> {
//!         let data = std::fs::read(path)?;
//!         /* ... */
//!     }
//! }
//! ```
//!
//! ## From a global static
//!
//! For applications that share a single pool without threading it through structs:
//!
//! ```ignore
//! static THUNKER: LazyLock<Thunker> = LazyLock::new(|| Thunker::builder().build());
//!
//! impl MyService {
//!     #[thunk(from = THUNKER)]
//!     async fn work(&self) -> u64 { /* ... */ }
//! }
//! ```

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk/favicon.ico")]

extern crate self as sync_thunk;

mod macros;
mod stack_state;
mod thunk_future;
mod thunker;
mod thunker_builder;
mod work_item;

pub use macros::thunk;
#[doc(hidden)]
pub use stack_state::StackState;
#[doc(hidden)]
pub use thunk_future::ThunkFuture;
pub use thunker::Thunker;
pub use thunker_builder::ThunkerBuilder;
#[doc(hidden)]
pub use work_item::WorkItem;
