// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;
use crate::domain::Domain;
use crate::heap::general::Options as GeneralOptions;

fn new_domain_state() -> *mut DomainState {
    crate::domain::state(Domain::new().unwrap())
}

fn new_retirable_heap(domain: *mut DomainState, options: GeneralOptions) -> *mut ReusableHeapState {
    let heap = hal::map(size_of::<ReusableHeapState>()).cast::<ReusableHeapState>();
    assert!(!heap.is_null());
    unsafe {
        heap.write(ReusableHeapState::new(options, domain));
        initialize_general_heap(heap);
    }
    heap
}

fn domain_region_count(domain: *mut DomainState) -> usize {
    let regions = unsafe { domain_regions(domain) };
    let state = regions.state.lock();
    let mut count = 0;
    let mut region = state.regions;
    while !region.is_null() {
        count += 1;
        region = unsafe { (*region).next.load(Ordering::Relaxed) };
    }
    count
}

fn domain_used_slices(domain: *mut DomainState) -> usize {
    let regions = unsafe { domain_regions(domain) };
    let state = regions.state.lock();
    let mut total = 0;
    let mut region = state.regions;
    while !region.is_null() {
        for slice_index in 0..MEDIUM_REGION_SLICE_COUNT {
            if unsafe { slice_is_used(&(*region).used, slice_index) } {
                total += 1;
            }
        }
        region = unsafe { (*region).next.load(Ordering::Relaxed) };
    }
    total
}

fn region_and_slice(address: *mut u8) -> (*mut RegionState, usize) {
    let region = region_containing(address).unwrap();
    let slice_index = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
    (region, slice_index)
}

fn slice_marked(region: *mut RegionState, slice_index: usize) -> bool {
    let regions = unsafe { domain_regions((*region).domain) };
    let _state = regions.state.lock();
    unsafe { slice_is_used(&(*region).used, slice_index) }
}

fn mark_model(model: &mut [bool; MEDIUM_REGION_SLICE_COUNT], start: usize, count: usize, value: bool) {
    for occupied in &mut model[start..start + count] {
        *occupied = value;
    }
}

fn reference_find(model: &[bool; MEDIUM_REGION_SLICE_COUNT], start: usize, count: usize) -> Option<usize> {
    reference_find_in(model, start, model.len(), count).or_else(|| reference_find_in(model, 0, model.len(), count))
}

fn reference_find_in(model: &[bool; MEDIUM_REGION_SLICE_COUNT], start: usize, end: usize, count: usize) -> Option<usize> {
    let mut run_start = start;
    let mut run_length = 0;
    for (slice_index, occupied) in model.iter().enumerate().take(end).skip(start) {
        if *occupied {
            run_start = slice_index + 1;
            run_length = 0;
        } else {
            run_length += 1;
            if run_length == count {
                return Some(run_start);
            }
        }
    }
    None
}

#[test]
fn mixed_slab_medium_and_direct_cycle_bounds_retained_slices_and_reuses_cached_backing() {
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let domain = new_domain_state();
    let heap = new_retirable_heap(domain, GeneralOptions::from_values(MEDIUM_SLICE_SIZE, MEDIUM_SLICE_SIZE));
    let heap_ref = unsafe { &mut *heap };
    let small_class = 0;
    let small_size = ConfigSizeClasses::<Standard>::SIZES[small_class];
    let medium_layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
    let direct_layout = Layout::from_size_align(128, MAX_MEDIUM_ALIGNMENT * 2).unwrap();

    let small = allocator.refill(small_class, heap_ref);
    assert!(!small.is_null());
    assert_eq!(domain_region_count(domain), 1);
    assert_eq!(domain_used_slices(domain), 1);

    let medium = allocator.allocate_medium(medium_layout, heap_ref, None);
    assert!(!medium.is_null());
    assert_eq!(domain_region_count(domain), 1);
    assert_eq!(domain_used_slices(domain), 2);

    #[cfg(not(miri))]
    let unmappings = hal::unmap_count();
    let direct = unsafe { allocator.allocate_direct(direct_layout, false, None, heap) };
    assert!(!direct.is_null());
    assert_eq!(domain_used_slices(domain), 2);
    unsafe {
        direct.write(0xDA);
        let header = read_header(direct).map_addr(|address| address & !TAG_MASK);
        allocator.deallocate_direct(direct, direct_layout, header);
    }
    #[cfg(not(miri))]
    assert_eq!(hal::unmap_count(), unmappings + 1);
    assert_eq!(domain_used_slices(domain), 2);

    let mut thread = ThreadState::new();
    thread.default_heap = heap;
    unsafe {
        push_block::<crate::config::Standard>(small, small_class, small_size, ptr::from_mut(&mut thread));
        allocator.deallocate_medium(medium, medium_layout, heap);
    }
    assert_eq!(unsafe { (*heap).medium_cache[0] }, medium);
    assert_eq!(domain_used_slices(domain), 2);

    let reused_small = allocator.pop_or_refill(small_class, unsafe { &mut *heap });
    assert_eq!(reused_small, small);
    let reused_medium = allocator.allocate_medium(medium_layout, unsafe { &mut *heap }, None);
    assert_eq!(reused_medium, medium);
    unsafe {
        push_block::<crate::config::Standard>(reused_small, small_class, small_size, ptr::from_mut(&mut thread));
        allocator.deallocate_medium(reused_medium, medium_layout, heap);
    }
    assert_eq!(domain_region_count(domain), 1);
    assert_eq!(domain_used_slices(domain), 2);

    unsafe { retire_general_heap(heap) };
    assert_eq!(domain_region_count(domain), 1);
    assert_eq!(domain_used_slices(domain), 0);
}

