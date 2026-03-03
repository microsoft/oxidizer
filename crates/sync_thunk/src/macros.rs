// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runs a function body on a worker thread instead of the async executor.
///
/// Apply `#[thunk]` to any `async fn` whose body performs blocking work — file I/O,
/// CPU-heavy computation, FFI calls, or anything else that should not block the
/// async runtime. The method signature stays `async`, but the body executes on a
/// [`Thunker`](crate::Thunker) worker thread.
///
/// # The `from` parameter
///
/// The `from` parameter tells the macro where to find the [`Thunker`](crate::Thunker).
/// There are four supported patterns:
///
/// **Struct field** — the most common pattern:
/// ```ignore
/// #[thunk(from = self.thunker)]
/// async fn work(&self) -> u64 { /* blocking code */ }
/// ```
///
/// **Method call** — when the thunker is behind a getter:
/// ```ignore
/// #[thunk(from = self.thunker())]
/// async fn work(&self) -> u64 { /* blocking code */ }
/// ```
///
/// **Function parameter** — for associated functions with no `self`:
/// ```ignore
/// #[thunk(from = thunker)]
/// async fn create(thunker: &Thunker, path: &Path) -> Result<Self> { /* ... */ }
/// ```
///
/// **Global static** — for applications that share a single pool:
/// ```ignore
/// #[thunk(from = THUNKER)]
/// async fn work(&self) -> u64 { /* blocking code */ }
/// ```
///
/// # Parameters and return values
///
/// All parameter types and return types work naturally:
///
/// - **`&self` / `&mut self`** — fully supported.
/// - **References (`&T`, `&mut T`)** — borrowed data is safe because the future's
///   drop guard blocks until the worker completes.
/// - **Owned values** — moved to the worker thread and available in the body.
/// - **Any return type** — the result is written back to the caller's stack and
///   returned from the `.await`.
///
/// # Cancellation
///
/// If the future is dropped (canceled) before the worker finishes, a drop
/// guard blocks the dropping thread until the worker completes. The blocking
/// operation always runs to completion — it cannot be interrupted mid-flight.
/// This is the same guarantee that makes borrowed references safe across threads.
///
/// # Panics
///
/// If the function body panics, the panic propagates to the `.await` site on the
/// calling task, just as it would for a synchronous call.
///
/// # Example
///
/// ```ignore
/// use sync_thunk::{Thunker, thunk};
///
/// struct FileService {
///     thunker: Thunker,
/// }
///
/// impl FileService {
///     #[thunk(from = self.thunker)]
///     async fn read_file(&self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
///         std::fs::read(path)
///     }
///
///     #[thunk(from = self.thunker)]
///     async fn write_file(&mut self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
///         std::fs::write(path, data)
///     }
/// }
/// ```
pub use sync_thunk_macros::thunk;
