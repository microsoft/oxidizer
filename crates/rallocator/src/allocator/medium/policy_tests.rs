// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(miri))]
use super::super::tests::slices_are_free;
use super::*;

#[cfg(not(miri))]
#[test]
fn purge_budgets_account_for_small_remainders_in_the_large_extent_list() {
    for force in [false, true] {
        let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
        // SAFETY: the private domain and its shard are process-retained.
        let backing = unsafe { domain_shard(domain, 0) };
        let allocated_slices = MEDIUM_MAX_SLICES + 1;
        let remainder_slices = PURGE_BYTES / MEDIUM_SLICE_SIZE / 2;
        let extent_slices = allocated_slices + remainder_slices;
        let stride = extent_slices + 1;
        let (base, _) = backing.reserve_slices(domain, 3 * stride).unwrap();
        for index in 0..3 {
            let address = base.wrapping_add(index * stride * MEDIUM_SLICE_SIZE);
            let bytes = extent_slices * MEDIUM_SLICE_SIZE;
            // SAFETY: gaps separate three exclusively owned committed extents.
            assert!(unsafe { hal::commit(address, bytes) });
            tracking::record_mapping(bytes);
            // SAFETY: cache publication transfers each complete extent.
            unsafe { insert_cached_span(&mut backing.state.lock(), address, extent_slices, 1) };
        }
        for _ in 0..3 {
            let mut taken = [ptr::null_mut()];
            assert_eq!(backing.take_batch(allocated_slices, &mut taken), 1);
            // SAFETY: taking a large allocation leaves a smaller linked remainder;
            // the returned prefix is exclusively owned and can be decommitted.
            assert!(unsafe { backing.decommit_span(taken[0], allocated_slices, 0) });
        }
        // SAFETY: the shard lock protects publication. An empty successor makes
        // prematurely advancing the purge cursor observable.
        unsafe {
            let mut state = backing.state.lock();
            append_region(&mut state, &backing.regions, domain).unwrap();
        }
        let remaining_bytes = remainder_slices * MEDIUM_SLICE_SIZE;
        assert_eq!(backing.state.lock().retained_bytes, 3 * remaining_bytes);
        let budget = MemoryBudget { limit: 0, pressured: true };
        backing.purge_with_budget(force, 1, budget);
        assert_eq!(backing.state.lock().retained_bytes, if force { 0 } else { remaining_bytes });
        if !force {
            backing.purge_with_budget(false, 1, budget);
            assert_eq!(backing.state.lock().retained_bytes, 0);
        }
        for index in 0..3 {
            let gap = base.wrapping_add((index * stride + extent_slices) * MEDIUM_SLICE_SIZE);
            // SAFETY: only these uncommitted separators remain reserved.
            unsafe { backing.release_slices(gap, 1) };
        }
    }
}

#[cfg(not(miri))]
#[test]
fn reaching_the_memory_budget_does_not_expire_a_young_extent() {
    let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
    // SAFETY: the domain and its shard are process-retained.
    let backing = unsafe { domain_shard(domain, 0) };
    let slices = MEDIUM_MAX_SLICES + 1;
    let bytes = slices * MEDIUM_SLICE_SIZE;
    let (address, _) = backing.reserve_slices(domain, slices).unwrap();
    // SAFETY: the fixture owns the whole reservation.
    assert!(unsafe { hal::commit(address, bytes) });
    tracking::record_mapping(bytes);
    // SAFETY: publication transfers the committed span under the shard lock.
    unsafe { insert_cached_span(&mut backing.state.lock(), address, slices, 100) };
    let budget = MemoryBudget {
        limit: bytes,
        pressured: false,
    };
    backing.purge_with_budget(false, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, bytes);
    backing.purge_with_budget(false, 100, budget);
    assert_eq!(backing.state.lock().retained_bytes, 0);
    assert!(slices_are_free(address, slices));
}