#[cfg(not(miri))]
#[test]
fn failed_medium_commit_rolls_back_the_only_candidate_slice_and_retries_in_place() {
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let domain = new_domain_state();
    let heap = new_retirable_heap(domain, GeneralOptions::from_values(MEDIUM_SLICE_SIZE, 0));
    let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
    let live = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
    assert!(!live.is_null());
    unsafe {
        live.write(0xA5);
        live.add(MEDIUM_SLICE_SIZE - 1).write(0x5A);
    }

    let (region, live_slice) = region_and_slice(live);
    let candidate_slice = if MEDIUM_REGION_SLICE_COUNT > 64 {
        64
    } else {
        MEDIUM_REGION_SLICE_COUNT - 1
    };
    assert_ne!(candidate_slice, live_slice);
    let candidate = unsafe { (*region).base.add(candidate_slice * MEDIUM_SLICE_SIZE) };
    {
        let regions = unsafe { domain_regions(domain) };
        let state = regions.state.lock();
        unsafe {
            (*region).used = [0; MEDIUM_REGION_BITMAP_WORDS];
            mark_slices(&mut (*region).used, 0, MEDIUM_REGION_SLICE_COUNT, true);
            mark_slices(&mut (*region).used, candidate_slice, 1, false);
            (*region).next_slice = candidate_slice;
        }
        assert_eq!(state.regions, region);
    }
    assert_eq!(domain_used_slices(domain), MEDIUM_REGION_SLICE_COUNT - 1);

    hal::fail_next_commit();
    assert!(allocator.allocate_medium(layout, unsafe { &mut *heap }, None).is_null());
    assert!(!slice_marked(region, candidate_slice));
    assert_eq!(domain_region_count(domain), 1);
    assert_eq!(unsafe { live.read() }, 0xA5);
    assert_eq!(unsafe { live.add(MEDIUM_SLICE_SIZE - 1).read() }, 0x5A);

    let retried = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
    assert_eq!(retried, candidate);
    assert!(slice_marked(region, candidate_slice));
    assert_eq!(domain_region_count(domain), 1);
    assert_eq!(unsafe { live.read() }, 0xA5);
    assert_eq!(unsafe { live.add(MEDIUM_SLICE_SIZE - 1).read() }, 0x5A);

    unsafe {
        allocator.deallocate_medium(retried, layout, heap);
        allocator.deallocate_medium(live, layout, heap);
    }
    {
        let regions = unsafe { domain_regions(domain) };
        let mut state = regions.state.lock();
        allocator.purge_medium_locked(&mut state, true);
        unsafe { (*region).used = [0; MEDIUM_REGION_BITMAP_WORDS] };
    }
    unsafe { retire_general_heap(heap) };
}

