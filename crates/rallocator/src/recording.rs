// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stateless event bridge. The recorder owns policy and suppression.

use std::alloc::Layout;

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, EventClass, Record};

static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    seismograph_rallocator::source::ID,
    seismograph_rallocator::source::NAME,
    seismograph_rallocator::source::SCHEMA_VERSION,
    capture,
);

pub(crate) fn register_source() {
    // Owner creation already holds the pool lock. Source registration performs
    // only atomic linkage and cannot reenter the allocator.
    seismograph::snapshot::register_source(&SOURCE);
}

#[inline]
pub(crate) fn allocate(layout: Layout, zeroed: bool) -> *mut u8 {
    // Keep the recorder's closure environment and register saves off the
    // disabled path; the enabled body still uses the lazy record closure.
    if seismograph::recorder::recording_enabled_for(EventClass::Allocation) {
        return allocate_recorded(layout, zeroed);
    }
    allocate_core(layout, zeroed)
}

#[inline]
fn allocate_core(layout: Layout, zeroed: bool) -> *mut u8 {
    if zeroed {
        crate::thread::allocate_zeroed(layout)
    } else {
        crate::thread::allocate(layout)
    }
}

#[inline(never)]
fn allocate_recorded(layout: Layout, zeroed: bool) -> *mut u8 {
    let address = allocate_core(layout, zeroed);
    if let Some(session) = emit(address, layout.size(), layout.align(), false) {
        crate::observation::publish(session);
    }
    address
}

/// # Safety
/// `address` is a live allocation with `layout` from this allocator.
#[inline]
pub(crate) unsafe fn deallocate(address: *mut u8, layout: Layout) {
    if seismograph::recorder::recording_enabled_for(EventClass::Allocation) {
        // SAFETY: The caller transfers a live allocation for destruction.
        return unsafe { deallocation_recorded(address, layout) };
    }
    // SAFETY: The caller transfers a live allocation for destruction.
    unsafe { crate::thread::deallocate(address) };
}

#[inline(never)]
unsafe fn deallocation_recorded(address: *mut u8, layout: Layout) {
    let session = emit(address, layout.size(), layout.align(), true);
    // SAFETY: The caller transfers a live allocation for destruction.
    unsafe { crate::thread::deallocate(address) };
    if let Some(session) = session {
        crate::observation::publish(session);
    }
}

/// # Safety
/// `address` is live for `layout`, and `new_size` satisfies `GlobalAlloc`'s contract.
#[inline]
pub(crate) unsafe fn reallocate(address: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    if seismograph::recorder::recording_enabled_for(EventClass::Allocation) {
        // SAFETY: The caller's live allocation and resize contract are forwarded.
        return unsafe { reallocate_recorded(address, layout, new_size) };
    }
    // SAFETY: The caller's live allocation and resize contract are forwarded.
    unsafe { crate::reallocate_core(address, layout, new_size) }
}

#[inline(never)]
unsafe fn reallocate_recorded(address: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    // SAFETY: The caller's live allocation and resize contract are forwarded.
    let replacement = unsafe { crate::reallocate_core(address, layout, new_size) };
    if !replacement.is_null() && new_size != layout.size() {
        let freed = emit(address, layout.size(), layout.align(), true);
        let allocated = emit(replacement, new_size, layout.align(), false);
        if let Some(session) = allocated.or(freed) {
            crate::observation::publish(session);
        }
    }
    replacement
}

#[inline]
fn emit(address: *mut u8, size: usize, alignment: usize, freed: bool) -> Option<seismograph::recorder::RecordingSession> {
    seismograph::record_session(EventClass::Allocation, || {
        if address.is_null() {
            return None;
        }
        let allocation = Allocation {
            // This is an address correlation key, not a unique lifetime ID.
            allocation_id: AllocationId::new(address.addr() as u64),
            // The container already records the actor. Zero means no separate
            // allocator thread or heap identity is available.
            event_thread_id: EventThreadId::new(0),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::from_ptr(address),
            size: size as u64,
            alignment: alignment as u64,
        };
        Some(if freed {
            Record::deallocation(allocation)
        } else {
            Record::allocation(allocation)
        })
    })
}

fn capture(context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let _suppression = seismograph::recorder::SuppressionGuard::enter();
    let round = seismograph_rallocator::native::observation_round();
    let captured_nanos = seismograph_rallocator::native::captured_nanos();
    let (owners, owner_count, owners_complete) = crate::thread::inventory(round, captured_nanos)?;
    // Pool collection has ended before taking the global backend lock; owner
    // creation takes pool -> backend, so the collector must not reverse that order.
    let global = crate::backend::observe();
    let snapshot = seismograph_rallocator::native::Snapshot {
        captured_nanos: seismograph_rallocator::native::captured_nanos(),
        session_id: context.recording_observation().map_or(0, |observation| observation.session.get()),
        round,
        owner_count,
        owners_complete,
        publication_enabled: seismograph_rallocator::native::publication_enabled(),
        owners: Vec::new(),
        global,
    };
    let len = seismograph_rallocator::encoded_len_with_owners(&snapshot, owners.as_slice())
        .map_err(|_error| seismograph::Error::new("rallocator source sizing failed"))?;
    let mut data = seismograph::snapshot::SourceData::zeroed(len)?;
    seismograph_rallocator::encode_with_owners(&snapshot, owners.as_slice(), data.as_mut_bytes())
        .map_err(|_error| seismograph::Error::new("rallocator source encoding failed"))?;
    // Existing monitor/snapshot polling supplies the next cooperative round.
    seismograph_rallocator::native::request_observation();
    Ok(data)
}
