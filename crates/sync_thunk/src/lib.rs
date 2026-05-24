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
//! use std::sync::Arc;
//! use sync_thunk::{Thunker, thunk};
//!
//! struct MyService {
//!     thunker: Thunker,
//! }
//!
//! impl MyService {
//!     #[thunk(from = me.thunker)]
//!     async fn blocking_work(me: Arc<Self>) -> String {
//!         // This body runs on a worker thread, not the async executor.
//!         std::fs::read_to_string("/etc/hostname").unwrap()
//!     }
//! }
//! ```
//!
//! ## Why
//!
//! Async runtimes assume tasks yield quickly. Blocking operations — filesystem I/O,
//! DNS lookups, CPU-heavy computation — stall the executor and hurt throughput.
//! The traditional fix is `spawn_blocking`, but that allocates a closure, boxes the
//! return value, and may spawn an unbounded number of OS threads.
//!
//! `sync_thunk` solves this differently:
//!
//! - **Zero-allocation dispatch.** Arguments are packed into a stack-allocated struct
//!   and sent through a pre-allocated bounded channel. No `Box`, no per-call `Arc`,
//!   no closure on the heap.
//!
//! - **Auto-scaling thread pool.** The [`Thunker`] starts with a single worker thread
//!   and automatically scales up when the queue backs up — up to a configurable
//!   maximum. Idle workers exit after a configurable cool-down interval, but at least
//!   one worker is always kept alive.
//!
//! `#[thunk]` deliberately mirrors `tokio::task::spawn_blocking`'s soundness model:
//! every value handed to the worker must be **owned** and **`Send + 'static`**.
//! Methods are therefore expressed as associated functions taking
//! `me: Arc<Self>` (or another owned `'static` value) rather than `&self`.
//! See the comparison section below for details.
//!
//! ## Getting Started
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
//! The `from` parameter tells the macro where to find the [`Thunker`]. It can be
//! a struct field reached through an owned `Arc<Self>` parameter, a method call,
//! a function parameter, or a static.
//!
//! ```ignore
//! use std::path::PathBuf;
//! use std::sync::Arc;
//!
//! #[thunk(from = me.thunker)]
//! async fn do_io(me: Arc<MyService>, path: PathBuf) -> std::io::Result<Vec<u8>> {
//!     std::fs::read(path)
//! }
//! ```
//!
//! **3. Call it like any other async method:**
//!
//! ```ignore
//! let data = MyService::do_io(Arc::clone(&service), path).await?;
//! ```
//!
//! ## Where the Thunker Comes From
//!
//! The `from` parameter is flexible. The four common patterns:
//!
//! ### Through an `Arc<Self>` parameter (struct field)
//!
//! The canonical replacement for `&self`:
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! struct MyService { thunker: Thunker }
//!
//! impl MyService {
//!     #[thunk(from = me.thunker)]
//!     async fn work(me: Arc<Self>) -> u64 { /* ... */ }
//! }
//! ```
//!
//! ### From a method call on an `Arc<Self>` parameter
//!
//! Useful when the thunker is behind a getter or shared via an accessor:
//!
//! ```ignore
//! impl MyService {
//!     fn thunker(&self) -> &Thunker { &self.inner_thunker }
//!
//!     #[thunk(from = me.thunker())]
//!     async fn work(me: Arc<Self>) -> u64 { /* ... */ }
//! }
//! ```
//!
//! ### From a function parameter
//!
//! Useful for associated/free functions with no `Self`:
//!
//! ```ignore
//! impl MyService {
//!     #[thunk(from = thunker)]
//!     async fn create(thunker: Thunker, path: PathBuf) -> std::io::Result<Self> {
//!         let data = std::fs::read(path)?;
//!         /* ... */
//!     }
//! }
//! ```
//!
//! ### From a global static
//!
//! For applications that share a single pool without threading it through structs:
//!
//! ```ignore
//! static THUNKER: LazyLock<Thunker> = LazyLock::new(|| Thunker::builder().build());
//!
//! impl MyService {
//!     #[thunk(from = THUNKER)]
//!     async fn work(input: Vec<u8>) -> u64 { /* ... */ }
//! }
//! ```
//!
//! ## Comparison with `tokio::task::spawn_blocking`
//!
//! `sync_thunk` solves the same problem as [`tokio::task::spawn_blocking`] and
//! adopts an **identical soundness contract**: every value that crosses to the
//! worker thread must be owned and satisfy `Send + 'static`. Borrowed parameters
//! and receivers are rejected at compile time because `mem::forget`-ing the
//! wrapper future under safe code would release the borrow while the worker is
//! still using it (use-after-free). Any program expressible with `#[thunk]` is
//! also expressible with `spawn_blocking` and vice versa — the two are
//! **semantically equivalent**.
//!
//! The differences are operational and ergonomic, not expressive.
//!
//! ### Side-by-side
//!
//! With `tokio::spawn_blocking`, you wrap `Self` in an `Arc` and clone it into a
//! `move` closure:
//!
//! ```ignore
//! impl MyService {
//!     async fn read_file(self: &Arc<Self>, path: PathBuf) -> std::io::Result<Vec<u8>> {
//!         let me = Arc::clone(self);
//!         tokio::task::spawn_blocking(move || me.read_file_sync(path))
//!             .await
//!             .expect("worker panicked")
//!     }
//!
//!     fn read_file_sync(&self, path: PathBuf) -> std::io::Result<Vec<u8>> {
//!         std::fs::read(path)
//!     }
//! }
//! ```
//!
//! With `#[thunk]`, the dispatch shim is generated for you and the body stays
//! `async`:
//!
//! ```ignore
//! impl MyService {
//!     #[thunk(from = me.thunker)]
//!     async fn read_file(me: Arc<Self>, path: PathBuf) -> std::io::Result<Vec<u8>> {
//!         std::fs::read(path)
//!     }
//! }
//! ```
//!
//! ### What `#[thunk]` gives you over `spawn_blocking`
//!
//! - **Zero per-call allocations.** `spawn_blocking` heap-allocates the boxed
//!   closure and a oneshot channel on every call. `#[thunk]` stores the work item
//!   inline in the wrapper future on the caller's stack and signals completion
//!   through an atomic flag + waker. For hot paths (thousands of calls per
//!   second) this removes a measurable amount of allocator pressure.
//!
//! - **Per-pool sizing and isolation.** Each [`Thunker`] owns its own worker
//!   pool with its own thread-count cap and cool-down interval. Different
//!   subsystems can have different pools, so a misbehaving component cannot
//!   starve every other blocking caller in the process. `spawn_blocking` uses a
//!   single global pool shared by everything on the runtime.
//!
//! - **Bounded backpressure.** A `Thunker`'s queue is bounded; when full,
//!   callers wait for capacity instead of unboundedly enqueueing work.
//!   `spawn_blocking` queues are effectively unbounded.
//!
//! - **Single source of truth for the signature.** The async signature *is* the
//!   sync signature; the macro generates the dispatch shim, so there is no
//!   parallel sync wrapper to keep in sync.
//!
//! ### What `#[thunk]` does **not** give you
//!
//! - **No scoped borrows.** Just like `spawn_blocking`, you cannot pass a
//!   reference to stack-local non-`'static` data. The `mem::forget`-on-future
//!   leak hazard is fundamental to async Rust and forces the `'static` bound on
//!   any cross-thread API that doesn't use a `std::thread::scope`-style scoped
//!   pattern.
//!
//! - **No receivers.** `self`, `&self`, `&mut self`, and typed receivers
//!   (`self: Arc<Self>`) are all rejected. Wrap your call site in a thin
//!   convenience method that takes `Arc<Self>` and forwards to the thunked
//!   associated function.
//!
//! - **No reference parameters.** Pass owned values (`T`, `Arc<T>`, `Box<T>`,
//!   `Vec<T>`, …) instead.
//!
//! - **No different soundness story.** If your data shape doesn't fit
//!   `spawn_blocking`, it won't fit `#[thunk]` either.
//!
//! ### When to pick which
//!
//! - Reach for `spawn_blocking` for **one-off** blocking calls in code that
//!   doesn't already own a `Thunker` and isn't called frequently enough for
//!   allocation overhead or pool isolation to matter.
//!
//! - Reach for `#[thunk]` when you have a **component** with multiple blocking
//!   entry points, want the calls to share a sized/isolated pool, want zero
//!   per-call allocations on hot paths, or want the cleaner method-level syntax.
//!
//! [`tokio::task::spawn_blocking`]: https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk/favicon.ico")]

