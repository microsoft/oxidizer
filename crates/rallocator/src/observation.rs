// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Optional, owner-published copies. Never inspect another leased core.

#[cfg(not(test))]
use std::alloc::System;
use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};

use seismograph::recorder::{RecordingSession, SuppressionGuard};
use seismograph_rallocator::native::{Observation, ObservationSource};

#[cfg(test)]
use self::tests::FailingSystem as System;

/// One visit budget bounds all slab, buddy and message walks in an observation.
pub(crate) const WALK_BUDGET: usize = 4096;
/// Bound one inventory capture, reporting omitted endpoints explicitly.
pub(crate) const OWNER_LIMIT: usize = 1024;

pub(crate) struct Metadata {
    all_next: UnsafeCell<usize>,
    generation: UnsafeCell<u64>,
    slot: AtomicPtr<Slot>,
    unavailable: AtomicBool,
}

impl Metadata {
    pub(crate) const fn new() -> Self {
        Self {
            all_next: UnsafeCell::new(0),
            generation: UnsafeCell::new(0),
            slot: AtomicPtr::new(std::ptr::null_mut()),
            unavailable: AtomicBool::new(false),
        }
    }

    /// # Safety
    /// The caller holds the pool lock; the owner is being registered exactly once.
    pub(crate) unsafe fn register(&self, next: usize) {
        // SAFETY: Inventory linkage is accessed under the existing pool lock.
        unsafe { *self.all_next.get() = next };
    }

    /// # Safety
    /// The caller holds the pool lock.
    pub(crate) unsafe fn next(&self) -> usize {
        // SAFETY: The pool lock serializes inventory linkage access.
        unsafe { *self.all_next.get() }
    }

    /// # Safety
    /// The caller holds the pool lock or this owner's exclusive lease.
    pub(crate) unsafe fn generation(&self) -> u64 {
        // SAFETY: The generation cannot change during a current owner lease.
        unsafe { *self.generation.get() }
    }

    /// # Safety
    /// The caller holds the pool lock and is transferring an inactive owner.
    pub(crate) unsafe fn begin_lease(&self) {
        // SAFETY: The pool lock exclusively owns lease-state updates.
        let generation = unsafe { &mut *self.generation.get() };
        debug_assert_eq!(*generation & 1, 0);
        *generation = generation.checked_add(1).unwrap_or_else(|| std::process::abort());
    }

    /// # Safety
    /// The caller holds the pool lock and the previous core action has ended.
    pub(crate) unsafe fn end_lease(&self) {
        // SAFETY: The pool lock exclusively owns lease-state updates.
        let generation = unsafe { &mut *self.generation.get() };
        // Isolated pool tests may return a freshly initialized, unleased owner.
        if *generation & 1 != 0 {
            *generation = generation.checked_add(1).unwrap_or_else(|| std::process::abort());
        }
    }

    /// Copies only the separately synchronized publication slot, never the core.
    pub(crate) fn published(&self) -> (ObservationSource, Option<Observation>) {
        let slot = self.slot.load(Ordering::Acquire);
        if slot.is_null() {
            return (
                if self.unavailable.load(Ordering::Relaxed) {
                    ObservationSource::Unavailable
                } else {
                    ObservationSource::Unobserved
                },
                None,
            );
        }
        // SAFETY: Published slots use System and live for their persistent owner's lifetime.
        let slot = unsafe { &*slot };
        match slot.value.try_lock() {
            Ok(value) => (
                if value.is_some() {
                    ObservationSource::Published
                } else {
                    ObservationSource::Unobserved
                },
                *value,
            ),
            Err(TryLockError::WouldBlock) => (ObservationSource::Busy, None),
            Err(TryLockError::Poisoned(_)) => std::process::abort(),
        }
    }

