// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free TLS lookup and persistent reusable owner leases.
//! Non-dropping TLS remains usable during late TLS destructors. Once the
//! destructor-bearing registration has died, each operation uses a temporary
//! pool lease and returns it before finishing, rather than resurrecting TLS.

use std::alloc::Layout;
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{Ordering, fence};

use crate::backend;
use crate::classes::Request;
use crate::core::Owner;

std::thread_local! {
    static CURRENT: Cell<usize> = const { Cell::new(0) };
    static EXPIRED: Cell<bool> = const { Cell::new(false) };
    static TEARDOWN_COUNT: Cell<usize> = const { Cell::new(0) };
    static REGISTRATION: Registration = const { Registration };
}

struct Registration;

impl Drop for Registration {
    fn drop(&mut self) {
        EXPIRED.with(|expired| expired.set(true));
        CURRENT.with(|current| {
            let owner = current.replace(0);
            if owner != 0 {
                // SAFETY: This thread relinquishes its sole owner lease.
                unsafe { release(owner) };
            }
        });
    }
}

struct Pool {
    free: usize,
    back: usize,
    metadata: Option<backend::Local>,
    all: usize,
    count: usize,
}

impl Pool {
    // The source pool is FIFO, not a stack: cycling through idle endpoints also
    // makes their late remote messages progress when fresh threads acquire them.
    unsafe fn pop(&mut self) -> usize {
        let owner = self.free;
        if owner != 0 {
            // SAFETY: The pool lock (or exclusive test instance) owns linkage.
            let endpoint = unsafe { &*(owner as *const Owner) };
            // SAFETY: The pool has exclusive access to this inactive link.
            self.free = unsafe { endpoint.pool_next() };
            if self.free == 0 {
                self.back = 0;
            }
            // SAFETY: The existing pool lock grants this inactive owner a new lease.
            unsafe { endpoint.observation().begin_lease() };
        }
        owner
    }

    unsafe fn push(&mut self, owner: usize) {
        // SAFETY: Caller returns its exclusive lease to this stable endpoint.
        let endpoint = unsafe { &*(owner as *const Owner) };
        // SAFETY: The core action ended before this exclusive pool transfer.
        unsafe { endpoint.observation().end_lease() };
        // SAFETY: The pool exclusively owns the returned endpoint's linkage.
        unsafe { endpoint.set_pool_next(0) };
        if self.back == 0 {
            self.free = owner;
        } else {
            // SAFETY: The existing back is an inactive endpoint owned by the pool.
            let back = unsafe { &*(self.back as *const Owner) };
            // SAFETY: The fresh link appends the exclusively returned endpoint.
            unsafe { back.set_pool_next(owner) };
        }
        self.back = owner;
    }
}

static POOL: Mutex<Pool> = Mutex::new(Pool {
    free: 0,
    back: 0,
    metadata: None,
    all: 0,
    count: 0,
});

fn lock_pool() -> std::sync::MutexGuard<'static, Pool> {
    match POOL.lock() {
        Ok(guard) => guard,
        Err(_) => crate::abort::abort(),
    }
}

fn acquire() -> usize {
    let mut pool = lock_pool();
    // SAFETY: The lock exclusively owns the free endpoint queue.
    let owner = unsafe { pool.pop() };
    if owner != 0 {
        return owner;
    }
    create_owner(&mut pool)
}

fn create_owner(pool: &mut Pool) -> usize {
    let Some(map) = backend::map() else {
        return 0;
    };
    // SAFETY: map is the unique backend map in which global refills register.
    let metadata = pool.metadata.get_or_insert_with(|| unsafe { backend::Local::new(map) });
    let address = metadata.alloc_meta(Owner::ALLOC_SIZE);
    if address != 0 {
        // SAFETY: Fresh, committed ALLOC_SIZE-aligned envelope backing remains
        // live permanently. Only this creation path initializes and donates;
        // acquire returns existing pool entries without reinitialization.
        unsafe { Owner::initialize(map, address) };
        // SAFETY: The freshly initialized persistent owner is still exclusively pool-owned.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: The pool lock serializes one-time registration and first lease creation.
        unsafe { owner.observation().register(pool.all) };
        // SAFETY: This new owner starts inactive and the pool grants its first lease.
        unsafe { owner.observation().begin_lease() };
        pool.all = address;
        pool.count += 1;
        crate::recording::register_source();
    }
    address
}