extern crate self as sync_thunk;

mod internal;
mod macros;
mod stack_state;
mod thunk_future;
mod thunker;
mod thunker_builder;
mod thunker_sender;
mod work_item;

pub use macros::thunk;
#[doc(hidden)]
pub use stack_state::StackState;
#[doc(hidden)]
pub use thunk_future::ThunkFuture;
pub use thunker::Thunker;
pub use thunker_builder::ThunkerBuilder;
pub use thunker_sender::ThunkerSender;
#[doc(hidden)]
pub use work_item::WorkItem;

/// Hidden helpers used by the `#[thunk]` macro. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    /// Compile-time assertion that `T: Send + 'static`.
    ///
    /// The `'static` bound is required for the same reason
    /// [`tokio::task::spawn_blocking`] requires it: the wrapper future may be
    /// `mem::forget`-ed by safe code, releasing any non-`'static` borrows it
    /// held while the worker thread is still executing. Without `'static`,
    /// safe code could observe a use-after-free.
    ///
    /// [`tokio::task::spawn_blocking`]: https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html
    pub const fn assert_send_static<T: ?Sized + Send + 'static>() {}

    /// Helper used by the `#[thunk]` macro to obtain a cheap dispatch handle
    /// from the `Thunker` referenced by the `from = ...` expression.
    ///
    /// Taking `&crate::Thunker` (rather than going through `Clone::clone`
    /// directly on the macro's expression) lets deref coercion paper over the
    /// difference between `&Thunker`, `&&Thunker` (e.g. when
    /// `from = self.get_thunker()` returns `&Thunker`), and `&Arc<Thunker>`.
    /// It also guarantees the returned value is a real
    /// [`crate::ThunkerSender`], never an accidental reference from a blanket
    /// `Clone` impl on references.
    ///
    /// Returns a [`crate::ThunkerSender`] (not a `Thunker`) so that the
    /// per-dispatch clone bumps only the shared `Arc` strong count instead of
    /// also touching the `handle_count` atomic that governs pool shutdown.
    #[must_use]
    #[inline]
    pub fn clone_thunker(thunker: &crate::Thunker) -> crate::ThunkerSender {
        thunker.sender()
    }
}
