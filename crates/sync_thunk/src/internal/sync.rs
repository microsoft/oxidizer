// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sync-primitive shim that switches between the real `std` types and the
//! `loom` instrumented equivalents under `cfg(loom)`.
//!
//! Only `stack_state.rs` routes through this module today. The `Thunker`
//! pipeline transitively uses `crossbeam-channel`, which is not loom-instrumented,
//! so the rest of the crate cannot be model-checked end-to-end. Tests under
//! `cfg(loom)` should therefore stick to `StackState` and the publish /
//! wake / drop race that lives inside it.

// Mutex / PoisonError are no longer used by stack_state (it switched to
// AtomicWaker), but keep AtomicBool / AtomicUsize / Ordering shimmed for
// the rest of the crate and for any future state primitives.
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Loom provides its own `UnsafeCell` wrapper that instruments raw access for
// race detection. Outside `cfg(loom)` we just re-export the std type so the
// rest of the crate can write a single `.with_mut(|p| …)` accessor (matching
// the loom API) via a tiny extension trait below.
#[cfg(loom)]
pub(crate) use loom::cell::UnsafeCell;
#[cfg(not(loom))]
pub(crate) use std::cell::UnsafeCell;

/// Tiny adapter that gives std's `UnsafeCell` the same `with_mut` API as
/// loom's, so callers can share code between configurations.
#[cfg(not(loom))]
pub(crate) trait UnsafeCellExt<T> {
    fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R;
}

#[cfg(not(loom))]
impl<T> UnsafeCellExt<T> for UnsafeCell<T> {
    #[inline]
    fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        f(self.get())
    }
}

/// Yield to the scheduler in a way that progresses the loom model.
///
/// Outside `cfg(loom)` this is a real CPU `spin_loop` hint. Under loom we
/// must call `loom::thread::yield_now()` instead — a tight spin would never
/// advance loom's deterministic scheduler and the test would hang.
#[cfg(not(loom))]
#[inline]
pub(crate) fn spin_loop_hint() {
    std::hint::spin_loop();
}

#[cfg(loom)]
#[inline]
pub(crate) fn spin_loop_hint() {
    loom::thread::yield_now();
}

/// Long-form yield used after a short spin run.
#[cfg(not(loom))]
#[inline]
pub(crate) fn yield_now() {
    std::thread::yield_now();
}

#[cfg(loom)]
#[inline]
pub(crate) fn yield_now() {
    loom::thread::yield_now();
}