/// Lends only an existing current-thread lease; observation never creates one.
pub(crate) fn observe_current(action: impl FnOnce(&Owner)) {
    // Non-dropping CURRENT may be unavailable during platform TLS teardown.
    let _ = CURRENT.try_with(|current| {
        let address = current.get();
        if address != 0 {
            // SAFETY: CURRENT holds the native owner lease, outside any core action.
            action(unsafe { &*(address as *const Owner) });
        }
    });
}

pub(crate) fn inventory(round: u64, captured_nanos: u64) -> Result<(crate::observation::OwnerBuffer, u64, bool), seismograph::Error> {
    let capacity = {
        let pool = lock_pool();
        pool.count.saturating_add(8).min(crate::observation::OWNER_LIMIT)
    };
    let mut rows = crate::observation::OwnerBuffer::new(capacity)?;
    let pool = lock_pool();
    let total = pool.count as u64;
    let mut address = pool.all;
    while address != 0 && !rows.full() {
        // SAFETY: Inventory owners persist, and linkage and lease state are pool-protected.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: Lease generations are serialized by this pool lock.
        let generation = unsafe { owner.observation().generation() };
        let leased = generation & 1 != 0;
        let (source, observation) = if leased {
            owner.observation().published()
        } else {
            let mut budget = crate::observation::WALK_BUDGET;
            // SAFETY: An even generation denotes a returned owner; holding the
            // pool lock prevents acquisition or any mutation of its core.
            let mut state = unsafe { (&*owner.core_ptr()).observe(&mut budget) };
            state.generation = generation;
            state.round = round;
            state.captured_nanos = captured_nanos;
            (seismograph_rallocator::native::ObservationSource::IdleInspection, Some(state))
        };
        rows.push(&seismograph_rallocator::native::Owner {
            id: address as u64,
            generation,
            leased,
            source,
            observation,
        });
        // SAFETY: Inventory linkage is immutable after registration and pool-protected.
        address = unsafe { owner.observation().next() };
    }
    Ok((rows, total, address == 0))
}

/// # Safety
/// Caller owns the unique endpoint lease and relinquishes it permanently.
unsafe fn release(address: usize) {
    // SAFETY: Endpoint is stable; its core is uniquely leased by this caller.
    let owner = unsafe { &*(address as *const Owner) };
    // SAFETY: Queue publishers do not access the disjoint UnsafeCell core.
    unsafe { flush_for_teardown(&mut *owner.core_ptr()) };
    let mut pool = lock_pool();
    // SAFETY: pool_next is accessed only under this lock and is not published
    // to a new lease holder until after this function has stopped touching it.
    unsafe { pool.push(address) };
}

fn flush_for_teardown(core: &mut crate::core::Core) {
    // threadalloc.h:110-121 counts the first normal teardown, then throttles
    // flushes of post-teardown leases. The unsigned counter wraps like size_t.
    let count = TEARDOWN_COUNT.with(|counter| {
        let count = counter.get().wrapping_add(1);
        counter.set(count);
        count
    });
    if count < 128 || count.is_power_of_two() {
        core.flush();
    }
}

fn with_owner<R>(failure: R, action: impl FnOnce(&mut crate::core::Core) -> R) -> R {
    CURRENT.with(|current| {
        let owner = current.get();
        if owner == 0 {
            return with_owner_slow(current, failure, action);
        }
        // SAFETY: CURRENT exclusively owns the core.
        // Allocator internals never invoke user code or allocate through GlobalAlloc.
        let core = unsafe { (*(owner as *const Owner)).core_ptr() };
        // SAFETY: CURRENT holds the only core lease for this action.
        unsafe { action(&mut *core) }
    })
}

