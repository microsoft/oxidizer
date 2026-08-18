// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::marker::{PhantomData, PhantomPinned};
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{self, AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::task::{RawWaker, RawWakerVTable, Waker};

use pin_project::{pin_project, pinned_drop};

use crate::TaskRef;

/// A wake signal intended to be allocated inline as part of the task to be woken up.
///
/// The wake signal supplies the waker that can be used to set the signal and enables the owner
/// to inspect the awakened state. It also integrates with the executor's shared state to signal
/// the awakened state directly to the executor, eliminating the requirement to access the
/// task to detect that it has awakened.
///
/// # Ownership
///
/// The type uses interior mutability because its state is accessed concurrently from multiple
/// threads - by the executor (and/or task) that owns it on one hand, and by any number of
/// awaited async futures on any number of threads on the other hand (via wakers).
///
/// Creating `&mut` exclusive references to a `WakeSignal` may cause a violation of Miri stacked
/// borrowing rules at minimum. The entire API surface of this type is designed to be used via
/// shared references.
///
/// # Thread safety
///
/// The type itself is single-threaded, although the `std::task::Waker` instances obtained
/// from it are thread-safe as required by the waker API contract.
#[derive(Debug)]
#[pin_project(PinnedDrop)]
pub(crate) struct WakeSignal {
    /// The task that we are waking up.
    ///
    /// We will insert this into the list of awakened tasks for fast path wake-up signaling.
    task_ref: TaskRef,

    /// The queue of tasks that have been awakened. If we can lock the mutex without blocking
    /// and if there is room in the queue, we add our task on wake. Otherwise, we only update
    /// the signal itself and set the "ask every task to find the awakened ones" flag.
    awakened_queue: Arc<Mutex<VecDeque<TaskRef>>>,

    /// If we cannot add the task to `awakened_queue`, we set this flag to inform the task engine
    /// that it needs to read each task's wake signal to identify what has woken up.
    probe_embedded_wake_signals: Arc<AtomicBool>,

    /// Counts each waker we have created (both the initial one and any clones). The instance cannot
    /// be dropped until the clones are all gone because each clone holds a reference to the signal.
    waker_count: AtomicUsize,

    /// Whether the waker has been awakened in signal-probing mode. If the wake signal can be sent
    /// via the `awakened_queue`, this is not set. It is a fallback for an unusable queue.
    awakened: AtomicBool,

    /// After the wake signal enters the signaled state, we also signal the parent waker. This
    /// serves to give an opportunity to also wake up the owner of the executor that needs to
    /// handle the wake-up of the task.
    parent_waker: Waker,

    /// The type is single threaded... as far as other modules are concerned.
    ///
    /// We are secretly thread-safe internally but that is just a side-effect of
    /// the same type also masquerading as zero or more thread-safe `Waker` instances.
    ///
    /// When we act under the personality of a `Waker` within this module, Rust does not
    /// see the cross-thread access so will not complain about this marker.
    _single_threaded: PhantomData<*const ()>,

    /// This type cannot be unpinned once it has been pinned (latest when calling `waker()`).
    _requires_pin: PhantomPinned,
}

impl WakeSignal {
    pub(crate) fn new(
        awakened_queue: Arc<Mutex<VecDeque<TaskRef>>>,
        probe_embedded_wake_signals: Arc<AtomicBool>,
        parent_waker: Waker,
        task_ref: TaskRef,
    ) -> Self {
        Self {
            task_ref,
            awakened_queue,
            probe_embedded_wake_signals,
            waker_count: AtomicUsize::new(0),
            awakened: AtomicBool::new(false),
            parent_waker,
            _single_threaded: PhantomData,
            _requires_pin: PhantomPinned,
        }
    }

    /// Creates a fake wake signal that is not connected to an executor. This can be useful
    /// for testing, where you need a wake signal but do not care about what it actually does.
    #[cfg(test)]
    pub(crate) fn fake() -> Self {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        // This is fine because `WakeSignal` only uses the `TaskRef`, not the task behind it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        Self::new(
            Arc::new(Mutex::new(VecDeque::with_capacity(0))),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            fake_task_ref,
        )
    }

    /// Returns whether the signal has received a wake-up notification.
    ///
    /// If it has, resets the signal to a not-awakened state.
    pub(crate) fn consume_awakened(&self) -> bool {
        // Most of the time, the flag will be false so we at first probe it with Relaxed ordering.
        // If it is false, we can return early. If it is true, we need to ensure that we see all
        // memory operations that happened before the flag was set (i.e. the state changes that
        // led to the task being awakened). An Acquire fence sequenced after the relaxed RMW that
        // observes the Release store establishes the required synchronization.
        if self.awakened.swap(false, atomic::Ordering::Relaxed) {
            // This does nothing on x86 but on weaker memory models, the visibility of
            // writes to arbitrary locations may be delayed without this fence.
            atomic::fence(atomic::Ordering::Acquire);
            true
        } else {
            false
        }
    }

    /// Returns whether the signal is inert, meaning that no wakers are currently active and it is
    /// safe to drop the signal.
    #[cfg_attr(test, mutants::skip)] // Mutation causes infinite loops as executor will never shut down.
    pub(crate) fn is_inert(&self) -> bool {
        // We use Acquire ordering to ensure we see all writes to the waker before we declare it
        // inert. Generally, we expect wakers to already be inert by the time their inertness is
        // queried, because this query will happen when a task has completed and its future has
        // been dropped, which should (unless there is a resource leak) drop any wakers held by
        // pending awaits triggered deeper in the future.
        self.waker_count.load(atomic::Ordering::Acquire) == 0
    }

    /// Returns a reference to the waker associated with this signal.
    ///
    /// # Safety
    ///
    /// After calling this, the owner of the wake signal must query `is_inert()` for permission
    /// to drop the object. Until `is_inert()` signals `false`, the wake signal must not be dropped.
    pub(crate) unsafe fn waker(self: Pin<&Self>) -> Waker {
        // Reference count increment is independent of state transitions, so Relaxed is enough.
        self.waker_count.fetch_add(1, atomic::Ordering::Relaxed);

        let signal_ptr: *const Self = ptr::from_ref(self.get_ref());

        // SAFETY: We are required to correctly implement the waker API contract, which we do.
        // This includes being thread-safe, etc. The `WakeSignal` is thread-safe and all the
        // methods use interior mutability, so we are only using shared references, thereby
        // ensuring we do not violate Rust aliasing rules (as long as the owner of the `WakeSignal`
        // does not create any mutable references - though given that all methods are `&self` that
        // might still be relatively harmless and anyway likely detected by Miri as a bug.
        unsafe { Waker::from_raw(RawWaker::new(signal_ptr.cast(), &WAKER_VTABLE)) }
    }

    fn wake(&self) {
        if let Ok(mut awakened_set) = self.awakened_queue.try_lock() {
            // We only add if we can do so without increasing capacity, because increasing capacity
            // from an arbitrary thread may require reallocation, which we do not want to do on a
            // different thread than the one that owns the set.
            if awakened_set.len() < awakened_set.capacity() {
                // If we experienced spurious awakenings, we might push the same task multiple
                // times. That is fine - it is up to the receiver of the notifications to deal
                // with spurious notifications (which may arrive anyway through other means).
                awakened_set.push_back(self.task_ref);
                drop(awakened_set);
                self.parent_waker.wake_by_ref();
                return;
            }
        }

        // We release the awakened flag here, which means when someone acquires it
        // they will see all the memory operations that happened up to this point.
        self.awakened.store(true, atomic::Ordering::Release);

        // We failed to add the task to the awakened set, so the owner must walk the long road.
        // We use Release ordering, as we are releasing the synchronization block for `awakened`.
        self.probe_embedded_wake_signals.store(true, atomic::Ordering::Release);

        self.parent_waker.wake_by_ref();
    }
}

