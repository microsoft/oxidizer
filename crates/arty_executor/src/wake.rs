// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{self, AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::{fmt, ptr};

use pin_project::{pin_project, pinned_drop};

use crate::TaskRef;

const MAX_WAKER_COUNT: usize = usize::MAX / 2;

#[derive(Debug)]
pub(crate) enum WakeNotification {
    Inline(TaskRef),
    Independent(Weak<WakeState>),
}

pub(crate) type AwakenedQueue = Arc<Mutex<VecDeque<WakeNotification>>>;

/// Inline task-owned access to independently owned wake metadata.
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
/// The borrowed polling waker is scoped to a shared borrow of the pinned signal and does not
/// contribute to `waker_count`. Cloning it creates an independently owned, counted waker that may
/// outlive the poll. Owned wakers retain only shared wake metadata, not the future or task.
/// Retiring the signal makes those wakers inert before task storage is released.
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
    awakened_queue: AwakenedQueue,

    /// If we cannot add the task to `awakened_queue`, we set this flag to inform the task engine
    /// that it needs to read each task's wake signal to identify what has woken up.
    probe_embedded_wake_signals: Arc<AtomicBool>,

    state: OnceLock<Arc<WakeState>>,
    retired: AtomicBool,
    independent_wakers: bool,
    waker_count: AtomicUsize,
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

    /// This type cannot be unpinned once it has been pinned (latest when calling `waker_ref()`).
    _requires_pin: PhantomPinned,
}

pub(crate) struct WakeState {
    task_ref: TaskRef,
    awakened_queue: AwakenedQueue,
    probe_embedded_wake_signals: Arc<AtomicBool>,
    waker_count: AtomicUsize,
    awakened: AtomicBool,
    active: AtomicBool,
    parent_waker: Waker,
}

// SAFETY: TaskRef is only copied into the queue, never dereferenced here.
// All mutable state is atomic or mutex-protected. Only the executor's owner
// resolves an active notification back to a live task.
unsafe impl Sync for WakeState {}

