// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Diagnostic copies live outside allocator heaps and page metadata.
//!
//! The bounded descriptor table survives thread exit. Collectors read only
//! synchronized copies and immutable descriptors, never another thread's TLS.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, UnsafeCell};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use seismograph::recorder::event::EventClass;
use seismograph::recorder::{is_enabled, without_recording};
use seismograph_rallocator::mimalloc::{Observation, ObservationSource, Owner};
use seismograph_rallocator::snapshot::Header;

use crate::backend::abort::require;
use crate::backend::mimalloc::{heap, memory};

pub(super) const WALK_BUDGET: usize = 4096;
const OWNER_LIMIT: usize = 1024;
const UNREGISTERED: usize = usize::MAX;
const OMITTED: usize = usize::MAX - 1;

struct Metadata {
    id: AtomicUsize,
    active: AtomicBool,
    slot: AtomicPtr<Slot>,
    unavailable: AtomicBool,
}

impl Metadata {
    const fn new() -> Self {
        Self {
            id: AtomicUsize::new(0),
            active: AtomicBool::new(false),
            slot: AtomicPtr::new(std::ptr::null_mut()),
            unavailable: AtomicBool::new(false),
        }
    }

    fn published(&self) -> (ObservationSource, Option<Observation>) {
        let pointer = self.slot.load(Ordering::Acquire);
        if pointer.is_null() {
            return (
                if self.unavailable.load(Ordering::Relaxed) {
                    ObservationSource::Unavailable
                } else {
                    ObservationSource::Unobserved
                },
                None,
            );
        }
        // SAFETY: An initialized slot persists for this process's lifetime.
        let slot = unsafe { &*pointer };
        let Some(_guard) = slot.try_lock() else {
            return (ObservationSource::Busy, None);
        };
        // SAFETY: The guard excludes every writer until this Copy read ends.
        let value = unsafe { *slot.value.get() };
        (
            if value.is_some() {
                ObservationSource::Published
            } else {
                ObservationSource::Unobserved
            },
            value,
        )
    }

    /// # Safety
    /// Only the registered owning thread can initialize this slot.
    unsafe fn slot(&self) -> Option<&Slot> {
        let allocate = |layout| {
            // SAFETY: slot_with supplies the valid nonzero Slot layout.
            unsafe { System.alloc(layout) }
        };
        // SAFETY: The only initializer is this metadata's owning thread.
        unsafe { self.slot_with(allocate) }
    }

    /// # Safety
    /// The caller uniquely owns initialization. Non-null storage returned by
    /// allocate must be owned, writable, and valid for the supplied layout.
    #[expect(clippy::cast_ptr_alignment, reason = "The allocator receives Layout::new::<Slot>()")]
    unsafe fn slot_with(&self, allocate: impl FnOnce(Layout) -> *mut u8) -> Option<&Slot> {
        let mut pointer = self.slot.load(Ordering::Acquire);
        if pointer.is_null() {
            pointer = allocate(Layout::new::<Slot>()).cast::<Slot>();
            if pointer.is_null() {
                self.unavailable.store(true, Ordering::Relaxed);
                return None;
            }
            // SAFETY: The supplied allocation uniquely owns aligned Slot storage.
            unsafe { pointer.write(Slot::new()) };
            self.slot.store(pointer, Ordering::Release);
        }
        // SAFETY: The slot is initialized and is not reclaimed while published.
        Some(unsafe { &*pointer })
    }
}

// A try-only guard is intentional: neither a collector nor a publisher waits
// for the other. It also avoids platform mutex initialization recursing into
// the global allocator. The protected value is copied, never lent to callers.
struct Slot {
    busy: AtomicBool,
    value: UnsafeCell<Option<Observation>>,
    round: AtomicU64,
}

// SAFETY: All accesses to value hold the acquire/release guard. Other fields
// are atomic, and slot initialization precedes publication.
unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            value: UnsafeCell::new(None),
            round: AtomicU64::new(0),
        }
    }

    fn try_lock(&self) -> Option<Guard<'_>> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Guard(self))
    }

    fn publish(&self, observation: &Observation) -> bool {
        let Some(_guard) = self.try_lock() else { return false };
        // SAFETY: The guard excludes every reader and writer until this copy ends.
        unsafe { *self.value.get() = Some(*observation) };
        self.round.store(observation.round, Ordering::Relaxed);
        true
    }
}

struct Guard<'a>(&'a Slot);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

static METADATA: [Metadata; OWNER_LIMIT] = [const { Metadata::new() }; OWNER_LIMIT];
static OWNER_COUNT: AtomicUsize = AtomicUsize::new(0);

std::thread_local! {
    static OWNER: Cell<usize> = const { Cell::new(UNREGISTERED) };
}

static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    seismograph_rallocator::source::MIMALLOC_ID,
    seismograph_rallocator::source::MIMALLOC_NAME,
    seismograph_rallocator::snapshot::SCHEMA_VERSION,
    capture,
);

