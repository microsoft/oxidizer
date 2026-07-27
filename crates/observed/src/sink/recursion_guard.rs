// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local reentrancy guard that stops a sink from dispatching into
//! itself while an emission is already in progress on the current thread.

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
/// guard is already held, signalling a reentrant sink invocation that the
/// caller must skip to avoid unbounded recursion.
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