#[pinned_drop]
impl PinnedDrop for WakeSignal {
    #[cfg_attr(test, mutants::skip)] // Only used for assertions, effect-free.
    fn drop(self: Pin<&mut Self>) {
        // This is too common to do a release-mode assert.
        debug_assert!(self.is_inert());
    }
}

static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone_waker, waker_wake, waker_wake_by_ref, waker_drop_waker);

fn waker_clone_waker(ptr: *const ()) -> RawWaker {
    let signal = resurrect_signal_ref(ptr);

    // Cloning just increments the ref count, that's all. There is no "object" for the waker.
    // Reference count increment is independent of state transitions, so Relaxed is enough.
    signal.waker_count.fetch_add(1, atomic::Ordering::Relaxed);

    RawWaker::new(ptr, &WAKER_VTABLE)
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake(ptr: *const ()) {
    let signal = resurrect_signal_ref(ptr);

    // This consumes the waker!
    signal.wake();

    // We use Release ordering as we are releasing the synchronization block of the wake signal.
    signal.waker_count.fetch_sub(1, atomic::Ordering::Release);
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake_by_ref(ptr: *const ()) {
    let signal = resurrect_signal_ref(ptr);

    signal.wake();
}

#[cfg_attr(test, mutants::skip)] // It's well tested, but causes ocasional test timeouts
fn waker_drop_waker(ptr: *const ()) {
    let signal = resurrect_signal_ref(ptr);

    // We use Release ordering as we are releasing the synchronization block of the wake signal.
    signal.waker_count.fetch_sub(1, atomic::Ordering::Release);
}

/// Resurrects the `WakeSignal` reference that hides behind the waker's state pointer.
///
/// We return it with `'static` because there is no Rust lifetime that corresponds to
/// the waker reference's real lifetime. Just do not use it after the waker vtable methods.
#[cfg_attr(coverage_nightly, coverage(off))] // A null pointer would violate this module's RawWaker invariant.
fn resurrect_signal_ref(ptr: *const ()) -> &'static WakeSignal {
    // SAFETY: We only ever pass `&WakeSignal` into the Waker mechanisms, so it must be valid to
    // bring it back as a `&WakeSignal`. For lifetime logic, see function API comments.
    // The WakeSignal is marked for API contract purposes as single-threaded but is actually
    // thread-safe, so we can do this on any thread.
    let wake_signal = unsafe { ptr.cast::<WakeSignal>().as_ref() };

    let Some(wake_signal) = wake_signal else {
        unreachable!("waker has a null pointer for its inner state - impossible")
    };

    wake_signal
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use super::*;
    use crate::testing::TestWaker;

    #[test]
    fn never_used() {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        // This is fine because `WakeSignal` only uses the `TaskRef`, not the task behind it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(10)));
        let probe_embedded_wake_signals = Arc::new(AtomicBool::new(false));

        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::clone(&probe_embedded_wake_signals),
            Waker::noop().clone(),
            fake_task_ref
        ));

        assert!(signal.is_inert());
    }

    #[test]
    fn awaken_via_embedded_signal() {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        // This is fine because `WakeSignal` only uses the `TaskRef`, not the task behind it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(10)));
        let probe_embedded_wake_signals = Arc::new(AtomicBool::new(false));
        let parent_waker = Arc::new(TestWaker::new());

        // We hold the lock - the signal cannot use the set.
        let _awakened_set_lock_guard = awakened_queue.lock().unwrap();

        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::clone(&probe_embedded_wake_signals),
            Arc::clone(&parent_waker).into(),
            fake_task_ref
        ));

        // WakeSignal is only meant to be consumed via shared references.
        let signal = signal.as_ref();

        // SAFETY: Must not be dropped until `is_inert()` returns true.
        // We expect that the test leaves us in this state and rely on assertions to verify it.
        let waker = unsafe { signal.waker() };

        assert!(!signal.consume_awakened());
        assert!(!parent_waker.awakened.load(atomic::Ordering::Relaxed));

        waker.wake_by_ref();
        assert!(probe_embedded_wake_signals.load(atomic::Ordering::Relaxed));
        assert!(signal.consume_awakened());
        assert!(parent_waker.awakened.load(atomic::Ordering::Relaxed));

        // Verify that it is now consumed.
        assert!(!signal.consume_awakened());
    }

    #[test]
    fn awaken_via_awakened_set() {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        // This is fine because `WakeSignal` only uses the `TaskRef`, not the task behind it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(10)));
        let probe_embedded_wake_signals = Arc::new(AtomicBool::new(false));
        let parent_waker = Arc::new(TestWaker::new());

        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::clone(&probe_embedded_wake_signals),
            Arc::clone(&parent_waker).into(),
            fake_task_ref
        ));

        // WakeSignal is only meant to be consumed via shared references.
        let signal = signal.as_ref();

        // SAFETY: Must not be dropped until `is_inert()` returns true.
        // We expect that the test leaves us in this state and rely on assertions to verify it.
        let waker = unsafe { signal.waker() };

        assert!(!signal.consume_awakened());
        assert!(!parent_waker.awakened.load(atomic::Ordering::Relaxed));

        waker.wake_by_ref();
        // It should not have set the embedded signal here because we use the awakened set.
        assert!(!probe_embedded_wake_signals.load(atomic::Ordering::Relaxed));
        assert!(!signal.consume_awakened());
        assert!(!awakened_queue.lock().unwrap().is_empty());

        // But it should always signal the parent waker.
        assert!(parent_waker.awakened.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn awaken_via_full_awakened_set() {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        // This is fine because `WakeSignal` only uses the `TaskRef`, not the task behind it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        // Capacity is 0 so the queue is not allowed to allocate (== is never used).
        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(0)));
        let probe_embedded_wake_signals = Arc::new(AtomicBool::new(false));
        let parent_waker = Arc::new(TestWaker::new());

        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::clone(&probe_embedded_wake_signals),
            Arc::clone(&parent_waker).into(),
            fake_task_ref
        ));

        // WakeSignal is only meant to be consumed via shared references.
        let signal = signal.as_ref();

        // SAFETY: Must not be dropped until `is_inert()` returns true.
        // We expect that the test leaves us in this state and rely on assertions to verify it.
        let waker = unsafe { signal.waker() };

        assert!(!signal.consume_awakened());
        assert!(!parent_waker.awakened.load(atomic::Ordering::Relaxed));

        waker.wake_by_ref();
        // Even though it could lock the set, it could not use it because it was at capacity.
        assert!(probe_embedded_wake_signals.load(atomic::Ordering::Relaxed));
        assert!(signal.consume_awakened());
        assert!(parent_waker.awakened.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn is_inert_when_expected() {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        // This is fine because `WakeSignal` only uses the `TaskRef`, not the task behind it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(10)));
        let probe_embedded_wake_signals = Arc::new(AtomicBool::new(false));

        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::clone(&probe_embedded_wake_signals),
            Waker::noop().clone(),
            fake_task_ref
        ));

        // WakeSignal is only meant to be consumed via shared references.
        let signal = signal.as_ref();

        // SAFETY: Must not be dropped until `is_inert()` returns true.
        // We expect that the test leaves us in this state and rely on assertions to verify it.
        let waker = unsafe { signal.waker() };

        // A waker now exists, so it cannot be inert.
        assert!(!signal.is_inert());

        let waker_clone = waker.clone();

        // A second one, just for extra measure.
        assert!(!signal.is_inert());
        assert_eq!(signal.waker_count.load(atomic::Ordering::Relaxed), 2);

        // Drop the wakers and we are inert again.
        drop(waker);
        drop(waker_clone);

        assert_eq!(signal.waker_count.load(atomic::Ordering::Relaxed), 0);
        assert!(signal.is_inert());

        // And back to having wakers!

        // SAFETY: Must not be dropped until `is_inert()` returns true.
        // We expect that the test leaves us in this state and rely on assertions to verify it.
        let waker = unsafe { signal.waker() };

        // A waker now exists, so it cannot be inert.
        assert!(!signal.is_inert());

        let waker_clone = waker.clone();

        // A second one, just for extra measure.
        assert!(!signal.is_inert());
        assert_eq!(signal.waker_count.load(atomic::Ordering::Relaxed), 2);

        // Drop the wakers and we are inert again.
        drop(waker);
        drop(waker_clone);

        assert_eq!(signal.waker_count.load(atomic::Ordering::Relaxed), 0);
        assert!(signal.is_inert());
    }

    #[test]
    fn consuming_waker_wakes_signal() {
        // SAFETY: We can use it as a placeholder value but not actually dereference it.
        let fake_task_ref = unsafe { TaskRef::fake() };

        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let probe_embedded_wake_signals = Arc::new(AtomicBool::new(false));

        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::clone(&probe_embedded_wake_signals),
            Waker::noop().clone(),
            fake_task_ref
        ));
        let signal = signal.as_ref();

        // SAFETY: The consuming wake releases the only waker before the signal is dropped.
        let waker = unsafe { signal.waker() };
        waker.wake();

        assert_eq!(signal.waker_count.load(atomic::Ordering::Relaxed), 0);
        assert!(!awakened_queue.lock().unwrap().is_empty());
        assert!(signal.is_inert());
    }
}
