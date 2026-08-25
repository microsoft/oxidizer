// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local reentrancy guard that stops telemetry emitted *while
//! processors are running* from re-entering the pipeline on the current
//! thread.
//!
//! The guard covers processor dispatch only. Building an event value is
//! ordinary user code - a field initializer may call a helper that emits
//! telemetry of its own - so the guard is taken after the event has been
//! constructed and only for the dispatch itself.
//!
//! # Scope: thread-wide, not per-sink
//!
//! The guard is a single un-keyed thread-local flag shared by **every**
//! [`Sink`](crate::Sink) on the thread, not one slot per sink identity. While
//! an event is being dispatched to processors, *any* nested `emit!` on that
//! thread is skipped - including one targeting a completely unrelated sink.
//!
//! This is deliberate: nested telemetry is not a supported scenario. A
//! processor that emits while handling an event (e.g. reporting its own
//! failure to a separate diagnostics sink) would otherwise risk unbounded
//! recursion, and a per-sink guard would only push that risk one hop away
//! (sink A's processor emits to sink B, whose processor emits back to A).
//!
//! The consequence is that such nested events are dropped silently - there is
//! no warning, log, or error return, because reporting the drop would itself
//! require an emission. Processors must therefore not rely on emitting
//! telemetry from inside `process()`.

/// RAII guard that releases the current thread's reentrancy slot on drop.
struct ReentrancyGuard;
impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        AVAILABLE.set(true);
    }
}

thread_local! {
    static AVAILABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Attempts to acquire the current thread's reentrancy guard.
///
/// Returns `Some(guard)` when no emission is in progress on this thread; the
/// slot is released when the returned guard is dropped. Returns `None` when a
/// guard is already held, signaling a reentrant sink invocation that the
/// caller must skip to avoid unbounded recursion.
///
/// The slot is shared across all sinks on the thread, so a `None` here means
/// *some* emission is in progress - not necessarily one on the same sink. See
/// the [module docs](self) for why the guard is thread-wide.
pub(super) fn try_acquire_reentrancy_guard() -> Option<impl Drop> {
    AVAILABLE.get().then(|| {
        AVAILABLE.set(false);
        ReentrancyGuard
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_allows_single_acquisition() {
        assert!(try_acquire_reentrancy_guard().is_some());
    }

    #[test]
    fn guard_blocks_reentrancy() {
        let _guard = try_acquire_reentrancy_guard().expect("should acquire guard");
        assert!(try_acquire_reentrancy_guard().is_none(), "should block reentrancy");
    }

    #[test]
    fn guard_allows_after_drop() {
        {
            let _guard = try_acquire_reentrancy_guard().expect("should acquire guard");
        }
        assert!(try_acquire_reentrancy_guard().is_some(), "should allow after drop");
    }
}