#[cfg(not(miri))]
#[test]
fn mixed_bins_distinguish_remaining_capacity_from_an_oversized_next_span() {
    for (second_slices, expected_count, expected_slices) in [(2, 2, 3), (PURGE_BYTES / MEDIUM_SLICE_SIZE, 1, 1)] {
        let mut blocks = std::array::from_fn::<_, 2, _>(|_| MediumFreeBlock { next: ptr::null_mut() });
        let mut state = MediumState::new();
        for (index, slices) in [1, second_slices].into_iter().enumerate() {
            let class = slices - 1;
            state.bins[class].free_list = ptr::from_mut(&mut blocks[index]);
            state.bins[class].purge_after = 1;
            state.nonempty_bins[class / 64] |= 1 << (class % 64);
        }
        let mut pending = [(ptr::null_mut(), 0); PURGE_WORK];
        assert_eq!(
            state.detach_bins(&mut pending, false, false, 1),
            (expected_count, expected_slices * MEDIUM_SLICE_SIZE)
        );
        assert_eq!(state.bins[second_slices - 1].free_list.is_null(), expected_count == 2);
    }
}

#[cfg(not(miri))]
#[test]
fn a_fixed_bin_purge_defers_an_oversized_extent_until_the_next_pass() {
    let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
    // SAFETY: the domain and its shard are process-retained.
    let backing = unsafe { domain_shard(domain, 0) };
    let large_slices = MEDIUM_MAX_SLICES + 1;
    let large_bytes = large_slices * MEDIUM_SLICE_SIZE;
    let (small, _) = backing.reserve_slices(domain, large_slices + 1).unwrap();
    let large = small.wrapping_add(MEDIUM_SLICE_SIZE);
    // SAFETY: the fixture owns both adjacent spans.
    assert!(unsafe { hal::commit(small, large_bytes + MEDIUM_SLICE_SIZE) });
    tracking::record_mapping(large_bytes + MEDIUM_SLICE_SIZE);
    // SAFETY: cache publication transfers both committed spans under the shard lock.
    unsafe {
        let mut state = backing.state.lock();
        insert_cached_span(&mut state, small, 1, 1);
        insert_cached_span(&mut state, large, large_slices, 1);
        append_region(&mut state, &backing.regions, domain).unwrap();
    }
    let budget = MemoryBudget { limit: 0, pressured: true };
    backing.purge_with_budget(false, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, large_bytes);
    assert!(slices_are_free(small, 1));
    assert!(!slices_are_free(large, large_slices));
    backing.purge_with_budget(false, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, 0);
}

#[cfg(not(miri))]
#[test]
fn invalid_reservation_sizes_do_not_publish_a_region() {
    let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
    // SAFETY: the domain and its shard are process-retained.
    let backing = unsafe { domain_shard(domain, 0) };
    assert_eq!(backing.reserve_slices(domain, 0), None);
    assert_eq!(backing.reserve_slices(domain, MEDIUM_REGION_SLICE_COUNT + 1), None);
    assert!(backing.state.lock().regions.is_null());
}

#[cfg(not(miri))]
#[test]
fn forced_purge_restores_a_failed_large_span_without_retrying_it() {
    let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
    // SAFETY: the private domain and its shard are process-retained.
    let backing = unsafe { domain_shard(domain, 0) };
    let slices = MEDIUM_MAX_SLICES + 1;
    let bytes = slices * MEDIUM_SLICE_SIZE;
    let (address, _) = backing.reserve_slices(domain, slices).unwrap();
    // SAFETY: the fixture owns the entire reservation.
    assert!(unsafe { hal::commit(address, bytes) });
    tracking::record_mapping(bytes);
    // SAFETY: the shard lock receives the committed span's ownership.
    unsafe { insert_cached_span(&mut backing.state.lock(), address, slices, 1) };
    hal::fail_next_decommit();
    let budget = MemoryBudget { limit: 0, pressured: true };
    backing.purge_with_budget(true, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, bytes);
    assert!(!slices_are_free(address, slices));
    backing.purge_with_budget(true, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, 0);
    assert!(slices_are_free(address, slices));
}

#[cfg(not(miri))]
#[test]
fn nonempty_bin_search_matches_cyclic_class_order_at_every_start() {
    let classes = [1, 63, 64, 65, 127, MEDIUM_MAX_SLICES - 1];
    let mut state = MediumState::new();
    for class in classes {
        state.nonempty_bins[class / 64] |= 1 << (class % 64);
    }
    for start in 0..MEDIUM_MAX_SLICES {
        let expected = classes.iter().copied().find(|class| *class >= start).or(Some(classes[0]));
        assert_eq!(state.next_bin(start), expected, "search starts at {start}");
    }
}

