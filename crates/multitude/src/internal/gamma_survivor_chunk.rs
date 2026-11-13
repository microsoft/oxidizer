// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::collections::BTreeMap;
use std::sync::Mutex;

use allocator_api2::alloc::{AllocError as BackingError, Global};

use super::*;

#[derive(Clone, Default)]
struct LayoutTrace(Arc<Mutex<BTreeMap<usize, core::alloc::Layout>>>);

#[test]
fn gamma_nov4_chunk_release_transitions_acquire_only_at_zero_and_zero_credits_do_no_rmw() {
    for combined in [false, true] {
        let trace = LayoutTrace::default();
        let allocator = Arc::new(trace.clone());
        let provider = ChunkProvider::new(trace.clone(), super::super::chunk_provider::ChunkProviderConfig::default());
        let chunk = Chunk::allocate(allocator, Arc::downgrade(&provider), 32).unwrap();
        // SAFETY: This test exclusively owns the initialized chunk.
        let reference = unsafe { chunk.as_ref() };
        reference.pre_credit_refs(2);
        let events = crate::sync_fence::trace::Trace::start();
        reference.pre_credit_refs(0);
        // SAFETY: Returning zero credited references does not transfer ownership.
        unsafe { reference.refund_refs(0) };
        assert!(events.events().is_empty());
        if combined {
            // SAFETY: The test owns two surplus references plus the original reference.
            assert!(unsafe { Chunk::refund_refs_and_release_one(chunk, 2) });
        } else {
            // SAFETY: Three separately owned references are transferred exactly once each.
            assert!(!unsafe { Chunk::dec_ref(chunk) });
            assert!(events.events().is_empty());
            // SAFETY: Two references still belong to this test.
            assert!(!unsafe { Chunk::dec_ref(chunk) });
            assert!(events.events().is_empty());
            // SAFETY: This is the final uniquely owned strong reference.
            assert!(unsafe { Chunk::dec_ref(chunk) });
        }
        assert_eq!(events.events(), [Ordering::Acquire]);
        // SAFETY: The previous release reached zero and this test uniquely owns destruction.
        unsafe { Chunk::destroy(chunk) };
        assert!(trace.0.lock().unwrap().is_empty());
    }
}

#[test]
fn gamma_nov4_rejected_numeric_address_deallocates_the_actual_live_backing_once() {
    let trace = LayoutTrace::default();
    let result = super::super::chunk_alloc::alloc_chunk_raw_with_address(&trace, 8, 8, 8, 8, |pointer| {
        assert!(trace.0.lock().unwrap().contains_key(&(pointer as usize)));
        isize::MAX as usize - 15
    });
    assert_eq!(result.unwrap_err(), crate::AllocError::CAPACITY_OVERFLOW);
    assert!(
        trace.0.lock().unwrap().is_empty(),
        "rejection must free the real allocation, not only invoke a planner"
    );
}