pub(super) fn register(id: usize) {
    OWNER.with(|owner| {
        if owner.get() != UNREGISTERED {
            return;
        }
        let index = OWNER_COUNT.fetch_add(1, Ordering::AcqRel);
        require(index < usize::MAX - 1);
        owner.set(register_at(&METADATA, index, id));
        seismograph::snapshot::register_source(&SOURCE);
    });
}

fn register_at(metadata: &[Metadata], index: usize, id: usize) -> usize {
    let Some(metadata) = metadata.get(index) else {
        // Header coverage exposes every lifetime omitted by the fixed bound.
        return OMITTED;
    };
    metadata.active.store(true, Ordering::Relaxed);
    metadata.id.store(id, Ordering::Release);
    index
}

pub(super) fn retire() {
    // Scalar TLS has no destructor. An unavailable key cannot retain a descriptor.
    if let Ok(index) = OWNER.try_with(Cell::get)
        && let Some(metadata) = METADATA.get(index)
    {
        metadata.active.store(false, Ordering::Release);
    }
}

pub(super) fn publish() {
    if !seismograph_rallocator::snapshot::publication_enabled() || !is_enabled(EventClass::Allocation) {
        return;
    }
    without_recording(publish_current);
}

fn publish_current() {
    let Ok(index) = OWNER.try_with(Cell::get) else { return };
    let Some(metadata) = METADATA.get(index) else { return };
    if !metadata.active.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: OWNER identifies this thread's unique heap lifetime.
    let Some(slot) = (unsafe { metadata.slot() }) else { return };
    let round = seismograph_rallocator::snapshot::observation_round();
    if slot.round.load(Ordering::Relaxed) == round {
        return;
    }
    let mut observation = Observation {
        session_id: 0,
        round,
        thread_id: seismograph::recorder::thread_id().get(),
        captured_nanos: seismograph_rallocator::snapshot::captured_nanos(),
        ..Observation::EMPTY
    };
    if !heap::observe_current(&mut observation) {
        return;
    }
    // Busy publication is optional. Its round stays unchanged so the next
    // enabled operation retries without blocking the allocator.
    slot.publish(&observation);
}

struct OwnerBuffer {
    pointer: NonNull<Owner>,
    length: usize,
    capacity: usize,
    layout: Layout,
}

impl OwnerBuffer {
    #[expect(clippy::cast_ptr_alignment, reason = "System is passed the exact Owner array layout")]
    fn new(capacity: usize) -> Result<Self, seismograph::Error> {
        let capacity = capacity.clamp(1, OWNER_LIMIT);
        let layout =
            Layout::array::<Owner>(capacity).map_err(|_overflow| seismograph::Error::new("mimalloc owner buffer size overflow"))?;
        // SAFETY: The valid array layout bypasses the selected global allocator.
        let pointer = unsafe { System.alloc(layout) }.cast::<Owner>();
        let pointer = NonNull::new(pointer).ok_or_else(|| seismograph::Error::new("mimalloc owner buffer allocation failed"))?;
        Ok(Self {
            pointer,
            length: 0,
            capacity,
            layout,
        })
    }

    fn push(&mut self, value: &Owner) {
        require(self.length < self.capacity);
        // SAFETY: length identifies the next writable slot in the owned array.
        let destination = unsafe { self.pointer.as_ptr().add(self.length) };
        // SAFETY: The destination is aligned, uninitialized Owner storage.
        unsafe { destination.write(*value) };
        self.length += 1;
    }

    fn as_slice(&self) -> &[Owner] {
        // SAFETY: Exactly length rows are initialized and remain owned by self.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.length) }
    }
}

impl Drop for OwnerBuffer {
    fn drop(&mut self) {
        // SAFETY: Owner is Copy with no owned resources. System owns this layout.
        unsafe { System.dealloc(self.pointer.as_ptr().cast(), self.layout) };
    }
}

fn capture(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    without_recording(capture_state)
}

fn capture_state() -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let total = OWNER_COUNT.load(Ordering::Acquire);
    let owners = inventory(&METADATA, total)?;
    let header = Header {
        captured_nanos: seismograph_rallocator::snapshot::captured_nanos(),
        session_id: 0,
        round: seismograph_rallocator::snapshot::observation_round(),
        owner_count: total as u64,
        owners_complete: owners.length == total,
        publication_enabled: seismograph_rallocator::snapshot::publication_enabled(),
    };
    let global = memory::observe(WALK_BUDGET);
    let len = seismograph_rallocator::snapshot::encoded_len_mimalloc(&header, &global, owners.as_slice())
        .expect("bounded inventory emits unique nonzero persistent owners and valid allocator observations");
    let mut data = seismograph::snapshot::SourceData::zeroed(len)?;
    seismograph_rallocator::snapshot::encode_mimalloc(&header, &global, owners.as_slice(), data.as_mut_bytes())
        .expect("output length was computed from the same immutable snapshot and owner rows");
    seismograph_rallocator::snapshot::request_observation();
    Ok(data)
}

