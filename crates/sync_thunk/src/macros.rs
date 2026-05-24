// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runs a function body on a worker thread instead of the async executor.
///
/// Apply `#[thunk]` to any `async fn` whose body performs blocking work — file
/// I/O, CPU-heavy computation, FFI calls, or anything else that should not
/// block the async runtime. The function stays `async`, but its body executes
/// on a [`Thunker`](crate::Thunker) worker thread.
///
/// # Soundness model
///
/// `#[thunk]` mirrors `tokio::task::spawn_blocking`'s soundness model: every
/// value handed to the worker thread is **owned**, **`Send`**, and
/// **`'static`**. This guarantees that even if the wrapper future is leaked
/// (e.g. via `mem::forget`), the worker is still operating on memory it owns.
///
/// As a consequence, the macro **rejects** the following at compile time:
///
/// - Any receiver — `self`, `&self`, `&mut self`, or typed (`self: Arc<Self>`).
///   Borrowed receivers permit safe code to `mem::forget` the future and then
///   drop the receiver while the worker still holds a raw pointer to it —
///   straight use-after-free.
/// - Any reference parameter — `&T` or `&mut T` — for the same reason.
///
/// Pass everything by owned move instead. For methods, wrap the call site in
/// a thin convenience method that takes `Arc<Self>`:
///
/// ```ignore
/// impl Service {
///     pub async fn work(self: &Arc<Self>, input: Vec<u8>) -> u64 {
///         Self::work_thunked(Arc::clone(self), input).await
///     }
///
///     #[thunk(from = me.thunker)]
///     async fn work_thunked(me: Arc<Self>, input: Vec<u8>) -> u64 {
///         // blocking code using `me` and `input`
///         input.len() as u64
///     }
/// }
/// ```
///
/// # The `from` parameter
///
/// `from = <expr>` tells the macro where to find the [`Thunker`](crate::Thunker).
/// The expression is evaluated each call and must produce a value that
/// `.clone()` produces a `Thunker` (or any `Send + 'static` handle with a
/// `send(WorkItem)` method). `Thunker` is cheaply cloneable (it is an `Arc`
/// underneath), so this is essentially free.
///
/// Common shapes:
///
/// **Through an `Arc<Self>` parameter** — the canonical replacement for `&self`:
/// ```ignore
/// #[thunk(from = me.thunker)]
/// async fn work(me: Arc<Self>) -> u64 { /* blocking code */ }
/// ```
///
/// **Method call on an `Arc<Self>` parameter** — when the thunker is behind a getter:
/// ```ignore
/// #[thunk(from = me.thunker())]
/// async fn work(me: Arc<Self>) -> u64 { /* blocking code */ }
/// ```
///
/// **Function parameter** — for associated/free functions:
/// ```ignore
/// #[thunk(from = thunker)]
/// async fn create(thunker: Thunker, path: PathBuf) -> Result<Self> { /* ... */ }
/// ```
///
/// **Global static** — for applications that share a single pool:
/// ```ignore
/// #[thunk(from = THUNKER)]
/// async fn work(input: Vec<u8>) -> u64 { /* blocking code */ }
/// ```
///
/// # Cancellation
///
/// If the future is dropped (canceled) before the worker finishes, a drop
/// guard blocks the dropping thread until the worker completes. The blocking
/// operation always runs to completion — it cannot be interrupted mid-flight.
/// `mem::forget`-ing the future skips this guard; the parameters then live on
/// inside the leaked allocation and the worker completes against still-valid
/// memory (a sound leak, not UB).
///
/// # Panics
///
/// If the function body panics, the panic propagates to the `.await` site on
/// the calling task, just as it would for a synchronous call. The panic
/// payload is preserved verbatim — including downcastable types like
/// `String`, `&'static str`, or user-defined types — so existing
/// `catch_unwind`-based handling at the call site behaves identically to a
/// synchronous panic.
///
/// # Compile-fail guarantees (U1 regression guard)
///
/// These doctests verify that the macro rejects the shapes that would
/// reopen the `mem::forget`-on-future use-after-free hole. They must keep
/// failing to compile; if any of them ever passes, the macro has regressed.
///
/// `&self` receiver — rejected:
///
/// ```compile_fail
/// use std::sync::Arc;
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = self.thunker)]
///     async fn work(&self) -> u64 { 0 }
/// }
/// ```
///
/// `&mut self` receiver — rejected:
///
/// ```compile_fail
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = self.thunker)]
///     async fn work(&mut self) -> u64 { 0 }
/// }
/// ```
///
/// Owned `self` receiver — rejected:
///
/// ```compile_fail
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = self.thunker)]
///     async fn work(self) -> u64 { 0 }
/// }
/// ```
///
/// Typed `self: Arc<Self>` receiver — rejected (use `me: Arc<Self>` instead):
///
/// ```compile_fail
/// use std::sync::Arc;
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = self.thunker)]
///     async fn work(self: Arc<Self>) -> u64 { 0 }
/// }
/// ```
///
/// Shared reference parameter — rejected:
///
/// ```compile_fail
/// use std::sync::Arc;
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = me.thunker)]
///     async fn work(me: Arc<Self>, data: &Vec<u8>) -> usize { data.len() }
/// }
/// ```
///
/// Mutable reference parameter — rejected:
///
/// ```compile_fail
/// use std::sync::Arc;
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = me.thunker)]
///     async fn work(me: Arc<Self>, buf: &mut Vec<u8>) -> usize { buf.len() }
/// }
/// ```
///
/// Non-`'static` borrow smuggled via an owned parameter — rejected by the
/// `Send + 'static` assertion the macro emits:
///
/// ```compile_fail
/// use std::sync::Arc;
/// use sync_thunk::{Thunker, thunk};
///
/// struct Svc { thunker: Thunker }
/// impl Svc {
///     #[thunk(from = me.thunker)]
///     async fn work<'a>(me: Arc<Self>, data: &'a [u8]) -> usize { data.len() }
/// }
/// ```
pub use sync_thunk_macros::thunk;