    /// # Safety
    /// The caller holds this owner's unique lease, with no core borrow retained.
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "System receives Layout::new::<Slot>(), guaranteeing Slot alignment"
    )]
    unsafe fn slot(&self) -> Option<&Slot> {
        let mut slot = self.slot.load(Ordering::Acquire);
        if slot.is_null() {
            let layout = Layout::new::<Slot>();
            // SAFETY: System bypasses GlobalAlloc and the layout describes Slot.
            slot = unsafe { System.alloc(layout) }.cast::<Slot>();
            if slot.is_null() {
                self.unavailable.store(true, Ordering::Relaxed);
                return None;
            }

            // SAFETY: Fresh System storage is aligned, writable, and exclusively initialized.
            unsafe {
                slot.write(Slot {
                    round: AtomicU64::new(0),
                    session: AtomicU64::new(0),
                    generation: AtomicU64::new(0),
                    value: Mutex::new(None),
                });
            }
            self.slot.store(slot, Ordering::Release);
        }
        // SAFETY: This initialized slot remains live for the owner's process lifetime.
        Some(unsafe { &*slot })
    }
}

/// Snapshot-source storage must bypass `GlobalAlloc`: allocating while holding
/// the owner-pool lock can reenter that lock, and would perturb native inventory.
pub(crate) struct OwnerBuffer {
    pointer: std::ptr::NonNull<seismograph_rallocator::native::Owner>,
    length: usize,
    capacity: usize,
    layout: Layout,
}

impl OwnerBuffer {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "System receives the owner array layout, guaranteeing row alignment"
    )]
    pub(crate) fn new(capacity: usize) -> Result<Self, seismograph::Error> {
        let capacity = capacity.clamp(1, OWNER_LIMIT);
        let layout = Layout::array::<seismograph_rallocator::native::Owner>(capacity)
            .map_err(|_error| seismograph::Error::new("rallocator inventory size overflow"))?;
        // SAFETY: System bypasses GlobalAlloc; layout describes an array of owner rows.
        let pointer = unsafe { System.alloc(layout) }.cast::<seismograph_rallocator::native::Owner>();
        let pointer =
            std::ptr::NonNull::new(pointer).ok_or_else(|| seismograph::Error::new("rallocator inventory storage allocation failed"))?;
        Ok(Self {
            pointer,
            length: 0,
            capacity,
            layout,
        })
    }

    pub(crate) fn full(&self) -> bool {
        self.length == self.capacity
    }

    pub(crate) fn push(&mut self, owner: &seismograph_rallocator::native::Owner) {
        assert!(!self.full(), "inventory capture checks capacity before adding an owner");
        // SAFETY: The next row lies within uniquely owned aligned System storage.
        let destination = unsafe { self.pointer.as_ptr().add(self.length) };
        // SAFETY: This uninitialized destination has room for one Copy owner row.
        unsafe { destination.write(*owner) };
        self.length += 1;
    }

    pub(crate) fn as_slice(&self) -> &[seismograph_rallocator::native::Owner] {
        // SAFETY: Exactly length Copy rows were initialized, and self owns their backing.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.length) }
    }
}

impl Drop for OwnerBuffer {
    fn drop(&mut self) {
        // SAFETY: Rows are Copy without owned allocations; System owns this exact layout.
        unsafe { System.dealloc(self.pointer.as_ptr().cast(), self.layout) };
    }
}

struct Slot {
    round: AtomicU64,
    session: AtomicU64,
    generation: AtomicU64,
    value: Mutex<Option<Observation>>,
}