#[cfg(not(miri))]
#[test]
fn failed_direct_reallocation_preserves_original_payload_and_allows_retry() {
    std::thread::spawn(|| {
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let align = MAX_MEDIUM_ALIGNMENT * 2;
        let layout = Layout::from_size_align(64, align).unwrap();
        let grown_layout = Layout::from_size_align(128, align).unwrap();
        let original = unsafe { allocator.alloc(layout) };
        assert!(!original.is_null());
        for index in 0..layout.size() {
            unsafe { original.add(index).write((index as u8).wrapping_mul(3).wrapping_add(1)) };
        }

        let unmappings = hal::unmap_count();
        hal::fail_next_map();
        let failed = unsafe { allocator.realloc(original, layout, grown_layout.size()) };
        assert!(failed.is_null());
        assert_eq!(hal::unmap_count(), unmappings);
        for index in 0..layout.size() {
            assert_eq!(unsafe { original.add(index).read() }, (index as u8).wrapping_mul(3).wrapping_add(1));
        }

        let grown = unsafe { allocator.realloc(original, layout, grown_layout.size()) };
        assert!(!grown.is_null());
        for index in 0..layout.size() {
            assert_eq!(unsafe { grown.add(index).read() }, (index as u8).wrapping_mul(3).wrapping_add(1));
        }
        assert_eq!(hal::unmap_count(), unmappings + 1);
        unsafe { allocator.dealloc(grown, grown_layout) };
        assert_eq!(hal::unmap_count(), unmappings + 2);
    })
    .join()
    .unwrap();
}

#[test]
fn recycled_slab_bitmap_reuses_word_boundary_and_last_block_indices_in_model_order() {
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let slab = hal::map(SLAB_SIZE);
    assert!(!slab.is_null());
    let mut domain = DomainState::new();
    let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
    let class_index = 0;
    let block_size = ConfigSizeClasses::<Standard>::SIZES[class_index];
    let (first_block, block_count) = slab_block_layout(block_size);
    assert!(first_block <= 63);
    assert!(block_count > 128);

    let first = allocator.initialize_slab(
        SlabAllocation {
            address: slab,
            segment_slices: DIRECT_SLAB_SEGMENT,
            committed_bytes: SLAB_SIZE,
        },
        class_index,
        &mut heap,
        SLAB_MARKER,
    );
    assert_eq!(first, unsafe { slab.add(first_block * block_size) });
    let header = slab.cast::<SlabHeader>();
    unsafe {
        (*header).fresh_next = ptr::null_mut();
        (*header).free_count = 0;
        (*header).recycled_summary = 0;
        (*header).recycled_batch_word = 0;
        (*header).recycled_batch = 0;
        (*header).recycled = [0; RECYCLED_BITMAP_WORDS];
    }

    for block_index in [block_count - 1, 128, 64, 127, 63] {
        unsafe {
            recycle_local_block::<crate::config::Standard>(header, slab.add(block_index * block_size), class_index);
        }
    }
    assert_eq!(unsafe { (*header).free_count }, 5);

    for block_index in [63, 64, 127, 128, block_count - 1] {
        let block = unsafe { take_local_slab_block::<crate::config::Standard>(header, class_index) };
        assert_eq!(block, unsafe { slab.add(block_index * block_size) });
    }
    assert_eq!(unsafe { (*header).free_count }, 0);
    assert!(unsafe { take_local_slab_block::<crate::config::Standard>(header, class_index) }.is_null());

    unsafe { hal::unmap(slab, SLAB_SIZE) };
}

#[test]
fn slice_search_matches_reference_model_across_fragmented_word_and_end_boundaries() {
    let mut used = [0; MEDIUM_REGION_BITMAP_WORDS];
    let mut model = [true; MEDIUM_REGION_SLICE_COUNT];
    mark_slices(&mut used, 0, MEDIUM_REGION_SLICE_COUNT, true);

    mark_slices(&mut used, 0, 2, false);
    mark_model(&mut model, 0, 2, false);
    if MEDIUM_REGION_SLICE_COUNT > 66 {
        mark_slices(&mut used, 63, 3, false);
        mark_model(&mut model, 63, 3, false);
    }
    mark_slices(&mut used, MEDIUM_REGION_SLICE_COUNT - 3, 3, false);
    mark_model(&mut model, MEDIUM_REGION_SLICE_COUNT - 3, 3, false);

    let starts = [
        0,
        1,
        2.min(MEDIUM_REGION_SLICE_COUNT - 1),
        62.min(MEDIUM_REGION_SLICE_COUNT - 1),
        64.min(MEDIUM_REGION_SLICE_COUNT - 1),
        MEDIUM_REGION_SLICE_COUNT - 2,
    ];
    for start in starts {
        for count in [1, 2, 3, 4] {
            assert_eq!(find_free_slices(&used, start, count), reference_find(&model, start, count));
            assert_eq!(
                find_free_slices_in(&used, start, MEDIUM_REGION_SLICE_COUNT, count),
                reference_find_in(&model, start, MEDIUM_REGION_SLICE_COUNT, count)
            );
        }
    }
}