impl fmt::Debug for WakeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WakeState")
            .field("active", &self.active.load(atomic::Ordering::Relaxed))
            .field("waker_count", &self.waker_count.load(atomic::Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl WakeSignal {
    pub(crate) fn new(
        awakened_queue: AwakenedQueue,
        probe_embedded_wake_signals: Arc<AtomicBool>,
        parent_waker: Waker,
        task_ref: TaskRef,
    ) -> Self {
        Self {
            task_ref,
            awakened_queue,
            probe_embedded_wake_signals,
            state: OnceLock::new(),
            retired: AtomicBool::new(false),
            independent_wakers: false,
            waker_count: AtomicUsize::new(0),
            awakened: AtomicBool::new(false),
            parent_waker,
            _single_threaded: PhantomData,
            _requires_pin: PhantomPinned,
        }
    }

    pub(crate) fn independent_wakers(mut self, enabled: bool) -> Self {
        self.independent_wakers = enabled;
        self
    }

    fn state(&self) -> &Arc<WakeState> {
        // A ready task that never clones or wakes its polling waker stays allocation-free.
        let state = self.state.get_or_init(|| {
            Arc::new(WakeState {
                task_ref: self.task_ref,
                awakened_queue: Arc::clone(&self.awakened_queue),
                probe_embedded_wake_signals: Arc::clone(&self.probe_embedded_wake_signals),
                waker_count: AtomicUsize::new(0),
                awakened: AtomicBool::new(false),
                active: AtomicBool::new(true),
                parent_waker: self.parent_waker.clone(),
            })
        });
        if self.retired.load(atomic::Ordering::Acquire) {
            state.active.store(false, atomic::Ordering::Release);
        }
        state
    }

    pub(crate) fn retire(&self) {
        if self.independent_wakers {
            self.retired.store(true, atomic::Ordering::Release);
            if let Some(state) = self.state.get() {
                state.active.store(false, atomic::Ordering::Release);
            }
        } else {
            let mut queued = self
                .awakened_queue
                .lock()
                .expect("wake queue is never held while invoking user code");
            self.retired.store(true, atomic::Ordering::Release);
            queued.retain(|notification| !matches!(notification, WakeNotification::Inline(task) if *task == self.task_ref));
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
            Arc::new(Mutex::new(VecDeque::new())),
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
        let awakened = if self.independent_wakers {
            self.state
                .get()
                .is_some_and(|state| state.awakened.swap(false, atomic::Ordering::Relaxed))
        } else {
            self.awakened.swap(false, atomic::Ordering::Relaxed)
        };
        if awakened {
            // This does nothing on x86 but on weaker memory models, the visibility of
            // writes to arbitrary locations may be delayed without this fence.
            atomic::fence(atomic::Ordering::Acquire);
            true
        } else {
            false
        }
    }

    /// Retired signals no longer expose task storage through their owned wakers.
    /// Any borrowed polling waker must still be released before dropping the signal.
    #[cfg_attr(test, mutants::skip)] // Mutation causes infinite loops as executor will never shut down.
    pub(crate) fn is_inert(&self) -> bool {
        // We use Acquire ordering to ensure we see all writes to the waker before we declare it
        // inert. Generally, we expect wakers to already be inert by the time their inertness is
        // queried, because this query will happen when a task has completed and its future has
        // been dropped, which should (unless there is a resource leak) drop any wakers held by
        // pending awaits triggered deeper in the future.
        (self.independent_wakers && self.retired.load(atomic::Ordering::Acquire)) || self.owned_waker_count() == 0
    }

    fn owned_waker_count(&self) -> usize {
        if self.independent_wakers {
            self.state
                .get()
                .map_or(0, |state| state.waker_count.load(atomic::Ordering::Acquire))
        } else {
            self.waker_count.load(atomic::Ordering::Acquire)
        }
    }

    /// Borrows a waker associated with this signal without incrementing its reference count.
    ///
    /// # Safety
    ///
    /// The signal must remain pinned for this borrow. Before releasing task storage,
    /// the owner must retire the signal or release all owned wakers.
    pub(crate) unsafe fn waker_ref(self: Pin<&Self>) -> impl Deref<Target = Waker> + '_ {
        let signal_ptr: *const Self = ptr::from_ref(self.get_ref());

        // Like a borrowed Arc waker, this borrow keeps the signal alive without an owned count.
        // Only Waker::clone creates an owned reference. Hide ManuallyDrop behind Deref so callers
        // cannot extract or consume the uncounted Waker; its lifetime stays tied to this borrow.
        // SAFETY: We are required to correctly implement the waker API contract, which we do.
        // This includes being thread-safe, etc. The `WakeSignal` is thread-safe and all the
        // methods use interior mutability, so we are only using shared references, thereby
        // ensuring we do not violate Rust aliasing rules (as long as the owner of the `WakeSignal`
        // does not create any mutable references - though given that all methods are `&self` that
        // might still be relatively harmless and anyway likely detected by Miri as a bug.
        ManuallyDrop::new(unsafe { Waker::from_raw(RawWaker::new(signal_ptr.cast(), &BORROWED_WAKER_VTABLE)) })
    }

    /// Creates an owned waker for tests of the reference-counting protocol.
    ///
    /// # Safety
    ///
    /// The signal must remain pinned until `is_inert()` returns true.
    #[cfg(test)]
    pub(crate) unsafe fn waker(self: Pin<&Self>) -> Waker {
        // SAFETY: The caller keeps the signal alive until all owned wakers are released.
        unsafe { self.waker_ref() }.clone()
    }

    fn wake_inline(&self) {
        if self.retired.load(atomic::Ordering::Acquire) {
            return;
        }
        if self.enqueue_inline_wake() {
            return;
        }
        self.awakened.store(true, atomic::Ordering::Release);
        self.probe_embedded_wake_signals.store(true, atomic::Ordering::Release);
        self.parent_waker.wake_by_ref();
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // The second retirement check is an inherently concurrent race guard.
    fn enqueue_inline_wake(&self) -> bool {
        let Ok(mut queue) = self.awakened_queue.try_lock() else {
            return false;
        };
        if self.retired.load(atomic::Ordering::Acquire) {
            return true;
        }
        if queue.len() >= queue.capacity() {
            return false;
        }
        queue.push_back(WakeNotification::Inline(self.task_ref));
        drop(queue);
        self.parent_waker.wake_by_ref();
        true
    }
}

impl WakeState {
    pub(crate) fn task_ref(&self) -> TaskRef {
        self.task_ref
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(atomic::Ordering::Acquire)
    }

    fn wake(self: &Arc<Self>) {
        if !self.is_active() {
            return;
        }
        if let Ok(mut awakened_set) = self.awakened_queue.try_lock() {
            // We only add if we can do so without increasing capacity, because increasing capacity
            // from an arbitrary thread may require reallocation, which we do not want to do on a
            // different thread than the one that owns the set.
            if awakened_set.len() < awakened_set.capacity() {
                // If we experienced spurious awakenings, we might push the same task multiple
                // times. That is fine - it is up to the receiver of the notifications to deal
                // with spurious notifications (which may arrive anyway through other means).
                // Weak notifications cannot retain a queue/state cycle. The executor
                // checks active before resolving TaskRef, including after slot reuse.
                awakened_set.push_back(WakeNotification::Independent(Arc::downgrade(self)));
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

static BORROWED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(borrowed_clone, borrowed_consume, borrowed_wake_by_ref, borrowed_drop);
static OWNED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone_waker, waker_wake, waker_wake_by_ref, waker_drop_waker);
static INLINE_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(inline_clone, inline_wake, inline_wake_by_ref, inline_drop);

fn borrowed_clone(ptr: *const ()) -> RawWaker {
    let signal = resurrect_signal_ref(ptr);
    if signal.independent_wakers {
        owned_waker(signal.state())
    } else {
        inline_clone(ptr)
    }
}

fn borrowed_wake_by_ref(ptr: *const ()) {
    let signal = resurrect_signal_ref(ptr);
    if signal.independent_wakers {
        signal.state().wake();
    } else {
        signal.wake_inline();
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Unreachable through safe public Waker APIs.
fn borrowed_consume(_: *const ()) {
    unreachable!("a borrowed polling waker cannot be consumed");
}

#[cfg_attr(coverage_nightly, coverage(off))] // Unreachable through safe public Waker APIs.
fn borrowed_drop(_: *const ()) {
    unreachable!("a borrowed polling waker cannot be dropped");
}

fn owned_waker(state: &Arc<WakeState>) -> RawWaker {
    increment_waker_count(&state.waker_count, std::process::abort);
    RawWaker::new(Arc::into_raw(Arc::clone(state)).cast(), &OWNED_WAKER_VTABLE)
}

fn waker_clone_waker(ptr: *const ()) -> RawWaker {
    // SAFETY: each owned raw waker holds one Arc strong reference.
    let state = ManuallyDrop::new(unsafe { Arc::from_raw(ptr.cast::<WakeState>()) });
    owned_waker(&state)
}

fn increment_waker_count(waker_count: &AtomicUsize, on_overflow: fn() -> !) {
    // Reference count increment is independent of state transitions, so Relaxed is enough.
    let previous = waker_count.fetch_add(1, atomic::Ordering::Relaxed);

    // Like Arc, leave half the range for concurrent increments before aborting.
    // Unwinding instead would let repeated clone attempts keep growing the count.
    if previous >= MAX_WAKER_COUNT {
        on_overflow();
    }
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake(ptr: *const ()) {
    // SAFETY: consuming the raw waker transfers its Arc reference to this guard.
    let state = OwnedWaker(unsafe { Arc::from_raw(ptr.cast::<WakeState>()) });
    state.0.wake();
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake_by_ref(ptr: *const ()) {
    // SAFETY: the caller keeps its owned Arc reference alive throughout this call.
    let state = ManuallyDrop::new(unsafe { Arc::from_raw(ptr.cast::<WakeState>()) });
    state.wake();
}

#[cfg_attr(test, mutants::skip)] // It's well tested, but causes ocasional test timeouts
fn waker_drop_waker(ptr: *const ()) {
    // SAFETY: dropping the raw waker transfers its Arc reference to this guard.
    drop(OwnedWaker(unsafe { Arc::from_raw(ptr.cast::<WakeState>()) }));
}

struct OwnedWaker(Arc<WakeState>);

impl Drop for OwnedWaker {
    fn drop(&mut self) {
        self.0.waker_count.fetch_sub(1, atomic::Ordering::Release);
    }
}

fn inline_clone(ptr: *const ()) -> RawWaker {
    increment_waker_count(&resurrect_signal_ref(ptr).waker_count, std::process::abort);
    RawWaker::new(ptr, &INLINE_WAKER_VTABLE)
}

fn inline_wake(ptr: *const ()) {
    struct CountGuard<'a>(&'a AtomicUsize);
    impl Drop for CountGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, atomic::Ordering::Release);
        }
    }
    let signal = resurrect_signal_ref(ptr);
    let _count = CountGuard(&signal.waker_count);
    signal.wake_inline();
}

fn inline_wake_by_ref(ptr: *const ()) {
    resurrect_signal_ref(ptr).wake_inline();
}

fn inline_drop(ptr: *const ()) {
    resurrect_signal_ref(ptr).waker_count.fetch_sub(1, atomic::Ordering::Release);
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

    // Safe public APIs expose borrowed polling wakers only through Deref.
    #[test]
    #[should_panic(expected = "a borrowed polling waker cannot be consumed")]
    fn borrowed_polling_waker_cannot_be_consumed() {
        borrowed_consume(ptr::null());
    }

    #[test]
    #[should_panic(expected = "a borrowed polling waker cannot be dropped")]
    fn borrowed_polling_waker_cannot_be_dropped() {
        borrowed_drop(ptr::null());
    }

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
    fn borrowed_waker_reference_count() {
        let signal = pin!(WakeSignal::fake());
        let signal = signal.as_ref();

        {
            // SAFETY: The signal stays pinned until the borrowed waker and its clone are dropped.
            let waker = unsafe { signal.waker_ref() };
            assert_eq!(signal.owned_waker_count(), 0);

            waker.wake_by_ref();
            assert!(signal.consume_awakened());
            assert_eq!(signal.owned_waker_count(), 0);

            let clone = waker.clone();
            assert_eq!(signal.owned_waker_count(), 1);
            drop(clone);
            assert_eq!(signal.owned_waker_count(), 0);
        }

        assert_eq!(signal.owned_waker_count(), 0);
        assert!(signal.is_inert());
    }

    #[test]
    fn independent_wakers_retire_without_retaining_task_state() {
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let signal = pin!(WakeSignal::new(
            Arc::clone(&queue),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            // SAFETY: the fake task reference is only used as an opaque wake identity.
            unsafe { TaskRef::fake() },
        )
        .independent_wakers(true));
        let signal = signal.as_ref();
        signal.retire();
        let state = signal.state();
        assert!(format!("{state:?}").contains("WakeState"));

        // SAFETY: the pinned signal remains alive until the owned clone is dropped.
        let borrowed = unsafe { signal.waker_ref() };
        borrowed.wake_by_ref();
        let waker = borrowed.clone();
        assert_eq!(signal.owned_waker_count(), 1);
        signal.retire();
        waker.wake_by_ref();
        assert!(queue.lock().unwrap().is_empty());
        assert!(!signal.consume_awakened());
        assert!(signal.is_inert());
        let consuming = waker.clone();
        consuming.wake();
        drop(waker);
        assert_eq!(signal.owned_waker_count(), 0);
    }

    #[test]
    fn inline_retirement_removes_queued_task_notifications() {
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let signal = pin!(WakeSignal::new(
            Arc::clone(&queue),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            // SAFETY: the fake task reference is only used as an opaque wake identity.
            unsafe { TaskRef::fake() },
        ));
        let signal = signal.as_ref();
        // SAFETY: the signal remains pinned until the owned waker is dropped.
        let waker = unsafe { signal.waker() };
        waker.wake_by_ref();
        assert_eq!(queue.lock().unwrap().len(), 1);
        signal.retire();
        assert!(queue.lock().unwrap().is_empty());
    }

    #[test]
    fn independent_wake_uses_queue_and_probe_paths() {
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let signal = pin!(WakeSignal::new(
            Arc::clone(&queue),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            // SAFETY: the fake task reference is only used as an opaque wake identity.
            unsafe { TaskRef::fake() },
        )
        .independent_wakers(true));
        let signal = signal.as_ref();
        // SAFETY: the signal remains pinned until the owned waker is dropped.
        let waker = unsafe { signal.as_ref().waker() };
        waker.wake_by_ref();
        assert!(!queue.lock().unwrap().is_empty());
        queue.lock().unwrap().clear();

        let full_queue = Arc::new(Mutex::new(VecDeque::with_capacity(0)));
        let signal = pin!(WakeSignal::new(
            full_queue,
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            // SAFETY: the fake task reference is only used as an opaque wake identity.
            unsafe { TaskRef::fake() },
        )
        .independent_wakers(true));
        let signal = signal.as_ref();
        // SAFETY: the signal remains pinned until the owned waker is dropped.
        let waker = unsafe { signal.as_ref().waker() };
        waker.wake_by_ref();
        assert!(signal.consume_awakened());
    }

    #[test]
    fn independent_wake_falls_back_when_the_queue_is_locked() {
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let lock = queue.lock().unwrap();
        let signal = pin!(WakeSignal::new(
            Arc::clone(&queue),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            // SAFETY: the fake task reference is only used as an opaque wake identity.
            unsafe { TaskRef::fake() },
        )
        .independent_wakers(true));
        let signal = signal.as_ref();
        // SAFETY: the signal remains pinned until the owned waker is dropped.
        let waker = unsafe { signal.waker() };
        waker.wake_by_ref();
        drop(lock);
        assert!(signal.consume_awakened());
    }

    #[test]
    fn inline_retirement_race_does_not_publish_stale_notifications() {
        let signal = pin!(WakeSignal::fake());
        // SAFETY: the signal remains pinned until the worker has stopped using the waker.
        let waker = unsafe { signal.as_ref().waker() };
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let started = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            started.wait();
            for _ in 0..10_000 {
                waker.wake_by_ref();
            }
        });
        barrier.wait();
        signal.retire();
        worker.join().unwrap();
    }

    #[test]
    fn waker_count_reaches_limit() {
        let count = AtomicUsize::new(MAX_WAKER_COUNT - 1);

        increment_waker_count(&count, || panic!("unexpected waker count overflow"));

        assert_eq!(count.load(atomic::Ordering::Relaxed), MAX_WAKER_COUNT);
    }

    #[test]
    fn waker_count_overflow_is_rejected() {
        for initial in [MAX_WAKER_COUNT, MAX_WAKER_COUNT + 1] {
            let count = AtomicUsize::new(initial);

            testing_aids::assert_panic!(increment_waker_count(&count, || panic!("waker count overflow")));

            assert_eq!(count.load(atomic::Ordering::Relaxed), initial + 1);
        }
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
        let awakened_queue = Arc::new(Mutex::new(VecDeque::new()));
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
        assert_eq!(signal.owned_waker_count(), 2);

        // Drop the wakers and we are inert again.
        drop(waker);
        drop(waker_clone);

        assert_eq!(signal.owned_waker_count(), 0);
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
        assert_eq!(signal.owned_waker_count(), 2);

        // Drop the wakers and we are inert again.
        drop(waker);
        drop(waker_clone);

        assert_eq!(signal.owned_waker_count(), 0);
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

        assert_eq!(signal.owned_waker_count(), 0);
        assert!(!awakened_queue.lock().unwrap().is_empty());
        assert!(signal.is_inert());
    }
}