// SAFETY: allocations are forwarded to Global unchanged; each deallocation
// checks that it uses exactly the layout originally supplied for that pointer.
unsafe impl Allocator for LayoutTrace {
    fn allocate(&self, layout: core::alloc::Layout) -> Result<NonNull<[u8]>, BackingError> {
        let allocation = Global.allocate(layout)?;
        // SAFETY: Global returned fresh writable storage of layout.size() bytes.
        unsafe { allocation.cast::<u8>().as_ptr().write_bytes(0xa5, layout.size()) };
        assert!(
            self.0
                .lock()
                .expect("the single-threaded trace has no earlier panic")
                .insert(allocation.cast::<u8>().as_ptr() as usize, layout)
                .is_none()
        );
        Ok(allocation)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: core::alloc::Layout) {
        assert_eq!(
            self.0
                .lock()
                .expect("the single-threaded trace has no earlier panic")
                .remove(&(ptr.as_ptr() as usize)),
            Some(layout)
        );
        // SAFETY: the pointer and exact original layout came from Global.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[test]
fn gamma_survivor_chunk_exact_layout_header_counts_links_and_cleanup() {
    for payload in (0_usize..=16).chain([64, 511]) {
        let trace = LayoutTrace::default();
        let allocator = Arc::new(trace.clone());
        let provider = ChunkProvider::new(trace.clone(), super::super::chunk_provider::ChunkProviderConfig::default());
        let chunk = Chunk::allocate(Arc::clone(&allocator), Arc::downgrade(&provider), payload).unwrap();
        assert_eq!(Arc::strong_count(&allocator), 2);
        assert_eq!(Arc::weak_count(&provider), 2);
        assert_eq!(trace.0.lock().unwrap().len(), 1);
        // SAFETY: allocation gave this test the sole reference to a live initialized chunk.
        let reference = unsafe { chunk.as_ref() };
        // SAFETY: the data field belongs to that live chunk; projection does not read payload bytes.
        let data = unsafe { &raw const (*chunk.as_ptr()).data };
        let header = data.cast::<u8>() as usize - chunk.cast::<u8>().as_ptr() as usize;
        assert_eq!(Chunk::<LayoutTrace>::header_size(), header);
        assert_eq!(reference.capacity, payload);
        assert_eq!(reference.capacity(), payload);
        assert_eq!(reference.data.len(), payload);
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 1);
        assert!(reference.next.load(Ordering::Relaxed).is_null());
        assert!(Arc::ptr_eq(&reference.provider.upgrade().unwrap(), &provider));
        // SAFETY: chunk remains live and initialized.
        let value = unsafe { Chunk::<LayoutTrace>::payload_ptr(chunk) };
        assert_eq!(value.as_ptr() as usize, chunk.cast::<u8>().as_ptr() as usize + header);
        assert_eq!(Chunk::<LayoutTrace>::header_from_value_ptr(value), chunk.cast::<u8>());
        // SAFETY: this is the initialized chunk's original allocation base.
        let projected = unsafe { Chunk::<LayoutTrace>::header_to_fat(chunk.cast::<u8>().as_ptr()) };
        assert_eq!(projected, chunk.as_ptr());
        let align = mem::align_of::<usize>().max(mem::align_of::<Arc<LayoutTrace>>());
        let expected_size = (header + payload).div_ceil(align) * align;
        let expected_align = CHUNK_ALIGN.max(align);
        assert_eq!(
            trace.0.lock().unwrap().values().next().copied(),
            Some(core::alloc::Layout::from_size_align(expected_size, expected_align).unwrap())
        );
        assert_eq!(Chunk::<LayoutTrace>::footprint(payload).unwrap(), expected_size);
        gamma_survivor_expect_error(Chunk::<LayoutTrace>::footprint(usize::MAX));
        // SAFETY: this test exclusively owns the live chunk's link.
        unsafe { Chunk::set_next(chunk, chunk.cast::<u8>().as_ptr()) };
        // SAFETY: the link belongs to this live initialized chunk.
        assert_eq!(unsafe { Chunk::next(chunk) }, chunk.cast::<u8>().as_ptr());
        // SAFETY: clear the exclusively owned link before destroying the chunk.
        unsafe { Chunk::set_next(chunk, ptr::null_mut()) };
        // SAFETY: the link still belongs to the live initialized chunk.
        assert!(unsafe { Chunk::next(chunk) }.is_null());
        #[cfg(feature = "stats")]
        {
            assert_eq!(reference.wasted_at_retire(), 0);
            reference.set_wasted_at_retire(17);
            assert_eq!(reference.wasted_at_retire(), 17);
            reference.set_wasted_at_retire(0);
        }
        reference.pre_credit_refs(3);
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 4);
        // SAFETY: pre_credit_refs above supplied three outstanding credits.
        unsafe { reference.refund_refs(1) };
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 3);
        // SAFETY: two of the three outstanding credits remain before this refund.
        unsafe { reference.refund_refs(1) };
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 2);
        reference.inc_ref();
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 3);
        // SAFETY: this test owns the reference just added by inc_ref.
        assert!(!unsafe { Chunk::dec_ref(chunk) });
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 2);
        // SAFETY: the final outstanding credit and original allocation reference are both owned here.
        assert!(unsafe { Chunk::refund_refs_and_release_one(chunk, 1) });
        assert_eq!(reference.ref_count.load(Ordering::Relaxed), 0);
        // SAFETY: the count is zero and every test-owned reference has been returned.
        unsafe { Chunk::destroy(chunk) };
        assert!(trace.0.lock().unwrap().is_empty());
        assert_eq!(Arc::strong_count(&allocator), 1);
        assert_eq!(Arc::weak_count(&provider), 1);
    }
}

#[cfg(test)]
fn gamma_survivor_expect_error<T, E>(result: Result<T, E>) {
    let Err(_) = result else {
        panic!("the fixture requires this operation to return its injected error");
    };
    drop(result);
}
