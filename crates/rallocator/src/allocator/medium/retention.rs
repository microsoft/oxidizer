// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free memory-pressure hints and shard-local demand history.
//! Host/container headroom is sampled at most four times per second, never
//! under a shard lock. Hints affect retention only, not allocation correctness.

use super::*;

const SAMPLE_INTERVAL_MS: u64 = 250;
static NEXT_SAMPLE: AtomicU64 = AtomicU64::new(0);
static CACHE_LIMIT: AtomicUsize = AtomicUsize::new(SHARED_CACHE_BYTES * SHARD_COUNT);
static PRESSURED: AtomicBool = AtomicBool::new(false);
static CREDITS: CreditPool = CreditPool::new();
const CREDIT_QUANTUM: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct MemoryBudget {
    pub(super) limit: usize,
    pub(super) pressured: bool,
}

impl MemoryBudget {
    pub(super) fn sample(now: u64) -> Self {
        let next = NEXT_SAMPLE.load(Ordering::Relaxed);
        if now >= next
            && NEXT_SAMPLE
                .compare_exchange(next, now.saturating_add(SAMPLE_INTERVAL_MS), Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            && let Some(memory) = hal::memory_status()
        {
            let budget = Self::from_memory(memory, CREDITS.allocated.load(Ordering::Relaxed));
            CACHE_LIMIT.store(budget.limit, Ordering::Relaxed);
            PRESSURED.store(budget.pressured, Ordering::Relaxed);
        }
        Self {
            limit: CACHE_LIMIT.load(Ordering::Relaxed),
            pressured: PRESSURED.load(Ordering::Relaxed),
        }
    }

    fn from_memory(memory: hal::MemoryStatus, credits: usize) -> Self {
        Self {
            // Existing grants need not shrink just because retained pages reduce
            // available memory. Reserve half the remaining headroom for other
            // users, and never grant more than half of effective memory.
            // This is capacity for measured demand, not a preallocated cache.
            limit: (memory.total / 2).min(credits.saturating_add(memory.available / 2)),
            pressured: memory.available < memory.total / 16,
        }
    }
}

struct CreditPool {
    allocated: AtomicUsize,
}

impl CreditPool {
    const fn new() -> Self {
        Self {
            allocated: AtomicUsize::new(0),
        }
    }

    fn acquire(&self, wanted: usize, limit: usize) -> usize {
        self.acquire_with(wanted, limit, |current, next| {
            self.allocated
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
        })
    }

    fn acquire_with(&self, wanted: usize, limit: usize, mut exchange: impl FnMut(usize, usize) -> Result<usize, usize>) -> usize {
        let mut allocated = self.allocated.load(Ordering::Relaxed);
        // This is a rare quota-growth transaction, not a per-span operation.
        // Bound retries; a contended/empty pool simply grants less retention.
        for _ in 0..8 {
            let granted = wanted.min(limit.saturating_sub(allocated));
            if granted == 0 {
                return 0;
            }
            match exchange(allocated, allocated + granted) {
                Ok(_) => return granted,
                Err(current) => allocated = current,
            }
        }
        0
    }

    fn release(&self, bytes: usize) {
        let previous = self.allocated.fetch_sub(bytes, Ordering::Relaxed);
        debug_assert!(previous >= bytes);
    }
}

/// One shard's retention capacity, shared across all of its heaps.
/// Credits survive short reuse cycles and return to the pool as demand expires.
pub(super) struct CreditLease {
    bytes: usize,
    retry_after: u64,
}

impl CreditLease {
    pub(super) const fn new() -> Self {
        Self { bytes: 0, retry_after: 0 }
    }

    pub(super) fn budget(&mut self, desired: usize, retained: usize, now: u64, budget: MemoryBudget) -> MemoryBudget {
        self.adjust(&CREDITS, desired, retained, now, budget)
    }

