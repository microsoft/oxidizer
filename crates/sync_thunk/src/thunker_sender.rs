// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::WorkItem;
use crate::thunker::{ThunkerInner, send_impl};

/// A cheap, dispatch-only handle to a [`Thunker`](crate::Thunker)'s worker
/// pool.
///
/// `ThunkerSender` shares the same underlying worker pool as the parent
/// [`Thunker`] but does **not** participate in the handle-count bookkeeping
/// that governs pool shutdown. Cloning a `ThunkerSender` therefore costs only
/// a single `Arc` reference-count bump, not two separate atomic increments —
/// keeping the `#[thunk]` macro's dispatch path lean.
///
/// As long as at least one [`Thunker`] handle exists, dispatching via a
/// `ThunkerSender` is guaranteed to enqueue work. If every [`Thunker`] has
/// been dropped after items have already been enqueued, the worker pool
/// drains the channel before exiting — every queued item runs to
/// completion, including the `mark_worker_done` notification each
/// `#[thunk]` shim issues on its caller's `StackState`. New dispatches
/// submitted *after* the last `Thunker` is dropped may, however, block
/// indefinitely on a saturated queue (no worker remains to drain it).
/// Callers should ensure a `Thunker` outlives any active senders, just as
/// the macro does implicitly by holding `&self` to the providing
/// expression.
///
/// `ThunkerSender` is `Send + Sync`.
///
/// [`Thunker`]: crate::Thunker
#[derive(Clone)]
pub struct ThunkerSender {
    pub(crate) inner: Arc<ThunkerInner>,
}

impl ThunkerSender {
    /// Sends a work item to be executed on a worker thread.
    ///
    /// Automatically scales up the underlying thread pool if the queue is
    /// backing up and the current thread count is below the configured
    /// maximum.
    ///
    /// # Panics
    ///
    /// Panics if the channel is closed. This is unreachable in practice
    /// because the receiver is co-owned with the sender via the shared
    /// `Arc`.
    #[doc(hidden)]
    #[inline]
    pub fn send(&self, item: WorkItem) {
        send_impl(&self.inner, item);
    }
}

impl core::fmt::Debug for ThunkerSender {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThunkerSender").finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::{Thunker, WorkItem};

    fn set_flag(ptr: *mut ()) {
        // SAFETY: ptr is a valid Arc<AtomicBool> created via Arc::into_raw.
        let arc = unsafe { Arc::from_raw(ptr.cast::<AtomicBool>()) };
        arc.store(true, Ordering::SeqCst);
    }

    #[test]
    fn debug_impl() {
        let t = Thunker::new();
        let s = t.sender();
        let debug = format!("{s:?}");
        assert!(debug.contains("ThunkerSender"));
    }

    #[test]
    fn clone_shares_pool() {
        let t = Thunker::new();
        let s1 = t.sender();
        let s2 = s1.clone();
        // Use the clone so it isn't optimized away.

        let executed = Arc::new(AtomicBool::new(false));
        let executed2 = Arc::clone(&executed);
        let flag_ptr = Arc::into_raw(executed2).cast_mut().cast::<()>();
        // SAFETY: `flag_ptr` is an `Arc<AtomicBool>` raw pointer.
        let item = unsafe { WorkItem::new(flag_ptr, set_flag) };
        s2.send(item);

        // Original `s1` is still usable too.
        drop(s1);

        for _ in 0..100 {
            if executed.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(executed.load(Ordering::SeqCst));
    }
}