#[cfg(not(miri))]
#[test]
fn detaching_an_expired_bin_preserves_other_words_and_advances_the_cursor() {
    let mut expired = MediumFreeBlock { next: ptr::null_mut() };
    let mut young = MediumFreeBlock { next: ptr::null_mut() };
    let mut state = MediumState::new();
    let expired_class = MEDIUM_MAX_SLICES - 2;
    let young_class = 1;
    state.bins[expired_class].free_list = ptr::from_mut(&mut expired);
    state.bins[expired_class].purge_after = 10;
    state.bins[young_class].free_list = ptr::from_mut(&mut young);
    state.bins[young_class].purge_after = 11;
    state.nonempty_bins[expired_class / 64] = 1 << (expired_class % 64);
    state.nonempty_bins[0] |= 1 << young_class;
    state.purge_class = expired_class;
    let mut pending = [(ptr::null_mut(), 0); PURGE_WORK];
    let detached = state.detach_bins(&mut pending, false, false, 10);
    assert_eq!(detached, (1, (expired_class + 1) * MEDIUM_SLICE_SIZE));
    assert_eq!(pending[0], (ptr::from_mut(&mut expired).cast(), expired_class + 1));
    assert_eq!(state.next_bin(0), Some(young_class));
    assert_eq!(state.nonempty_bins[expired_class / 64], 0);
    assert_eq!(state.purge_class, expired_class + 1);
    assert!(state.bins[expired_class].free_list.is_null());
    assert_eq!(state.bins[expired_class].purge_after, 0);
}

#[cfg(not(miri))]
#[test]
fn detach_byte_budget_accepts_an_exact_fit_and_leaves_the_next_span_linked() {
    let slices = PURGE_BYTES / MEDIUM_SLICE_SIZE / 2;
    let mut blocks = std::array::from_fn::<_, 3, _>(|_| MediumFreeBlock { next: ptr::null_mut() });
    blocks[0].next = ptr::from_mut(&mut blocks[1]);
    blocks[1].next = ptr::from_mut(&mut blocks[2]);
    let mut state = MediumState::new();
    let class = slices - 1;
    state.bins[class].free_list = ptr::from_mut(&mut blocks[0]);
    state.bins[class].purge_after = 1;
    state.nonempty_bins[class / 64] = 1 << (class % 64);
    let mut pending = [(ptr::null_mut(), 0); PURGE_WORK];
    assert_eq!(state.detach_bins(&mut pending, true, false, 1), (2, PURGE_BYTES));
    assert_eq!(state.bins[class].free_list, ptr::from_mut(&mut blocks[2]));
    assert_eq!(state.next_bin(0), Some(class));
    assert_eq!(
        pending[..2],
        [
            (ptr::from_mut(&mut blocks[0]).cast(), slices),
            (ptr::from_mut(&mut blocks[1]).cast(), slices)
        ]
    );
}

#[cfg(not(miri))]
#[test]
fn taking_cached_spans_removes_exact_bytes_and_the_exhausted_bin_bit() {
    for slices in [2, 66, MEDIUM_MAX_SLICES + 1] {
        let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
        // SAFETY: the domain and its shard are process-retained.
        let backing = unsafe { domain_shard(domain, 0) };
        let bytes = slices * MEDIUM_SLICE_SIZE;
        let (address, _) = backing.reserve_slices(domain, slices).unwrap();
        // SAFETY: the fixture owns the entire reserved span.
        assert!(unsafe { hal::commit(address, bytes) });
        tracking::record_mapping(bytes);
        let now = hal::monotonic_millis().max(1);
        // SAFETY: transfer the committed, unregistered span to the private shard.
        unsafe {
            backing.return_span_with_budget(
                address,
                slices,
                100_000,
                now,
                MemoryBudget {
                    limit: usize::MAX,
                    pressured: false,
                },
            );
        }

        assert_eq!(backing.state.lock().retained_bytes, bytes);
        let mut batch = [ptr::null_mut()];
        assert_eq!(backing.take_batch(slices, &mut batch), 1);
        assert_eq!(batch[0], address);
        let mut state = backing.state.lock();
        assert_eq!(state.retained_bytes, 0);
        assert_eq!(state.next_bin(0), None);
        assert_eq!(
            state.demand.target(
                now,
                MemoryBudget {
                    limit: usize::MAX,
                    pressured: false
                }
            ),
            bytes.max(SHARED_CACHE_BYTES)
        );
        drop(state);
        // SAFETY: taking the batch transferred exclusive ownership back here.
        assert!(unsafe { backing.decommit_span(address, slices, 0) });
    }
}