    fn adjust(&mut self, pool: &CreditPool, desired: usize, retained: usize, now: u64, budget: MemoryBudget) -> MemoryBudget {
        let wanted = desired.min(budget.limit);
        // Keep enough credit for still-cached pages until reclamation completes.
        // Rounded grants avoid global atomic traffic on every refill or free.
        let needed = wanted.max(retained).saturating_add(CREDIT_QUANTUM - 1) / CREDIT_QUANTUM * CREDIT_QUANTUM;
        if needed < self.bytes {
            pool.release(self.bytes - needed);
            self.bytes = needed;
            self.retry_after = 0;
        }
        if !budget.pressured && retained != 0 && wanted > self.bytes && now >= self.retry_after {
            let goal = wanted.saturating_add(CREDIT_QUANTUM - 1) / CREDIT_QUANTUM * CREDIT_QUANTUM;
            self.bytes += pool.acquire(goal - self.bytes, budget.limit);
            if self.bytes < wanted {
                self.retry_after = now.saturating_add(SAMPLE_INTERVAL_MS);
            }
        }
        let total = pool.allocated.load(Ordering::Relaxed);
        let limit = if total > budget.limit {
            // A lower memory limit applies proportionally until maintenance
            // returns excess credits; no shard gets to ignore global pressure.
            (self.bytes as u128 * budget.limit as u128 / total as u128) as usize
        } else {
            self.bytes
        };
        MemoryBudget {
            limit,
            pressured: budget.pressured,
        }
    }

    pub(super) fn release_unused(&mut self, retained: usize) {
        let keep = retained.min(self.bytes);
        if keep < self.bytes {
            CREDITS.release(self.bytes - keep);
            self.bytes = keep;
            self.retry_after = 0;
        }
    }
}

pub(super) struct Demand {
    leased: usize,
    peak: usize,
    window_peak: usize,
    window_end: u64,
    last_return: u64,
    delay: u64,
}

impl Demand {
    pub(super) const fn new() -> Self {
        Self {
            leased: 0,
            peak: 0,
            window_peak: 0,
            window_end: 0,
            last_return: 0,
            delay: 1000,
        }
    }

    pub(super) fn acquire(&mut self, bytes: usize, now: u64) {
        self.leased += bytes;
        self.observe(now);
    }

    pub(super) fn release(&mut self, bytes: usize, now: u64, delay: u64) {
        // Raw-reservation unit fixtures do not acquire a medium lease. Saturation
        // also keeps this heuristic independent of small/bump region accounting.
        self.leased = self.leased.saturating_sub(bytes);
        self.delay = delay;
        self.last_return = now;
        self.observe(now);
    }

    pub(super) fn retire(&mut self, bytes: usize) {
        self.leased = self.leased.saturating_sub(bytes);
    }

    fn observe(&mut self, now: u64) {
        if now >= self.window_end {
            self.peak = (self.peak / 2).max(self.window_peak).max(self.leased);
            self.window_peak = self.leased;
            self.window_end = now.saturating_add(self.delay.max(1));
        }
        self.window_peak = self.window_peak.max(self.leased);
        self.peak = self.peak.max(self.leased);
    }

    pub(super) fn quiet(&self, now: u64) -> bool {
        self.last_return == 0 || now >= self.last_return.saturating_add(self.delay)
    }

