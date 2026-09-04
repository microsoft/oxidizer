// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configurable general-purpose and bump allocation heaps.

pub(crate) mod bump;
pub(crate) mod general;

use std::alloc::Layout;
use std::ptr::NonNull;

use bump as common_bump;
use bump::BumpState;
use general as common_general;

use crate::allocator::{RemoteHeapState, ReusableHeapState};
use crate::hal;

#[derive(Clone, Copy, Debug)]
pub(crate) enum HeapTarget {
    General(GeneralHeapPtr),
    Bump(BumpHeapPtr),
    Thread(RemoteHeapPtr),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GeneralHeapPtr(NonNull<ReusableHeapState>);

#[derive(Clone, Copy, Debug)]
pub(crate) struct BumpHeapPtr(NonNull<BumpState>);

#[derive(Clone, Copy, Debug)]
pub(crate) struct RemoteHeapPtr(NonNull<RemoteHeapState>);

// SAFETY: Every target points to allocator-owned stable state whose individual
// access paths enforce their required synchronization and retirement rules.
unsafe impl Send for HeapTarget {}
// SAFETY: Sharing a target does not dereference it; heap operations synchronize before access.
unsafe impl Sync for HeapTarget {}

macro_rules! native_heap_pointer {
    ($name:ident, $target:ty) => {
        impl $name {
            fn new(pointer: *mut $target) -> Self {
                Self(NonNull::new(pointer).expect("allocator heap targets must not be null"))
            }

            pub(crate) const fn as_ptr(self) -> *mut $target {
                self.0.as_ptr()
            }
        }
    };
}

native_heap_pointer!(GeneralHeapPtr, ReusableHeapState);
native_heap_pointer!(BumpHeapPtr, BumpState);
native_heap_pointer!(RemoteHeapPtr, RemoteHeapState);

pub(crate) fn create_passive_hint(hint: allocation_hints::heaps::ActiveHint) -> Option<HeapTarget> {
    match hint.kind() {
        allocation_hints::heaps::Kind::General(options) => {
            let options = common_general::Options::from_values(options.locality_segment_bytes(), options.medium_cache_max_bytes());
            new_general(options, crate::domain::default_state()).map(|state| HeapTarget::General(GeneralHeapPtr::new(state)))
        }
        allocation_hints::heaps::Kind::Bump(options) => {
            let options = common_bump::Options::new()
                .with_max_allocation_bytes(options.max_allocation_bytes())
                .with_max_alignment(options.max_alignment())
                .with_retained_chunks(options.retained_chunks())
                .with_max_retained_chunks(options.max_retained_chunks());
            let domain = crate::domain::default_state();
            let state = crate::allocator::take_pooled_bump(domain).or_else(|| create_bump_state(options, domain))?;
            if !ensure_bump_fallback_heap(state) {
                crate::allocator::return_pooled_bump(state);
                return None;
            }
            unsafe { bump::reset_state(state, options) };
            Some(HeapTarget::Bump(BumpHeapPtr::new(state)))
        }
        allocation_hints::heaps::Kind::Thread(thread_id) => {
            crate::allocator::passive_thread_heap(thread_id).map(|state| HeapTarget::Thread(RemoteHeapPtr::new(state)))
        }
    }
}

fn new_general(options: common_general::Options, domain: *mut crate::allocator::DomainState) -> Option<*mut ReusableHeapState> {
    let layout = Layout::new::<ReusableHeapState>();
    let state = map_heap_state(layout);
    if state.is_null() {
        return None;
    }
    unsafe { state.write(ReusableHeapState::new(options, domain)) };
    unsafe { crate::allocator::initialize_general_heap(state) };
    Some(state)
}

fn create_bump_state(options: common_bump::Options, domain: *mut crate::allocator::DomainState) -> Option<*mut BumpState> {
    bump::create_state(options, domain)
}

fn ensure_bump_fallback_heap(state: *mut BumpState) -> bool {
    bump::ensure_fallback_heap(state)
}

fn map_heap_state(layout: Layout) -> *mut ReusableHeapState {
    hal::map(layout.size()).cast()
}

#[cfg(test)]
mod tests {
    use allocation_hints::heaps::{Heap, bump as hint_bump, general};
    use allocation_hints::with_hint;

    use super::*;

    #[cfg(not(miri))]
    #[test]
    fn passive_heap_creation_reports_general_and_bump_backing_failures() {
        let _test = crate::telemetry::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::thread::spawn(|| {
            let domain = crate::domain::default_state();
            let general_heap = Heap::general(general::Options::new());
            let general_hint = with_hint(&general_heap, || allocation_hints::active_hint().unwrap());
            hal::fail_next_map();
            assert!(create_passive_hint(general_hint).is_none());

            let options = common_bump::Options::new();
            let state = create_bump_state(options, domain).unwrap();
            let fallback = unsafe { bump::take_fallback_heap(state) };
            unsafe { crate::allocator::retire_general_heap(fallback) };
            crate::allocator::return_pooled_bump(state);

            let bump_heap = Heap::bump(hint_bump::Options::new());
            let bump_hint = with_hint(&bump_heap, || allocation_hints::active_hint().unwrap());
            hal::fail_next_map();
            assert!(create_passive_hint(bump_hint).is_none());
        })
        .join()
        .unwrap();
    }
}
