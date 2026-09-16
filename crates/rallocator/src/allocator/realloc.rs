// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Origin-based resizing, deliberately separate from ordinary allocation/free dispatch.
//!
//! Normal slabs keep owner-private padding totals: only their current owning
//! reusable heap may adjust them. Foreign, retired, remote-heap and context slabs
//! use fallback. Medium requested sizes are atomic, and their process-retained
//! region metadata can be updated without touching an owner, even after exit.
//! Tracking IDs exclude medium records independently of current recording state.
//! Direct mappings, bump allocations and snapshot storage also use fallback.

use super::*;

/// Private checked boundary; unlike `GlobalAlloc::realloc`, accepts any new size.
///
/// # Safety
///
/// `address` must be an exclusively owned live allocation of `allocator` with
/// the current `layout`. Its allocator personality must agree with `T`.
/// No nonzero/representability precondition is imposed on `new_size`.
pub(super) unsafe fn reallocate<A: GlobalAlloc, T: Tunables>(allocator: &A, address: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
        return ptr::null_mut();
    };
    if new_size == 0 {
        return ptr::null_mut();
    }
    if new_size == layout.size() {
        return address;
    }
    // The mapping fallback adds headers, alignment slack and slab rounding.
    // Reject unrepresentable backing before entering its infallible arithmetic.
    if direct_mapping_size(new_size, layout.align(), layout.align())
        .and_then(|size| size.checked_add(SLAB_SIZE - 1))
        .is_none_or(|size| size > isize::MAX as usize)
    {
        return ptr::null_mut();
    }
    // SAFETY: the old allocation remains live and exclusively owned for resizing.
    if unsafe { try_in_place::<T>(address, layout, new_layout) } {
        return address;
    }
    // SAFETY: new_layout is valid and the original block remains live on failure.
    let replacement = unsafe { allocator.alloc(new_layout) };
    if !replacement.is_null() {
        // SAFETY: distinct live allocations do not overlap; both cover this prefix.
        unsafe {
            ptr::copy_nonoverlapping(address, replacement, layout.size().min(new_size));
            allocator.dealloc(address, layout);
        }
    }
    replacement
}

unsafe fn try_in_place<T: Tunables>(address: *mut u8, layout: Layout, new_layout: Layout) -> bool {
    let Some(region) = region_containing(address) else {
        return false;
    };
    // SAFETY: region lookup returns process-retained metadata, and this thread
    // exclusively accesses its TLS. No borrowed owner state survives fallback.
    let state = unsafe { thread_state() };
    if unsafe { (*state).tearing_down || (*state).in_tracking } || seismograph::recorder::is_suppressed() {
        return false;
    }
    // SAFETY: identical to free-side hint synchronization; does not create a heap.
    unsafe { synchronize_passive_hint(state, false, allocation_hints::active_hint()) };
    let offset = address.addr() - unsafe { (*region).base.addr() };
    let slice = offset / MEDIUM_SLICE_SIZE;
    // SAFETY: the range-checked address identifies a valid metadata index.
    let physical = unsafe { &(*region).physical[slice] };
    let kind = physical.kind_and_span.load(Ordering::Acquire);
    let class = match kind & PHYSICAL_KIND_MASK {
        PHYSICAL_SLICE_SMALL => {
            let segment = allocation_segment(address);
            let slab = segment.cast::<SlabHeader>();
            let class = (address != segment)
                .then(|| {
                    // SAFETY: a live small allocation pins its slab. Its marker
                    // is immutable, including while retirement repurposes other fields.
                    unsafe { (*slab).marker.load(Ordering::Acquire) }
                })
                .and_then(slab_class_from_marker::<T>);
            let Some(class) = class else {
                return false;
            };
            if default_class::<T>(layout) != Some(class) || default_class::<T>(new_layout) != Some(class) {
                return false;
            }
            // SAFETY: only inspect mutable slab fields after identifying the
            // current owning heap, which cannot retire concurrently on this thread.
            let heap = unsafe { current_initialized_reusable_heap(state) };
            if heap.is_null() || unsafe { (*slab).owner != (*heap).owner } || unsafe { is_remote_slab(slab) } {
                return false;
            }
            let bytes = T::SizeClasses::SIZES[class];
            // SAFETY: ordinary live blocks contribute exactly their padding to
            // this owner-private total. Other threads publish frees, not padding updates.
            unsafe {
                (*slab).requested_bytes = (*slab).requested_bytes - (bytes - layout.size()) + (bytes - new_layout.size());
            }
            Some((class, bytes))
        }
        PHYSICAL_SLICE_MEDIUM => {
            let Some(slices) = medium_slice_count(new_layout) else {
                return false;
            };
            if !offset.is_multiple_of(MEDIUM_SLICE_SIZE) || kind >> PHYSICAL_SPAN_SHIFT != slices {
                return false;
            }
            // SAFETY: only the live span's start has allocation metadata. Atomic
            // fields are snapshot-readable; no owner pointer is dereferenced.
            let metadata = unsafe { &(*region).allocations[slice] };
            if metadata.owner.load(Ordering::Acquire).is_null()
                || metadata.usable_bytes.load(Ordering::Relaxed) != slices * MEDIUM_SLICE_SIZE
                || metadata.requested_bytes.load(Ordering::Relaxed) != layout.size()
                || metadata.tracking_allocation_id.load(Ordering::Relaxed) != 0
                || metadata.tracking_session_id.load(Ordering::Relaxed) != 0
            {
                return false;
            }
            metadata.requested_bytes.store(new_layout.size(), Ordering::Relaxed);
            None
        }
        _ => return false,
    };
    // Publish any older local allocations before the byte-only resize delta.
    // SAFETY: state belongs exclusively to this thread; no event/count is fabricated.
    unsafe { flush_aggregate_batch(state) };
    tracking::record_resize(class, layout.size(), new_layout.size());
    true
}

#[cfg(test)]
mod tests;