/// Called only after an event was accepted and the native operation ended.
pub(crate) fn publish(session: RecordingSession) {
    if !seismograph_rallocator::native::publication_enabled() || seismograph::recorder::active_recording_session() != Some(session) {
        return;
    }
    let round = seismograph_rallocator::native::observation_round();
    crate::thread::observe_current(|owner| {
        let metadata = owner.observation();
        // SAFETY: observe_current lends this thread's existing exclusive owner lease.
        let generation = unsafe { metadata.generation() };
        // SAFETY: No native core borrow is retained across slot initialization.
        let Some(slot) = (unsafe { metadata.slot() }) else { return };
        if slot.round.load(Ordering::Relaxed) == round
            && slot.session.load(Ordering::Relaxed) == session.get()
            && slot.generation.load(Ordering::Relaxed) == generation
        {
            return;
        }
        let _suppression = SuppressionGuard::enter();
        // The accepted event already initialized recorder TLS. Native actions
        // do not run user destructors; temporary late-TLS leases are skipped by observe_current.
        let thread_id = seismograph::recorder::current_thread_id().get();
        let captured_nanos = seismograph_rallocator::native::captured_nanos();
        let mut budget = WALK_BUDGET;
        // SAFETY: The current thread holds the sole core lease; copying ends
        // before publishing, locking, serializing, or invoking any callback.
        let mut observation = unsafe { (&*owner.core_ptr()).observe(&mut budget) };
        observation.generation = generation;
        observation.session_id = session.get();
        observation.round = round;
        observation.thread_id = thread_id;
        observation.captured_nanos = captured_nanos;
        match slot.value.try_lock() {
            Ok(mut value) => {
                *value = Some(observation);
                slot.generation.store(generation, Ordering::Relaxed);
                slot.session.store(session.get(), Ordering::Relaxed);
                slot.round.store(round, Ordering::Relaxed);
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => std::process::abort(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static FAIL_NEXT_ALLOCATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub(super) struct FailingSystem;

    // SAFETY: Successful allocations and all deallocations delegate to System with unchanged layouts.
    unsafe impl GlobalAlloc for FailingSystem {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if FAIL_NEXT_ALLOCATION.with(|fail| fail.replace(false)) {
                std::ptr::null_mut()
            } else {
                // SAFETY: GlobalAlloc's caller supplies a valid layout.
                unsafe { std::alloc::System.alloc(layout) }
            }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: Every nonnull allocation originated from System with this layout.
            unsafe { std::alloc::System.dealloc(ptr, layout) };
        }
    }

    #[test]
    fn observation_storage_failure_stays_unavailable_and_can_retry() {
        let metadata = Metadata::new();
        FAIL_NEXT_ALLOCATION.with(|fail| fail.set(true));
        // SAFETY: This test uniquely owns the metadata lease.
        assert!(unsafe { metadata.slot() }.is_none());
        assert!(metadata.slot.load(Ordering::Acquire).is_null());
        assert_eq!(metadata.published(), (ObservationSource::Unavailable, None));
        // SAFETY: The test retains the same unique metadata lease.
        assert!(unsafe { metadata.slot() }.is_some());
        assert_eq!(metadata.published(), (ObservationSource::Unobserved, None));
        let pointer = metadata.slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: No publication reader or slot borrow remains; storage came from System.
        unsafe { pointer.drop_in_place() };
        // SAFETY: The slot's value was dropped above and its allocation has the original layout.
        unsafe { System.dealloc(pointer.cast(), Layout::new::<Slot>()) };

        FAIL_NEXT_ALLOCATION.with(|fail| fail.set(true));
        let error = OwnerBuffer::new(1).err().unwrap();
        assert!(error.to_string().contains("rallocator inventory storage allocation failed"));
        drop(OwnerBuffer::new(1).unwrap());
    }

    #[test]
    fn publication_distinguishes_missing_empty_busy_and_unavailable_slots() {
        let metadata = Metadata::new();
        assert_eq!(metadata.published(), (ObservationSource::Unobserved, None));
        metadata.unavailable.store(true, Ordering::Relaxed);
        assert_eq!(metadata.published(), (ObservationSource::Unavailable, None));
        let mut slot = Box::new(Slot {
            round: AtomicU64::new(0),
            session: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            value: Mutex::new(None),
        });
        metadata.slot.store(&raw mut *slot, Ordering::Release);
        assert_eq!(metadata.published(), (ObservationSource::Unobserved, None));
        let held = slot.value.lock().unwrap();
        assert_eq!(metadata.published(), (ObservationSource::Busy, None));
        drop(held);
        *slot.value.lock().unwrap() = Some(Observation::default());
        assert_eq!(metadata.published(), (ObservationSource::Published, Some(Observation::default())));
        metadata.slot.store(std::ptr::null_mut(), Ordering::Release);
    }
}