    pub(super) fn target(&mut self, now: u64, budget: MemoryBudget) -> usize {
        self.observe(now);
        if budget.pressured {
            return 0;
        }
        let warm = !self.quiet(now);
        let demand = if warm {
            self.peak.max(SHARED_CACHE_BYTES)
        } else {
            SHARED_CACHE_BYTES
        };
        demand.min(budget.limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(miri))]
    #[test]
    fn unused_credit_and_dropped_shards_return_only_their_own_grants() {
        const CHILD: &str = "RALLOCATOR_CREDIT_RELEASE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "allocator::medium::retention::tests::unused_credit_and_dropped_shards_return_only_their_own_grants",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        // The child runs only this test, so unrelated allocators cannot change
        // the process-wide pool while these grant returns are observed.
        assert_eq!((CACHE_LIMIT.load(Ordering::Relaxed), CREDIT_QUANTUM), (256 << 20, 8 << 20));
        MemoryBudget::sample(1);
        assert_eq!(NEXT_SAMPLE.load(Ordering::Relaxed), 1 + SAMPLE_INTERVAL_MS);
        assert_eq!(CREDITS.allocated.load(Ordering::Relaxed), 0);
        CREDITS.allocated.fetch_add(3 * CREDIT_QUANTUM, Ordering::Relaxed);
        let mut lease = CreditLease {
            bytes: 2 * CREDIT_QUANTUM,
            retry_after: 500,
        };
        lease.release_unused(CREDIT_QUANTUM);
        assert_eq!(
            (lease.bytes, lease.retry_after, CREDITS.allocated.load(Ordering::Relaxed)),
            (CREDIT_QUANTUM, 0, 2 * CREDIT_QUANTUM)
        );
        lease.retry_after = 600;
        lease.release_unused(CREDIT_QUANTUM);
        assert_eq!((lease.bytes, lease.retry_after), (CREDIT_QUANTUM, 600));
        lease.release_unused(0);
        assert_eq!(CREDITS.allocated.load(Ordering::Relaxed), CREDIT_QUANTUM);
        let mut state = MediumState::new();
        state.credit.bytes = CREDIT_QUANTUM;
        drop(state);
        assert_eq!(CREDITS.allocated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn zero_budget_does_not_require_proportional_credit_division() {
        let pool = CreditPool::new();
        let mut lease = CreditLease::new();
        let budget = lease.adjust(&pool, 0, 0, 1, MemoryBudget { limit: 0, pressured: true });
        assert_eq!((budget.limit, budget.pressured), (0, true));
    }

    #[test]
    fn exact_pressure_boundary_keeps_the_budget_unpressured() {
        let memory = hal::MemoryStatus {
            total: 1600,
            available: 100,
        };
        let budget = MemoryBudget::from_memory(memory, 0);
        assert_eq!((budget.limit, budget.pressured), (50, false));
    }

    #[test]
    fn growing_an_existing_credit_lease_rounds_only_the_additional_grant() {
        let pool = CreditPool::new();
        pool.allocated.store(CREDIT_QUANTUM, Ordering::Relaxed);
        let mut lease = CreditLease {
            bytes: CREDIT_QUANTUM,
            retry_after: 0,
        };
        let budget = MemoryBudget {
            limit: 3 * CREDIT_QUANTUM,
            pressured: false,
        };
        let now = (1..=32)
            .find(|attempt| {
                let granted = lease
                    .adjust(&pool, 2 * CREDIT_QUANTUM, 1, attempt * SAMPLE_INTERVAL_MS, budget)
                    .limit;
                assert!(granted <= 2 * CREDIT_QUANTUM, "admission must never exceed the rounded requirement");
                granted == 2 * CREDIT_QUANTUM
            })
            .unwrap()
            * SAMPLE_INTERVAL_MS;
        assert_eq!(
            (lease.bytes, lease.retry_after, pool.allocated.load(Ordering::Relaxed)),
            (2 * CREDIT_QUANTUM, 0, 2 * CREDIT_QUANTUM)
        );
        lease.adjust(&pool, 2 * CREDIT_QUANTUM, 1, now, budget);
        assert_eq!((lease.bytes, lease.retry_after), (2 * CREDIT_QUANTUM, 0));
    }

    #[test]
    fn unchanged_credit_requirement_preserves_the_admission_retry_deadline() {
        let pool = CreditPool::new();
        pool.allocated.store(CREDIT_QUANTUM, Ordering::Relaxed);
        let mut lease = CreditLease {
            bytes: CREDIT_QUANTUM,
            retry_after: 500,
        };
        let budget = MemoryBudget {
            limit: 2 * CREDIT_QUANTUM,
            pressured: false,
        };
        lease.adjust(&pool, CREDIT_QUANTUM, 1, 1, budget);
        assert_eq!(
            (lease.bytes, lease.retry_after, pool.allocated.load(Ordering::Relaxed)),
            (CREDIT_QUANTUM, 500, CREDIT_QUANTUM)
        );
    }

    #[test]
    fn partial_credit_admission_waits_before_requesting_the_remainder() {
        let pool = CreditPool::new();
        pool.allocated.store(CREDIT_QUANTUM / 2, Ordering::Relaxed);
        let mut lease = CreditLease::new();
        let partial = CREDIT_QUANTUM + CREDIT_QUANTUM / 2;
        let budget = MemoryBudget {
            limit: 2 * CREDIT_QUANTUM,
            pressured: false,
        };
        let mut now = 1;
        adjust_after_transient_failures(&mut lease, &pool, 2 * CREDIT_QUANTUM, 1, &mut now, budget, partial);
        assert_eq!(lease.retry_after, now + SAMPLE_INTERVAL_MS);
        let larger = MemoryBudget {
            limit: 3 * CREDIT_QUANTUM,
            pressured: false,
        };
        assert_eq!(
            lease
                .adjust(&pool, 2 * CREDIT_QUANTUM, 1, now + SAMPLE_INTERVAL_MS - 1, larger)
                .limit,
            partial
        );
        now += SAMPLE_INTERVAL_MS;
        assert_eq!(lease.adjust(&pool, partial, 1, now, larger).limit, partial);
        adjust_after_transient_failures(&mut lease, &pool, 2 * CREDIT_QUANTUM, 1, &mut now, larger, 2 * CREDIT_QUANTUM);
        assert_eq!(pool.allocated.load(Ordering::Relaxed), 2 * CREDIT_QUANTUM + CREDIT_QUANTUM / 2);
    }

    #[test]
    fn retired_demand_is_subtracted_and_old_peaks_decay_each_window() {
        let mut demand = Demand::new();
        let peak = 8 * SHARED_CACHE_BYTES;
        demand.acquire(peak, 1);
        demand.retire(peak / 4);
        assert_eq!(demand.leased, peak * 3 / 4);
        demand.retire(usize::MAX);
        assert_eq!(demand.leased, 0);
        demand.observe(1001);
        demand.observe(2001);
        assert_eq!((demand.peak, demand.window_peak, demand.window_end), (peak / 2, 0, 3001));
        demand.observe(3001);
        assert_eq!(demand.peak, peak / 4);
    }

    #[test]
    fn fresh_medium_refill_leases_every_committed_span() {
        let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
        // SAFETY: this test uses the standard allocator configuration.
        let allocator: Rallocator = unsafe { Rallocator::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain);
        let layout = Layout::from_size_align(2 * MEDIUM_SLICE_SIZE, 16).unwrap();
        let backing = heap_regions(&mut heap);
        let mut observations = [(0, 0); 3];
        let addresses: [_; 3] = std::array::from_fn(|index| {
            let address = allocator.allocate_medium(layout, &mut heap, None);
            assert!(!address.is_null());
            observations[index] = (backing.state.lock().demand.leased, heap.medium_batch.bytes);
            address
        });
        for address in addresses {
            // SAFETY: each allocation is live and owned exclusively by this heap.
            unsafe { allocator.deallocate_medium(address, layout, ptr::from_mut(&mut heap)) };
        }
        while let Some(cached) = heap.medium_batch.pop(1) {
            // SAFETY: the cache transfers exclusive ownership of this two-slice span.
            unsafe { backing.decommit_span(cached, 2, 0) };
        }

        assert_eq!(
            observations,
            [(layout.size(), 0), (3 * layout.size(), layout.size()), (3 * layout.size(), 0)]
        );
    }

    #[cfg(not(miri))]
    fn assert_cache_misses_release_idle_credits(size: usize, cache_limit: usize) {
        let domain = crate::domain::state(crate::domain::Domain::new().unwrap());
        // SAFETY: this test retains its isolated domain and bounds every index.
        let idle_shards: [_; SHARD_COUNT - 1] = std::array::from_fn(|index| unsafe { domain_shard(domain, index + 1) });
        for shard in idle_shards {
            let mut state = shard.state.lock();
            assert_eq!(state.credit.bytes, 0);
            assert_eq!(state.retained_bytes, 0);
            // Seed an existing lease independently of admission contention and OS
            // memory hints. Add to the pool; never reset other tests' credits.
            CREDITS.allocated.fetch_add(CREDIT_QUANTUM, Ordering::Relaxed);
            state.credit.bytes = CREDIT_QUANTUM;
        }

        // SAFETY: this test uses the standard allocator configuration.
        let allocator: Rallocator = unsafe { Rallocator::new() };
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        // SAFETY: the test exclusively owns this heap until retirement below.
        unsafe { (*heap).medium_cache_max_bytes = cache_limit };
        let layout = Layout::from_size_align(size, 16).unwrap();
        for allocation in 1..=MAINTENANCE_INTERVAL * SHARD_COUNT {
            // SAFETY: the heap is live, exclusively borrowed, and not moved.
            let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
            assert!(!address.is_null());
            // SAFETY: this is the only live allocation and belongs to this heap.
            unsafe { allocator.deallocate_medium(address, layout, heap) };
            assert_eq!(unsafe { (*heap).medium_batch.bytes }, 0);
            if allocation == MAINTENANCE_INTERVAL - 1 {
                assert_eq!(idle_shards[0].state.lock().credit.bytes, CREDIT_QUANTUM);
            }
            if allocation == MAINTENANCE_INTERVAL {
                assert_eq!(idle_shards[0].state.lock().credit.bytes, 0);
            }
        }
        for shard in idle_shards {
            assert_eq!(shard.state.lock().credit.bytes, 0);
        }
        // SAFETY: every allocation was freed and this test relinquishes the heap.
        unsafe { retire_general_heap(heap) };
        // SAFETY: shard zero is embedded in the test's process-retained domain.
        unsafe { domain_shard(domain, 0) }.purge(true, u64::MAX);
    }

    #[cfg(not(miri))]
    #[test]
    fn spans_above_the_local_budget_release_idle_credits() {
        let size = 8 * 1024 * 1024;
        assert!(size > LOCAL_CACHE_BYTES);
        assert_cache_misses_release_idle_credits(size, size);
    }

    #[cfg(not(miri))]
    #[test]
    fn disabled_local_cache_releases_idle_credits() {
        assert_cache_misses_release_idle_credits(MEDIUM_SLICE_SIZE, 0);
    }

    #[test]
    fn demand_retains_short_reuse_then_shrinks_and_honors_pressure() {
        let mut demand = Demand::new();
        let budget = MemoryBudget {
            limit: 1024 * 1024 * 1024,
            pressured: false,
        };
        let burst = 64 * 1024 * 1024;
        demand.acquire(burst, 1);
        demand.release(burst, 2, 1000);
        assert_eq!(demand.target(500, budget), burst);
        assert_eq!(demand.target(1002, budget), SHARED_CACHE_BYTES);
        assert_eq!(demand.target(500, MemoryBudget { pressured: true, ..budget }), 0);
        assert_eq!(
            demand.target(
                500,
                MemoryBudget {
                    limit: MEDIUM_SLICE_SIZE,
                    ..budget
                }
            ),
            MEDIUM_SLICE_SIZE
        );
    }

    #[test]
    fn budget_is_a_fraction_of_effective_memory_not_a_fixed_large_cache() {
        let memory = hal::MemoryStatus {
            total: 256 * 1024 * 1024 * 1024,
            available: 128 * 1024 * 1024 * 1024,
        };
        let budget = MemoryBudget::from_memory(memory, 0);
        assert_eq!(budget.limit, memory.available / 2);
        assert!(!budget.pressured);
        assert!(MemoryBudget::from_memory(hal::MemoryStatus { available: 1, ..memory }, 0).pressured);
        assert_eq!(MemoryBudget::from_memory(memory, memory.total).limit, memory.total / 2);
    }

    fn adjust_after_transient_failures(
        lease: &mut CreditLease,
        pool: &CreditPool,
        desired: usize,
        retained: usize,
        now: &mut u64,
        budget: MemoryBudget,
        expected: usize,
    ) {
        // Admission deliberately permits bounded CAS retry failure. Even an
        // uncontended weak CAS can fail spuriously (Miri exercises this), so
        // policy tests must allow the documented later retry rather than assume
        // every first admission attempt succeeds.
        (0..32)
            .find(|_| {
                if lease.adjust(pool, desired, retained, *now, budget).limit == expected {
                    true
                } else {
                    *now += SAMPLE_INTERVAL_MS;
                    false
                }
            })
            .unwrap();
    }

    #[test]
    fn credit_admission_stops_after_bounded_contention() {
        let pool = CreditPool::new();
        let mut attempts = 0;
        let granted = pool.acquire_with(CREDIT_QUANTUM, 2 * CREDIT_QUANTUM, |current, _| {
            attempts += 1;
            Err(current)
        });
        assert_eq!((granted, attempts, pool.allocated.load(Ordering::Relaxed)), (0, 8, 0));
    }

    #[test]
    fn credit_admission_resumes_after_the_retry_deadline() {
        let pool = CreditPool::new();
        let mut lease = CreditLease::new();
        lease.retry_after = SAMPLE_INTERVAL_MS;
        let mut now = 0;
        let budget = MemoryBudget {
            limit: CREDIT_QUANTUM,
            pressured: false,
        };
        adjust_after_transient_failures(
            &mut lease,
            &pool,
            CREDIT_QUANTUM,
            MEDIUM_SLICE_SIZE,
            &mut now,
            budget,
            CREDIT_QUANTUM,
        );
        assert!(now >= SAMPLE_INTERVAL_MS);
        assert_eq!(
            (lease.bytes, pool.allocated.load(Ordering::Relaxed)),
            (CREDIT_QUANTUM, CREDIT_QUANTUM)
        );
    }

    #[test]
    fn credits_follow_demand_and_redistribute_unused_capacity() {
        let pool = CreditPool::new();
        let mut first = CreditLease::new();
        let mut second = CreditLease::new();
        let budget = MemoryBudget {
            limit: 64 * 1024 * 1024,
            pressured: false,
        };
        let demand = 40 * 1024 * 1024;
        let mut now = 1;
        adjust_after_transient_failures(&mut first, &pool, demand, MEDIUM_SLICE_SIZE, &mut now, budget, demand);
        adjust_after_transient_failures(
            &mut second,
            &pool,
            demand,
            MEDIUM_SLICE_SIZE,
            &mut now,
            budget,
            budget.limit - demand,
        );
        assert_eq!(pool.allocated.load(Ordering::Relaxed), budget.limit);
        // Repeated short reuse does not return/reacquire capacity.
        assert_eq!(first.adjust(&pool, demand, 0, now, budget).limit, demand);
        assert_eq!(pool.allocated.load(Ordering::Relaxed), budget.limit);
        now += SAMPLE_INTERVAL_MS;
        first.adjust(&pool, 0, 0, now, budget);
        adjust_after_transient_failures(&mut second, &pool, demand, MEDIUM_SLICE_SIZE, &mut now, budget, demand);
        assert_eq!(pool.allocated.load(Ordering::Relaxed), demand);
    }

    #[test]
    fn concurrent_credit_admission_never_exceeds_the_shared_limit() {
        let pool = CreditPool::new();
        let barrier = std::sync::Barrier::new(16);
        let limit = 8 * CREDIT_QUANTUM;
        let granted = std::thread::scope(|scope| {
            let workers: [_; 16] = std::array::from_fn(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    pool.acquire(2 * CREDIT_QUANTUM, limit)
                })
            });
            workers.into_iter().map(|worker| worker.join().unwrap()).sum::<usize>()
        });
        assert!(granted <= limit);
        assert_eq!(pool.allocated.load(Ordering::Relaxed), granted);
    }

    #[test]
    fn credit_shrink_respects_cached_pages_and_reduced_memory_limit() {
        let pool = CreditPool::new();
        let mut lease = CreditLease::new();
        let budget = MemoryBudget {
            limit: 64 * 1024 * 1024,
            pressured: false,
        };
        let mut now = 1;
        adjust_after_transient_failures(&mut lease, &pool, budget.limit, budget.limit, &mut now, budget, budget.limit);
        let reduced = MemoryBudget {
            limit: budget.limit / 2,
            pressured: false,
        };
        assert_eq!(lease.adjust(&pool, 0, budget.limit, now, reduced).limit, reduced.limit);
        // Credits cannot be returned before their cached pages have been trimmed.
        assert_eq!(pool.allocated.load(Ordering::Relaxed), budget.limit);
        lease.adjust(&pool, 0, 0, now, reduced);
        assert_eq!(pool.allocated.load(Ordering::Relaxed), 0);
    }
}
