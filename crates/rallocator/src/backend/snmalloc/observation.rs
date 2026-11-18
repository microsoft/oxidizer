// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Optional, owner-published copies. Never inspect another leased core.

#[cfg(not(test))]
use std::alloc::System;
use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
#[cfg(not(target_os = "macos"))]
use std::sync::Mutex;
use std::sync::TryLockError;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

use seismograph::recorder::event::EventClass;
use seismograph::recorder::{is_enabled, without_recording};
use seismograph_rallocator::native::{Observation, ObservationSource};

#[cfg(test)]
use self::tests::FailingSystem as System;
#[cfg(target_os = "macos")]
use crate::backend::snmalloc::hal::Mutex;

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

fn next_generation(generation: u64) -> u64 {
    generation
        .checked_add(1)
        .unwrap_or_else(|| crate::backend::snmalloc::abort::abort())
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
    /// The caller holds the pool lock or this owner's exclusive lease,
    /// or exclusively owns unpublished metadata.
    pub(crate) unsafe fn generation(&self) -> u64 {
        // SAFETY: The caller excludes concurrent lease-state updates.
        unsafe { *self.generation.get() }
    }

    /// # Safety
    /// The caller holds the pool lock or exclusively owns unpublished metadata.
    /// The owner is inactive.
    pub(crate) unsafe fn begin_lease(&self) {
        // SAFETY: The caller guarantees exclusive access to lease-state updates.
        let generation = unsafe { &mut *self.generation.get() };
        debug_assert_eq!(*generation & 1, 0);
        *generation = next_generation(*generation);
    }

    /// # Safety
    /// The caller holds the pool lock or exclusively owns unpublished metadata.
    /// The previous core action has ended.
    pub(crate) unsafe fn end_lease(&self) {
        // SAFETY: The caller guarantees exclusive access to lease-state updates.
        let generation = unsafe { &mut *self.generation.get() };
        // Isolated pool tests may return a freshly initialized, unleased owner.
        if *generation & 1 != 0 {
            *generation = next_generation(*generation);
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
            Err(TryLockError::Poisoned(_)) => crate::backend::snmalloc::abort::abort(),
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
        // #[gamma::skip(try.propagate_to_unwrap, literal.str_to_empty, literal.str_to_xyzzy, reason = "Capacity is clamped to at most 1024 fixed-size native Owner records, so this layout cannot overflow on the supported 64-bit targets")]
        let layout = Layout::array::<seismograph_rallocator::native::Owner>(capacity)
            .expect("capacity is clamped to OWNER_LIMIT, whose owner array fits within isize::MAX");
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
    generation: AtomicU64,
    value: Mutex<Option<Observation>>,
}

fn already_published(slot: &Slot, round: u64, generation: u64) -> bool {
    slot.round.load(Ordering::Relaxed) == round && slot.generation.load(Ordering::Relaxed) == generation
}

fn stamp_observation(observation: &mut Observation, generation: u64, session: u64, round: u64, thread_id: u64, captured_nanos: u64) {
    observation.generation = generation;
    observation.session_id = session;
    observation.round = round;
    observation.thread_id = thread_id;
    observation.captured_nanos = captured_nanos;
}

/// Publishes at most once per lease and requested round after a native operation.
pub(crate) fn publish() {
    if !seismograph_rallocator::native::publication_enabled() || !is_enabled(EventClass::Allocation) {
        return;
    }
    let round = seismograph_rallocator::native::observation_round();
    crate::backend::snmalloc::thread::observe_current(|owner| {
        let metadata = owner.observation();
        // SAFETY: observe_current lends this thread's existing exclusive owner lease.
        let generation = unsafe { metadata.generation() };
        // SAFETY: No native core borrow is retained across slot initialization.
        let Some(slot) = (unsafe { metadata.slot() }) else { return };
        if already_published(slot, round, generation) {
            return;
        }
        without_recording(|| {
            // Metadata follows observation rounds, not event sampling or recorder epochs.
            let thread_id = seismograph::recorder::thread_id().get();
            let captured_nanos = seismograph_rallocator::native::captured_nanos();
            let mut budget = WALK_BUDGET;
            // SAFETY: The current thread holds the sole core lease; copying ends
            // before publishing, locking, serializing, or invoking any callback.
            let mut observation = unsafe { (&*owner.core_ptr()).observe(&mut budget) };
            stamp_observation(&mut observation, generation, 0, round, thread_id, captured_nanos);
            match slot.value.try_lock() {
                Ok(mut value) => {
                    *value = Some(observation);
                    slot.generation.store(generation, Ordering::Relaxed);
                    slot.round.store(round, Ordering::Relaxed);
                }
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Poisoned(_)) => crate::backend::snmalloc::abort::abort(),
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn gamma_round2_publication_guards_compare_generation_and_round_independently() {
        use super::*;

        let slot = Slot {
            round: AtomicU64::new(13),
            generation: AtomicU64::new(19),
            value: Mutex::new(None),
        };
        for (round, generation, expected) in [(13, 19, true), (23, 19, false), (13, 23, false), (23, 31, false)] {
            assert_eq!(already_published(&slot, round, generation), expected);
        }
    }

    #[test]
    fn gamma_round2_observation_context_uses_supplied_values_without_changing_payload() {
        use super::*;

        for captured in [0, 123] {
            let mut observation = Observation::EMPTY;
            observation.large.counts[9] = 37;
            observation.remote.budget_remaining = 41;
            observation.classes[1].capacity = 43;
            stamp_observation(&mut observation, 3, 7, 11, 13, captured);
            assert_eq!(
                (
                    observation.generation,
                    observation.session_id,
                    observation.round,
                    observation.thread_id,
                    observation.captured_nanos
                ),
                (3, 7, 11, 13, captured)
            );
            assert_eq!(observation.large.counts[9], 37);
            assert_eq!(observation.remote.budget_remaining, 41);
            assert_eq!(observation.classes[1].capacity, 43);
        }
    }

    #[test]
    fn gamma_survivor_metadata_generations_and_owner_buffer_capacity() {
        use seismograph_rallocator::native::ObservationSource;

        use super::*;

        let metadata = Metadata::new();
        assert_eq!(metadata.published(), (ObservationSource::Unobserved, None));
        // SAFETY: This standalone metadata is exclusively test-owned; it is never published.
        unsafe { metadata.register(0x4000) };
        // SAFETY: The standalone metadata's next field has no concurrent access.
        assert_eq!(unsafe { metadata.next() }, 0x4000);
        // SAFETY: The standalone metadata has no concurrent lease changes.
        assert_eq!(unsafe { metadata.generation() }, 0);
        // SAFETY: The exclusively held metadata starts inactive.
        unsafe { metadata.end_lease() };
        // SAFETY: No other lease holder can change this standalone generation.
        assert_eq!(unsafe { metadata.generation() }, 0);
        // SAFETY: This test is the metadata's sole lease holder.
        unsafe { metadata.begin_lease() };
        // SAFETY: The generation is read only by its sole lease holder.
        assert_eq!(unsafe { metadata.generation() }, 1);
        // SAFETY: The test ends the lease it just began.
        unsafe { metadata.end_lease() };
        // SAFETY: The standalone metadata cannot be concurrently reused.
        assert_eq!(unsafe { metadata.generation() }, 2);
        // SAFETY: No other thread can acquire this standalone metadata.
        unsafe { metadata.begin_lease() };
        // SAFETY: This is the metadata's sole active lease.
        assert_eq!(unsafe { metadata.generation() }, 3);
        // SAFETY: This test ends its final standalone lease.
        unsafe { metadata.end_lease() };
        for capacity in [0, 1, 3, OWNER_LIMIT, OWNER_LIMIT + 1, usize::MAX] {
            let mut buffer = OwnerBuffer::new(capacity).unwrap();
            let expected = capacity.clamp(1, OWNER_LIMIT);
            assert_eq!(
                buffer.layout.size(),
                expected * std::mem::size_of::<seismograph_rallocator::native::Owner>()
            );
            assert!(buffer.as_slice().is_empty());
            assert!(!buffer.full());
            for index in 0..expected {
                let row = seismograph_rallocator::native::Owner {
                    id: index as u64,
                    generation: 2,
                    leased: false,
                    source: ObservationSource::Unobserved,
                    observation: None,
                };
                buffer.push(&row);
                assert_eq!(buffer.as_slice().last().unwrap(), &row);
                assert_eq!(buffer.full(), index + 1 == expected);
            }
            assert_eq!(buffer.as_slice().len(), expected);
        }
        // SAFETY: The standalone metadata is exclusively owned and no other thread accesses its slot.
        let slot = unsafe { metadata.slot() }.unwrap();
        assert_eq!(slot.round.load(Ordering::Relaxed), 0);
        assert_eq!(slot.generation.load(Ordering::Relaxed), 0);
        assert!(slot.value.lock().unwrap().is_none());
        let pointer = metadata.slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: The slot borrow ended and no publication readers exist.
        unsafe { pointer.drop_in_place() };
        // SAFETY: The original System allocation layout is preserved after its value was dropped.
        unsafe { System.dealloc(pointer.cast(), Layout::new::<Slot>()) };
    }

    use super::*;

    // SAFETY: These tests keep every block within this linked snmalloc instance.
    const ALLOCATOR: crate::Rallocator = unsafe { crate::Rallocator::new(crate::backend::Snmalloc) };

    #[test]
    fn an_unleased_owner_can_be_returned_before_its_first_lease() {
        let metadata = Metadata::new();

        // SAFETY: This isolated metadata has no concurrent users or live core action.
        unsafe {
            metadata.end_lease();
        }
        // SAFETY: The metadata still has no concurrent users or live core action.
        unsafe {
            metadata.begin_lease();
        }

        // SAFETY: The test exclusively owns this metadata.
        assert_eq!(unsafe { metadata.generation() } & 1, 1);
    }

    #[test]
    fn inventory_buffer_stops_exactly_at_its_capacity() {
        for capacity in [1, 2] {
            let mut buffer = OwnerBuffer::new(capacity).unwrap();
            for id in 1..=capacity {
                assert!(!buffer.full());
                buffer.push(&seismograph_rallocator::native::Owner {
                    id: id as u64,
                    ..Default::default()
                });
            }
            assert!(buffer.full());
            assert_eq!(buffer.as_slice().len(), capacity);
            assert_eq!(buffer.as_slice()[capacity - 1].id, capacity as u64);
            let layout = buffer.layout;
            let pointer = buffer.pointer.as_ptr().addr();
            drop(buffer);
            assert_eq!(LAST_DEALLOCATION.with(std::cell::Cell::get), Some((pointer, layout)));
        }
    }

    #[test]
    fn contended_publisher_preserves_the_previous_round_and_retries_after_unlock() {
        const CASE: &str = "RALLOCATOR_PUBLICATION_CASE";
        if std::env::var_os(CASE).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "backend::snmalloc::observation::tests::contended_publisher_preserves_the_previous_round_and_retries_after_unlock",
                ])
                .env(CASE, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        seismograph_rallocator::native::set_publication_enabled(true);
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::Policy::all(true),
            ..Default::default()
        });
        let layout = Layout::from_size_align(32, 16).unwrap();
        // SAFETY: The layout is valid and nonzero.
        let pointer = unsafe { ALLOCATOR.alloc(layout) };
        assert!(!pointer.is_null());
        let mut observed = false;
        crate::backend::snmalloc::thread::observe_current(|owner| {
            observed = true;
            // SAFETY: The existing current-thread lease remains live throughout
            // this callback, which retains no core borrow across publication.
            let slot = unsafe { owner.observation().slot() }.unwrap();
            let original = *slot.value.lock().unwrap();
            let second_layout = Layout::from_size_align(64, 16).unwrap();
            // SAFETY: The second allocation has a valid independent layout.
            let second = unsafe { ALLOCATOR.alloc(second_layout) };
            assert!(!second.is_null());
            assert_eq!(*slot.value.lock().unwrap(), original);
            // SAFETY: This distinct allocation is uniquely owned and retired once.
            unsafe { ALLOCATOR.dealloc(second, second_layout) };
            assert_eq!(*slot.value.lock().unwrap(), original);
            let previous = slot.value.lock().unwrap();
            let round = slot.round.load(Ordering::Relaxed);
            let generation = slot.generation.load(Ordering::Relaxed);
            assert_eq!(previous.as_ref().unwrap().round, round);
            seismograph_rallocator::native::request_observation();
            publish();
            assert_eq!(slot.round.load(Ordering::Relaxed), round);
            assert_eq!(slot.generation.load(Ordering::Relaxed), generation);
            assert_eq!(previous.as_ref().unwrap().round, round);
            drop(previous);
            publish();
            assert_eq!(
                slot.round.load(Ordering::Relaxed),
                seismograph_rallocator::native::observation_round()
            );
            assert_ne!(slot.round.load(Ordering::Relaxed), round);
            assert_eq!(
                slot.value.lock().unwrap().as_ref().unwrap().round,
                slot.round.load(Ordering::Relaxed)
            );
        });
        assert!(observed);
        // SAFETY: The allocation is exclusively owned and has no remaining accesses.
        unsafe { ALLOCATOR.dealloc(pointer, layout) };
    }

    fn poison_slot(slot: &Slot) {
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = slot.value.lock().unwrap();
            panic!("poison an exclusively owned publication slot");
        }));
        assert!(poisoned.is_err());
        assert!(slot.value.is_poisoned());
    }

    #[test]
    fn poisoned_publication_copy_aborts_instead_of_reporting_empty_state() {
        crate::backend::snmalloc::abort::assert_aborts(
            "backend::snmalloc::observation::tests::poisoned_publication_copy_aborts_instead_of_reporting_empty_state",
            || {
                let metadata = Metadata::new();
                // SAFETY: This subprocess exclusively owns the metadata and its slot through termination.
                let slot = unsafe { metadata.slot() }.unwrap();
                poison_slot(slot);
                metadata.published();
            },
        );
    }

    #[test]
    fn poisoned_publisher_aborts_instead_of_overwriting_failed_state() {
        crate::backend::snmalloc::abort::assert_aborts(
            "backend::snmalloc::observation::tests::poisoned_publisher_aborts_instead_of_overwriting_failed_state",
            || {
                seismograph_rallocator::native::set_publication_enabled(true);
                seismograph::recorder(seismograph::recorder::Configuration {
                    allocations: seismograph::recorder::Policy::all(true),
                    ..Default::default()
                });
                // SAFETY: The layout is valid and nonzero.
                let pointer = unsafe { ALLOCATOR.alloc(Layout::from_size_align(32, 16).unwrap()) };
                assert!(!pointer.is_null());
                crate::backend::snmalloc::thread::observe_current(|owner| {
                    // SAFETY: The callback holds this existing owner's exclusive lease, outside its core action.
                    let slot = unsafe { owner.observation().slot() }.unwrap();
                    poison_slot(slot);
                });
                seismograph_rallocator::native::request_observation();
                publish();
            },
        );
    }

    #[test]
    fn generation_overflow_aborts_instead_of_reusing_a_lease_identity() {
        crate::backend::snmalloc::abort::assert_aborts(
            "backend::snmalloc::observation::tests::generation_overflow_aborts_instead_of_reusing_a_lease_identity",
            || {
                next_generation(u64::MAX);
            },
        );
        assert_eq!(next_generation(u64::MAX - 1), u64::MAX);
    }

    thread_local! {
        static FAIL_NEXT_ALLOCATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static LAST_DEALLOCATION: std::cell::Cell<Option<(usize, Layout)>> = const { std::cell::Cell::new(None) };
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
            LAST_DEALLOCATION.with(|last| last.set(Some((ptr.addr(), layout))));
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