#[test]
fn large_free_extents_coalesce_across_fragmented_free_order_and_word_boundary() {
    let domain = new_domain_state();
    let regions = unsafe { domain_regions(domain) };
    let span_slices = if MEDIUM_REGION_SLICE_COUNT > 70 { 70 } else { 8 };
    let base = regions.allocate_slices(domain, span_slices).unwrap();
    assert!(unsafe { hal::commit(base, span_slices * MEDIUM_SLICE_SIZE) });
    let region = region_containing(base).unwrap();

    let extents: &[(usize, usize)] = if span_slices == 70 {
        &[(62, 2), (66, 4), (64, 2), (0, 62)]
    } else {
        &[(2, 2), (6, 2), (4, 2), (0, 2)]
    };
    for &(start, count) in extents {
        unsafe { insert_large_extent(region, base.add(start * MEDIUM_SLICE_SIZE), count) };
    }

    let first_count = span_slices - 5;
    let first = unsafe { take_large_extent(region, first_count) }.unwrap();
    let second = unsafe { take_large_extent(region, 5) }.unwrap();
    assert_eq!(first, base);
    assert_eq!(second, unsafe { base.add(first_count * MEDIUM_SLICE_SIZE) });
    assert!(unsafe { take_large_extent(region, 1) }.is_none());

    assert!(unsafe { hal::decommit(base, span_slices * MEDIUM_SLICE_SIZE) });
    unsafe { regions.release_slices(base, span_slices) };
}

#[test]
fn escaped_medium_and_direct_allocations_keep_backing_until_release_then_explicit_purge() {
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let domain = new_domain_state();
    let regions = unsafe { domain_regions(domain) };
    let heap = new_retirable_heap(domain, GeneralOptions::from_values(MEDIUM_SLICE_SIZE, 0));
    let medium_layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
    let direct_layout = Layout::from_size_align(96, MAX_MEDIUM_ALIGNMENT * 2).unwrap();

    let medium = allocator.allocate_medium(medium_layout, unsafe { &mut *heap }, None);
    let direct = unsafe { allocator.allocate_direct(direct_layout, false, None, heap) };
    assert!(!medium.is_null());
    assert!(!direct.is_null());
    unsafe {
        medium.write(0xC3);
        medium.add(MEDIUM_SLICE_SIZE - 1).write(0x3C);
        direct.write(0xD1);
        direct.add(direct_layout.size() - 1).write(0x1D);
    }
    let (region, medium_slice) = region_and_slice(medium);
    let retirement = unsafe { (*(*heap).owner).retirement };
    assert_eq!(unsafe { (*retirement).external_allocations.load(Ordering::Acquire) }, 2);

    unsafe { retire_general_heap(heap) };
    assert!(slice_marked(region, medium_slice));
    assert_eq!(unsafe { medium.read() }, 0xC3);
    assert_eq!(unsafe { medium.add(MEDIUM_SLICE_SIZE - 1).read() }, 0x3C);
    assert_eq!(unsafe { direct.read() }, 0xD1);
    assert_eq!(unsafe { direct.add(direct_layout.size() - 1).read() }, 0x1D);
    assert_eq!(unsafe { (*retirement).external_allocations.load(Ordering::Acquire) }, 2);

    unsafe { allocator.deallocate_medium(medium, medium_layout, ptr::null_mut()) };
    assert!(slice_marked(region, medium_slice));
    assert_eq!(unsafe { (*retirement).external_allocations.load(Ordering::Acquire) }, 1);
    {
        let mut state = regions.state.lock();
        allocator.purge_medium_locked(&mut state, true);
    }
    assert!(!slice_marked(region, medium_slice));

    unsafe {
        let header = read_header(direct).map_addr(|address| address & !TAG_MASK);
        allocator.deallocate_direct(direct, direct_layout, header);
    }
}