fn inventory(metadata: &[Metadata], total: usize) -> Result<OwnerBuffer, seismograph::Error> {
    let mut owners = OwnerBuffer::new(total)?;
    for metadata in metadata.iter().take(total) {
        let id = metadata.id.load(Ordering::Acquire);
        if id == 0 {
            continue; // Registration is in flight; header coverage remains partial.
        }
        let (source, observation) = metadata.published();
        owners.push(&Owner {
            id: id as u64,
            active: metadata.active.load(Ordering::Acquire),
            source,
            observation,
        });
    }
    Ok(owners)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_registration_preserves_identity_and_retirement_prevents_publication() {
        std::thread::spawn(|| {
            register(usize::MAX);
            let index = OWNER.get();
            register(usize::MAX - 16);
            let metadata = &METADATA[index];
            assert_eq!((OWNER.get(), metadata.id.load(Ordering::Acquire)), (index, usize::MAX));

            retire();
            publish_current();

            assert!(!metadata.active.load(Ordering::Acquire));
            assert!(metadata.slot.load(Ordering::Acquire).is_null());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn registration_without_an_available_heap_does_not_publish_empty_state() {
        std::thread::spawn(|| {
            register(usize::MAX - 32);
            let metadata = &METADATA[OWNER.get()];

            publish_current();

            assert!(!metadata.slot.load(Ordering::Acquire).is_null());
            assert_eq!(metadata.published(), (ObservationSource::Unobserved, None));
            retire();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn bounded_inventory_preserves_registered_owners_and_exposes_registration_gaps() {
        let metadata = [const { Metadata::new() }; 2];
        assert_eq!(register_at(&metadata, 1, 16), 1);
        assert_eq!(register_at(&metadata, 2, 32), OMITTED);

        let rows = inventory(&metadata, 3).unwrap();

        assert_eq!(
            rows.as_slice(),
            [Owner {
                id: 16,
                active: true,
                source: ObservationSource::Unobserved,
                observation: None
            }]
        );
        assert_eq!(rows.length, 1);
    }

    #[test]
    fn publication_reports_contention_without_destroying_the_previous_copy() {
        let metadata = Metadata::new();
        let mut slot = Box::new(Slot::new());
        metadata.slot.store(&raw mut *slot, Ordering::Release);
        let first = Observation {
            session_id: 7,
            round: 11,
            ..Observation::EMPTY
        };
        let second = Observation {
            session_id: 7,
            round: 12,
            ..Observation::EMPTY
        };
        assert!(slot.publish(&first));

        let guard = slot.try_lock().unwrap();
        assert_eq!(metadata.published(), (ObservationSource::Busy, None));
        assert!(!slot.publish(&second));
        assert_eq!(slot.round.load(Ordering::Relaxed), 11);
        drop(guard);
        assert_eq!(metadata.published(), (ObservationSource::Published, Some(first)));
        assert!(slot.publish(&second));
        assert_eq!(metadata.published(), (ObservationSource::Published, Some(second)));
        metadata.slot.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn failed_slot_allocation_is_explicit_and_a_later_attempt_can_succeed() {
        let metadata = Metadata::new();
        // SAFETY: This fixture owns initialization and intentionally returns no storage.
        assert!(unsafe { metadata.slot_with(|_| std::ptr::null_mut()) }.is_none());
        assert_eq!(metadata.published(), (ObservationSource::Unavailable, None));

        // SAFETY: The same fixture retains exclusive initialization ownership.
        assert!(unsafe { metadata.slot() }.is_some());
        assert_eq!(metadata.published(), (ObservationSource::Unobserved, None));
        let pointer = metadata.slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: No publication reader or slot reference remains in this test.
        unsafe { pointer.drop_in_place() };
        // SAFETY: The slot originated from System with this exact layout.
        unsafe { System.dealloc(pointer.cast(), Layout::new::<Slot>()) };
    }

    #[test]
    fn inventory_storage_copies_only_initialized_owner_rows() {
        let mut buffer = OwnerBuffer::new(2).unwrap();
        let rows = [
            Owner {
                id: 16,
                ..Owner::default()
            },
            Owner {
                id: 32,
                ..Owner::default()
            },
        ];
        buffer.push(&rows[0]);
        buffer.push(&rows[1]);

        assert_eq!(buffer.as_slice(), rows);
    }

    #[test]
    fn concurrent_publication_never_exposes_a_partially_copied_observation() {
        let mut slot = Box::new(Slot::new());
        let metadata = Metadata::new();
        metadata.slot.store(&raw mut *slot, Ordering::Release);
        assert!(slot.publish(&Observation::EMPTY));
        std::thread::scope(|scope| {
            let slot = &*slot;
            scope.spawn(move || {
                for round in 1..=16 {
                    let mut observation = Observation {
                        round,
                        ..Observation::EMPTY
                    };
                    observation.classes[8].used_blocks = round;
                    slot.publish(&observation);
                }
            });
            for _ in 0..16 {
                let observed = metadata.published().1.map(|value| (value.round, value.classes[8].used_blocks));
                assert!(observed.is_none_or(|(round, used)| round == used));
            }
        });
        let (source, value) = metadata.published();
        assert_eq!(source, ObservationSource::Published);
        let value = value.unwrap();
        assert_eq!(value.round, value.classes[8].used_blocks);
        metadata.slot.store(std::ptr::null_mut(), Ordering::Release);
    }
}