// TLS registration and temporary-lease cleanup otherwise force saved registers
// and a stack frame onto operations that already have an owner.
#[inline(never)]
fn with_owner_slow<R>(current: &Cell<usize>, failure: R, action: impl FnOnce(&mut crate::core::Core) -> R) -> R {
    let owner = acquire();
    if owner == 0 {
        return failure;
    }
    // A throttled teardown may leave prepared fast lists in this reused owner.
    // Publish everything inherited through the pool mutex before this new lease
    // can return one of those objects for a relaxed free-only handoff.
    fence(Ordering::Release);
    // Publish before registration: platform TLS registration can allocate,
    // but no core borrow exists yet if it re-enters through CURRENT.
    current.set(owner);
    let temporary = EXPIRED.with(Cell::get) || REGISTRATION.try_with(|_| ()).is_err();
    if temporary {
        current.set(0);
    }
    // SAFETY: CURRENT or this temporary lease exclusively owns the core.
    let core = unsafe { (*(owner as *const Owner)).core_ptr() };
    // SAFETY: The action ends before the current or temporary lease is released.
    let result = unsafe { action(&mut *core) };
    if temporary {
        // SAFETY: The action ended and no core borrow is retained by its result.
        unsafe { release(owner) };
    }
    result
}

pub(crate) fn allocate(layout: Layout) -> *mut u8 {
    let Some(request) = Request::new(layout) else {
        return std::ptr::null_mut();
    };
    allocate_request(request)
}

#[inline]
pub(crate) fn allocate_request(request: Request) -> *mut u8 {
    with_owner(std::ptr::null_mut(), |owner| owner.allocate(request))
}

pub(crate) fn allocate_zeroed(layout: Layout) -> *mut u8 {
    let Some(request) = Request::new(layout) else {
        return std::ptr::null_mut();
    };
    with_owner(std::ptr::null_mut(), |owner| {
        let ptr = owner.allocate(request);
        if !ptr.is_null() {
            // SAFETY: The core grants the entire class-size allocation.
            // DefaultConts<YesZero> zeros round_size, not merely requested bytes.
            unsafe { ptr.write_bytes(0, request.size()) };
        }
        ptr
    })
}

/// # Safety
/// ptr is a live allocation and `copy_len` fits both it and request.
pub(crate) unsafe fn reallocate(ptr: *mut u8, request: Request, copy_len: usize) -> *mut u8 {
    with_owner(std::ptr::null_mut(), |owner| {
        // SAFETY: The caller retains ptr only when replacement allocation fails.
        unsafe { owner.reallocate(ptr as usize, request, copy_len) }
    })
}

/// # Safety
/// ptr is a live allocation belonging to this allocator instance.
pub(crate) unsafe fn deallocate(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let processed = with_owner(false, |owner| {
        // SAFETY: The caller transfers its live allocation to this owner lease.
        unsafe { owner.deallocate(ptr as usize) };
        true
    });
    // Deallocation cannot fail. An inability to acquire even allocator metadata
    // is fatal rather than silently leaking the allocation.
    crate::abort::require(processed);
}

#[cfg(test)]
pub(crate) fn flush() {
    CURRENT.with(|current| {
        let owner = current.get();
        if owner != 0 {
            // SAFETY: CURRENT is this thread's exclusive core lease.
            let core = unsafe { (*(owner as *const Owner)).core_ptr() };
            // SAFETY: CURRENT uniquely leases this core until flush returns.
            unsafe { (&mut *core).flush() };
        }
    });
}

