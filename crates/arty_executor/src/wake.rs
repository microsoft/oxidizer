// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::collections::VecDeque;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::{Arc, Mutex, Weak};
use std::task::{RawWaker, RawWakerVTable, Waker};

use pin_project::{pin_project, pinned_drop};
use plurality::{Box as PoolBox, Pool};

use crate::TaskRef;

const MAX_WAKER_COUNT: usize = usize::MAX / 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Retirement {
    Active,
    WithoutState,
    WithState,
}

pub(crate) type AwakenedQueue = Arc<Mutex<VecDeque<TaskRef>>>;

#[derive(Debug)]
pub(crate) struct WakeShared {
    pub(crate) awakened: AwakenedQueue,
    pub(crate) probe_embedded_wake_signals: Arc<AtomicBool>,
    parent_waker: Waker,
    waker_states: Mutex<Pool<WakerState>>,
}

impl WakeShared {
    pub(crate) fn new(awakened: AwakenedQueue, probe_embedded_wake_signals: Arc<AtomicBool>, parent_waker: Waker) -> Self {
        Self {
            awakened,
            probe_embedded_wake_signals,
            parent_waker,
            waker_states: Mutex::new(Pool::new()),
        }
    }
}

/// Task-owned access to independently pooled wake metadata.
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
/// The borrowed polling waker is scoped to the poll. Cloning it retains only the pooled
/// [`WakerState`], not task storage. Completion and cancellation retire that state before the
/// task is released, so retained wakers remain valid across shutdown but become inert no-ops.
///
/// # Thread safety
///
/// The type itself is single-threaded, although the `std::task::Waker` instances obtained
/// from it are thread-safe as required by the waker API contract.
#[derive(Debug)]
#[pin_project(PinnedDrop)]
pub(crate) struct WakeSignal {
    task_ref: TaskRef,
    shared: Arc<WakeShared>,
    state: AtomicPtr<WakerState>,
    retirement: Cell<Retirement>,
    awakened: AtomicBool,
    queued: AtomicBool,

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

impl WakeSignal {
    pub(crate) fn new_pooled(shared: Arc<WakeShared>, task_ref: TaskRef) -> Self {
        Self {
            task_ref,
            shared,
            state: AtomicPtr::new(std::ptr::null_mut()),
            retirement: Cell::new(Retirement::Active),
            awakened: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            _single_threaded: PhantomData,
            _requires_pin: PhantomPinned,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(
        awakened_queue: AwakenedQueue,
        probe_embedded_wake_signals: Arc<AtomicBool>,
        parent_waker: Waker,
        task_ref: TaskRef,
    ) -> Self {
        let shared = Arc::new(WakeShared::new(awakened_queue, probe_embedded_wake_signals, parent_waker));
        Self::new_pooled(shared, task_ref)
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

    fn state(&self) -> &WakerState {
        let mut state = self.state.load(atomic::Ordering::Acquire);
        if state.is_null() {
            state = self.install_state(self.allocate_state());
        }
        // SAFETY: The atomic pointer owns one intrusive reference until `WakeSignal::drop`.
        unsafe { state.as_ref() }.expect("initialized wake state pointer is never null")
    }

    fn allocate_state(&self) -> WakerStateRef {
        let value = WakerState::new(Arc::downgrade(&self.shared), self.task_ref);
        let candidate = self
            .shared
            .waker_states
            .lock()
            .expect("waker state pool is never held while invoking user code")
            .alloc_box(value);
        WakerStateRef::from_pool_box(candidate)
    }

    fn install_state(&self, candidate: WakerStateRef) -> *mut WakerState {
        let candidate_ptr = candidate.as_ptr().cast_mut();
        match self.state.compare_exchange(
            std::ptr::null_mut(),
            candidate_ptr,
            atomic::Ordering::AcqRel,
            atomic::Ordering::Acquire,
        ) {
            Ok(_) => candidate.into_raw().as_ptr(),
            Err(existing) => {
                drop(candidate);
                existing
            }
        }
    }

    fn state_if_initialized(&self) -> Option<&WakerState> {
        // SAFETY: A non-null atomic pointer owns one intrusive reference until `WakeSignal::drop`.
        unsafe { self.state.load(atomic::Ordering::Acquire).as_ref() }
    }

    #[inline]
    pub(crate) fn retire(&self) {
        let state = self.state.load(atomic::Ordering::Acquire);
        self.retirement.set(if state.is_null() {
            Retirement::WithoutState
        } else {
            Retirement::WithState
        });
        // SAFETY: A non-null atomic pointer owns one intrusive reference until
        // `WakeSignal::drop`.
        if let Some(state) = unsafe { state.as_ref() } {
            state.retire();
        }
        remove_queued_notification(&self.shared.awakened, &self.queued, self.task_ref);
    }

    pub(crate) fn clear_queued_notification(&self) {
        self.queued.store(false, atomic::Ordering::Release);
        if let Some(state) = self.state_if_initialized() {
            state.queued.store(false, atomic::Ordering::Release);
        }
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
        let inline = self.awakened.swap(false, atomic::Ordering::Relaxed);
        let independent = self.state_if_initialized().is_some_and(WakerState::consume_awakened);
        if inline || independent {
            atomic::fence(atomic::Ordering::Acquire);
            true
        } else {
            false
        }
    }

    /// Returns whether task-owned signal storage is safe to drop.
    ///
    /// Before retirement this requires no owned wakers. Retirement releases the task's owner,
    /// after which independent wakers retain only pooled state and this returns `true`.
    #[cfg_attr(test, mutants::skip)] // Mutation causes infinite loops as executor will never shut down.
    #[cfg(test)]
    pub(crate) fn is_inert(&self) -> bool {
        // We use Acquire ordering to ensure we see all writes to the waker before we declare it
        // inert. Generally, we expect wakers to already be inert by the time their inertness is
        // queried, because this query will happen when a task has completed and its future has
        // been dropped, which should (unless there is a resource leak) drop any wakers held by
        // pending awaits triggered deeper in the future.
        self.state_if_initialized()
            .is_none_or(|state| self.retirement.get() != Retirement::Active || state.external_waker_count() == 0)
    }

    /// Borrows a waker associated with this signal without incrementing its reference count.
    ///
    /// # Safety
    ///
    /// The signal must remain pinned for the returned borrow. Any clones own independent pooled
    /// state and do not extend this borrow.
    pub(crate) unsafe fn waker_ref(self: Pin<&Self>) -> impl Deref<Target = Waker> + '_ {
        let signal_ptr: *const Self = self.get_ref();

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
    /// The signal must remain pinned until this call returns.
    #[cfg(test)]
    pub(crate) unsafe fn waker(self: Pin<&Self>) -> Waker {
        // SAFETY: The caller keeps the signal alive until all owned wakers are released.
        unsafe { self.waker_ref() }.clone()
    }

    #[cfg(test)]
    fn external_waker_count(&self) -> usize {
        self.state_if_initialized().map_or(0, WakerState::external_waker_count)
    }

    #[cfg(test)]
    pub(crate) fn has_queued_notification(&self) -> bool {
        self.queued.load(atomic::Ordering::Acquire)
    }

    fn wake_inline(&self) {
        if enqueue_wake(
            &self.shared.awakened,
            &self.queued,
            self.task_ref,
            &self.shared.parent_waker,
            || true,
        ) {
            return;
        }
        self.awakened.store(true, atomic::Ordering::Release);
        self.shared.probe_embedded_wake_signals.store(true, atomic::Ordering::Release);
        self.shared.parent_waker.wake_by_ref();
    }
}

#[pinned_drop]
impl PinnedDrop for WakeSignal {
    fn drop(self: Pin<&mut Self>) {
        if self.retirement.get() == Retirement::Active {
            self.retire();
        }
        if self.retirement.get() == Retirement::WithState {
            let state = self.state.swap(std::ptr::null_mut(), atomic::Ordering::AcqRel);
            let state = NonNull::new(state).expect("wake state was present before the owner swap");
            // SAFETY: The atomic pointer owns exactly one intrusive reference.
            drop(unsafe { WakerStateRef::from_non_null(state) });
        }
    }
}

pub(crate) struct WakerState {
    task_ref: TaskRef,
    shared: Weak<WakeShared>,
    ref_count: AtomicUsize,
    waker_count: AtomicUsize,
    awakened: AtomicBool,
    queued: AtomicBool,
    active: AtomicBool,
}

// SAFETY: `TaskRef` is only returned to the executor owner and never dereferenced here. All
// cross-thread mutation is atomic or mutex-protected.
unsafe impl Send for WakerState {}
// SAFETY: Same invariant as `Send`.
unsafe impl Sync for WakerState {}

impl WakerState {
    pub(crate) fn new(shared: Weak<WakeShared>, task_ref: TaskRef) -> Self {
        Self {
            task_ref,
            shared,
            ref_count: AtomicUsize::new(1),
            waker_count: AtomicUsize::new(0),
            awakened: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            active: AtomicBool::new(true),
        }
    }

    fn wake(&self) {
        if !self.active.load(atomic::Ordering::Acquire) {
            return;
        }

        if let Some(shared) = self.shared.upgrade()
            && enqueue_wake(&shared.awakened, &self.queued, self.task_ref, &shared.parent_waker, || {
                self.active.load(atomic::Ordering::Acquire)
            })
        {
            return;
        }

        if !self.active.load(atomic::Ordering::Acquire) {
            return;
        }
        self.awakened.store(true, atomic::Ordering::Release);
        if let Some(shared) = self.shared.upgrade() {
            shared.probe_embedded_wake_signals.store(true, atomic::Ordering::Release);
            shared.parent_waker.wake_by_ref();
        }
    }

    fn retire(&self) {
        self.active.store(false, atomic::Ordering::Release);
        if let Some(shared) = self.shared.upgrade() {
            remove_queued_notification(&shared.awakened, &self.queued, self.task_ref);
        }
    }

    fn consume_awakened(&self) -> bool {
        self.awakened.swap(false, atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    fn external_waker_count(&self) -> usize {
        self.waker_count.load(atomic::Ordering::Acquire)
    }
}

impl std::fmt::Debug for WakerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("active", &self.active.load(atomic::Ordering::Relaxed))
            .field("ref_count", &self.ref_count.load(atomic::Ordering::Relaxed))
            .field("waker_count", &self.waker_count.load(atomic::Ordering::Relaxed))
            .field("queued", &self.queued.load(atomic::Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

fn enqueue_wake(
    queue: &AwakenedQueue,
    queued: &AtomicBool,
    task_ref: TaskRef,
    parent_waker: &Waker,
    is_active: impl FnOnce() -> bool,
) -> bool {
    let Ok(mut queue) = queue.try_lock() else {
        return false;
    };
    if !is_active() {
        return true;
    }
    if queue.len() >= queue.capacity() {
        return false;
    }
    queued.store(true, atomic::Ordering::Release);
    queue.push_back(task_ref);
    drop(queue);
    parent_waker.wake_by_ref();
    true
}

fn remove_queued_notification(queue: &AwakenedQueue, queued: &AtomicBool, task_ref: TaskRef) {
    if !queued.load(atomic::Ordering::Acquire) {
        return;
    }
    if !queued.swap(false, atomic::Ordering::AcqRel) {
        return;
    }
    queue
        .lock()
        .expect("wake queue is never held while invoking user code")
        .retain(|queued_task_ref| *queued_task_ref != task_ref);
}

struct WakerStateRef(NonNull<WakerState>);

// SAFETY: The pointee is `Send + Sync`, uses an atomic reference count, and remains allocated
// until the final reference returns its slot to the pool.
unsafe impl Send for WakerStateRef {}
// SAFETY: Same invariant as `Send`.
unsafe impl Sync for WakerStateRef {}

impl WakerStateRef {
    fn from_pool_box(state: PoolBox<WakerState>) -> Self {
        Self(PoolBox::into_raw(state))
    }

    unsafe fn from_raw(ptr: *const ()) -> Self {
        Self(NonNull::new(ptr.cast_mut().cast()).expect("raw waker state pointer is never null"))
    }

    unsafe fn from_non_null(ptr: NonNull<WakerState>) -> Self {
        Self(ptr)
    }

    fn as_ptr(&self) -> *const WakerState {
        self.0.as_ptr()
    }

    fn into_raw(self) -> NonNull<WakerState> {
        let ptr = self.0;
        std::mem::forget(self);
        ptr
    }
}

impl Deref for WakerStateRef {
    type Target = WakerState;

    fn deref(&self) -> &Self::Target {
        // SAFETY: This reference contributes to the intrusive count, so the pooled value remains
        // alive for the returned borrow.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for WakerStateRef {
    fn drop(&mut self) {
        if self.ref_count.fetch_sub(1, atomic::Ordering::Release) != 1 {
            return;
        }
        atomic::fence(atomic::Ordering::Acquire);
        // SAFETY: The count reached zero, so this is the unique final reconstruction of the exact
        // pointer produced by `PoolBox::into_raw`.
        drop(unsafe { PoolBox::<WakerState>::from_raw(self.0) });
    }
}

impl std::fmt::Debug for WakerStateRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

static BORROWED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(borrowed_clone, borrowed_consume, borrowed_wake_by_ref, borrowed_drop);
static OWNED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone_waker, waker_wake, waker_wake_by_ref, waker_drop_waker);

fn borrowed_clone(ptr: *const ()) -> RawWaker {
    let state = resurrect_signal_ref(ptr).state();
    increment_waker_count(&state.waker_count, std::process::abort);
    increment_reference_count(&state.ref_count, std::process::abort);
    RawWaker::new(std::ptr::from_ref(state).cast(), &OWNED_WAKER_VTABLE)
}

fn borrowed_wake_by_ref(ptr: *const ()) {
    resurrect_signal_ref(ptr).wake_inline();
}

#[cfg_attr(coverage_nightly, coverage(off))] // Unreachable through safe public Waker APIs.
#[cfg_attr(test, mutants::skip)] // Safe Waker APIs never consume the borrowed polling waker.
fn borrowed_consume(_: *const ()) {
    unreachable!("a borrowed polling waker cannot be consumed");
}

#[cfg_attr(coverage_nightly, coverage(off))] // Unreachable through safe public Waker APIs.
#[cfg_attr(test, mutants::skip)] // Safe Waker APIs never drop the borrowed polling waker.
fn borrowed_drop(_: *const ()) {
    unreachable!("a borrowed polling waker cannot be dropped");
}

fn waker_clone_waker(ptr: *const ()) -> RawWaker {
    let state = resurrect_state_ref(ptr);
    increment_waker_count(&state.waker_count, std::process::abort);
    increment_reference_count(&state.ref_count, std::process::abort);

    RawWaker::new(ptr, &OWNED_WAKER_VTABLE)
}

fn increment_reference_count(ref_count: &AtomicUsize, on_overflow: fn() -> !) {
    let previous = ref_count.fetch_add(1, atomic::Ordering::Relaxed);
    if previous >= MAX_WAKER_COUNT {
        on_overflow();
    }
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
    // SAFETY: Consuming wake transfers this raw waker's counted ownership into the guard. Its
    // destructor releases that ownership even if the configured owner waker panics.
    let state = RawWakerOwner(unsafe { WakerStateRef::from_raw(ptr) });
    state.wake();
}

#[cfg_attr(test, mutants::skip)] // If tasks do not wake up, tests tend to infinite loop.
fn waker_wake_by_ref(ptr: *const ()) {
    resurrect_state_ref(ptr).wake();
}

#[cfg_attr(test, mutants::skip)] // It's well tested, but causes ocasional test timeouts
fn waker_drop_waker(ptr: *const ()) {
    // SAFETY: Dropping transfers this raw waker's counted ownership into the guard.
    drop(RawWakerOwner(unsafe { WakerStateRef::from_raw(ptr) }));
}

struct RawWakerOwner(WakerStateRef);

impl Deref for RawWakerOwner {
    type Target = WakerState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for RawWakerOwner {
    fn drop(&mut self) {
        self.waker_count.fetch_sub(1, atomic::Ordering::Release);
    }
}

/// Resurrects the pooled state reference that hides behind the waker's state pointer.
///
/// We return it with `'static` because there is no Rust lifetime that corresponds to
/// the waker reference's real lifetime. Just do not use it after the waker vtable methods.
#[cfg_attr(coverage_nightly, coverage(off))] // A null pointer would violate this module's RawWaker invariant.
fn resurrect_state_ref(ptr: *const ()) -> &'static WakerState {
    // SAFETY: Every raw waker pointer comes from a live pooled state retained by either the task
    // owner or the raw waker's own counted reference.
    let state = unsafe { ptr.cast::<WakerState>().as_ref() };

    let Some(state) = state else {
        unreachable!("waker has a null pointer for its inner state - impossible")
    };

    state
}

fn resurrect_signal_ref(ptr: *const ()) -> &'static WakeSignal {
    // SAFETY: Borrowed polling wakers exist only for the poll scope while their pinned signal
    // remains alive.
    let signal = unsafe { ptr.cast::<WakeSignal>().as_ref() };

    let Some(signal) = signal else {
        unreachable!("borrowed waker has a null pointer for its wake signal - impossible")
    };

    signal
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
    fn borrowed_waker_reference_count() {
        let signal = pin!(WakeSignal::fake());
        let signal = signal.as_ref();

        {
            // SAFETY: The signal stays pinned until the borrowed waker and its clone are dropped.
            let waker = unsafe { signal.waker_ref() };
            assert_eq!(signal.external_waker_count(), 0);

            waker.wake_by_ref();
            assert!(signal.consume_awakened());
            assert_eq!(signal.external_waker_count(), 0);

            let clone = waker.clone();
            assert_eq!(signal.external_waker_count(), 1);
            drop(clone);
            assert_eq!(signal.external_waker_count(), 0);
        }

        assert_eq!(signal.external_waker_count(), 0);
        assert!(signal.is_inert());
    }

    #[test]
    fn borrowed_wake_uses_inline_queue_without_allocating_state() {
        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
            fake_task_ref
        ));
        let signal = signal.as_ref();

        // SAFETY: The signal remains pinned for the borrowed wake.
        unsafe { signal.waker_ref() }.wake_by_ref();

        assert!(signal.state_if_initialized().is_none());
        assert_eq!(awakened_queue.lock().unwrap().len(), 1);
    }

    #[test]
    fn duplicate_state_installation_releases_the_unused_candidate() {
        let signal = pin!(WakeSignal::fake());
        let signal = signal.as_ref();
        let installed = std::ptr::from_ref(signal.state()).cast_mut();

        let candidate = signal.allocate_state();
        assert!(format!("{candidate:?}").contains("WakerState"));
        assert_eq!(signal.install_state(candidate), installed);
    }

    #[test]
    fn retirement_removes_notifications_and_makes_owned_wakers_inert() {
        // SAFETY: Tests use this only as an opaque identity.
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
        // SAFETY: The signal remains pinned through retirement.
        let waker = unsafe { signal.waker() };

        waker.wake_by_ref();
        assert_eq!(awakened_queue.lock().unwrap().len(), 1);

        signal.retire();
        assert!(awakened_queue.lock().unwrap().is_empty());
        waker.wake_by_ref();
        assert!(awakened_queue.lock().unwrap().is_empty());
        assert!(!probe_embedded_wake_signals.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn dropping_an_active_signal_retires_retained_wakers() {
        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let shared = Arc::new(WakeShared {
            awakened: Arc::clone(&awakened_queue),
            probe_embedded_wake_signals: Arc::new(AtomicBool::new(false)),
            parent_waker: Waker::noop().clone(),
            waker_states: Mutex::new(Pool::builder().chunk_size(1).max_chunks(1).build()),
        });
        let retained = {
            let signal = pin!(WakeSignal::new_pooled(Arc::clone(&shared), fake_task_ref));
            // SAFETY: The signal remains pinned until the clone owns independent state.
            unsafe { signal.as_ref().waker() }
        };

        retained.wake_by_ref();
        assert!(awakened_queue.lock().unwrap().is_empty());
        drop(retained);

        let state = WakerState::new(Arc::downgrade(&shared), fake_task_ref);
        drop(shared.waker_states.lock().unwrap().try_alloc_box(state).unwrap());
    }

    #[test]
    fn final_state_reference_returns_its_pool_slot() {
        let pool = Pool::builder().chunk_size(1).max_chunks(1).build();
        let shared = Arc::new(WakeShared::new(
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(AtomicBool::new(false)),
            Waker::noop().clone(),
        ));
        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        let state = || WakerState::new(Arc::downgrade(&shared), fake_task_ref);

        drop(WakerStateRef::from_pool_box(pool.alloc_box(state())));

        drop(pool.try_alloc_box(state()).unwrap());
    }

    #[test]
    fn wake_state_debug_reports_lifecycle_counts() {
        let signal = pin!(WakeSignal::fake());
        let debug = format!("{:?}", signal.state());

        assert!(debug.contains("active"));
        assert!(debug.contains("waker_count"));
        assert!(debug.contains("queued"));
    }

    #[test]
    fn inactive_notification_is_not_queued() {
        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let queued = AtomicBool::new(false);

        assert!(enqueue_wake(&queue, &queued, fake_task_ref, Waker::noop(), || false));
        assert!(queue.lock().unwrap().is_empty());
        assert!(!queued.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn untracked_notification_is_not_removed() {
        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        let queue = Arc::new(Mutex::new(VecDeque::from([fake_task_ref])));
        let queued = AtomicBool::new(false);

        remove_queued_notification(&queue, &queued, fake_task_ref);

        assert_eq!(queue.lock().unwrap().len(), 1);
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
    fn reference_count_overflow_is_rejected() {
        let count = AtomicUsize::new(MAX_WAKER_COUNT);

        testing_aids::assert_panic!(increment_reference_count(&count, || panic!("reference count overflow")));

        assert_eq!(count.load(atomic::Ordering::Relaxed), MAX_WAKER_COUNT + 1);
    }

    #[test]
    fn null_borrowed_signal_pointer_is_rejected() {
        testing_aids::assert_panic!(resurrect_signal_ref(std::ptr::null()));
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
        assert_eq!(signal.external_waker_count(), 2);

        // Drop the wakers and we are inert again.
        drop(waker);
        drop(waker_clone);

        assert_eq!(signal.external_waker_count(), 0);
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
        assert_eq!(signal.external_waker_count(), 2);

        // Drop the wakers and we are inert again.
        drop(waker);
        drop(waker_clone);

        assert_eq!(signal.external_waker_count(), 0);
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

        assert_eq!(signal.external_waker_count(), 0);
        assert!(!awakened_queue.lock().unwrap().is_empty());
        assert!(signal.is_inert());
    }

    #[test]
    fn consuming_waker_releases_ownership_when_owner_panics() {
        struct PanicWake;

        impl std::task::Wake for PanicWake {
            fn wake(self: Arc<Self>) {
                panic!("owner wake panic");
            }
        }

        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        let awakened_queue = Arc::new(Mutex::new(VecDeque::with_capacity(1)));
        let signal = pin!(WakeSignal::new(
            Arc::clone(&awakened_queue),
            Arc::new(AtomicBool::new(false)),
            Waker::from(Arc::new(PanicWake)),
            fake_task_ref
        ));
        let signal = signal.as_ref();

        // SAFETY: The signal remains pinned through the consuming wake.
        let waker = unsafe { signal.waker() };
        testing_aids::assert_panic!(waker.wake());

        assert_eq!(signal.external_waker_count(), 0);
        assert!(signal.is_inert());
    }
}
