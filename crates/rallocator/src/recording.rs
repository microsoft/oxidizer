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
    let release = |pointer| {
        // SAFETY: Successful replacement transfers the still-live old allocation.
        unsafe { crate::thread::deallocate(pointer) };
    };
    // SAFETY: The caller supplies a live allocation, retired once by the callback.
    unsafe { reallocate_recorded_with_release(address, layout, new_size, release) }
}

unsafe fn reallocate_recorded_with_release(address: *mut u8, layout: Layout, new_size: usize, release: impl FnOnce(*mut u8)) -> *mut u8 {
    let Some(request) = crate::classes::Request::with_size(layout, new_size) else {
        return std::ptr::null_mut();
    };
    let retained = crate::classes::Request::new(layout) == Some(request);
    let replacement = if retained {
        address
    } else {
        // Match the core's acquisition of a possibly cross-thread pointer before
        // copying, but keep the original live until recording leaves its borrow.
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        let replacement = crate::thread::allocate_request(request);
        if replacement.is_null() {
            return replacement;
        }
        // SAFETY: Both allocations are live, disjoint, and cover the copy bound.
        unsafe { std::ptr::copy_nonoverlapping(address, replacement, layout.size().min(new_size)) };
        replacement
    };
    if !replacement.is_null() && new_size != layout.size() {
        let freed = emit(address, layout.size(), layout.align(), true);
        if !retained {
            release(address);
        }
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
    let (owners, owner_count, owners_complete) = crate::thread::inventory(round, captured_nanos, crate::observation::WALK_BUDGET)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_realloc_records_free_before_cross_thread_address_reuse() {
        const NAME: &str = "recording::tests::moved_realloc_records_free_before_cross_thread_address_reuse";
        if std::env::var("RALLOCATOR_REALLOC_RECORDING_TEST").as_deref() != Ok("1") {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env("RALLOCATOR_REALLOC_RECORDING_TEST", "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy::all(false),
            ..Default::default()
        });
        let layout = Layout::from_size_align(2 << 20, 16).unwrap();
        let left = allocate(layout, false);
        let original = allocate(layout, false);
        let right = allocate(layout, false);
        assert!(!left.is_null() && !original.is_null() && !right.is_null());
        let (request, requests) = std::sync::mpsc::channel();
        let (ready, responses) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Warm this owner's metadata before it can consume the returned span.
            let warm_layout = Layout::from_size_align(32, 16).unwrap();
            let warm = allocate(warm_layout, false);
            assert!(!warm.is_null());
            // SAFETY: The warm allocation is still live with its original layout.
            unsafe { deallocate(warm, warm_layout) };
            ready.send(0).unwrap();
            let expected = requests.recv().unwrap();
            let reused = allocate(layout, false);
            assert_eq!(reused.addr(), expected);
            ready.send(reused.addr()).unwrap();
            requests.recv().unwrap();
            // SAFETY: The worker owns this replacement for its entire lifetime.
            unsafe { deallocate(reused, layout) };
        });
        responses.recv().unwrap();
        let release = |pointer: *mut u8| {
            let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
            let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
            assert!(decoded.events.events.iter().any(|event| {
                event.kind == seismograph::recorder::event::EventKind::Deallocation
                    && event
                        .allocation()
                        .is_some_and(|allocation| allocation.address.get() == pointer.addr() as u64)
            }));
            // SAFETY: The callback receives the still-live old allocation once.
            unsafe { crate::thread::deallocate(pointer) };
            request.send(pointer.addr()).unwrap();
            responses.recv().unwrap();
        };
        // SAFETY: The original is live; the callback retires it once after its
        // free is recorded and leaves the reused allocation live for capture.
        let replacement = unsafe { reallocate_recorded_with_release(original, layout, 4 << 20, release) };
        assert!(!replacement.is_null());
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let callers = seismograph_rallocator::events::callers(&decoded.events);
        let matching = callers
            .events
            .iter()
            .filter(|event| event.address == original.addr() as u64)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 3);
        let free = matching
            .iter()
            .find(|event| event.kind == seismograph_rallocator::callers::EventKind::Deallocated)
            .unwrap();
        assert!(free.allocation_recorded);
        assert_eq!(matching.iter().filter(|event| event.allocation_id == free.allocation_id).count(), 2);
        request.send(0).unwrap();
        worker.join().unwrap();
        // SAFETY: These three allocations remain live and are retired once.
        unsafe { deallocate(left, layout) };
        // SAFETY: The right guard allocation remains live with its original layout.
        unsafe { deallocate(right, layout) };
        // SAFETY: A successful moved realloc owns the new layout.
        unsafe { deallocate(replacement, Layout::from_size_align(4 << 20, 16).unwrap()) };
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }
}
