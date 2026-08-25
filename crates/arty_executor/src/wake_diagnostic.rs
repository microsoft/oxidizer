// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::Backtrace;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, atomic};
use std::task::{RawWaker, RawWakerVTable, Waker};

use foldhash::{HashMap, HashMapExt};

use crate::ERR_POISONED_LOCK;

static NEXT_KEY: AtomicU64 = AtomicU64::new(0);

/// A wrapping waker that captures a backtrace when an instance is cloned, storing the backtrace in
/// a shared registry where it can be inspected for "what wakers are still alive" diagnostic
/// purposes.
///
/// Note that we only store the CLONES because the original waker is created and owned by the
/// executor, so is not useful for diagnostic purposes - we omit it because it would be confusing.
#[derive(Debug)]
pub(crate) struct DiagnosticWaker {
    inner: Waker,

    /// All the backtraces of all the cloned wakers in this family.
    registry: Arc<DiagnosticWakerRegistry>,

    /// The backtrace key of the current instance. Used to remove the backtrace from the
    /// collection on drop. `None` for the initial waker that is not recorded in the registry.
    key: Option<u64>,
}

impl DiagnosticWaker {
    /// Creates the initial instance of a waker, owned by the executor.
    ///
    /// It is valid to call this multiple times for the same family - the point is merely that as
    /// this instance is owned by the executor, there is no value in including its backtrace in the
    /// diagnostic data set as it is not possible to leak this instance, only to reference it.
    pub(crate) fn with_inner_and_registry(inner: Waker, registry: Arc<DiagnosticWakerRegistry>) -> Waker {
        let instance = Self {
            inner,
            registry,
            key: None,
        };

        let ptr = Box::into_raw(Box::new(instance));

        // SAFETY: We are required to properly implement the waker contract in the vtable functions.
        // We do - everything is thread safe and proper.
        unsafe { Waker::from_raw(RawWaker::new(ptr.cast(), &WAKER_VTABLE)) }
    }

    fn clone(&self) -> Self {
        // We clone the inner waker and create a new DiagnosticWaker with the same family backtraces.
        let inner_clone = self.inner.clone();

        let registry = Arc::clone(&self.registry);

        // Clones can be leaked, so we register the backtrace in the registry.
        let key = NEXT_KEY.fetch_add(1, atomic::Ordering::Relaxed);
        registry
            .backtraces
            .lock()
            .expect(ERR_POISONED_LOCK)
            .insert(key, Backtrace::capture());

        Self {
            inner: inner_clone,
            registry,
            key: Some(key),
        }
    }
}

impl Drop for DiagnosticWaker {
    fn drop(&mut self) {
        let Some(key) = self.key else {
            return;
        };

        let mut family_backtraces = self.registry.backtraces.lock().expect(ERR_POISONED_LOCK);
        assert!(family_backtraces.remove(&key).is_some());
    }
}

static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone_waker, waker_wake, waker_wake_by_ref, waker_drop_waker);

fn waker_clone_waker(ptr: *const ()) -> RawWaker {
    let waker = unwrap_diagnostic_waker(ptr);

    let clone = Box::into_raw(Box::new(waker.clone()));

    RawWaker::new(clone.cast(), &WAKER_VTABLE)
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake(ptr: *const ()) {
    let waker = unwrap_diagnostic_waker(ptr);

    waker.inner.wake_by_ref();

    // This consumes the waker!
    // SAFETY: We only pass `Box<DiagnosticWaker>::into_raw()` into the Waker mechanisms, so it
    // must be legal to bring it back as a box of `DiagnosticWaker`.
    drop(unsafe { Box::from_raw(ptr.cast_mut().cast::<DiagnosticWaker>()) });
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake_by_ref(ptr: *const ()) {
    let waker = unwrap_diagnostic_waker(ptr);

    waker.inner.wake_by_ref();
}

#[cfg_attr(test, mutants::skip)] // It is not practical to test that memory is deallocated. If something leaks, Miri will complain.
fn waker_drop_waker(ptr: *const ()) {
    // SAFETY: We only pass `Box<DiagnosticWaker>::into_raw()` into the Waker mechanisms, so it
    // must be legal to bring it back as a box of `DiagnosticWaker`.
    drop(unsafe { Box::from_raw(ptr.cast_mut().cast::<DiagnosticWaker>()) });
}

/// Resurrects the `DiagnosticWaker` reference that hides behind the waker's state pointer.
///
/// We return it with `'static` because there is no Rust lifetime that corresponds to
/// the waker reference's real lifetime. Just do not use it after the waker vtable methods.
#[cfg_attr(coverage_nightly, coverage(off))] // A null pointer would violate this module's RawWaker invariant.
fn unwrap_diagnostic_waker(ptr: *const ()) -> &'static DiagnosticWaker {
    // SAFETY: We only pass `Box<DiagnosticWaker>::into_raw()` into the Waker mechanisms, so it
    // must be legal to bring it back as a `DiagnosticWaker`.
    let waker = unsafe { ptr.cast::<DiagnosticWaker>().as_ref() };

    let Some(waker) = waker else {
        unreachable!("waker has a null pointer for its inner state - impossible")
    };

    waker
}

/// A registry where active diagnostic wakers from the same family register their backtraces.
#[derive(Debug)]
pub(crate) struct DiagnosticWakerRegistry {
    backtraces: Mutex<HashMap<u64, Backtrace>>,
}

impl DiagnosticWakerRegistry {
    pub(crate) fn new() -> Self {
        Self {
            backtraces: Mutex::new(HashMap::new()),
        }
    }

    /// Uses a closure to inspect every backtrace saved in the registry.
    ///
    /// Each backtrace is either a place where a waker was cloned.
    pub(crate) fn inspect_backtraces(&self, mut f: impl FnMut(&Backtrace)) {
        let backtraces = self.backtraces.lock().expect(ERR_POISONED_LOCK);

        for bt in backtraces.values() {
            f(bt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::redundant_clone, reason = "intentional - testing cloning logic")]
    fn smoke_test() {
        let registry = Arc::new(DiagnosticWakerRegistry::new());

        let inner = Waker::noop().clone();

        let waker = DiagnosticWaker::with_inner_and_registry(inner, Arc::clone(&registry));

        // The original is not added, only clones.
        assert!(registry.backtraces.lock().unwrap().is_empty());

        let waker_clone = waker.clone();

        assert_eq!(registry.backtraces.lock().unwrap().len(), 1);

        // Consumes the waker.
        waker_clone.wake();

        assert_eq!(registry.backtraces.lock().unwrap().len(), 0);

        // A clone of a clone also registers, even without the original.
        let waker_clone = waker.clone();
        drop(waker);
        let _waker_clone_clone = waker_clone.clone();

        assert_eq!(registry.backtraces.lock().unwrap().len(), 2);

        let mut inspect_count: usize = 0;

        registry.inspect_backtraces(|_| {
            inspect_count += 1;
        });

        assert_eq!(inspect_count, 2);
    }
}