#[cfg(not(miri))]
#[test]
fn opportunistic_purge_resumes_after_its_region_scan_budget() {
    let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
    // SAFETY: the private test domain and its shard are process-retained.
    let backing = unsafe { domain_shard(domain, 0) };
    let mut state = backing.state.lock();
    for _ in 0..=PURGE_SCAN {
        // SAFETY: the shard lock protects publication of each fresh region.
        unsafe { append_region(&mut state, &backing.regions, domain).unwrap() };
    }
    let last = state.last_region;
    drop(state);
    let slices = MEDIUM_MAX_SLICES + 1;
    let bytes = slices * MEDIUM_SLICE_SIZE;
    let (address, region) = backing.reserve_slices(domain, slices).unwrap();
    assert_eq!(region, last);
    // SAFETY: the fixture owns this reservation until it enters the cache.
    assert!(unsafe { hal::commit(address, bytes) });
    tracking::record_mapping(bytes);
    // SAFETY: the private shard lock owns the reserved, committed span.
    unsafe { insert_cached_span(&mut backing.state.lock(), address, slices, 1) };
    let budget = MemoryBudget {
        limit: usize::MAX,
        pressured: false,
    };
    backing.purge_with_budget(false, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, bytes);
    backing.purge_with_budget(false, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, 0);
    assert!(slices_are_free(address, slices));
}

#[cfg(not(miri))]
#[test]
fn large_extent_expiry_is_inclusive_and_forcing_ignores_age() {
    for (force, deadline, now, pressured, purged) in [
        (false, 100, 99, false, false),
        (false, 100, 100, false, true),
        (false, 0, 1, false, false),
        (true, 100, 1, false, true),
        (false, 100, 1, true, true),
    ] {
        let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
        // SAFETY: the domain and its shard are process-retained.
        let backing = unsafe { domain_shard(domain, 0) };
        let slices = MEDIUM_MAX_SLICES + 1;
        let bytes = slices * MEDIUM_SLICE_SIZE;
        let (address, _) = backing.reserve_slices(domain, slices).unwrap();
        // SAFETY: the fixture owns this reservation until cache publication.
        assert!(unsafe { hal::commit(address, bytes) });
        tracking::record_mapping(bytes);
        // SAFETY: the private shard lock owns this committed span.
        unsafe { insert_cached_span(&mut backing.state.lock(), address, slices, deadline) };
        let budget = MemoryBudget {
            limit: usize::MAX,
            pressured,
        };
        backing.purge_with_budget(force, now, budget);
        let retained = backing.state.lock().retained_bytes;
        assert_eq!(
            (retained, slices_are_free(address, slices)),
            (if purged { 0 } else { bytes }, purged),
            "force={force}, deadline={deadline}, now={now}"
        );
        backing.purge_with_budget(true, now, budget);
        assert!(slices_are_free(address, slices));
    }
}

#[cfg(not(miri))]
#[test]
fn opportunistic_large_purge_reclaims_only_one_oversized_span() {
    let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
    // SAFETY: the private domain and its shard are process-retained.
    let backing = unsafe { domain_shard(domain, 0) };
    let slices = MEDIUM_MAX_SLICES + 1;
    let bytes = slices * MEDIUM_SLICE_SIZE;
    let (first, _) = backing.reserve_slices(domain, 2 * slices + 1).unwrap();
    // The reserved gap prevents the two cached spans from coalescing.
    let gap = first.wrapping_add(bytes);
    let second = gap.wrapping_add(MEDIUM_SLICE_SIZE);
    for address in [first, second] {
        // SAFETY: each extent lies entirely in the test's exclusive reservation.
        assert!(unsafe { hal::commit(address, bytes) });
        tracking::record_mapping(bytes);
        // SAFETY: the shard lock receives a distinct committed span.
        unsafe { insert_cached_span(&mut backing.state.lock(), address, slices, 1) };
    }
    let budget = MemoryBudget { limit: 0, pressured: true };
    backing.purge_with_budget(false, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, bytes);
    assert_eq!(
        usize::from(slices_are_free(first, slices)) + usize::from(slices_are_free(second, slices)),
        1
    );
    backing.purge_with_budget(true, 1, budget);
    assert_eq!(backing.state.lock().retained_bytes, 0);
    // SAFETY: only the unused, uncommitted gap remains reserved.
    unsafe { backing.release_slices(gap, 1) };
}