/// # Safety
/// ptr is a live allocation, not concurrently freed.
#[cfg(test)]
pub(crate) unsafe fn usable_size(ptr: *mut u8) -> usize {
    CURRENT.with(|current| {
        let owner = current.get();
        if owner != 0 {
            // SAFETY: CURRENT owns a live stable endpoint and its immutable map.
            let core = unsafe { (*(owner as *const Owner)).core_ptr() };
            // SAFETY: The current lease keeps this core live throughout the lookup.
            let core = unsafe { &mut *core };
            // SAFETY: Forward the caller's live-allocation contract.
            return unsafe { core.usable_size(ptr as usize) };
        }
        // A live allocation implies initialization succeeded already.
        let map = backend::initialized_map(backend::map());
        // Pair with a relaxed atomic pointer handoff before reading its entry.
        fence(Ordering::Acquire);
        // SAFETY: The caller guarantees immutable live frontend metadata.
        let (_, encoded) = unsafe { map.lookup(ptr as usize) };
        crate::classes::size(encoded & 127)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes;

    #[test]
    fn poisoned_pool_aborts_before_lending_an_owner() {
        crate::abort::assert_aborts("thread::tests::poisoned_pool_aborts_before_lending_an_owner", || {
            let poisoned = std::panic::catch_unwind(|| {
                let _guard = POOL.lock().unwrap();
                panic!("poison the exclusively locked empty pool");
            });
            assert!(poisoned.is_err());
            drop(lock_pool());
        });
    }

    #[test]
    fn owner_creation_failure_preserves_empty_tls_and_pool() {
        const CHILD: &str = "RALLOCATOR_OWNER_CREATION_FAILURE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // A fresh process isolates first-map failure from other tests' permanent owners.
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "thread::tests::owner_creation_failure_preserves_empty_tls_and_pool"])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        crate::hal::fail_next(crate::hal::Failure::Reserve);
        {
            let mut pool = lock_pool();
            assert_eq!(create_owner(&mut pool), 0);
            assert_eq!(pool.count, 0);
            assert_eq!(pool.all, 0);
            assert!(pool.metadata.is_none());
        }

        let called = Cell::new(false);
        let action = |_: &mut crate::core::Core| {
            called.set(true);
            123
        };
        CURRENT.with(|current| {
            assert_eq!(current.get(), 0);
            crate::hal::fail_next(crate::hal::Failure::Reserve);
            assert_eq!(with_owner_slow(current, -1, action), -1);
            assert!(!called.get());
            assert_eq!(current.get(), 0);
            assert_eq!(lock_pool().count, 0);

            assert_eq!(with_owner_slow(current, -1, action), 123);
            assert!(called.get());
            assert_ne!(current.get(), 0);
            assert_eq!(lock_pool().count, 1);
        });
    }

    #[test]
    fn rejected_layouts_and_null_frees_do_not_acquire_an_owner() {
        std::thread::spawn(|| {
            assert_eq!(CURRENT.with(Cell::get), 0);
            let layout = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
            assert!(allocate(layout).is_null());
            assert!(allocate_zeroed(layout).is_null());
            // SAFETY: Null is explicitly accepted as a no-op before looking up metadata.
            unsafe { deallocate(std::ptr::null_mut()) };
            flush();
            assert_eq!(CURRENT.with(Cell::get), 0);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn usable_size_without_a_thread_lease_reads_the_live_pagemap_entry() {
        let layout = Layout::from_size_align(513, 16).unwrap();
        let address = allocate(layout) as usize;
        assert_ne!(address, 0);
        std::thread::spawn(move || {
            assert_eq!(CURRENT.with(Cell::get), 0);
            // SAFETY: Thread spawn transfers this allocation without freeing or mutating it.
            assert!(unsafe { usable_size(address as *mut u8) } >= layout.size());
            assert_eq!(CURRENT.with(Cell::get), 0);
            // SAFETY: The spawned thread exclusively owns and returns the live allocation once.
            unsafe { deallocate(address as *mut u8) };
            flush();
        })
        .join()
        .unwrap();
        flush();
    }

    #[test]
    fn upstream_sizeclass_oracle() {
        // Independent snmalloc::round_size output from upstream commit
        // 511e91a2de604ce716841f32f5e929a066c4a978, C++20, x64 defaults.
        // Each row stores the inclusive end of a constant-result interval;
        // the fixture was generated from snmalloc, not Rust class tables.
        let reference = include_str!("../tests/fixtures/snmalloc-round-size.csv");
        let mut first = 0;
        for line in reference.lines() {
            let (last, expected) = line.split_once(',').unwrap();
            let last = last.parse::<usize>().unwrap();
            let expected = expected.parse::<usize>().unwrap();
            assert!(last >= first, "oracle intervals must be ordered and nonempty");
            for requested in first..=last {
                let layout = Layout::from_size_align(requested.max(1), 1).unwrap();
                let ptr = allocate(layout);
                assert!(!ptr.is_null(), "allocation failed for {requested}");
                // SAFETY: The checked pointer is the start of a live allocation.
                let actual = unsafe { usable_size(ptr) };
                // SAFETY: The allocation is returned once with its original layout.
                unsafe { deallocate(ptr) };
                assert_eq!(actual, expected, "snmalloc oracle mismatch for {requested}");
            }
            first = last + 1;
        }
        assert_eq!(first, 131_074);
        flush();
    }

    #[test]
    fn usable_size_covers_requested_layouts_without_exposing_a_diagnostic_api() {
        for shift in 0..=20 {
            for size in [1, 15, 16, 17, 31, 48, 63, 64, 65, 513, 4095, 16385, 65535, 65536, 65537] {
                let layout = Layout::from_size_align(size, 1 << shift).unwrap();
                let ptr = allocate(layout);
                assert!(!ptr.is_null());
                // SAFETY: The checked pointer belongs to this live allocation.
                assert!(unsafe { usable_size(ptr) } >= layout.size());
                // SAFETY: The diagnostic lookup ended; the allocation is retired once.
                unsafe { deallocate(ptr) };
            }
        }
        flush();
    }

    fn fresh_owner() -> usize {
        create_owner(&mut lock_pool())
    }

    #[test]
    fn routing_collisions_and_same_slab_rings() {
        // Select the sender from a colliding pair so its own-slot forwarding
        // path runs independently of the OS reservation addresses.
        let mut owners = (0..300).map(|_| fresh_owner()).collect::<Vec<_>>();
        assert!(owners.iter().all(|&owner| owner != 0));
        let mut slots = [None; 256];
        let sender_index = owners
            .iter()
            .enumerate()
            .find_map(|(index, &owner)| {
                let slot = (owner >> Owner::ALLOC_BITS) & 255;
                slots[slot].replace(index).map(|_| index)
            })
            .unwrap();
        let sender = owners.swap_remove(sender_index);
        let mut addresses = Vec::new();
        // These endpoints are leased exclusively to this test, despite
        // modelling many owners without needing hundreds of OS threads.
        {
            for &owner in &owners {
                // SAFETY: Every owner is a fresh persistent endpoint.
                let core = unsafe { (*(owner as *const Owner)).core_ptr() };
                // SAFETY: This test holds its sole core lease.
                let core = unsafe { &mut *core };
                for _ in 0..120 {
                    let request = Request::new(Layout::from_size_align(48, 16).unwrap()).unwrap();
                    let ptr = core.allocate(request);
                    assert!(!ptr.is_null());
                    addresses.push(ptr as usize);
                }
            }
            // SAFETY: sender is another fresh persistent endpoint.
            let core = unsafe { (*(sender as *const Owner)).core_ptr() };
            // SAFETY: The sender is exclusively leased by this test.
            let core = unsafe { &mut *core };
            for address in addresses {
                // SAFETY: Each live allocation is transferred exactly once.
                unsafe { core.deallocate(address) };
            }
            core.flush();
            for _ in 0..10 {
                for &owner in &owners {
                    // SAFETY: Each endpoint remains live and exclusively leased.
                    let core = unsafe { (*(owner as *const Owner)).core_ptr() };
                    // SAFETY: No other consumer accesses this core.
                    unsafe { (&mut *core).flush() };
                }
            }
            let pending: usize = owners
                .iter()
                .chain(std::iter::once(&sender))
                .map(|&address| {
                    // SAFETY: Every endpoint remains permanently live.
                    let queue = unsafe { (*(address as *const Owner)).queue() };
                    assert!(!queue.can_dequeue());
                    queue.retained_message().map_or(0, |message| {
                        // SAFETY: The retained message has an immutable initialized ring.
                        usize::from(unsafe { crate::remote::open(message) }.1)
                    })
                })
                .sum();
            let outstanding: usize = owners
                .iter()
                .chain(std::iter::once(&sender))
                .map(|&address| {
                    // SAFETY: The stable endpoint is exclusively leased here.
                    let core = unsafe { (*(address as *const Owner)).core_ptr() };
                    // SAFETY: No competing lease can modify this core during inspection.
                    unsafe { (&*core).outstanding_objects() }
                })
                .sum();
            assert_eq!(
                outstanding, pending,
                "only the native retained message rings may remain outstanding"
            );
            assert!(pending > 0, "native flush must not manufacture a sentinel to consume every tail");
            for owner in owners {
                // SAFETY: All references from this test's lease have ended.
                unsafe { release(owner) };
            }
            // SAFETY: The sender's exclusive lease is likewise finished.
            unsafe { release(sender) };
        }
    }

    #[test]
    fn flush_retains_user_tail_until_another_real_message_arrives() {
        let owner = fresh_owner();
        let sender = fresh_owner();
        assert_ne!(owner, 0);
        assert_ne!(sender, 0);
        // Two fresh endpoints are leased exclusively by this test;
        // all queue publishing is complete before each consumer operation.
        {
            // SAFETY: owner is a fresh persistent endpoint.
            let endpoint = unsafe { &*(owner as *const Owner) };
            // SAFETY: The test holds its exclusive lease.
            let core = unsafe { &mut *endpoint.core_ptr() };
            // SAFETY: sender is a distinct fresh endpoint.
            let other = unsafe { (*(sender as *const Owner)).core_ptr() };
            // SAFETY: Its lease also belongs solely to this test.
            let other = unsafe { &mut *other };
            let request = Request::new(Layout::from_size_align(1 << 21, 16).unwrap()).unwrap();
            let ptr = core.allocate(request);
            assert!(!ptr.is_null());
            // SAFETY: Offset64 lies within the checked 2MiB allocation.
            let payload = unsafe { ptr.add(64) };
            // SAFETY: This byte is exclusively writable before remote return.
            unsafe { payload.write(0x5a) };
            // SAFETY: The live object is transferred to the sender once.
            unsafe { other.deallocate(ptr as usize) };
            core.flush();
            assert_eq!(endpoint.queue().retained_message(), Some(ptr as usize));
            assert_eq!(core.outstanding_objects(), 1);
            // SAFETY: The retained message keeps its frontend map entry live.
            assert_eq!(unsafe { core.map().lookup(ptr as usize) }.1, owner | request.tag());
            // This retained message still owns its 2MiB committed range.
            // SAFETY: The retained object's byte remains committed and initialized.
            assert_eq!(unsafe { *payload }, 0x5a);

            let successor = core.allocate(Request::new(Layout::from_size_align(16, 16).unwrap()).unwrap());
            assert!(!successor.is_null());
            // SAFETY: The new live object is transferred exactly once.
            unsafe { other.deallocate(successor as usize) };
            other.flush();
            core.flush();
            assert_eq!(endpoint.queue().retained_message(), Some(successor as usize));
            assert_eq!(core.outstanding_objects(), 1);
            // SAFETY: The test has finished using the sender lease.
            unsafe { release(sender) };
            // SAFETY: The owner lease is finished; its retained endpoint persists.
            unsafe { release(owner) };
        }
    }

    #[test]
    fn teardown_fallback_does_not_resurrect_thread_cache() {
        std::thread::spawn(|| {
            let layout = Layout::from_size_align(80, 16).unwrap();
            let ptr = allocate(layout);
            assert!(!ptr.is_null());
            // Exercise the exact destructor transition before subsequent operations.
            drop(Registration);
            assert_eq!(CURRENT.with(Cell::get), 0);
            assert_eq!(TEARDOWN_COUNT.with(Cell::get), 1);
            for i in 0..128 {
                let late = allocate(layout);
                assert!(!late.is_null());
                assert_eq!(CURRENT.with(Cell::get), 0);
                assert_eq!(TEARDOWN_COUNT.with(Cell::get), 2 + i * 2);
                // SAFETY: This newly allocated pointer has not yet been freed.
                unsafe { deallocate(late) };
                assert_eq!(CURRENT.with(Cell::get), 0);
                assert_eq!(TEARDOWN_COUNT.with(Cell::get), 3 + i * 2);
            }
            // SAFETY: ptr remains live across its allocating owner's return to pool.
            unsafe { deallocate(ptr) };
        })
        .join()
        .unwrap();
    }

    #[test]
    fn teardown_throttle_preserves_budget_rings_and_explicit_flush() {
        for (count, should_flush) in [(127, true), (128, true), (129, false), (255, false), (256, true)] {
            std::thread::spawn(move || {
                let owner = fresh_owner();
                let sender = fresh_owner();
                assert_ne!(owner, 0);
                assert_ne!(sender, 0);
                // Both fresh endpoint leases belong exclusively to this test.
                CURRENT.with(|current| current.set(owner));
                let layout = Layout::from_size_align(48, 16).unwrap();
                let first = allocate(layout);
                let second = allocate(layout);
                assert!(!first.is_null() && !second.is_null());
                CURRENT.with(|current| current.set(sender));
                // Both objects remain live until transferred to sender.
                // Incoming queues are stable and have no other publishers here.
                {
                    // SAFETY: owner is a fresh permanent endpoint.
                    let queue = unsafe { (*(owner as *const Owner)).queue() };
                    // SAFETY: first is live and transferred once.
                    unsafe { deallocate(first) };
                    assert_eq!(queue.retained_message(), None);
                    TEARDOWN_COUNT.with(|counter| counter.set(count - 1));
                    // SAFETY: CURRENT now exclusively leases sender.
                    let core = unsafe { (*(sender as *const Owner)).core_ptr() };
                    // SAFETY: No competing lease can access this core.
                    flush_for_teardown(unsafe { &mut *core });
                    assert_eq!(TEARDOWN_COUNT.with(Cell::get), count);
                    assert_eq!(queue.retained_message().is_some(), should_flush);

                    // A performed flush resets the budget, so this free posts
                    // immediately. A skipped flush keeps both objects cached.
                    // SAFETY: second remains live until this transfer.
                    unsafe { deallocate(second) };
                    assert_eq!(queue.can_dequeue(), should_flush);
                    flush();
                    assert_eq!(TEARDOWN_COUNT.with(Cell::get), count);
                    assert!(queue.retained_message().is_some());

                    CURRENT.with(|current| current.set(owner));
                    flush();
                    // Separate posts retain one final object; the unflushed
                    // same-slab ring retains both, matching native queue policy.
                    // SAFETY: CURRENT once again leases owner.
                    let core = unsafe { (*(owner as *const Owner)).core_ptr() };
                    // SAFETY: The unique lease prevents mutation during inspection.
                    assert_eq!(unsafe { (&*core).outstanding_objects() }, if should_flush { 1 } else { 2 });
                    assert_eq!(TEARDOWN_COUNT.with(Cell::get), count);
                    CURRENT.with(|current| current.set(0));
                    // SAFETY: No current TLS lease or reference remains to sender.
                    unsafe { release(sender) };
                    // SAFETY: CURRENT was cleared after the last owner access.
                    unsafe { release(owner) };
                }
            })
            .join()
            .unwrap();
        }
    }

    #[test]
    fn pool_reuse_preserves_live_bootstrap_metadata() {
        let mut pool = Pool {
            free: 0,
            back: 0,
            metadata: None,
            all: 0,
            count: 0,
        };
        let address = create_owner(&mut pool);
        assert_ne!(address, 0);
        let start = address + Owner::LOGICAL_SIZE;
        let end = address + Owner::ALLOC_SIZE;
        let mut live = Vec::new();
        // The isolated pool owns this persistent endpoint; each pop
        // transfers its sole lease here. Every object stays live across reuse.
        {
            // SAFETY: The fresh endpoint's lease transfers to this isolated pool.
            unsafe { pool.push(address) };
            for (index, class) in classes::CLASSES.into_iter().enumerate() {
                // SAFETY: Only this test accesses the pool's linkage.
                let owner = unsafe { pool.pop() };
                assert_eq!(owner, address);
                {
                    // SAFETY: The popped endpoint is live and uniquely leased.
                    let core = unsafe { (*(owner as *const Owner)).core_ptr() };
                    // SAFETY: The core borrow ends before the next pool push.
                    let core = unsafe { &mut *core };
                    let request = Request::new(Layout::from_size_align(class.size, 16).unwrap()).unwrap();
                    let ptr = core.allocate(request);
                    assert!(!ptr.is_null());
                    // SAFETY: The checked live allocation retains its map entry.
                    let (meta, encoded) = unsafe { core.map().lookup(ptr as usize) };
                    assert!(meta >= start && meta + 64 <= end);
                    assert_eq!(encoded, address | request.tag());
                    assert!(!live.iter().any(|&(_, previous)| previous == meta));
                    // SAFETY: The live allocation has at least one writable byte.
                    unsafe { ptr.write(u8::try_from(index).unwrap()) };
                    live.push((ptr, meta));
                    core.flush();
                }
                // SAFETY: The core borrow ended and the lease returns exactly once.
                unsafe { pool.push(owner) };
            }
            // SAFETY: This isolated pool still owns the endpoint's linkage.
            let owner = unsafe { pool.pop() };
            assert_eq!(owner, address);
            {
                // SAFETY: The popped endpoint remains permanently live.
                let core = unsafe { (*(owner as *const Owner)).core_ptr() };
                // SAFETY: This is its sole active lease.
                let core = unsafe { &mut *core };
                for (index, (ptr, meta)) in live.into_iter().enumerate() {
                    // SAFETY: Each initialized payload stayed live across pool reuse.
                    assert_eq!(unsafe { ptr.read() }, u8::try_from(index).unwrap());
                    // SAFETY: The object still retains its immutable frontend entry.
                    assert_eq!(unsafe { core.map().lookup(ptr as usize) }.0, meta);
                    // SAFETY: Each live object is retired exactly once.
                    unsafe { core.deallocate(ptr as usize) };
                }
                core.flush();
                assert!(core.is_empty());
            }
            // SAFETY: The last core borrow has ended.
            unsafe { release(owner) };
        }
    }

    #[test]
    fn sequential_owner_churn_reuses_endpoint() {
        let owners = [acquire(), acquire(), acquire()];
        assert!(owners.iter().all(|&owner| owner != 0));
        let mut pool = Pool {
            free: 0,
            back: 0,
            metadata: None,
            all: 0,
            count: 0,
        };
        // Exclusive leases are transferred to an isolated test pool,
        // using the same FIFO operations as actual thread acquisition/teardown.
        {
            for &owner in &owners {
                // SAFETY: The test transfers each exclusively held lease once.
                unsafe { pool.push(owner) };
            }
            for i in 0..10000 {
                // SAFETY: The isolated pool's linkage is exclusively owned.
                let owner = unsafe { pool.pop() };
                assert_eq!(owner, owners[i % owners.len()]);
                // SAFETY: The popped lease is returned exactly once.
                unsafe { pool.push(owner) };
            }
            while pool.free != 0 {
                // SAFETY: Only this test accesses the remaining pool links.
                let owner = unsafe { pool.pop() };
                // SAFETY: Popping transfers the exclusive lease before release.
                unsafe { release(owner) };
            }
        }
    }
}
