// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering, fence};
use std::{cmp, mem, ptr};

use allocation_hints::backend::RawHint;
use allocation_hints::heap::general::{AllocationUsage, Options as GeneralOptions, Usage as GeneralHeapUsage};
use allocation_hints::heap::{Usage as HeapUsage, UsageKind as HeapUsageKind};

use crate::config::{Config, Standard};
use crate::hal;
use crate::hal::{peek_free_requested, read_free_next, read_free_requested, release_free_metadata, write_free_next, write_free_requested};
use crate::heap::bump::{self, BumpState};
use crate::heap::{HeapTarget, target_from_hint};
use crate::telemetry::{self as tracking, HeapKind as TrackingHeapKind, PendingTracking, TrackingAllocation, TrackingState};
use crate::tunables::{MAX_SIZE_CLASSES, SizeClassLayout, SizeClassTables, Tunables, valid_size_classes};
#[cfg(feature = "tuning-telemetry")]
use crate::tuning_telemetry::{self, ClassEvent, MediumEvent};

type ConfigSizeClasses<C> = <<C as Config>::Tunables as Tunables>::SizeClasses;

const SLAB_SIZE: usize = 32 * 1024;
const MEDIUM_SLICE_SIZE: usize = 64 * 1024;
const MEDIUM_MAX_SLICES: usize = hal::MEDIUM_MAX_SLICES;
const LOCAL_MEDIUM_CLASSES: usize = 8;
const MEDIUM_REGION_SIZE: usize = hal::MEDIUM_REGION_SIZE;
const MEDIUM_REGION_SLICE_COUNT: usize = MEDIUM_REGION_SIZE / MEDIUM_SLICE_SIZE;
const MEDIUM_REGION_BITMAP_WORDS: usize = MEDIUM_REGION_SLICE_COUNT.div_ceil(64);
const HEADER_OFFSET: usize = 16;
const MAX_SMALL_ALIGNMENT: usize = 4096;
const MAX_MEDIUM_ALIGNMENT: usize = 64 * 1024;
const DIRECT_SLAB_SEGMENT: u16 = u16::MAX;
const RETIRED_REMOTE_SENTINEL: *mut u8 = ptr::without_provenance_mut(1);
const REMOTE_SLAB_SENTINEL: usize = usize::MAX;
const OPERATION_INSPECTING: usize = 1 << (usize::BITS - 1);
const OPERATION_RETIRED: usize = 1 << (usize::BITS - 2);
const OPERATION_FLAGS: usize = OPERATION_INSPECTING | OPERATION_RETIRED;
const DIRECT_TAG: usize = 15;
const CONTEXT_TAG: usize = 14;
const TAG_MASK: usize = 15;
const SLAB_MARKER: usize = 0x5241_4C4C_4F43_0000;
const CONTEXT_SLAB_MARKER: usize = 0x5241_4C4C_4F43_1000;
const PHYSICAL_SLICE_UNKNOWN: usize = 0;
const PHYSICAL_SLICE_SMALL: usize = 1;
const PHYSICAL_SLICE_MEDIUM: usize = 2;
const PHYSICAL_SLICE_MEDIUM_CONTINUATION: usize = 3;
const PHYSICAL_SLICE_BUMP: usize = 4;
const PHYSICAL_KIND_MASK: usize = 0xff;
const PHYSICAL_SPAN_SHIFT: usize = 8;
const PHYSICAL_SEGMENT_CONTEXT: usize = 1 << (usize::BITS - 1);
const RECYCLED_BITMAP_WORDS: usize = SLAB_SIZE / 16 / 64;

thread_local! {
    // A separate guard performs cleanup while the storage remains usable by later TLS destructors.
    static THREAD_STATE: UnsafeCell<mem::ManuallyDrop<ThreadState>> =
        const { UnsafeCell::new(mem::ManuallyDrop::new(ThreadState::new())) };
    static THREAD_STATE_GUARD: ThreadStateGuard = const { ThreadStateGuard };
    #[cfg(test)]
    static TEST_CAS_BARRIER: std::cell::RefCell<Option<std::sync::Arc<std::sync::Barrier>>> =
        const { std::cell::RefCell::new(None) };
    #[cfg(test)]
    static TEST_FAIL_REMOTE_POP_CAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[cfg(test)]
    static TEST_FAIL_REMOTE_PUSH_CAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[cfg(all(test, not(miri)))]
    static TEST_FAIL_REMOTE_REFILL_CAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[cfg(all(test, not(miri)))]
    static TEST_CLEAR_REMOTE_REFILL_AFTER_SPIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    // Production region metadata is retained for the process lifetime. Tests that unmap synthetic
    // regions clear this cache before releasing their metadata.
    static LAST_REGION: std::cell::Cell<*mut RegionState> = const { std::cell::Cell::new(ptr::null_mut()) };
}

/// A global allocator with thread-local general-purpose size-class slabs.
///
/// Aggregate allocation statistics are disabled by default so the normal
/// allocation path contains no atomic operations. Define a [`crate::config!`]
/// with `track_aggregates: true` when those counters are required.
pub struct Rallocator<C = Standard>
where
    C: Config + Send + Sync + 'static,
    C::Tunables: Send + Sync + 'static,
    ConfigSizeClasses<C>: Send + Sync + 'static,
{
    config: PhantomData<C>,
}
static DOMAINS: AtomicPtr<DomainState> = AtomicPtr::new(ptr::null_mut());
static NEXT_DOMAIN_ID: AtomicUsize = AtomicUsize::new(1);
static DIRECT_ALLOCATIONS: SpinLock<DirectAllocationState> = SpinLock::new(DirectAllocationState { head: ptr::null_mut() });
static NEXT_THREAD_TOKEN: AtomicUsize = AtomicUsize::new(1);

#[cfg(test)]
fn set_test_cas_barrier(barrier: std::sync::Arc<std::sync::Barrier>) {
    TEST_CAS_BARRIER.with(|slot| *slot.borrow_mut() = Some(barrier));
}

#[cfg(test)]
fn wait_at_test_cas_barrier() {
    if let Some(barrier) = TEST_CAS_BARRIER.with(|slot| slot.borrow_mut().take()) {
        barrier.wait();
    }
}

#[cfg(test)]
fn fail_next_test_remote_pop_cas() {
    TEST_FAIL_REMOTE_POP_CAS.with(|fail| fail.set(true));
}

#[cfg(test)]
fn fail_next_test_remote_push_cas() {
    TEST_FAIL_REMOTE_PUSH_CAS.with(|fail| fail.set(true));
}

#[cfg(all(test, not(miri)))]
fn force_next_test_remote_refill_contention() {
    TEST_FAIL_REMOTE_REFILL_CAS.with(|fail| fail.set(true));
    TEST_CLEAR_REMOTE_REFILL_AFTER_SPIN.with(|clear| clear.set(true));
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct ExtraHeader {
    mapping_address: *mut u8,
    mapping_size: usize,
    tracking: TrackingAllocation,
    class_index: usize,
    owner: *mut ReusableHeapState,
    next_direct: *mut Self,
    requested_bytes: usize,
    usable_bytes: usize,
}

#[repr(C, align(64))]
struct ThreadState {
    cleanup_registered: bool,
    tearing_down: bool,
    token: usize,
    default_heap: *mut ReusableHeapState,
    remote_heap: *mut RemoteHeapState,
    tracking_session: usize,
    tracking_log: *const TrackingState,
    tracking_identity_registered: bool,
    in_tracking: bool,
    active_heap: *mut ReusableHeapState,
    active_bump: *mut BumpState,
    active_remote: *mut RemoteHeapState,
    bump_pool: [*mut BumpState; 4],
    bump_pool_len: usize,
}

struct ThreadStateGuard;

#[repr(C, align(64))]
pub(crate) struct ReusableHeapState {
    classes: [ClassHot; MAX_SIZE_CLASSES],
    context_classes: [ClassHot; MAX_SIZE_CLASSES],
    class_lists: [ClassCold; MAX_SIZE_CLASSES],
    context_class_lists: [ClassCold; MAX_SIZE_CLASSES],
    locality_next: *mut u8,
    locality_end: *mut u8,
    medium_cache: [*mut u8; LOCAL_MEDIUM_CLASSES],
    locality_segment_slices: usize,
    medium_cache_max_bytes: usize,
    pub(crate) domain: *mut DomainState,
    owner: *mut OwnerState,
    segments: *mut SlabHeader,
    locality_segment: *mut SlabHeader,
    track_aggregates: bool,
    retirable: bool,
    owner_storage: OwnerStorage,
}

#[derive(Clone, Copy)]
struct ClassHot {
    cached: [*mut u8; 2],
    active: *mut SlabHeader,
}

#[derive(Clone, Copy)]
struct ClassCold {
    partial: *mut SlabHeader,
}

#[repr(C)]
struct OwnerState {
    remote_slabs: AtomicPtr<SlabHeader>,
    retirement: *mut RetirementState,
}

#[repr(C)]
struct RetirementState {
    retiring: AtomicBool,
    retirement_ready: AtomicBool,
    operations: AtomicUsize,
    retired_releases: AtomicUsize,
    external_allocations: AtomicUsize,
    heap_state: *mut ReusableHeapState,
}

struct OwnerStorage {
    owner: OwnerState,
    retirement: RetirementState,
}

#[repr(C, align(64))]
pub(crate) struct RemoteHeapState {
    owner: *mut OwnerState,
    embedded_owner: OwnerState,
    owner_token: AtomicUsize,
    owner_heap: AtomicPtr<ReusableHeapState>,
    pub(crate) domain: *mut DomainState,
    options: GeneralOptions,
    usage: RemoteUsage,
    classes: [RemoteClass; MAX_SIZE_CLASSES],
    context_classes: [RemoteClass; MAX_SIZE_CLASSES],
}

struct RemoteUsage {
    operations: AtomicUsize,
    live_allocations: AtomicUsize,
    requested_bytes: AtomicUsize,
    usable_bytes: AtomicUsize,
    reserved_bytes: AtomicUsize,
    committed_bytes: AtomicUsize,
    slab_count: AtomicUsize,
    slice_count: AtomicUsize,
}

struct RemoteClass {
    blocks: AtomicPtr<u8>,
    popping: AtomicBool,
    refilling: AtomicBool,
}

#[repr(C, align(64))]
struct SlabHeader {
    marker: AtomicUsize,
    owner: *mut OwnerState,
    fresh_next: *mut u8,
    next_partial: *mut Self,
    free_count: usize,
    recycled_summary: u32,
    recycled_batch_word: u32,
    recycled_batch: u64,
    segment_next: *mut Self,
    remote_free: AtomicPtr<u8>,
    inbox_next: *mut Self,
    embedded_owner: OwnerState,
    remote_queued: AtomicBool,
    usable_blocks: u16,
    segment_slices: u16,
    block_size: u16,
    header_padding: u16,
    segment_committed_bytes: usize,
    requested_bytes: usize,
    remote_padding: [u8; 0],
    recycled: [u64; RECYCLED_BITMAP_WORDS],
}

#[repr(C)]
struct RetiredSliceState {
    marker: AtomicUsize,
    owner: *mut OwnerState,
    remaining: AtomicUsize,
    state: *mut Self,
    ready: AtomicBool,
    released: AtomicBool,
    track_aggregates: bool,
    direct_mapping: bool,
    state_padding: [u8; 4],
    committed_bytes: usize,
    release_bytes: usize,
}

struct SlabAllocation {
    address: *mut u8,
    segment_slices: u16,
    committed_bytes: usize,
}

struct MediumRegion {
    regions: AtomicPtr<RegionState>,
    state: SpinLock<MediumState>,
}

#[repr(C, align(64))]
pub(crate) struct DomainState {
    id: usize,
    is_default: AtomicBool,
    regions: MediumRegion,
    next: AtomicPtr<Self>,
}

struct MediumState {
    regions: *mut RegionState,
    last_region: *mut RegionState,
}

struct DirectAllocationState {
    head: *mut ExtraHeader,
}

struct RegionState {
    base: *mut u8,
    domain: *mut DomainState,
    next_slice: usize,
    large_free: *mut LargeFreeBlock,
    large_purge_after: u64,
    used: [u64; MEDIUM_REGION_BITMAP_WORDS],
    physical: [PhysicalSliceMeta; MEDIUM_REGION_SLICE_COUNT],
    allocations: [MediumAllocationMeta; MEDIUM_REGION_SLICE_COUNT],
    bins: [MediumBin; MEDIUM_MAX_SLICES],
    next: AtomicPtr<Self>,
}

struct PhysicalSliceMeta {
    kind_and_span: AtomicUsize,
    owner: AtomicUsize,
    segments: [AtomicUsize; 2],
    segment_usable_blocks: [AtomicUsize; 2],
    segment_live_blocks: [AtomicUsize; 2],
    segment_utilization_tracked: [AtomicBool; 2],
}

struct MediumAllocationMeta {
    owner: AtomicPtr<ReusableHeapState>,
    requested_bytes: AtomicUsize,
    usable_bytes: AtomicUsize,
}

#[derive(Clone, Copy)]
struct MediumBin {
    free_list: *mut MediumFreeBlock,
    purge_after: u64,
}

#[repr(C)]
struct MediumFreeBlock {
    next: *mut Self,
}

#[repr(C)]
struct LargeFreeBlock {
    next: *mut Self,
    slice_count: usize,
}

// SAFETY: MediumState is only accessed while its containing SpinLock is held.
unsafe impl Send for MediumState {}
// SAFETY: DomainState is process-retained. Its registry links are atomic, and
// each region's mutable allocator metadata is protected by its SpinLock.
unsafe impl Send for DomainState {}
unsafe impl Sync for DomainState {}
// SAFETY: list mutation is serialized by DIRECT_ALLOCATIONS, and next_direct
// is only accessed while its allocation remains registered in that list.
unsafe impl Send for DirectAllocationState {}
// SAFETY: RegionState is only accessed through the owning region lock, except
// for immutable identity and atomic publication fields.
unsafe impl Send for RegionState {}
// SAFETY: RemoteHeapState is process-retained. Published queues and counters
// are atomic; owner-only mutable fields are never accessed by remote threads.
unsafe impl Send for RemoteHeapState {}
unsafe impl Sync for RemoteHeapState {}

impl MediumBin {
    const fn new() -> Self {
        Self {
            free_list: ptr::null_mut(),
            purge_after: 0,
        }
    }
}

impl MediumAllocationMeta {
    const fn new() -> Self {
        Self {
            owner: AtomicPtr::new(ptr::null_mut()),
            requested_bytes: AtomicUsize::new(0),
            usable_bytes: AtomicUsize::new(0),
        }
    }
}

impl PhysicalSliceMeta {
    const fn new() -> Self {
        Self {
            kind_and_span: AtomicUsize::new(PHYSICAL_SLICE_UNKNOWN),
            owner: AtomicUsize::new(0),
            segments: [const { AtomicUsize::new(0) }; 2],
            segment_usable_blocks: [const { AtomicUsize::new(0) }; 2],
            segment_live_blocks: [const { AtomicUsize::new(0) }; 2],
            segment_utilization_tracked: [const { AtomicBool::new(false) }; 2],
        }
    }
}

#[repr(C, align(64))]
struct SpinLock<T> {
    locked: AtomicBool,
    lock_padding: [u8; 63],
    value: UnsafeCell<T>,
}

// SAFETY: SpinLock grants access to its UnsafeCell payload to only one holder,
// and T: Send permits that exclusive access to move between threads.
unsafe impl<T: Send> Sync for SpinLock<T> {}

struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

struct UsageInspectionGuard {
    retirement: *const RetirementState,
    remote: *mut RemoteHeapState,
}

struct HeapOperationGuard {
    retirement: *mut RetirementState,
}

struct ExternalAllocationReleaseGuard {
    retirement: *mut RetirementState,
}

impl Drop for UsageInspectionGuard {
    fn drop(&mut self) {
        if !self.remote.is_null() {
            unsafe { (*self.remote).usage.operations.store(0, Ordering::Release) };
        }
        unsafe { (*self.retirement).operations.store(0, Ordering::Release) };
    }
}

impl HeapOperationGuard {
    unsafe fn begin(retirement: *mut RetirementState) -> Option<Self> {
        unsafe { begin_heap_usage_operation(retirement) }.then(|| Self { retirement })
    }
}

impl Drop for HeapOperationGuard {
    fn drop(&mut self) {
        unsafe { end_heap_usage_operation(self.retirement) };
    }
}

impl ExternalAllocationReleaseGuard {
    fn new(retirement: *mut RetirementState, retained: bool) -> Option<Self> {
        retained.then_some(Self { retirement })
    }
}

impl Drop for ExternalAllocationReleaseGuard {
    fn drop(&mut self) {
        unsafe { release_external_allocation(self.retirement) };
    }
}

struct FastAllocation {
    block: *mut u8,
    kind: FastAllocationKind,
}

#[derive(Clone, Copy)]
enum FastAllocationKind {
    Small(usize),
    Context,
    Medium,
    Direct,
    Bump,
}

const EXTRA_SIZE: usize = size_of::<ExtraHeader>();
const _: () = assert!(bump::BUMP_SEGMENT_SIZE == SLAB_SIZE);
const _: () = assert!(std::mem::offset_of!(SlabHeader, remote_free) == 64);
const _: () = assert!(std::mem::offset_of!(SlabHeader, recycled) == 128);
const _: () = assert!(size_of::<SlabHeader>() == 384);
const _: () = assert!(std::mem::offset_of!(RetiredSliceState, remaining) == 16);
const _: () = assert!(std::mem::offset_of!(RetiredSliceState, state) == 24);
const _: () = assert!(std::mem::offset_of!(RetiredSliceState, ready) == 32);
const _: () = assert!(std::mem::offset_of!(RetiredSliceState, committed_bytes) == 40);
const _: () = assert!(std::mem::offset_of!(SpinLock<MediumState>, value) == 64);

impl ThreadState {
    const fn new() -> Self {
        Self {
            cleanup_registered: false,
            tearing_down: false,
            token: 0,
            default_heap: ptr::null_mut(),
            remote_heap: ptr::null_mut(),
            tracking_session: 0,
            tracking_log: ptr::null(),
            tracking_identity_registered: false,
            in_tracking: false,
            active_heap: ptr::null_mut(),
            active_bump: ptr::null_mut(),
            active_remote: ptr::null_mut(),
            bump_pool: [ptr::null_mut(); 4],
            bump_pool_len: 0,
        }
    }

    unsafe fn cleanup(state: *mut Self) {
        debug_assert!(unsafe { (*state).active_heap.is_null() });
        debug_assert!(unsafe { (*state).active_bump.is_null() });
        debug_assert!(unsafe { (*state).active_remote.is_null() });
        let tracking_log = unsafe { (*state).tracking_log };
        unsafe { (*state).tracking_log = ptr::null() };
        unsafe { tracking::release_thread_log(tracking_log) };

        while unsafe { (*state).bump_pool_len != 0 } {
            unsafe { (*state).bump_pool_len -= 1 };
            let index = unsafe { (*state).bump_pool_len };
            let bump_state = unsafe { (*state).bump_pool[index] };
            unsafe { (*state).bump_pool[index] = ptr::null_mut() };
            unsafe { bump::return_global(bump_state) };
        }

        let remote_heap = unsafe { (*state).remote_heap };
        if !remote_heap.is_null() {
            unsafe {
                (*remote_heap).owner_token.store(0, Ordering::Release);
                (*remote_heap).owner_heap.store(ptr::null_mut(), Ordering::Release);
            }
        }

        let heap = unsafe { (*state).default_heap };
        if !heap.is_null() {
            unsafe { (*state).default_heap = ptr::null_mut() };
            unsafe { retire_general_heap(heap) };
        }
    }
}

impl Drop for ThreadStateGuard {
    fn drop(&mut self) {
        THREAD_STATE.with(|storage| {
            let state = storage.get().cast::<ThreadState>();
            unsafe {
                (*state).tearing_down = true;
                ThreadState::cleanup(state);
            }
        });
    }
}

impl RemoteClass {
    const fn new() -> Self {
        Self {
            blocks: AtomicPtr::new(ptr::null_mut()),
            popping: AtomicBool::new(false),
            refilling: AtomicBool::new(false),
        }
    }
}

impl RemoteUsage {
    const fn new() -> Self {
        Self {
            operations: AtomicUsize::new(0),
            live_allocations: AtomicUsize::new(0),
            requested_bytes: AtomicUsize::new(0),
            usable_bytes: AtomicUsize::new(0),
            reserved_bytes: AtomicUsize::new(0),
            committed_bytes: AtomicUsize::new(0),
            slab_count: AtomicUsize::new(0),
            slice_count: AtomicUsize::new(0),
        }
    }
}

impl OwnerState {
    const fn new() -> Self {
        Self {
            remote_slabs: AtomicPtr::new(ptr::null_mut()),
            retirement: ptr::null_mut(),
        }
    }
}

impl RetirementState {
    const fn new() -> Self {
        Self {
            retiring: AtomicBool::new(false),
            retirement_ready: AtomicBool::new(false),
            operations: AtomicUsize::new(0),
            retired_releases: AtomicUsize::new(0),
            external_allocations: AtomicUsize::new(0),
            heap_state: ptr::null_mut(),
        }
    }
}

impl OwnerStorage {
    const fn new() -> Self {
        Self {
            owner: OwnerState::new(),
            retirement: RetirementState::new(),
        }
    }
}

impl ReusableHeapState {
    pub(crate) const fn new(options: GeneralOptions, domain: *mut DomainState) -> Self {
        Self {
            classes: [ClassHot::new(); MAX_SIZE_CLASSES],
            context_classes: [ClassHot::new(); MAX_SIZE_CLASSES],
            class_lists: [ClassCold::new(); MAX_SIZE_CLASSES],
            context_class_lists: [ClassCold::new(); MAX_SIZE_CLASSES],
            locality_next: ptr::null_mut(),
            locality_end: ptr::null_mut(),
            medium_cache: [ptr::null_mut(); LOCAL_MEDIUM_CLASSES],
            locality_segment_slices: options.locality_segment_bytes() / MEDIUM_SLICE_SIZE,
            medium_cache_max_bytes: options.medium_cache_max_bytes(),
            domain,
            owner: ptr::null_mut(),
            segments: ptr::null_mut(),
            locality_segment: ptr::null_mut(),
            track_aggregates: false,
            retirable: false,
            owner_storage: OwnerStorage::new(),
        }
    }
}

impl ClassHot {
    const fn new() -> Self {
        Self {
            cached: [ptr::null_mut(); 2],
            active: ptr::null_mut(),
        }
    }
}

impl ClassCold {
    const fn new() -> Self {
        Self { partial: ptr::null_mut() }
    }
}

impl MediumRegion {
    const fn new() -> Self {
        Self {
            regions: AtomicPtr::new(ptr::null_mut()),
            state: SpinLock::new(MediumState {
                regions: ptr::null_mut(),
                last_region: ptr::null_mut(),
            }),
        }
    }

    fn allocate_slices(&self, domain: *mut DomainState, count: usize) -> Option<*mut u8> {
        let mut state = self.state.lock();
        unsafe { allocate_slices_locked(&mut state, &self.regions, domain, count) }
    }

    unsafe fn release_slices(&self, address: *mut u8, count: usize) {
        let state = self.state.lock();
        let region = unsafe { find_region(&state, address) }.expect("released slices must belong to an allocator region");
        let slice_index = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
        debug_assert_eq!(address.addr() % MEDIUM_SLICE_SIZE, 0);
        debug_assert!(slice_index + count <= MEDIUM_REGION_SLICE_COUNT);
        unsafe {
            for metadata in &(&(*region).physical)[slice_index..slice_index + count] {
                metadata.kind_and_span.store(PHYSICAL_SLICE_UNKNOWN, Ordering::Release);
                metadata.owner.store(0, Ordering::Relaxed);
                metadata.segments[0].store(0, Ordering::Relaxed);
                metadata.segments[1].store(0, Ordering::Relaxed);
                metadata.segment_usable_blocks[0].store(0, Ordering::Relaxed);
                metadata.segment_usable_blocks[1].store(0, Ordering::Relaxed);
                metadata.segment_live_blocks[0].store(0, Ordering::Relaxed);
                metadata.segment_live_blocks[1].store(0, Ordering::Relaxed);
                metadata.segment_utilization_tracked[0].store(false, Ordering::Relaxed);
                metadata.segment_utilization_tracked[1].store(false, Ordering::Relaxed);
            }
            mark_slices(&mut (*region).used, slice_index, count, false);
        }
    }
}

impl DomainState {
    fn new() -> Self {
        Self {
            id: NEXT_DOMAIN_ID.fetch_add(1, Ordering::Relaxed),
            is_default: AtomicBool::new(false),
            regions: MediumRegion::new(),
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

pub(crate) fn mark_default_domain(domain: *mut DomainState) {
    unsafe { (*domain).is_default.store(true, Ordering::Release) };
}

pub(crate) fn create_domain() -> *mut DomainState {
    let state = hal::map(mem::size_of::<DomainState>()).cast::<DomainState>();
    if state.is_null() {
        return ptr::null_mut();
    }
    unsafe { state.write(DomainState::new()) };

    let mut head = DOMAINS.load(Ordering::Acquire);
    loop {
        unsafe { (*state).next.store(head, Ordering::Relaxed) };
        #[cfg(test)]
        wait_at_test_cas_barrier();
        match DOMAINS.compare_exchange_weak(head, state, Ordering::Release, Ordering::Acquire) {
            Ok(_) => return state,
            Err(current) => head = current,
        }
    }
}

#[inline(always)]
unsafe fn domain_regions(domain: *mut DomainState) -> &'static MediumRegion {
    debug_assert!(!domain.is_null());
    unsafe { &(*domain).regions }
}

impl<T> SpinLock<T> {
    const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            lock_padding: [0; 63],
            value: UnsafeCell::new(value),
        }
    }

    fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
        SpinLockGuard { lock: self }
    }
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<C> Rallocator<C>
where
    C: Config + Send + Sync + 'static,
    C::Tunables: Send + Sync + 'static,
    ConfigSizeClasses<C>: Send + Sync + 'static,
{
    const VALIDATE_TUNABLES: () = assert!(
        C::Tunables::PARTIAL_SLAB_SCAN_LIMIT != 0 && valid_size_classes(ConfigSizeClasses::<C>::SIZES),
        "invalid allocator tunables"
    );
    const VALIDATE_CONFIG: () = tracking::validate_config::<C>();

    /// Creates an allocator handle for the process-global allocator state.
    ///
    /// # Safety
    ///
    /// Every `Rallocator` used in the process must use identical tunables and
    /// caller-tracking configuration because those options affect allocation
    /// representation. `TRACK_AGGREGATES` may differ because aggregate counters
    /// do not alter shared allocator state. Prefer [`crate::rallocator!`] for
    /// the global allocator.
    #[must_use]
    pub const unsafe fn new() -> Self {
        let () = Self::VALIDATE_TUNABLES;
        let () = Self::VALIDATE_CONFIG;
        Self { config: PhantomData }
    }

    #[cold]
    #[inline(never)]
    fn refill(&self, class_index: usize, state: &mut ReusableHeapState) -> *mut u8 {
        let slab = self.allocate_slab(state);
        if slab.address.is_null() {
            return ptr::null_mut();
        }
        let block = self.initialize_slab(slab, class_index, state, SLAB_MARKER);
        if !block.is_null() {
            record_class_event(class_index, ClassEventKind::SlabRefill);
            record_class_event(class_index, ClassEventKind::Allocation);
        }
        block
    }

    #[cold]
    #[inline(never)]
    fn refill_context(&self, class_index: usize, state: &mut ReusableHeapState) -> *mut u8 {
        let slab = self.allocate_slab(state);
        if slab.address.is_null() {
            return ptr::null_mut();
        }
        let block = self.initialize_slab(slab, class_index, state, CONTEXT_SLAB_MARKER);
        if !block.is_null() {
            record_class_event(class_index, ClassEventKind::SlabRefill);
            record_class_event(class_index, ClassEventKind::Allocation);
        }
        block
    }

    #[cold]
    #[inline(never)]
    fn pop_or_refill_remote(&self, remote: *mut RemoteHeapState, class_index: usize, context: bool, requested_bytes: usize) -> *mut u8 {
        unsafe { begin_remote_usage_operation(remote) };
        let class = unsafe {
            if context {
                (*remote).context_classes.get_unchecked(class_index)
            } else {
                (*remote).classes.get_unchecked(class_index)
            }
        };
        loop {
            let block = unsafe { pop_remote_block(class) };
            if !block.is_null() {
                unsafe {
                    record_remote_small_allocation(remote, requested_bytes, ConfigSizeClasses::<C>::SIZES[class_index]);
                    end_remote_usage_operation(remote);
                }
                record_class_event(class_index, ClassEventKind::Allocation);
                return block;
            }
            #[cfg(all(test, not(miri)))]
            let refill = TEST_FAIL_REMOTE_REFILL_CAS.with(|fail| {
                if fail.replace(false) {
                    class.refilling.store(true, Ordering::Relaxed);
                    Err(true)
                } else {
                    class.refilling.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                }
            });
            #[cfg(any(not(test), miri))]
            let refill = class.refilling.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed);
            if refill.is_err() {
                while class.refilling.load(Ordering::Acquire) {
                    spin_loop();
                    #[cfg(all(test, not(miri)))]
                    TEST_CLEAR_REMOTE_REFILL_AFTER_SPIN.with(|clear| {
                        if clear.replace(false) {
                            class.refilling.store(false, Ordering::Release);
                        }
                    });
                }
                continue;
            }

            let regions = unsafe { domain_regions((*remote).domain) };
            let slice = regions.allocate_slices(unsafe { (*remote).domain }, 1).unwrap_or(ptr::null_mut());
            if slice.is_null() || !unsafe { hal::commit(slice, MEDIUM_SLICE_SIZE) } {
                if !slice.is_null() {
                    unsafe { regions.release_slices(slice, 1) };
                }
                class.refilling.store(false, Ordering::Release);
                unsafe { end_remote_usage_operation(remote) };
                return ptr::null_mut();
            }

            let marker = if context { CONTEXT_SLAB_MARKER } else { SLAB_MARKER };
            self.record_mapping(MEDIUM_SLICE_SIZE);
            unsafe {
                (*remote).usage.reserved_bytes.fetch_add(MEDIUM_SLICE_SIZE, Ordering::Relaxed);
                (*remote).usage.committed_bytes.fetch_add(MEDIUM_SLICE_SIZE, Ordering::Relaxed);
                (*remote).usage.slab_count.fetch_add(2, Ordering::Relaxed);
                (*remote).usage.slice_count.fetch_add(1, Ordering::Relaxed);
            }
            unsafe {
                self.initialize_remote_slab(slice, class_index, (*remote).owner, marker, class);
                self.initialize_remote_slab(slice.add(SLAB_SIZE), class_index, (*remote).owner, marker, class);
            }
            record_class_event(class_index, ClassEventKind::SlabRefill);
            record_class_event(class_index, ClassEventKind::SlabRefill);
            class.refilling.store(false, Ordering::Release);
        }
    }

    unsafe fn initialize_remote_slab(&self, slab: *mut u8, class_index: usize, owner: *mut OwnerState, marker: usize, class: &RemoteClass) {
        let block_size = ConfigSizeClasses::<C>::SIZES[class_index];
        let block_count = SLAB_SIZE / block_size;
        let first_block = size_of::<SlabHeader>().div_ceil(block_size);
        debug_assert!(first_block < block_count);
        let header = slab.cast::<SlabHeader>();
        unsafe {
            header.write(SlabHeader {
                marker: AtomicUsize::new(marker | class_index),
                owner,
                fresh_next: ptr::null_mut(),
                next_partial: ptr::null_mut(),
                free_count: 0,
                recycled_summary: 0,
                recycled_batch_word: 0,
                recycled_batch: 0,
                segment_next: ptr::null_mut(),
                remote_free: AtomicPtr::new(ptr::null_mut()),
                inbox_next: ptr::null_mut(),
                embedded_owner: OwnerState::new(),
                remote_queued: AtomicBool::new(false),
                usable_blocks: (block_count - first_block) as u16,
                segment_slices: 0,
                block_size: block_size as u16,
                header_padding: 0,
                segment_committed_bytes: REMOTE_SLAB_SENTINEL,
                requested_bytes: 0,
                remote_padding: [],
                recycled: [0; RECYCLED_BITMAP_WORDS],
            });
        }
        record_small_segment(
            slab,
            class_index,
            marker == CONTEXT_SLAB_MARKER,
            owner,
            block_count - first_block,
            C::TRACK_AGGREGATES,
        );
        for block_index in first_block..block_count {
            let block = unsafe { slab.add(block_index * block_size) };
            unsafe { push_remote_available(class, block) };
        }
    }

    fn initialize_slab(&self, allocation: SlabAllocation, class_index: usize, state: &mut ReusableHeapState, marker: usize) -> *mut u8 {
        let slab = allocation.address;
        let active = if marker == CONTEXT_SLAB_MARKER {
            &mut unsafe { state.context_classes.get_unchecked_mut(class_index) }.active
        } else {
            &mut unsafe { state.classes.get_unchecked_mut(class_index) }.active
        };
        let block_size = ConfigSizeClasses::<C>::SIZES[class_index];
        let (first_block, block_count) = slab_block_layout(block_size);

        let result = unsafe { slab.add(first_block * block_size) };
        let next_block = first_block + 1;
        let fresh_next = if next_block < block_count {
            unsafe { slab.add(next_block * block_size) }
        } else {
            ptr::null_mut()
        };
        let header = slab.cast::<SlabHeader>();
        let slab_owner = if state.owner.is_null() {
            unsafe { ptr::addr_of_mut!((*header).embedded_owner) }
        } else {
            state.owner
        };
        unsafe {
            header.write(SlabHeader {
                marker: AtomicUsize::new(marker | class_index),
                owner: slab_owner,
                fresh_next,
                next_partial: ptr::null_mut(),
                free_count: block_count - first_block - 1,
                recycled_summary: 0,
                recycled_batch_word: 0,
                recycled_batch: 0,
                segment_next: ptr::null_mut(),
                remote_free: AtomicPtr::new(ptr::null_mut()),
                inbox_next: ptr::null_mut(),
                embedded_owner: OwnerState::new(),
                remote_queued: AtomicBool::new(false),
                usable_blocks: (block_count - first_block) as u16,
                segment_slices: 0,
                block_size: block_size as u16,
                header_padding: 0,
                segment_committed_bytes: 0,
                requested_bytes: 0,
                remote_padding: [],
                recycled: [0; RECYCLED_BITMAP_WORDS],
            });
        }
        record_small_segment(
            slab,
            class_index,
            marker == CONTEXT_SLAB_MARKER,
            slab_owner,
            block_count - first_block,
            C::TRACK_AGGREGATES,
        );
        if state.owner.is_null() {
            state.owner = slab_owner;
        }
        if allocation.segment_slices != 0 {
            unsafe {
                (*header).segment_next = state.segments;
                (*header).segment_slices = allocation.segment_slices;
                (*header).segment_committed_bytes = allocation.committed_bytes;
            }
            state.segments = header;
            if allocation.segment_slices != DIRECT_SLAB_SEGMENT {
                state.locality_segment = header;
            }
        }
        *active = header;
        result
    }

    fn allocate_slab(&self, heap: &mut ReusableHeapState) -> SlabAllocation {
        heap.track_aggregates = C::TRACK_AGGREGATES;
        if heap.locality_next.addr() < heap.locality_end.addr() {
            let slab = heap.locality_next;
            let Some(committed_bytes) = (unsafe { hal::commit_locality_slab(slab, SLAB_SIZE) }) else {
                return SlabAllocation {
                    address: ptr::null_mut(),
                    segment_slices: 0,
                    committed_bytes: 0,
                };
            };
            if committed_bytes != 0 {
                self.record_mapping(committed_bytes);
                unsafe {
                    (*heap.locality_segment).segment_committed_bytes += committed_bytes;
                }
            }
            heap.locality_next = unsafe { slab.add(SLAB_SIZE) };
            return SlabAllocation {
                address: slab,
                segment_slices: 0,
                committed_bytes: 0,
            };
        }

        let slice_count = heap.locality_segment_slices;
        let regions = unsafe { domain_regions(heap.domain) };
        let segment = regions
            .allocate_slices(heap.domain, slice_count)
            .map(|address| (address, slice_count))
            .or_else(|| regions.allocate_slices(heap.domain, 1).map(|address| (address, 1)));
        let Some((slab, slice_count)) = segment else {
            let slab = hal::map(SLAB_SIZE);
            if !slab.is_null() {
                self.record_mapping(SLAB_SIZE);
            }
            return SlabAllocation {
                address: slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            };
        };

        let segment_size = slice_count * MEDIUM_SLICE_SIZE;
        let Some(committed_bytes) = (unsafe { hal::commit_locality_segment(slab, segment_size, SLAB_SIZE) }) else {
            unsafe { regions.release_slices(slab, slice_count) };
            return SlabAllocation {
                address: ptr::null_mut(),
                segment_slices: 0,
                committed_bytes: 0,
            };
        };
        heap.locality_next = unsafe { slab.add(SLAB_SIZE) };
        heap.locality_end = unsafe { slab.add(segment_size) };
        self.record_mapping(committed_bytes);
        SlabAllocation {
            address: slab,
            segment_slices: slice_count as u16,
            committed_bytes,
        }
    }

    #[cold]
    #[inline(never)]
    fn allocate_medium(&self, layout: Layout, heap: &mut ReusableHeapState) -> *mut u8 {
        let Some(slice_count) = medium_slice_count(layout) else {
            return ptr::null_mut();
        };
        let span_size = slice_count * MEDIUM_SLICE_SIZE;
        heap.track_aggregates = C::TRACK_AGGREGATES;
        if span_size <= heap.medium_cache_max_bytes
            && let Some(cache_index) = local_medium_class(slice_count)
        {
            let cached = unsafe { heap.medium_cache.get_unchecked_mut(cache_index) };
            if !cached.is_null() {
                let address = *cached;
                *cached = ptr::null_mut();
                unsafe { register_medium_allocation(address, layout, heap) };
                self.record_allocation(layout.size());
                record_medium_event(MediumEventKind::TlsCacheHit, 1);
                return address;
            }
        }
        let regions = unsafe { domain_regions(heap.domain) };
        let mut state = regions.state.lock();

        let class_index = medium_class(layout);
        let mut region = state.regions;
        while !region.is_null() {
            let cached = if let Some(class_index) = class_index {
                let bin = unsafe { &mut (*region).bins[class_index] };
                let cached = bin.free_list;
                if !cached.is_null() {
                    bin.free_list = unsafe { (*cached).next };
                    if bin.free_list.is_null() {
                        bin.purge_after = 0;
                    }
                }
                cached.cast()
            } else {
                unsafe { take_large_extent(region, slice_count) }.unwrap_or(ptr::null_mut())
            };
            if !cached.is_null() {
                unsafe { register_medium_allocation(cached, layout, heap) };
                self.record_allocation(layout.size());
                record_medium_event(MediumEventKind::GlobalCacheHit, 1);
                return cached;
            }
            region = unsafe { (*region).next.load(Ordering::Relaxed) };
        }

        self.purge_medium_locked(&mut state, false);
        let Some(address) = (unsafe { allocate_slices_locked(&mut state, &regions.regions, heap.domain, slice_count) }) else {
            return ptr::null_mut();
        };

        if !unsafe { hal::commit(address, span_size) } {
            let region = unsafe { find_region(&state, address) }.expect("allocated span must belong to an allocator region");
            let slice_index = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
            unsafe { mark_slices(&mut (*region).used, slice_index, slice_count, false) };
            return ptr::null_mut();
        }
        unsafe { register_medium_allocation(address, layout, heap) };
        self.record_mapping(span_size);
        self.record_allocation(layout.size());
        record_medium_event(MediumEventKind::FreshCommit, 1);
        address
    }

    #[cold]
    #[inline(never)]
    unsafe fn deallocate_medium(&self, address: *mut u8, layout: Layout, heap: *mut ReusableHeapState) {
        let Some(slice_count) = medium_slice_count(layout) else {
            return;
        };
        let span_size = slice_count * MEDIUM_SLICE_SIZE;
        let mut address = address;
        let (owner, owner_retirable, coordination, operation) = unsafe { unregister_medium_allocation(address, heap) };
        let _operation = operation;
        let _external_release = ExternalAllocationReleaseGuard::new(coordination, owner_retirable);
        let cache_locally = !heap.is_null() && ptr::eq(owner, heap) && span_size <= unsafe { (*heap).medium_cache_max_bytes };
        if cache_locally && let Some(cache_index) = local_medium_class(slice_count) {
            let cached = unsafe { (*heap).medium_cache.get_unchecked_mut(cache_index) };
            let displaced = *cached;
            *cached = address;
            self.record_deallocation(layout.size());
            record_medium_event(MediumEventKind::CachedFree, 1);
            if displaced.is_null() {
                return;
            }
            address = displaced;
        }
        let region = region_containing(address).expect("medium allocation must belong to an allocator region");
        let regions = unsafe { domain_regions((*region).domain) };
        let state = regions.state.lock();
        let region = unsafe { find_region(&state, address) }.expect("medium allocation must belong to an allocator region");
        if let Some(class_index) = medium_class(layout) {
            let bin = unsafe { &mut (*region).bins[class_index] };
            let block = address.cast::<MediumFreeBlock>();
            unsafe {
                block.write(MediumFreeBlock { next: bin.free_list });
            }
            bin.free_list = block;
            if bin.purge_after == 0 {
                bin.purge_after = hal::monotonic_millis().saturating_add(C::Tunables::MEDIUM_PURGE_DELAY_MS);
            }
        } else {
            unsafe { insert_large_extent(region, address, slice_count) };
            if unsafe { (*region).large_purge_after } == 0 {
                unsafe {
                    (*region).large_purge_after = hal::monotonic_millis().saturating_add(C::Tunables::MEDIUM_PURGE_DELAY_MS);
                }
            }
        }
        if !cache_locally || local_medium_class(slice_count).is_none() {
            self.record_deallocation(layout.size());
        }
        record_medium_event(MediumEventKind::GlobalFree, 1);
    }

    fn purge_medium_locked(&self, state: &mut MediumState, force: bool) {
        self.purge_medium_locked_at(state, force, hal::monotonic_millis());
    }

    fn purge_medium_locked_at(&self, state: &mut MediumState, force: bool, now: u64) {
        let mut region = state.regions;
        while !region.is_null() {
            let base = unsafe { (*region).base };
            for class_index in 0..MEDIUM_MAX_SLICES {
                let bin = unsafe { &mut (*region).bins[class_index] };
                let deadline = bin.purge_after;
                if bin.free_list.is_null() || (!force && (deadline == 0 || deadline > now)) {
                    continue;
                }

                let slice_count = class_index + 1;
                let span_size = slice_count * MEDIUM_SLICE_SIZE;
                let mut block = bin.free_list;
                bin.free_list = ptr::null_mut();
                bin.purge_after = 0;

                while !block.is_null() {
                    let next = unsafe { (*block).next };
                    let slice_index = (block.addr() - base.addr()) / MEDIUM_SLICE_SIZE;
                    let decommitted = unsafe { hal::decommit(block.cast(), span_size) };
                    debug_assert!(decommitted || cfg!(test));
                    if decommitted {
                        unsafe { mark_slices(&mut (*region).used, slice_index, slice_count, false) };
                        self.record_unmapping(span_size);
                        if !force {
                            record_medium_event(MediumEventKind::PurgedSpan, 1);
                        }
                    }
                    block = next;
                }
            }

            if !unsafe { (*region).large_free.is_null() }
                && (force || (unsafe { (*region).large_purge_after } != 0 && unsafe { (*region).large_purge_after } <= now))
            {
                let mut block = unsafe { (*region).large_free };
                unsafe {
                    (*region).large_free = ptr::null_mut();
                    (*region).large_purge_after = 0;
                }
                while !block.is_null() {
                    let next = unsafe { (*block).next };
                    let slice_count = unsafe { (*block).slice_count };
                    let span_size = slice_count * MEDIUM_SLICE_SIZE;
                    let slice_index = (block.addr() - base.addr()) / MEDIUM_SLICE_SIZE;
                    let decommitted = unsafe { hal::decommit(block.cast(), span_size) };
                    debug_assert!(decommitted || cfg!(test));
                    if decommitted {
                        unsafe { mark_slices(&mut (*region).used, slice_index, slice_count, false) };
                        self.record_unmapping(span_size);
                        if !force {
                            record_medium_event(MediumEventKind::PurgedSpan, 1);
                        }
                    }
                    block = next;
                }
            }
            region = unsafe { (*region).next.load(Ordering::Relaxed) };
        }
    }

    #[cold]
    #[inline(never)]
    /// Allocates a mapping-backed block without consulting the normal size-class routes.
    ///
    /// # Safety
    ///
    /// When non-null, `owner` must point to a live reusable heap whose domain
    /// remains valid for the allocation. A pending tracking record must belong
    /// to the active tracking state and describe `layout`.
    unsafe fn allocate_direct(
        &self,
        layout: Layout,
        has_context: bool,
        tracking: Option<PendingTracking>,
        owner: *mut ReusableHeapState,
    ) -> *mut u8 {
        let requested_size = cmp::max(layout.size(), 1);
        let default_class = if has_context { default_class::<C::Tunables>(layout) } else { None };
        let discriminator_space = if default_class.is_some() { layout.align() } else { 0 };
        let mapping_size = direct_mapping_size(requested_size, layout.align(), discriminator_space)
            .expect("valid allocation layouts must have a representable mapping size");

        let mapping_address = hal::map(mapping_size);
        if mapping_address.is_null() {
            return ptr::null_mut();
        }
        debug_assert_eq!(
            mapping_address.addr() & (SLAB_SIZE - 1),
            0,
            "direct mappings must be slab-aligned for deallocation classification"
        );
        self.record_mapping(mapping_size);

        let first_user_address = unsafe { mapping_address.add(EXTRA_SIZE + HEADER_OFFSET) };
        let offset = hal::align_offset(first_user_address, layout.align());
        if offset == usize::MAX {
            unsafe { hal::unmap(mapping_address, mapping_size) };
            self.record_unmapping(mapping_size);
            return ptr::null_mut();
        }

        let mut user_address = unsafe { first_user_address.add(offset) };
        if let Some(class_index) = default_class {
            let block_size = ConfigSizeClasses::<C>::SIZES[class_index];
            if user_address.addr() & (block_size - 1) == 0 {
                user_address = unsafe { user_address.add(layout.align()) };
            }
        }
        let extra = mapping_address.cast::<ExtraHeader>();
        let owner_retirable = !owner.is_null() && unsafe { (*owner).retirable };
        if !owner.is_null() {
            unsafe {
                if owner_retirable {
                    initialize_heap_owner(owner);
                }
                retain_external_allocation(owner);
            }
        }
        let tracking = tracking.map_or(TrackingAllocation::NONE, |tracking| {
            tracking.commit(user_address, layout, owner.addr(), TrackingHeapKind::General)
        });
        unsafe {
            extra.write(ExtraHeader {
                mapping_address,
                mapping_size,
                tracking,
                class_index: usize::MAX,
                owner: encode_heap_owner(owner, owner_retirable),
                next_direct: ptr::null_mut(),
                requested_bytes: layout.size(),
                usable_bytes: requested_size,
            });
            write_header(user_address, extra.map_addr(|address| address | DIRECT_TAG));
            register_direct_allocation(extra);
        }
        self.record_allocation(layout.size());
        user_address
    }

    #[cold]
    #[inline(never)]
    unsafe fn allocate_with_context(&self, layout: Layout, tracking: Option<PendingTracking>, state: *mut ThreadState) -> *mut u8 {
        let required_size =
            context_required_size(layout.size(), layout.align()).expect("valid allocation layouts must have a representable context size");
        if layout.align() > MAX_SMALL_ALIGNMENT {
            return unsafe { self.allocate_direct(layout, true, tracking, current_reusable_heap(state)) };
        }
        let Some(class_index) = class_index_for_alignment::<C::Tunables>(required_size, layout.align()) else {
            return unsafe { self.allocate_direct(layout, true, tracking, current_reusable_heap(state)) };
        };
        let heap = unsafe { current_reusable_heap(state) };
        let remote = unsafe { current_remote_heap(state) };
        let block = if remote.is_null() {
            self.pop_or_refill_context(class_index, unsafe { &mut *heap })
        } else {
            self.pop_or_refill_remote(remote, class_index, true, layout.size())
        };
        if block.is_null() {
            return ptr::null_mut();
        }

        let block_size = ConfigSizeClasses::<C>::SIZES[class_index];
        let user_address = unsafe { block.add(context_user_offset(layout.align())) };
        let extra = unsafe { block.add(block_size - EXTRA_SIZE).cast::<ExtraHeader>() };
        let tracking = tracking.map_or(TrackingAllocation::NONE, |tracking| {
            let (heap_id, heap_kind) = if remote.is_null() {
                (heap.addr(), TrackingHeapKind::General)
            } else {
                (remote.addr(), TrackingHeapKind::Thread)
            };
            tracking.commit(user_address, layout, heap_id, heap_kind)
        });
        unsafe {
            extra.write(ExtraHeader {
                mapping_address: ptr::null_mut(),
                mapping_size: 0,
                tracking,
                class_index,
                owner: ptr::null_mut(),
                next_direct: ptr::null_mut(),
                requested_bytes: layout.size(),
                usable_bytes: block_size,
            });
            write_header(user_address, extra.map_addr(|address| address | CONTEXT_TAG));
        }
        if remote.is_null() {
            unsafe { record_small_allocation::<C::Tunables>(block, class_index, layout.size(), remote) };
        }
        if C::TRACK_AGGREGATES {
            record_physical_small_allocation(block);
            tracking::record_small_allocation(class_index, block_size, layout.size());
        }
        user_address
    }

    #[cold]
    #[inline(never)]
    unsafe fn deallocate_direct(&self, address: *mut u8, layout: Layout, extra: *mut ExtraHeader) {
        let metadata = unsafe { *extra };
        let (_, retirement, owner_retirable) = decode_heap_owner(metadata.owner);
        let _operation = (!retirement.is_null())
            .then(|| unsafe { HeapOperationGuard::begin(retirement) })
            .flatten();
        let _external_release = ExternalAllocationReleaseGuard::new(retirement, owner_retirable);
        tracking::record_deallocation(metadata.tracking, address, layout, false);
        self.record_deallocation(layout.size());
        unsafe { unregister_direct_allocation(extra) };
        unsafe { hal::unmap(metadata.mapping_address, metadata.mapping_size) };
        self.record_unmapping(metadata.mapping_size);
    }

    #[cold]
    #[inline(never)]
    unsafe fn deallocate_with_context(&self, address: *mut u8, layout: Layout, extra: *mut ExtraHeader, state: *mut ThreadState) {
        let extra = unsafe { *extra };
        tracking::record_deallocation(extra.tracking, address, layout, false);
        let block = unsafe { address.sub(context_user_offset(layout.align())) };
        if C::TRACK_AGGREGATES {
            record_physical_small_deallocation(block);
            tracking::record_small_deallocation(extra.class_index, layout.size());
        }
        unsafe { push_context_block::<C::Tunables>(block, extra.class_index, layout.size(), state) };
    }

    #[inline(always)]
    fn pop_or_refill(&self, class_index: usize, state: &mut ReusableHeapState) -> *mut u8 {
        let class = unsafe { state.classes.get_unchecked_mut(class_index) };
        if !class.cached[0].is_null() {
            let block = class.cached[0];
            class.cached[0] = class.cached[1];
            class.cached[1] = ptr::null_mut();
            record_class_event(class_index, ClassEventKind::TlsCacheHit);
            record_class_event(class_index, ClassEventKind::Allocation);
            return block;
        }
        let slab = class.active;
        if !slab.is_null() {
            let block = unsafe { take_local_slab_block::<C::Tunables>(slab, class_index) };
            if !block.is_null() {
                return block;
            }
        }
        self.pop_or_refill_slow(class_index, state)
    }

    #[cold]
    #[inline(never)]
    fn pop_or_refill_slow(&self, class_index: usize, state: &mut ReusableHeapState) -> *mut u8 {
        let class = unsafe { state.classes.as_mut_ptr().add(class_index) };
        let lists = unsafe { state.class_lists.as_mut_ptr().add(class_index) };
        let mut slab = unsafe { (*class).active };
        if !slab.is_null() {
            let block = unsafe { take_slab_block::<C::Tunables>(slab, class_index) };
            if !block.is_null() {
                return block;
            }
            unsafe { (*class).active = ptr::null_mut() };
        }
        slab = unsafe { take_most_free_slab::<C::Tunables>(&mut (*lists).partial, class_index) };
        if slab.is_null() {
            unsafe { drain_remote_inbox::<C::Tunables>(state) };
            let lists = unsafe { state.class_lists.as_mut_ptr().add(class_index) };
            slab = unsafe { take_most_free_slab::<C::Tunables>(&mut (*lists).partial, class_index) };
            if !slab.is_null() {
                unsafe { (*slab).next_partial = ptr::null_mut() };
            }
        } else {
            unsafe { (*slab).next_partial = ptr::null_mut() };
        }
        if !slab.is_null() {
            let class = unsafe { state.classes.as_mut_ptr().add(class_index) };
            unsafe { (*class).active = slab };
            let block = unsafe { take_slab_block::<C::Tunables>(slab, class_index) };
            if !block.is_null() {
                return block;
            }
        }
        self.refill(class_index, state)
    }

    #[inline(always)]
    fn pop_or_refill_context(&self, class_index: usize, state: &mut ReusableHeapState) -> *mut u8 {
        let class = unsafe { state.context_classes.get_unchecked_mut(class_index) };
        if !class.cached[0].is_null() {
            let block = class.cached[0];
            class.cached[0] = class.cached[1];
            class.cached[1] = ptr::null_mut();
            record_class_event(class_index, ClassEventKind::TlsCacheHit);
            record_class_event(class_index, ClassEventKind::Allocation);
            return block;
        }
        let slab = class.active;
        if !slab.is_null() {
            let block = unsafe { take_local_slab_block::<C::Tunables>(slab, class_index) };
            if !block.is_null() {
                return block;
            }
        }
        self.pop_or_refill_context_slow(class_index, state)
    }

    #[cold]
    #[inline(never)]
    fn pop_or_refill_context_slow(&self, class_index: usize, state: &mut ReusableHeapState) -> *mut u8 {
        let class = unsafe { state.context_classes.as_mut_ptr().add(class_index) };
        let lists = unsafe { state.context_class_lists.as_mut_ptr().add(class_index) };
        let mut slab = unsafe { (*class).active };
        if !slab.is_null() {
            let block = unsafe { take_slab_block::<C::Tunables>(slab, class_index) };
            if !block.is_null() {
                return block;
            }
            unsafe { (*class).active = ptr::null_mut() };
        }
        slab = unsafe { take_most_free_slab::<C::Tunables>(&mut (*lists).partial, class_index) };
        if slab.is_null() {
            unsafe { drain_remote_inbox::<C::Tunables>(state) };
            let lists = unsafe { state.context_class_lists.as_mut_ptr().add(class_index) };
            slab = unsafe { take_most_free_slab::<C::Tunables>(&mut (*lists).partial, class_index) };
            if !slab.is_null() {
                unsafe { (*slab).next_partial = ptr::null_mut() };
            }
        } else {
            unsafe { (*slab).next_partial = ptr::null_mut() };
        }
        if !slab.is_null() {
            let class = unsafe { state.context_classes.as_mut_ptr().add(class_index) };
            unsafe { (*class).active = slab };
            let block = unsafe { take_slab_block::<C::Tunables>(slab, class_index) };
            if !block.is_null() {
                return block;
            }
        }
        self.refill_context(class_index, state)
    }

    fn record_mapping(&self, size: usize) {
        if C::TRACK_AGGREGATES {
            tracking::record_mapping(size);
        }
    }

    fn record_unmapping(&self, size: usize) {
        if C::TRACK_AGGREGATES {
            tracking::record_unmapping(size);
        }
    }

    #[inline(always)]
    fn record_allocation(&self, size: usize) {
        if C::TRACK_AGGREGATES {
            tracking::record_allocation(size);
        }
    }

    #[inline(always)]
    fn record_deallocation(&self, size: usize) {
        if C::TRACK_AGGREGATES {
            tracking::record_deallocation_stats(size);
        }
    }

    #[inline(always)]
    /// Deallocates a bump allocation previously classified by trusted chunk metadata.
    ///
    /// # Safety
    ///
    /// `address` must be a live allocation owned by `state`, and `layout` must
    /// be the layout used to allocate it. `state` must remain alive through the
    /// call. Tail reclamation is attempted only when the current thread has the
    /// same bump heap active, which provides the exclusive cursor access.
    unsafe fn deallocate_bump(&self, address: *mut u8, layout: Layout, state: *mut BumpState) {
        self.record_deallocation(layout.size());
        let thread = unsafe { thread_state() };
        let reclaim_tail = unsafe { (*thread).active_bump == state };
        if C::TRACK_CALLERS {
            unsafe { bump::deallocate_tracked(state, address, layout, reclaim_tail) };
        } else {
            unsafe { bump::deallocate(state, address, layout, reclaim_tail) };
        }
    }
}

unsafe impl<C> GlobalAlloc for Rallocator<C>
where
    C: Config + Send + Sync + 'static,
    C::Tunables: Send + Sync + 'static,
    ConfigSizeClasses<C>: Send + Sync + 'static,
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let tracking = if C::TRACK_CALLERS {
            tracking::begin_allocation::<C>()
        } else {
            None
        };
        let state = unsafe { thread_state() };
        if unsafe { (*state).tearing_down } {
            return unsafe { self.allocate_direct(layout, false, None, ptr::null_mut()) };
        }
        if C::TRACK_CALLERS && !unsafe { (*state).active_bump }.is_null() {
            let bump = unsafe { (*state).active_bump };
            let address = unsafe { bump::allocate_tracked(bump, layout) };
            if !address.is_null() {
                let allocation = if let Some(tracking) = tracking {
                    tracking.commit(address, layout, bump.addr(), TrackingHeapKind::Bump)
                } else {
                    TrackingAllocation::NONE
                };
                unsafe { bump::set_tracking(address, allocation) };
                self.record_allocation(layout.size());
                return address;
            }
        }
        let allocation = {
            let heap = unsafe { current_reusable_heap(state) };
            if heap.is_null() {
                return ptr::null_mut();
            }
            let remote = unsafe { current_remote_heap(state) };
            if tracking.is_some() {
                FastAllocation {
                    block: ptr::null_mut(),
                    kind: FastAllocationKind::Context,
                }
            } else if !C::TRACK_CALLERS && !unsafe { (*state).active_bump }.is_null() {
                let block = unsafe { bump::allocate((*state).active_bump, layout) };
                if !block.is_null() {
                    FastAllocation {
                        block,
                        kind: FastAllocationKind::Bump,
                    }
                } else if let Some(class_index) = default_class::<C::Tunables>(layout) {
                    FastAllocation {
                        block: if remote.is_null() {
                            self.pop_or_refill(class_index, unsafe { &mut *heap })
                        } else {
                            self.pop_or_refill_remote(remote, class_index, false, layout.size())
                        },
                        kind: FastAllocationKind::Small(class_index),
                    }
                } else if medium_slice_count(layout).is_some() {
                    FastAllocation {
                        block: ptr::null_mut(),
                        kind: FastAllocationKind::Medium,
                    }
                } else {
                    FastAllocation {
                        block: ptr::null_mut(),
                        kind: FastAllocationKind::Direct,
                    }
                }
            } else if let Some(class_index) = default_class::<C::Tunables>(layout) {
                FastAllocation {
                    block: if remote.is_null() {
                        self.pop_or_refill(class_index, unsafe { &mut *heap })
                    } else {
                        self.pop_or_refill_remote(remote, class_index, false, layout.size())
                    },
                    kind: FastAllocationKind::Small(class_index),
                }
            } else if medium_slice_count(layout).is_some() {
                FastAllocation {
                    block: ptr::null_mut(),
                    kind: FastAllocationKind::Medium,
                }
            } else {
                FastAllocation {
                    block: ptr::null_mut(),
                    kind: FastAllocationKind::Direct,
                }
            }
        };

        let class_index = match allocation.kind {
            FastAllocationKind::Context => return unsafe { self.allocate_with_context(layout, tracking, state) },
            FastAllocationKind::Bump => {
                self.record_allocation(layout.size());
                return allocation.block;
            }
            FastAllocationKind::Medium => {
                let heap = unsafe { &mut *current_reusable_heap(state) };
                let address = self.allocate_medium(layout, heap);
                if !address.is_null() {
                    return address;
                }
                return unsafe { self.allocate_direct(layout, false, None, heap) };
            }
            FastAllocationKind::Direct => {
                let heap = unsafe { current_reusable_heap(state) };
                return unsafe { self.allocate_direct(layout, false, None, heap) };
            }
            FastAllocationKind::Small(_) if allocation.block.is_null() => {
                let heap = unsafe { current_reusable_heap(state) };
                return unsafe { self.allocate_direct(layout, false, None, heap) };
            }
            FastAllocationKind::Small(class_index) => class_index,
        };

        let remote = unsafe { current_remote_heap(state) };
        if remote.is_null() {
            unsafe { record_small_allocation::<C::Tunables>(allocation.block, class_index, layout.size(), remote) };
        }
        if C::TRACK_AGGREGATES {
            record_physical_small_allocation(allocation.block);
            tracking::record_small_allocation(class_index, ConfigSizeClasses::<C>::SIZES[class_index], layout.size());
        }
        allocation.block
    }

    unsafe fn dealloc(&self, address: *mut u8, layout: Layout) {
        let region = region_containing(address);
        let segment = allocation_segment(address);
        // Allocations entering this branch are preceded within their segment by
        // allocator-owned metadata. Segment-base allocations are excluded above,
        // so user payload can never own the marker or bump-state prefix.
        let marker = if region.is_some() && address != segment {
            let marker = unsafe { (*segment.cast::<AtomicUsize>()).load(Ordering::Acquire) };
            if let Some(state) = unsafe { bump::state_for_allocation(segment, marker) } {
                unsafe { self.deallocate_bump(address, layout, state) };
                return;
            }
            marker
        } else {
            0
        };

        // Layouts without a small class are either region-backed medium spans or direct mappings.
        if default_class::<C::Tunables>(layout).is_none() {
            if region.is_some() && medium_slice_count(layout).is_some() {
                let state = unsafe { thread_state() };
                let heap = unsafe { current_initialized_reusable_heap(state) };
                unsafe { self.deallocate_medium(address, layout, heap) };
                return;
            }
            let header = unsafe { read_header(address) };
            debug_assert_eq!(header.addr() & TAG_MASK, DIRECT_TAG);
            let extra = header.map_addr(|address| address & !TAG_MASK);
            unsafe { self.deallocate_direct(address, layout, extra) };
            return;
        }
        // Ordinary layouts can still use a direct mapping after a slab fallback. A valid slab
        // marker is therefore required before the address is treated as a small allocation.
        if let Some(class_index) = slab_class_from_marker::<C::Tunables>(marker) {
            if C::TRACK_AGGREGATES {
                record_physical_small_deallocation(address);
                tracking::record_small_deallocation(class_index, layout.size());
            }
            let state = unsafe { thread_state() };
            unsafe { push_block::<C::Tunables>(address, class_index, layout.size(), state) };
            return;
        }

        // A region-owned ordinary layout may be a medium allocation; otherwise its tagged header
        // distinguishes direct mappings from context allocations.
        if !is_context_marker::<C::Tunables>(marker) && region.is_some() && medium_slice_count(layout).is_some() {
            let state = unsafe { thread_state() };
            let heap = unsafe { current_initialized_reusable_heap(state) };
            unsafe { self.deallocate_medium(address, layout, heap) };
            return;
        }

        let header = unsafe { read_header(address) };
        let tag = header.addr() & TAG_MASK;
        let extra = header.map_addr(|address| address & !TAG_MASK);
        if tag == DIRECT_TAG {
            unsafe { self.deallocate_direct(address, layout, extra) };
        } else {
            let state = unsafe { thread_state() };
            unsafe { self.deallocate_with_context(address, layout, extra, state) };
        }
    }
}

pub(crate) fn tracking_target<C: Config>(session: usize) -> Option<*const TrackingState> {
    let state = unsafe { thread_state() };
    if unsafe { (*state).in_tracking } {
        return None;
    }

    if session != unsafe { (*state).tracking_session } {
        let previous_log = unsafe { (*state).tracking_log };
        unsafe {
            (*state).tracking_session = session;
            (*state).tracking_log = ptr::null();
        }
        let previous = enter_tracking_internal();
        let _restore = RestoreTrackingInternal(previous);
        unsafe { tracking::release_thread_log(previous_log) };
        if session != 0 {
            unsafe { (*state).tracking_log = tracking::create_thread_log::<C>(session) };
        }
    }

    let log = unsafe { (*state).tracking_log };
    (!log.is_null()).then_some(log)
}

pub(crate) fn tracking_thread_token() -> usize {
    let state = unsafe { thread_state() };
    let token = unsafe { thread_token(state) };
    if !unsafe { (*state).tracking_identity_registered } {
        crate::telemetry::register_thread_identity(token);
        unsafe { (*state).tracking_identity_registered = true };
    }
    token
}

pub(crate) fn telemetry_region_snapshots() -> Vec<tracking::RegionSnapshot> {
    let mut regions = Vec::new();
    let mut domain = DOMAINS.load(Ordering::Acquire);
    while !domain.is_null() {
        let mut region = unsafe { (*domain).regions.regions.load(Ordering::Acquire) };
        while !region.is_null() {
            regions.push((domain, region));
            region = unsafe { (*region).next.load(Ordering::Acquire) };
        }
        domain = unsafe { (*domain).next.load(Ordering::Acquire) };
    }
    let mut used_bitmaps = (0..regions.len()).map(|_| vec![0; MEDIUM_REGION_BITMAP_WORDS]).collect::<Vec<_>>();
    for ((domain, region), used_bitmap) in regions.iter().zip(&mut used_bitmaps) {
        let _state = unsafe { (*(*domain)).regions.state.lock() };
        used_bitmap.copy_from_slice(unsafe { &(**region).used });
    }

    let mut snapshots = Vec::with_capacity(regions.len());
    for (region_index, ((_, region), used_bitmap)) in regions.into_iter().zip(used_bitmaps).enumerate() {
        let used_slices = used_bitmap.iter().map(|word| word.count_ones() as usize).sum();
        let mut slices = Vec::with_capacity(used_slices);
        for slice_index in 0..MEDIUM_REGION_SLICE_COUNT {
            if used_bitmap[slice_index / 64] & (1_u64 << (slice_index % 64)) == 0 {
                continue;
            }
            let physical = unsafe { &(*region).physical[slice_index] };
            let kind_and_span = physical.kind_and_span.load(Ordering::Acquire);
            let kind = match kind_and_span & PHYSICAL_KIND_MASK {
                PHYSICAL_SLICE_SMALL => tracking::PhysicalSliceKind::Small,
                PHYSICAL_SLICE_MEDIUM => tracking::PhysicalSliceKind::Medium,
                PHYSICAL_SLICE_MEDIUM_CONTINUATION => tracking::PhysicalSliceKind::MediumContinuation,
                PHYSICAL_SLICE_BUMP => tracking::PhysicalSliceKind::Bump,
                _ => tracking::PhysicalSliceKind::Unknown,
            };
            let metadata = unsafe { &(*region).allocations[slice_index] };
            let (requested_bytes, usable_bytes) = if kind == tracking::PhysicalSliceKind::Medium {
                (
                    metadata.requested_bytes.load(Ordering::Acquire),
                    metadata.usable_bytes.load(Ordering::Acquire),
                )
            } else {
                (0, 0)
            };
            let mut segments = Vec::with_capacity(2);
            for (segment_index, segment) in physical.segments.iter().enumerate() {
                let encoded = segment.load(Ordering::Acquire);
                let class = encoded & !PHYSICAL_SEGMENT_CONTEXT;
                if class != 0 {
                    segments.push(tracking::PhysicalSegmentSnapshot {
                        segment_index,
                        class_index: class - 1,
                        context: encoded & PHYSICAL_SEGMENT_CONTEXT != 0,
                        live_blocks: physical.segment_live_blocks[segment_index].load(Ordering::Relaxed),
                        usable_blocks: physical.segment_usable_blocks[segment_index].load(Ordering::Relaxed),
                        utilization_tracked: physical.segment_utilization_tracked[segment_index].load(Ordering::Relaxed),
                    });
                }
            }
            slices.push(tracking::PhysicalSliceSnapshot {
                slice_index,
                kind,
                span_slices: kind_and_span >> PHYSICAL_SPAN_SHIFT,
                owner: physical.owner.load(Ordering::Acquire),
                requested_bytes,
                usable_bytes,
                segments,
            });
        }
        for index in 0..slices.len() {
            if slices[index].kind != tracking::PhysicalSliceKind::Medium {
                continue;
            }
            let start_slice = slices[index].slice_index;
            let span_slices = slices[index].span_slices;
            let owner = slices[index].owner;
            for offset in 1..span_slices {
                let Some(continuation) = slices.get_mut(index + offset) else {
                    break;
                };
                if continuation.slice_index != start_slice + offset || continuation.kind != tracking::PhysicalSliceKind::Unknown {
                    break;
                }
                continuation.kind = tracking::PhysicalSliceKind::MediumContinuation;
                continuation.owner = owner;
            }
        }
        snapshots.push(tracking::RegionSnapshot {
            domain_id: unsafe { (*(*region).domain).id },
            region_index,
            base_address: unsafe { (*region).base.addr() },
            reserved_bytes: MEDIUM_REGION_SIZE,
            slice_bytes: MEDIUM_SLICE_SIZE,
            used_slices,
            free_slices: MEDIUM_REGION_SLICE_COUNT - used_slices,
            used_bitmap,
            slices,
        });
    }
    snapshots
}

pub(crate) fn telemetry_domain_snapshots() -> Vec<tracking::DomainSnapshot> {
    let mut snapshots = Vec::new();
    let mut domain = DOMAINS.load(Ordering::Acquire);
    while !domain.is_null() {
        snapshots.push(tracking::DomainSnapshot {
            domain_id: unsafe { (*domain).id },
            is_default: unsafe { (*domain).is_default.load(Ordering::Acquire) },
        });
        domain = unsafe { (*domain).next.load(Ordering::Acquire) };
    }
    snapshots.sort_unstable_by_key(|domain| domain.domain_id);
    snapshots
}

pub(crate) fn invalidate_tracking_cache() {
    let state = unsafe { thread_state() };
    let log = unsafe { (*state).tracking_log };
    unsafe {
        (*state).tracking_session = usize::MAX;
        (*state).tracking_log = ptr::null();
        tracking::release_thread_log(log);
    }
}

pub(crate) fn enter_tracking_internal() -> bool {
    let state = unsafe { &mut *thread_state() };
    let previous = state.in_tracking;
    state.in_tracking = true;
    previous
}

pub(crate) fn restore_tracking_internal(previous: bool) {
    unsafe { (*thread_state()).in_tracking = previous };
}

struct RestoreTrackingInternal(bool);

impl Drop for RestoreTrackingInternal {
    fn drop(&mut self) {
        restore_tracking_internal(self.0);
    }
}

pub(crate) unsafe fn initialize_general_heap(state: *mut ReusableHeapState) {
    unsafe {
        (*state).retirable = true;
        initialize_heap_owner(state);
    }
}

unsafe fn initialize_heap_owner(state: *mut ReusableHeapState) {
    if !unsafe { (*state).owner.is_null() } {
        return;
    }
    let embedded = unsafe { ptr::addr_of_mut!((*state).owner_storage) };
    let storage = unsafe { hal::initialize_storage(embedded, OwnerStorage::new()) };
    let owner = unsafe { ptr::addr_of_mut!((*storage).owner) };
    let retirement = unsafe { ptr::addr_of_mut!((*storage).retirement) };
    unsafe {
        (*retirement).heap_state = state;
        (*owner).retirement = retirement;
        (*state).owner = owner;
    }
}

pub(crate) unsafe fn general_heap_options(state: *mut ReusableHeapState) -> GeneralOptions {
    unsafe {
        GeneralOptions::from_values(
            (*state).locality_segment_slices * MEDIUM_SLICE_SIZE,
            (*state).medium_cache_max_bytes,
        )
    }
}

pub(crate) unsafe fn thread_heap_options(state: *mut RemoteHeapState) -> GeneralOptions {
    unsafe { (*state).options }
}

pub(crate) fn thread_heap_is_active(remote: *mut RemoteHeapState) -> bool {
    let state = unsafe { thread_state() };
    unsafe { (*state).active_remote == remote }
}

pub(crate) unsafe fn thread_heap_usage(remote: *mut RemoteHeapState) -> Result<HeapUsage, ()> {
    let state = unsafe { thread_state() };
    if unsafe { (*remote).owner_token.load(Ordering::Acquire) } != unsafe { thread_token(state) } {
        return Err(());
    }
    let owner_heap = unsafe { (*remote).owner_heap.load(Ordering::Acquire) };
    if owner_heap.is_null() {
        return Err(());
    }
    Ok(unsafe { general_heap_usage(owner_heap, remote) })
}

pub(crate) unsafe fn general_heap_usage(state: *mut ReusableHeapState, remote: *mut RemoteHeapState) -> HeapUsage {
    let heap = unsafe { &mut *state };
    if heap.retirable {
        unsafe { initialize_heap_owner(ptr::from_mut(heap)) };
    }
    let coordination = unsafe { &*(*heap.owner).retirement };
    acquire_heap_inspection(coordination);
    if !remote.is_null() {
        acquire_remote_inspection(unsafe { &(*remote).usage });
    }
    let _inspection = UsageInspectionGuard {
        retirement: coordination,
        remote,
    };

    let mut small = AllocationUsage::default();
    let mut reserved_bytes = 0;
    let mut committed_bytes = 0;
    let mut slab_count = 0;
    let mut slice_count = 0;
    let mut segment = heap.segments;
    while !segment.is_null() {
        let next = unsafe { (*segment).segment_next };
        let segment_slices = unsafe { (*segment).segment_slices };
        if segment_slices == DIRECT_SLAB_SEGMENT {
            reserved_bytes += SLAB_SIZE;
            committed_bytes += unsafe { (*segment).segment_committed_bytes };
            slab_count += 1;
            unsafe { add_slab_usage(heap, segment, &mut small) };
        } else {
            let total_slabs = segment_slices as usize * 2;
            let used_slabs = if segment == heap.locality_segment {
                (heap.locality_next.addr() - segment.addr()) / SLAB_SIZE
            } else {
                total_slabs
            };
            reserved_bytes += segment_slices as usize * MEDIUM_SLICE_SIZE;
            committed_bytes += unsafe { (*segment).segment_committed_bytes };
            slab_count += used_slabs;
            slice_count += segment_slices as usize;
            for slab_index in 0..used_slabs {
                let slab = unsafe { segment.cast::<u8>().add(slab_index * SLAB_SIZE).cast::<SlabHeader>() };
                unsafe { add_slab_usage(heap, slab, &mut small) };
            }
        }
        segment = next;
    }

    let mut cached_medium_bytes = 0;
    for (index, address) in heap.medium_cache.iter().enumerate() {
        if !address.is_null() {
            cached_medium_bytes += (1_usize << index) * MEDIUM_SLICE_SIZE;
        }
    }
    reserved_bytes += cached_medium_bytes;
    committed_bytes += cached_medium_bytes;

    let mut medium = AllocationUsage::default();
    let regions = unsafe { domain_regions(heap.domain) };
    let mut region = regions.regions.load(Ordering::Acquire);
    while !region.is_null() {
        for metadata in unsafe { &(*region).allocations } {
            let encoded_owner = metadata.owner.load(Ordering::Acquire);
            let (owner, _, _) = unsafe { decode_medium_owner(encoded_owner) };
            if owner == state {
                medium.add(
                    1,
                    metadata.requested_bytes.load(Ordering::Acquire),
                    metadata.usable_bytes.load(Ordering::Acquire),
                );
            }
        }
        region = unsafe { (*region).next.load(Ordering::Acquire) };
    }
    reserved_bytes += medium.usable_bytes();
    committed_bytes += medium.usable_bytes();

    let mut direct = AllocationUsage::default();
    {
        let direct_allocations = DIRECT_ALLOCATIONS.lock();
        let mut current = direct_allocations.head;
        while !current.is_null() {
            let (owner, _, _) = decode_heap_owner(unsafe { (*current).owner });
            if owner == state {
                direct.add(1, unsafe { (*current).requested_bytes }, unsafe { (*current).usable_bytes });
                reserved_bytes += unsafe { (*current).mapping_size };
                committed_bytes += unsafe { (*current).mapping_size };
            }
            current = unsafe { (*current).next_direct };
        }
    }

    if !remote.is_null() {
        let usage = unsafe { &(*remote).usage };
        small.add(
            usage.live_allocations.load(Ordering::Relaxed),
            usage.requested_bytes.load(Ordering::Relaxed),
            usage.usable_bytes.load(Ordering::Relaxed),
        );
        reserved_bytes += usage.reserved_bytes.load(Ordering::Relaxed);
        committed_bytes += usage.committed_bytes.load(Ordering::Relaxed);
        slab_count += usage.slab_count.load(Ordering::Relaxed);
        slice_count += usage.slice_count.load(Ordering::Relaxed);
    }

    let live_allocations = small.live_allocations() + medium.live_allocations() + direct.live_allocations();
    let live_requested_bytes = small.requested_bytes() + medium.requested_bytes() + direct.requested_bytes();
    let live_usable_bytes = small.usable_bytes() + medium.usable_bytes() + direct.usable_bytes();
    HeapUsage::new(
        live_allocations,
        live_requested_bytes,
        live_usable_bytes,
        reserved_bytes,
        committed_bytes,
        HeapUsageKind::General(GeneralHeapUsage::new(
            small,
            medium,
            direct,
            cached_medium_bytes,
            slab_count,
            slice_count,
        )),
    )
}

unsafe fn add_slab_usage(heap: &ReusableHeapState, slab: *mut SlabHeader, usage: &mut AllocationUsage) {
    let mut cached_blocks = 0;
    for classes in [&heap.classes, &heap.context_classes] {
        for class in classes {
            for cached in class.cached {
                if !cached.is_null() && (cached.addr() & !(SLAB_SIZE - 1)) == slab.addr() {
                    cached_blocks += 1;
                }
            }
        }
    }

    let mut remote_blocks = 0;
    let mut remote_padding_bytes = 0;
    let mut block = unsafe { (*slab).remote_free.load(Ordering::Acquire) };
    while !block.is_null() && block != RETIRED_REMOTE_SENTINEL {
        remote_blocks += 1;
        remote_padding_bytes += unsafe { peek_free_requested(block) };
        block = unsafe { read_free_next(block) };
    }
    let live_blocks = unsafe { (*slab).usable_blocks as usize - (*slab).free_count - cached_blocks - remote_blocks };
    let live_usable_bytes = live_blocks * unsafe { (*slab).block_size as usize };
    let live_padding_bytes = unsafe { (*slab).requested_bytes }.saturating_sub(remote_padding_bytes);
    usage.add(live_blocks, live_usable_bytes.saturating_sub(live_padding_bytes), live_usable_bytes);
}

pub(crate) unsafe fn retire_general_heap(state: *mut ReusableHeapState) {
    let heap = unsafe { &mut *state };
    let owner = heap.owner;
    let retirement = unsafe { (*owner).retirement };
    debug_assert!(!retirement.is_null());
    unsafe { (*retirement).retiring.store(true, Ordering::Release) };
    acquire_heap_retirement(unsafe { &*retirement });

    for classes in [&mut heap.classes, &mut heap.context_classes] {
        for class in classes {
            for cached in &mut class.cached {
                if !cached.is_null() {
                    let slab = allocation_slab(*cached);
                    unsafe { (*slab).free_count += 1 };
                    *cached = ptr::null_mut();
                }
            }
            class.active = ptr::null_mut();
        }
    }

    for (cache_index, cached) in heap.medium_cache.iter_mut().enumerate() {
        if cached.is_null() {
            continue;
        }
        let slice_count = 1_usize << cache_index;
        let bytes = slice_count * MEDIUM_SLICE_SIZE;
        let decommitted = unsafe { hal::decommit(*cached, bytes) };
        debug_assert!(decommitted || cfg!(test));
        if decommitted {
            let regions = unsafe { domain_regions(heap.domain) };
            unsafe { regions.release_slices(*cached, slice_count) };
            if heap.track_aggregates {
                tracking::record_unmapping(bytes);
            }
        }
        *cached = ptr::null_mut();
    }
    let mut retained = 0;
    let mut segment = heap.segments;
    while !segment.is_null() {
        let next = unsafe { (*segment).segment_next };
        let segment_slices = unsafe { (*segment).segment_slices };
        if segment_slices == DIRECT_SLAB_SEGMENT {
            if unsafe { prepare_retired_slice(segment, 1, SLAB_SIZE, true, heap.track_aggregates) } {
                retained += 1;
            }
        } else {
            let total_slabs = segment_slices as usize * 2;
            let used_slabs = if segment == heap.locality_segment {
                (heap.locality_next.addr() - segment.addr()) / SLAB_SIZE
            } else {
                total_slabs
            };
            let committed_slabs = unsafe { (*segment).segment_committed_bytes / SLAB_SIZE };

            for slice_index in 0..segment_slices as usize {
                let slice = unsafe { segment.cast::<u8>().add(slice_index * MEDIUM_SLICE_SIZE).cast::<SlabHeader>() };
                let initialized_slabs = used_slabs.saturating_sub(slice_index * 2).min(2);
                let committed_bytes = committed_slabs.saturating_sub(slice_index * 2).min(2) * SLAB_SIZE;
                if unsafe { prepare_retired_slice(slice, initialized_slabs, committed_bytes, false, heap.track_aggregates) } {
                    retained += 1;
                }
            }
        }
        segment = next;
    }
    if unsafe { (*retirement).external_allocations.load(Ordering::Acquire) } != 0 {
        retained += 1;
    }

    unsafe {
        (*retirement).retired_releases.store(retained, Ordering::Relaxed);
        (*retirement).retirement_ready.store(true, Ordering::Release);
    }
    if retained == 0 {
        unsafe { release_owner_storage(retirement) };
        unsafe { hal::unmap(state.cast(), mem::size_of::<ReusableHeapState>()) };
    }
}

unsafe fn prepare_retired_slice(
    slice: *mut SlabHeader,
    initialized_slabs: usize,
    committed_bytes: usize,
    direct_mapping: bool,
    track_aggregates: bool,
) -> bool {
    if initialized_slabs == 0 {
        unsafe {
            release_retired_storage(
                slice.cast(),
                direct_mapping,
                committed_bytes,
                if direct_mapping { SLAB_SIZE } else { MEDIUM_SLICE_SIZE },
                track_aggregates,
            )
        };
        return false;
    }

    let mut remaining = 0;
    for slab_index in 0..initialized_slabs {
        let slab = unsafe { slice.cast::<u8>().add(slab_index * SLAB_SIZE).cast::<SlabHeader>() };
        let mut block = unsafe { (*slab).remote_free.swap(RETIRED_REMOTE_SENTINEL, Ordering::Acquire) };
        let mut remote_frees = 0;
        while !block.is_null() && block != RETIRED_REMOTE_SENTINEL {
            remote_frees += 1;
            tracking::record_remote_drain();
            let next = unsafe { read_free_next(block) };
            unsafe { release_free_metadata(block) };
            block = next;
        }
        unsafe { (*slab).free_count += remote_frees };
        remaining += unsafe { (*slab).usable_blocks as usize - (*slab).free_count };
    }

    if remaining == 0 {
        unsafe {
            release_retired_storage(
                slice.cast(),
                direct_mapping,
                committed_bytes,
                if direct_mapping { SLAB_SIZE } else { MEDIUM_SLICE_SIZE },
                track_aggregates,
            )
        };
        return false;
    }

    let root = slice.cast::<RetiredSliceState>();
    unsafe {
        initialize_retired_slice(
            root,
            remaining,
            root,
            track_aggregates,
            direct_mapping,
            committed_bytes,
            if direct_mapping { SLAB_SIZE } else { MEDIUM_SLICE_SIZE },
        );
    }
    if initialized_slabs == 2 {
        let second = unsafe { slice.cast::<u8>().add(SLAB_SIZE).cast::<RetiredSliceState>() };
        unsafe {
            initialize_retired_slice(
                second,
                0,
                root,
                track_aggregates,
                direct_mapping,
                committed_bytes,
                MEDIUM_SLICE_SIZE,
            );
        };
    }
    true
}

unsafe fn initialize_retired_slice(
    state: *mut RetiredSliceState,
    remaining: usize,
    root: *mut RetiredSliceState,
    track_aggregates: bool,
    direct_mapping: bool,
    committed_bytes: usize,
    release_bytes: usize,
) {
    unsafe {
        ptr::addr_of_mut!((*state).remaining).write(AtomicUsize::new(remaining));
        ptr::addr_of_mut!((*state).state).write(root);
        ptr::addr_of_mut!((*state).ready).write(AtomicBool::new(true));
        ptr::addr_of_mut!((*state).released).write(AtomicBool::new(false));
        ptr::addr_of_mut!((*state).track_aggregates).write(track_aggregates);
        ptr::addr_of_mut!((*state).direct_mapping).write(direct_mapping);
        ptr::addr_of_mut!((*state).state_padding).write([0; 4]);
        ptr::addr_of_mut!((*state).committed_bytes).write(committed_bytes);
        ptr::addr_of_mut!((*state).release_bytes).write(release_bytes);
    }
}

unsafe fn release_retired_block(slab: *mut SlabHeader) {
    let root = unsafe { (*slab.cast::<RetiredSliceState>()).state };
    assert!(
        !root.is_null(),
        "retired slab {:#x} has no root (marker {:#x}, owner {:#x})",
        slab.addr(),
        unsafe { (*slab).marker.load(Ordering::Acquire) },
        unsafe { (*slab).owner.addr() }
    );
    if unsafe { (*root).remaining.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    fence(Ordering::Acquire);
    let owner = unsafe { (*root).owner };
    let released = unsafe {
        release_retired_storage(
            root.cast(),
            (*root).direct_mapping,
            (*root).committed_bytes,
            (*root).release_bytes,
            (*root).track_aggregates,
        )
    };
    if !released {
        return;
    }
    let retirement = unsafe { (*owner).retirement };
    unsafe { release_retired_group(retirement) };
}

unsafe fn retain_external_allocation(owner: *mut ReusableHeapState) {
    if unsafe { !(*owner).retirable } {
        return;
    }
    let retirement = unsafe { &*(*(*owner).owner).retirement };
    retirement.external_allocations.fetch_add(1, Ordering::Relaxed);
}

fn acquire_heap_inspection(retirement: &RetirementState) {
    loop {
        match retirement
            .operations
            .compare_exchange(0, OPERATION_INSPECTING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return,
            Err(state) if state & OPERATION_RETIRED != 0 => {
                unreachable!("retired heaps cannot be inspected")
            }
            Err(_) => spin_loop(),
        }
    }
}

fn acquire_heap_retirement(retirement: &RetirementState) {
    loop {
        match retirement
            .operations
            .compare_exchange(0, OPERATION_RETIRED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return,
            Err(_) => spin_loop(),
        }
    }
}

fn acquire_remote_inspection(usage: &RemoteUsage) {
    loop {
        match usage
            .operations
            .compare_exchange(0, OPERATION_INSPECTING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return,
            Err(_) => spin_loop(),
        }
    }
}

unsafe fn begin_heap_usage_operation(retirement: *mut RetirementState) -> bool {
    let retirement = unsafe { &*retirement };
    loop {
        let state = retirement.operations.load(Ordering::Acquire);
        if state & OPERATION_RETIRED != 0 || retirement.retiring.load(Ordering::Acquire) {
            while !retirement.retirement_ready.load(Ordering::Acquire) {
                spin_loop();
            }
            return false;
        }
        if state & OPERATION_INSPECTING != 0 {
            spin_loop();
            continue;
        }
        if retirement
            .operations
            .compare_exchange_weak(state, state + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
}

unsafe fn end_heap_usage_operation(retirement: *mut RetirementState) {
    let previous = unsafe { (*retirement).operations.fetch_sub(1, Ordering::Release) };
    debug_assert_eq!(previous & OPERATION_FLAGS, 0);
    debug_assert_ne!(previous, 0);
}

unsafe fn release_external_allocation(retirement: *mut RetirementState) {
    if retirement.is_null() {
        return;
    }
    let previous = unsafe { (*retirement).external_allocations.fetch_sub(1, Ordering::Release) };
    debug_assert_ne!(previous, 0);
    if previous == 1
        && unsafe { (*retirement).retiring.load(Ordering::Acquire) }
        && unsafe { (*retirement).retirement_ready.load(Ordering::Acquire) }
    {
        unsafe { release_retired_group(retirement) };
    }
}

unsafe fn release_retired_group(retirement: *const RetirementState) {
    if unsafe { (*retirement).retired_releases.fetch_sub(1, Ordering::Release) } == 1 {
        fence(Ordering::Acquire);
        let heap_state = unsafe { (*retirement).heap_state };
        unsafe { release_owner_storage(retirement) };
        unsafe { hal::unmap(heap_state.cast(), mem::size_of::<ReusableHeapState>()) };
    }
}

unsafe fn release_owner_storage(retirement: *const RetirementState) {
    let storage = unsafe {
        retirement
            .cast::<u8>()
            .sub(std::mem::offset_of!(OwnerStorage, retirement))
            .cast_mut()
            .cast::<OwnerStorage>()
    };
    let heap_state = unsafe { (*retirement).heap_state };
    let embedded = unsafe { ptr::addr_of_mut!((*heap_state).owner_storage) };
    unsafe { hal::release_storage(storage, embedded) };
}

unsafe fn release_retired_storage(
    address: *mut u8,
    direct_mapping: bool,
    committed_bytes: usize,
    release_bytes: usize,
    track_aggregates: bool,
) -> bool {
    if direct_mapping {
        unsafe { hal::unmap(address, release_bytes) };
    } else {
        let decommitted = committed_bytes == 0 || unsafe { hal::decommit(address, committed_bytes) };
        debug_assert!(decommitted || cfg!(test));
        if !decommitted {
            return false;
        }
        let region = region_containing(address).expect("retired slices must belong to an allocator region");
        let regions = unsafe { domain_regions((*region).domain) };
        unsafe { regions.release_slices(address, release_bytes / MEDIUM_SLICE_SIZE) };
    }
    if track_aggregates {
        tracking::record_unmapping(committed_bytes);
    }
    true
}

pub(crate) fn take_pooled_bump(domain: *mut DomainState) -> Option<*mut BumpState> {
    let state = unsafe { &mut *thread_state() };
    for index in 0..state.bump_pool_len {
        let bump = state.bump_pool[index];
        if unsafe { (*bump).domain } == domain {
            state.bump_pool_len -= 1;
            state.bump_pool[index] = state.bump_pool[state.bump_pool_len];
            return Some(bump);
        }
    }
    bump::take_global(domain)
}

pub(crate) fn thread_heap_state() -> Option<*mut RemoteHeapState> {
    let state = unsafe { &mut *thread_state() };
    let default_heap = unsafe { ensure_default_heap(ptr::from_mut(state)) };
    if default_heap.is_null() {
        return None;
    }
    if state.remote_heap.is_null() {
        let layout = Layout::new::<RemoteHeapState>();
        let remote = hal::map(layout.size()).cast::<RemoteHeapState>();
        if remote.is_null() {
            return None;
        }
        unsafe {
            remote.write(RemoteHeapState {
                owner: ptr::null_mut(),
                embedded_owner: OwnerState::new(),
                owner_token: AtomicUsize::new(thread_token(state)),
                owner_heap: AtomicPtr::new(default_heap),
                domain: (*default_heap).domain,
                options: general_heap_options(default_heap),
                usage: RemoteUsage::new(),
                classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
                context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            });
        }
        let owner = unsafe { ptr::addr_of_mut!((*remote).embedded_owner) };
        unsafe {
            (*remote).owner = owner;
        }
        state.remote_heap = remote;
    }
    Some(state.remote_heap)
}

pub(crate) fn allocate_bump_chunk(domain: *mut DomainState) -> *mut u8 {
    let regions = unsafe { domain_regions(domain) };
    let Some(address) = regions.allocate_slices(domain, 1) else {
        return ptr::null_mut();
    };
    if !unsafe { hal::commit(address, MEDIUM_SLICE_SIZE) } {
        unsafe { regions.release_slices(address, 1) };
        return ptr::null_mut();
    }

    tracking::record_bump_commit(MEDIUM_SLICE_SIZE);
    address
}

pub(crate) fn create_bump_fallback_heap(domain: *mut DomainState) -> *mut ReusableHeapState {
    let state = hal::map(mem::size_of::<ReusableHeapState>()).cast::<ReusableHeapState>();
    if state.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        state.write(ReusableHeapState::new(GeneralOptions::new(), domain));
        initialize_general_heap(state);
    }
    state
}

pub(crate) unsafe fn release_bump_chunk(address: *mut u8) {
    let decommitted = unsafe { hal::decommit(address, MEDIUM_SLICE_SIZE) };
    debug_assert!(decommitted || cfg!(test));
    if decommitted {
        tracking::record_bump_decommit(MEDIUM_SLICE_SIZE);
        let region = region_containing(address).expect("bump chunk must belong to an allocator region");
        let regions = unsafe { domain_regions((*region).domain) };
        unsafe { regions.release_slices(address, 1) };
    }
}

pub(crate) fn return_pooled_bump(bump: *mut BumpState) {
    let fallback_heap = unsafe { bump::take_fallback_heap(bump) };
    if !fallback_heap.is_null() {
        unsafe { retire_general_heap(fallback_heap) };
    }
    let state = unsafe { &mut *thread_state() };
    if state.bump_pool_len < state.bump_pool.len() {
        state.bump_pool[state.bump_pool_len] = bump;
        state.bump_pool_len += 1;
    } else {
        unsafe { crate::heap::bump::return_global(bump) };
    }
}

pub(crate) fn hint_thread_context() -> *mut () {
    unsafe { thread_state().cast() }
}

pub(crate) unsafe fn set_active_hint(thread_context: *mut (), hint: RawHint) {
    let state = unsafe { &mut *thread_context.cast::<ThreadState>() };
    state.active_heap = ptr::null_mut();
    state.active_bump = ptr::null_mut();
    state.active_remote = ptr::null_mut();
    match target_from_hint(hint) {
        Some(HeapTarget::General(heap)) => state.active_heap = heap.as_ptr(),
        Some(HeapTarget::Bump(bump)) => {
            state.active_heap = unsafe { (*bump.as_ptr()).fallback_heap };
            state.active_bump = bump.as_ptr();
        }
        Some(HeapTarget::Thread(remote)) => state.active_remote = remote.as_ptr(),
        None => {}
    }
}

#[inline(always)]
fn context_user_offset(alignment: usize) -> usize {
    cmp::max(HEADER_OFFSET, alignment)
}

fn slab_block_layout(block_size: usize) -> (usize, usize) {
    let block_count = SLAB_SIZE / block_size;
    let first_block = size_of::<SlabHeader>().div_ceil(block_size);
    (first_block, block_count)
}

fn context_required_size(size: usize, alignment: usize) -> Option<usize> {
    cmp::max(size, 1)
        .checked_add(context_user_offset(alignment))
        .and_then(|size| size.checked_add(EXTRA_SIZE))
}

fn direct_mapping_size(requested_size: usize, alignment: usize, discriminator_space: usize) -> Option<usize> {
    requested_size
        .checked_add(alignment - 1)
        .and_then(|size| size.checked_add(discriminator_space))
        .and_then(|size| size.checked_add(HEADER_OFFSET + EXTRA_SIZE))
}

#[inline(always)]
fn default_class<T: Tunables>(layout: Layout) -> Option<usize> {
    if layout.align() > MAX_SMALL_ALIGNMENT {
        return None;
    }
    let required_size = cmp::max(cmp::max(layout.size(), 1), layout.align());
    if layout.align() <= 16 {
        class_index::<T>(required_size)
    } else {
        class_index_for_alignment::<T>(required_size, layout.align())
    }
}

#[inline(always)]
fn medium_class(layout: Layout) -> Option<usize> {
    let slices = medium_slice_count(layout)?;
    if slices > MEDIUM_MAX_SLICES { None } else { Some(slices - 1) }
}

#[inline(always)]
fn local_medium_class(slice_count: usize) -> Option<usize> {
    (slice_count.is_power_of_two() && slice_count <= (1 << (LOCAL_MEDIUM_CLASSES - 1))).then(|| slice_count.trailing_zeros() as usize)
}

#[inline(always)]
fn medium_slice_count(layout: Layout) -> Option<usize> {
    if layout.align() > MAX_MEDIUM_ALIGNMENT {
        return None;
    }
    let size = cmp::max(layout.size(), 1);
    let slices = size.checked_add(MEDIUM_SLICE_SIZE - 1)? / MEDIUM_SLICE_SIZE;
    if slices == 0 || slices > MEDIUM_REGION_SLICE_COUNT {
        None
    } else {
        Some(slices)
    }
}

unsafe fn register_medium_allocation(address: *mut u8, layout: Layout, heap: &mut ReusableHeapState) {
    if heap.retirable {
        unsafe { initialize_heap_owner(ptr::from_mut(heap)) };
    }
    let region = region_containing(address).expect("medium allocation must belong to a region");
    let slice_index = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
    let metadata = unsafe { &(*region).allocations[slice_index] };
    let slice_count = medium_slice_count(layout).unwrap();
    metadata.requested_bytes.store(layout.size(), Ordering::Relaxed);
    metadata.usable_bytes.store(slice_count * MEDIUM_SLICE_SIZE, Ordering::Relaxed);
    unsafe { retain_external_allocation(heap) };
    let encoded_owner = encode_medium_owner(heap);
    metadata.owner.store(encoded_owner, Ordering::Release);
    record_medium_span(region, slice_index, slice_count, encoded_owner);
}

unsafe fn unregister_medium_allocation(
    address: *mut u8,
    current_heap: *mut ReusableHeapState,
) -> (*mut ReusableHeapState, bool, *mut RetirementState, Option<HeapOperationGuard>) {
    let region = region_containing(address).expect("medium allocation must belong to a region");
    let slice_index = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
    let metadata = unsafe { &(*region).allocations[slice_index] };
    let encoded_owner = metadata.owner.load(Ordering::Acquire);
    let (owner, coordination, owner_retirable) = unsafe { decode_medium_owner(encoded_owner) };
    debug_assert!(!owner.is_null());
    let local = ptr::eq(owner, current_heap);
    let operation = (!local && !coordination.is_null())
        .then(|| unsafe { HeapOperationGuard::begin(coordination) })
        .flatten();
    if local {
        metadata.owner.store(ptr::null_mut(), Ordering::Relaxed);
    } else {
        let removed = metadata
            .owner
            .compare_exchange(encoded_owner, ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire);
        debug_assert!(removed.is_ok());
    }
    metadata.requested_bytes.store(0, Ordering::Relaxed);
    metadata.usable_bytes.store(0, Ordering::Relaxed);
    unsafe { &(*region).physical[slice_index] }.owner.store(0, Ordering::Release);
    (owner, owner_retirable, coordination, operation)
}

unsafe fn register_direct_allocation(extra: *mut ExtraHeader) {
    let mut state = DIRECT_ALLOCATIONS.lock();
    unsafe { (*extra).next_direct = state.head };
    state.head = extra;
}

unsafe fn unregister_direct_allocation(extra: *mut ExtraHeader) {
    let mut state = DIRECT_ALLOCATIONS.lock();
    let mut previous = ptr::null_mut::<ExtraHeader>();
    let mut current = state.head;
    while current != extra {
        debug_assert!(!current.is_null());
        previous = current;
        current = unsafe { (*current).next_direct };
    }
    let next = unsafe { (*current).next_direct };
    if previous.is_null() {
        state.head = next;
    } else {
        unsafe { (*previous).next_direct = next };
    }
}

fn region_containing(address: *mut u8) -> Option<*mut RegionState> {
    let cached = LAST_REGION.get();
    if !cached.is_null() {
        let base = unsafe { (*cached).base };
        if address.addr() >= base.addr() && address.addr() < base.addr() + MEDIUM_REGION_SIZE {
            return Some(cached);
        }
    }

    let mut domain = DOMAINS.load(Ordering::Acquire);
    while !domain.is_null() {
        let mut region = unsafe { (*domain).regions.regions.load(Ordering::Acquire) };
        while !region.is_null() {
            let base = unsafe { (*region).base };
            if address.addr() >= base.addr() && address.addr() < base.addr() + MEDIUM_REGION_SIZE {
                LAST_REGION.set(region);
                return Some(region);
            }
            region = unsafe { (*region).next.load(Ordering::Acquire) };
        }
        domain = unsafe { (*domain).next.load(Ordering::Acquire) };
    }
    None
}

#[cfg(test)]
fn clear_region_cache() {
    LAST_REGION.set(ptr::null_mut());
}

fn record_small_segment(
    address: *mut u8,
    class_index: usize,
    context: bool,
    owner: *mut OwnerState,
    usable_blocks: usize,
    utilization_tracked: bool,
) {
    let Some(region) = region_containing(address) else {
        return;
    };
    let offset = address.addr() - unsafe { (*region).base.addr() };
    let slice_index = offset / MEDIUM_SLICE_SIZE;
    let segment_index = (offset % MEDIUM_SLICE_SIZE) / SLAB_SIZE;
    let metadata = unsafe { &(*region).physical[slice_index] };
    let mut segment = class_index + 1;
    if context {
        segment |= PHYSICAL_SEGMENT_CONTEXT;
    }
    metadata.segments[segment_index].store(segment, Ordering::Relaxed);
    metadata.segment_usable_blocks[segment_index].store(usable_blocks, Ordering::Relaxed);
    metadata.segment_utilization_tracked[segment_index].store(utilization_tracked, Ordering::Relaxed);
    metadata.owner.store(owner.addr(), Ordering::Relaxed);
    metadata.kind_and_span.store(PHYSICAL_SLICE_SMALL, Ordering::Release);
}

fn record_physical_small_allocation(address: *mut u8) {
    update_physical_small_live_blocks(address, true);
}

fn record_physical_small_deallocation(address: *mut u8) {
    update_physical_small_live_blocks(address, false);
}

fn update_physical_small_live_blocks(address: *mut u8, allocate: bool) {
    let Some(region) = region_containing(address) else {
        return;
    };
    let offset = address.addr() - unsafe { (*region).base.addr() };
    let slice_index = offset / MEDIUM_SLICE_SIZE;
    let segment_index = (offset % MEDIUM_SLICE_SIZE) / SLAB_SIZE;
    let counter = unsafe { &*ptr::addr_of!((*region).physical[slice_index].segment_live_blocks[segment_index]) };
    if allocate {
        counter.fetch_add(1, Ordering::Relaxed);
    } else {
        counter.fetch_sub(1, Ordering::Relaxed);
    }
}

fn record_medium_span(region: *mut RegionState, slice_index: usize, slice_count: usize, owner: *mut ReusableHeapState) {
    let start = unsafe { &(*region).physical[slice_index] };
    start.owner.store(owner.addr(), Ordering::Relaxed);
    start
        .kind_and_span
        .store(PHYSICAL_SLICE_MEDIUM | (slice_count << PHYSICAL_SPAN_SHIFT), Ordering::Release);
}

pub(crate) fn register_bump_chunk(address: *mut u8, state: *mut BumpState) {
    let region = bump_chunk_region_or_else(address, || -> *mut RegionState { std::process::abort() });
    let slice_index = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
    let metadata = unsafe { &(*region).physical[slice_index] };
    metadata.owner.store(state.addr(), Ordering::Relaxed);
    metadata
        .kind_and_span
        .store(PHYSICAL_SLICE_BUMP | (1 << PHYSICAL_SPAN_SHIFT), Ordering::Release);
}

fn bump_chunk_region_or_else(address: *mut u8, on_missing: impl FnOnce() -> *mut RegionState) -> *mut RegionState {
    match region_containing(address) {
        Some(region) => region,
        None => on_missing(),
    }
}

fn encode_heap_owner(owner: *mut ReusableHeapState, retirable: bool) -> *mut ReusableHeapState {
    if retirable {
        unsafe { (*owner).owner }
            .cast::<ReusableHeapState>()
            .map_addr(|address| address | 1)
    } else {
        owner
    }
}

fn decode_heap_owner(owner: *mut ReusableHeapState) -> (*mut ReusableHeapState, *mut RetirementState, bool) {
    let retirable = owner.addr() & 1 != 0;
    if !retirable {
        return (owner, ptr::null_mut(), false);
    }
    let owner = owner.map_addr(|address| address & !1).cast::<OwnerState>();
    let retirement = unsafe { (*owner).retirement };
    (unsafe { (*retirement).heap_state }, retirement, true)
}

fn encode_medium_owner(heap: &ReusableHeapState) -> *mut ReusableHeapState {
    if heap.owner.is_null() || unsafe { (*heap.owner).retirement.is_null() } {
        return ptr::from_ref(heap).cast_mut();
    }
    heap.owner
        .cast::<ReusableHeapState>()
        .map_addr(|address| address | 1 | (usize::from(heap.retirable) << 1))
}

unsafe fn decode_medium_owner(encoded: *mut ReusableHeapState) -> (*mut ReusableHeapState, *mut RetirementState, bool) {
    if encoded.addr() & 1 == 0 {
        return (encoded, ptr::null_mut(), false);
    }
    let retirable = encoded.addr() & 2 != 0;
    let owner = encoded.map_addr(|address| address & !3).cast::<OwnerState>();
    let retirement = unsafe { (*owner).retirement };
    let heap = unsafe { (*retirement).heap_state };
    (heap, retirement, retirable)
}

unsafe fn record_small_allocation<T: Tunables>(block: *mut u8, class_index: usize, requested_bytes: usize, remote: *mut RemoteHeapState) {
    let padding_bytes = T::SizeClasses::SIZES[class_index] - requested_bytes;
    let slab = allocation_slab(block);
    if remote.is_null() && !unsafe { is_remote_slab(slab) } {
        if padding_bytes != 0 {
            unsafe { (*slab).requested_bytes += padding_bytes };
        }
        return;
    }
    let remote = if remote.is_null() {
        unsafe { remote_from_owner((*slab).owner) }
    } else {
        remote
    };
    let usable_bytes = T::SizeClasses::SIZES[class_index];
    unsafe {
        begin_remote_usage_operation(remote);
        (*remote).usage.live_allocations.fetch_add(1, Ordering::Relaxed);
        (*remote).usage.requested_bytes.fetch_add(requested_bytes, Ordering::Relaxed);
        (*remote).usage.usable_bytes.fetch_add(usable_bytes, Ordering::Relaxed);
        end_remote_usage_operation(remote);
    }
}

unsafe fn begin_remote_usage_operation(remote: *mut RemoteHeapState) {
    loop {
        let operations = unsafe { &(*remote).usage.operations };
        let state = operations.load(Ordering::Acquire);
        if state & OPERATION_INSPECTING != 0 {
            spin_loop();
            continue;
        }
        if operations
            .compare_exchange_weak(state, state + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

unsafe fn end_remote_usage_operation(remote: *mut RemoteHeapState) {
    let previous = unsafe { (*remote).usage.operations.fetch_sub(1, Ordering::Release) };
    debug_assert_eq!(previous & OPERATION_FLAGS, 0);
    debug_assert_ne!(previous, 0);
}

unsafe fn record_remote_small_allocation(remote: *mut RemoteHeapState, requested_bytes: usize, usable_bytes: usize) {
    unsafe {
        (*remote).usage.live_allocations.fetch_add(1, Ordering::Relaxed);
        (*remote).usage.requested_bytes.fetch_add(requested_bytes, Ordering::Relaxed);
        (*remote).usage.usable_bytes.fetch_add(usable_bytes, Ordering::Relaxed);
    }
}

unsafe fn record_remote_small_free<T: Tunables>(slab: *mut SlabHeader, class_index: usize, requested_bytes: usize) {
    let remote = unsafe { remote_from_owner((*slab).owner) };
    debug_assert!(!remote.is_null());
    unsafe { begin_remote_usage_operation(remote) };
    let usable_bytes = T::SizeClasses::SIZES[class_index];
    unsafe {
        (*remote).usage.live_allocations.fetch_sub(1, Ordering::Relaxed);
        (*remote).usage.requested_bytes.fetch_sub(requested_bytes, Ordering::Relaxed);
        (*remote).usage.usable_bytes.fetch_sub(usable_bytes, Ordering::Relaxed);
        end_remote_usage_operation(remote);
    }
}

unsafe fn is_remote_slab(slab: *mut SlabHeader) -> bool {
    unsafe { (*slab).segment_committed_bytes == REMOTE_SLAB_SENTINEL }
}

unsafe fn remote_from_owner(owner: *mut OwnerState) -> *mut RemoteHeapState {
    unsafe { owner.cast::<u8>().sub(std::mem::offset_of!(RemoteHeapState, embedded_owner)).cast() }
}

#[inline(always)]
fn class_index<T: Tunables>(required_size: usize) -> Option<usize> {
    if required_size > 16384 {
        None
    } else {
        let bucket = (required_size + 15) >> 4;
        Some(unsafe { *<T::SizeClasses as SizeClassTables>::CLASS_MAP.get_unchecked(bucket) as usize })
    }
}

#[inline(always)]
fn class_index_for_alignment<T: Tunables>(required_size: usize, alignment: usize) -> Option<usize> {
    if required_size > 16384 || !alignment.is_power_of_two() || alignment > MAX_SMALL_ALIGNMENT {
        return None;
    }
    let bucket = required_size.div_ceil(16);
    let class_index = unsafe {
        *<T::SizeClasses as SizeClassTables>::ALIGNED_CLASS_MAP
            .get_unchecked(alignment.trailing_zeros() as usize)
            .get_unchecked(bucket)
    };
    (class_index != u8::MAX).then_some(class_index as usize)
}

#[inline(always)]
fn slab_class_from_marker<T: Tunables>(marker: usize) -> Option<usize> {
    let class_index = marker.wrapping_sub(SLAB_MARKER);
    if class_index >= T::SizeClasses::SIZES.len() || marker != (SLAB_MARKER | class_index) {
        return None;
    }
    Some(class_index)
}

#[inline(always)]
fn is_context_marker<T: Tunables>(marker: usize) -> bool {
    let class_index = marker.wrapping_sub(CONTEXT_SLAB_MARKER);
    class_index < T::SizeClasses::SIZES.len() && marker == (CONTEXT_SLAB_MARKER | class_index)
}

#[inline]
fn allocation_segment(address: *mut u8) -> *mut u8 {
    #[cfg(not(miri))]
    {
        address.map_addr(|value| value & !(SLAB_SIZE - 1))
    }
    #[cfg(miri)]
    hal::align_down(address, SLAB_SIZE, |segment_address| {
        region_containing(address)
            .map(|region| unsafe { (*region).base.with_addr(segment_address) })
            .unwrap_or_else(|| address.with_addr(segment_address))
    })
}

#[inline(always)]
fn allocation_slab(address: *mut u8) -> *mut SlabHeader {
    allocation_segment(address).cast()
}

unsafe fn allocate_slices_locked(
    state: &mut MediumState,
    published_regions: &AtomicPtr<RegionState>,
    domain: *mut DomainState,
    count: usize,
) -> Option<*mut u8> {
    let region =
        unsafe { find_region_with_free_slices(state, count) }.or_else(|| unsafe { append_region(state, published_regions, domain) })?;
    let slice_index = find_free_slices(unsafe { &(*region).used }, unsafe { (*region).next_slice }, count)?;
    unsafe {
        mark_slices(&mut (*region).used, slice_index, count, true);
        (*region).next_slice = (slice_index + count) % MEDIUM_REGION_SLICE_COUNT;
        Some((*region).base.add(slice_index * MEDIUM_SLICE_SIZE))
    }
}

unsafe fn append_region(
    state: &mut MediumState,
    published_regions: &AtomicPtr<RegionState>,
    domain: *mut DomainState,
) -> Option<*mut RegionState> {
    let base = hal::reserve(MEDIUM_REGION_SIZE);
    if base.is_null() {
        return None;
    }
    let metadata = hal::map(size_of::<RegionState>()).cast::<RegionState>();
    if metadata.is_null() {
        unsafe { hal::unmap(base, MEDIUM_REGION_SIZE) };
        return None;
    }
    unsafe {
        ptr::addr_of_mut!((*metadata).base).write(base);
        ptr::addr_of_mut!((*metadata).domain).write(domain);
        ptr::addr_of_mut!((*metadata).next_slice).write(0);
        ptr::addr_of_mut!((*metadata).large_free).write(ptr::null_mut());
        ptr::addr_of_mut!((*metadata).large_purge_after).write(0);
        ptr::addr_of_mut!((*metadata).used).write([0; MEDIUM_REGION_BITMAP_WORDS]);
        let physical = ptr::addr_of_mut!((*metadata).physical).cast::<PhysicalSliceMeta>();
        let allocations = ptr::addr_of_mut!((*metadata).allocations).cast::<MediumAllocationMeta>();
        for index in 0..MEDIUM_REGION_SLICE_COUNT {
            physical.add(index).write(PhysicalSliceMeta::new());
            allocations.add(index).write(MediumAllocationMeta::new());
        }
        let bins = ptr::addr_of_mut!((*metadata).bins).cast::<MediumBin>();
        for index in 0..MEDIUM_MAX_SLICES {
            bins.add(index).write(MediumBin::new());
        }
        ptr::addr_of_mut!((*metadata).next).write(AtomicPtr::new(ptr::null_mut()));
    }
    if state.regions.is_null() {
        state.regions = metadata;
        published_regions.store(metadata, Ordering::Release);
    } else {
        unsafe { (*state.last_region).next.store(metadata, Ordering::Release) };
    }
    state.last_region = metadata;
    Some(metadata)
}

unsafe fn find_region(state: &MediumState, address: *mut u8) -> Option<*mut RegionState> {
    let mut region = state.regions;
    while !region.is_null() {
        let base = unsafe { (*region).base };
        if address.addr() >= base.addr() && address.addr() < base.addr() + MEDIUM_REGION_SIZE {
            return Some(region);
        }
        region = unsafe { (*region).next.load(Ordering::Relaxed) };
    }
    None
}

unsafe fn find_region_with_free_slices(state: &mut MediumState, count: usize) -> Option<*mut RegionState> {
    let mut region = state.regions;
    while !region.is_null() {
        if find_free_slices(unsafe { &(*region).used }, unsafe { (*region).next_slice }, count).is_some() {
            return Some(region);
        }
        region = unsafe { (*region).next.load(Ordering::Relaxed) };
    }
    None
}

fn find_free_slices(used: &[u64; MEDIUM_REGION_BITMAP_WORDS], start: usize, count: usize) -> Option<usize> {
    find_free_slices_in(used, start, MEDIUM_REGION_SLICE_COUNT, count)
        .or_else(|| find_free_slices_in(used, 0, MEDIUM_REGION_SLICE_COUNT, count))
}

fn find_free_slices_in(used: &[u64; MEDIUM_REGION_BITMAP_WORDS], start: usize, end: usize, count: usize) -> Option<usize> {
    let mut run_start = start;
    let mut run_length = 0;
    for slice_index in start..end {
        if slice_is_used(used, slice_index) {
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

fn slice_is_used(used: &[u64; MEDIUM_REGION_BITMAP_WORDS], slice_index: usize) -> bool {
    let word = slice_index / 64;
    let bit = slice_index % 64;
    used[word] & (1_u64 << bit) != 0
}

fn mark_slices(used: &mut [u64; MEDIUM_REGION_BITMAP_WORDS], slice_index: usize, count: usize, value: bool) {
    for index in 0..count {
        let current = slice_index + index;
        let word = current / 64;
        let mask = 1_u64 << (current % 64);
        if value {
            used[word] |= mask;
        } else {
            used[word] &= !mask;
        }
    }
}

unsafe fn take_large_extent(state: *mut RegionState, slice_count: usize) -> Option<*mut u8> {
    let mut previous = ptr::null_mut::<LargeFreeBlock>();
    let mut current = unsafe { (*state).large_free };
    while !current.is_null() {
        let available = unsafe { (*current).slice_count };
        if available >= slice_count {
            let next = unsafe { (*current).next };
            if available == slice_count {
                if previous.is_null() {
                    unsafe { (*state).large_free = next };
                } else {
                    unsafe { (*previous).next = next };
                }
            } else {
                let remainder = unsafe { current.cast::<u8>().add(slice_count * MEDIUM_SLICE_SIZE).cast::<LargeFreeBlock>() };
                unsafe {
                    remainder.write(LargeFreeBlock {
                        next,
                        slice_count: available - slice_count,
                    });
                }
                if previous.is_null() {
                    unsafe { (*state).large_free = remainder };
                } else {
                    unsafe { (*previous).next = remainder };
                }
            }
            if unsafe { (*state).large_free.is_null() } {
                unsafe { (*state).large_purge_after = 0 };
            }
            return Some(current.cast());
        }
        previous = current;
        current = unsafe { (*current).next };
    }
    None
}

unsafe fn insert_large_extent(state: *mut RegionState, address: *mut u8, slice_count: usize) {
    let mut previous = ptr::null_mut::<LargeFreeBlock>();
    let mut current = unsafe { (*state).large_free };
    while !current.is_null() && current.addr() < address.addr() {
        previous = current;
        current = unsafe { (*current).next };
    }

    let block = if !previous.is_null() && previous.addr() + unsafe { (*previous).slice_count } * MEDIUM_SLICE_SIZE == address.addr() {
        unsafe { (*previous).slice_count += slice_count };
        previous
    } else {
        let block = address.cast::<LargeFreeBlock>();
        unsafe {
            block.write(LargeFreeBlock {
                next: current,
                slice_count,
            });
        }
        if previous.is_null() {
            unsafe { (*state).large_free = block };
        } else {
            unsafe { (*previous).next = block };
        }
        block
    };

    if !current.is_null() && block.addr() + unsafe { (*block).slice_count } * MEDIUM_SLICE_SIZE == current.addr() {
        unsafe {
            (*block).slice_count += (*current).slice_count;
            (*block).next = (*current).next;
        }
    }
}

#[inline(always)]
unsafe fn push_block<T: Tunables>(address: *mut u8, class_index: usize, requested_bytes: usize, thread: *mut ThreadState) {
    let state = unsafe { current_initialized_reusable_heap(thread) };
    let mut slab = allocation_slab(address);
    let address = slab.cast::<u8>().with_addr(address.addr());
    let remote_slab = unsafe { is_remote_slab(slab) };
    if remote_slab {
        unsafe { record_remote_small_free::<T>(slab, class_index, requested_bytes) };
    }
    let owner_local = (!state.is_null() && unsafe { (*slab).owner == (*state).owner })
        || (remote_slab
            && unsafe {
                let remote = remote_from_owner((*slab).owner);
                (*remote).owner_token.load(Ordering::Acquire) == thread_token(thread)
            });
    if owner_local {
        debug_assert!(!state.is_null());
        let state = unsafe { &mut *state };
        if !remote_slab {
            let padding_bytes = T::SizeClasses::SIZES[class_index] - requested_bytes;
            if padding_bytes != 0 {
                unsafe { (*slab).requested_bytes -= padding_bytes };
            }
        }
        record_class_event(class_index, ClassEventKind::LocalFree);
        let class = unsafe { state.classes.get_unchecked_mut(class_index) };
        let displaced = class.cached[1];
        class.cached[1] = class.cached[0];
        class.cached[0] = address;
        if displaced.is_null() {
            return;
        }
        slab = allocation_slab(displaced);
        let was_full = unsafe { (*slab).free_count == 0 };
        if was_full && class.active != slab {
            let lists = unsafe { state.class_lists.get_unchecked_mut(class_index) };
            unsafe { (*slab).next_partial = lists.partial };
            lists.partial = slab;
        }
        record_class_event(class_index, ClassEventKind::BitmapSpill);
        unsafe { recycle_local_block::<T>(slab, displaced, class_index) };
        return;
    }
    if remote_slab {
        let remote = unsafe { remote_from_owner((*slab).owner) };
        let class = unsafe { (*remote).classes.get_unchecked(class_index) };
        unsafe { push_remote_available(class, address) };
        return;
    }
    unsafe { push_remote_block(slab, address, class_index, requested_bytes) };
}

#[inline(always)]
unsafe fn push_context_block<T: Tunables>(address: *mut u8, class_index: usize, requested_bytes: usize, thread: *mut ThreadState) {
    let state = unsafe { current_initialized_reusable_heap(thread) };
    let mut slab = allocation_slab(address);
    let address = slab.cast::<u8>().with_addr(address.addr());
    let remote_slab = unsafe { is_remote_slab(slab) };
    if remote_slab {
        unsafe { record_remote_small_free::<T>(slab, class_index, requested_bytes) };
    }
    let owner_local = (!state.is_null() && unsafe { (*slab).owner == (*state).owner })
        || (remote_slab
            && unsafe {
                let remote = remote_from_owner((*slab).owner);
                (*remote).owner_token.load(Ordering::Acquire) == thread_token(thread)
            });
    if owner_local {
        debug_assert!(!state.is_null());
        let state = unsafe { &mut *state };
        if !remote_slab {
            let padding_bytes = T::SizeClasses::SIZES[class_index] - requested_bytes;
            if padding_bytes != 0 {
                unsafe { (*slab).requested_bytes -= padding_bytes };
            }
        }
        record_class_event(class_index, ClassEventKind::LocalFree);
        let class = unsafe { state.context_classes.get_unchecked_mut(class_index) };
        let displaced = class.cached[1];
        class.cached[1] = class.cached[0];
        class.cached[0] = address;
        if displaced.is_null() {
            return;
        }
        slab = allocation_slab(displaced);
        let was_full = unsafe { (*slab).free_count == 0 };
        if was_full && class.active != slab {
            let lists = unsafe { state.context_class_lists.get_unchecked_mut(class_index) };
            unsafe { (*slab).next_partial = lists.partial };
            lists.partial = slab;
        }
        record_class_event(class_index, ClassEventKind::BitmapSpill);
        unsafe { recycle_local_block::<T>(slab, displaced, class_index) };
        return;
    }
    if remote_slab {
        let remote = unsafe { remote_from_owner((*slab).owner) };
        let class = unsafe { (*remote).context_classes.get_unchecked(class_index) };
        unsafe { push_remote_available(class, address) };
        return;
    }
    unsafe { push_remote_block(slab, address, class_index, requested_bytes) };
}

#[cold]
#[inline(never)]
unsafe fn push_remote_block(slab: *mut SlabHeader, address: *mut u8, class_index: usize, requested_bytes: usize) {
    record_class_event(class_index, ClassEventKind::RemoteFree);
    let owner = unsafe { (*slab).owner };
    let retirement = unsafe { (*owner).retirement };
    let _operation = if retirement.is_null() {
        None
    } else {
        let Some(operation) = (unsafe { HeapOperationGuard::begin(retirement) }) else {
            tracking::record_remote_retired_free();
            unsafe { release_retired_block(slab) };
            return;
        };
        Some(operation)
    };
    let remote = unsafe { &(*slab).remote_free };
    tracking::begin_remote_free();
    let mut head = remote.load(Ordering::Relaxed);
    loop {
        unsafe {
            write_free_next(address, head);
            write_free_requested(address, (*slab).block_size as usize - requested_bytes);
        }
        #[cfg(test)]
        wait_at_test_cas_barrier();
        match remote.compare_exchange_weak(head, address, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => {
                tracking::finish_remote_free();
                unsafe { queue_remote_slab(slab) };
                return;
            }
            Err(actual) => head = actual,
        }
    }
}

#[cold]
#[inline(never)]
unsafe fn queue_remote_slab(slab: *mut SlabHeader) {
    if unsafe {
        (*slab)
            .remote_queued
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
    }
    .is_err()
    {
        return;
    }

    let inbox = unsafe { &(*(*slab).owner).remote_slabs };
    let mut head = inbox.load(Ordering::Relaxed);
    loop {
        unsafe { (*slab).inbox_next = head };
        #[cfg(test)]
        wait_at_test_cas_barrier();
        match inbox.compare_exchange_weak(head, slab, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => head = actual,
        }
    }
}

#[inline(always)]
unsafe fn take_local_slab_block<T: Tunables>(slab: *mut SlabHeader, class_index: usize) -> *mut u8 {
    let batch = unsafe { (*slab).recycled_batch };
    if batch != 0 {
        let bit_index = batch.trailing_zeros() as usize;
        unsafe {
            (*slab).recycled_batch = batch & (batch - 1);
            (*slab).free_count -= 1;
        }
        let block_index = unsafe { (*slab).recycled_batch_word as usize } * 64 + bit_index;
        record_class_event(class_index, ClassEventKind::RecycledBatchHit);
        record_class_event(class_index, ClassEventKind::Allocation);
        return unsafe { slab.cast::<u8>().add(block_index * T::SizeClasses::SIZES[class_index]) };
    }

    let summary = unsafe { (*slab).recycled_summary };
    if summary != 0 {
        let word_index = summary.trailing_zeros() as usize;
        let word = unsafe { *(*slab).recycled.get_unchecked(word_index) };
        let bit_index = word.trailing_zeros() as usize;
        let block_size = T::SizeClasses::SIZES[class_index];
        if block_size <= T::RECYCLED_BITMAP_BATCH_MAX_BLOCK_SIZE {
            unsafe { *(*slab).recycled.get_unchecked_mut(word_index) = 0 };
            unsafe { (*slab).recycled_summary &= !(1_u32 << word_index) };
            unsafe {
                (*slab).recycled_batch_word = word_index as u32;
                (*slab).recycled_batch = word & !(1_u64 << bit_index);
                (*slab).free_count -= 1;
            }
            let block_index = word_index * 64 + bit_index;
            record_class_event(class_index, ClassEventKind::RecycledWordRefill);
            record_class_event(class_index, ClassEventKind::Allocation);
            return unsafe { slab.cast::<u8>().add(block_index * block_size) };
        }
        let remaining = word & !(1_u64 << bit_index);
        unsafe { *(*slab).recycled.get_unchecked_mut(word_index) = remaining };
        if remaining == 0 {
            unsafe { (*slab).recycled_summary &= !(1_u32 << word_index) };
        }
        unsafe { (*slab).free_count -= 1 };
        let block_index = word_index * 64 + bit_index;
        record_class_event(class_index, ClassEventKind::RecycledSingleHit);
        record_class_event(class_index, ClassEventKind::Allocation);
        return unsafe { slab.cast::<u8>().add(block_index * block_size) };
    }

    let block = unsafe { (*slab).fresh_next };
    if block.is_null() {
        return ptr::null_mut();
    }
    let next = unsafe { block.add(T::SizeClasses::SIZES[class_index]) };
    let slab_end = unsafe { slab.cast::<u8>().add(SLAB_SIZE) };
    unsafe {
        (*slab).fresh_next = if slab_end.addr() - next.addr() >= T::SizeClasses::SIZES[class_index] {
            next
        } else {
            ptr::null_mut()
        }
    };
    unsafe { (*slab).free_count -= 1 };
    record_class_event(class_index, ClassEventKind::FreshHit);
    record_class_event(class_index, ClassEventKind::Allocation);
    block
}

#[inline(always)]
unsafe fn take_slab_block<T: Tunables>(slab: *mut SlabHeader, class_index: usize) -> *mut u8 {
    let block = unsafe { take_local_slab_block::<T>(slab, class_index) };
    if !block.is_null() {
        return block;
    }
    unsafe { drain_remote_blocks::<T>(slab, class_index) };
    unsafe { take_local_slab_block::<T>(slab, class_index) }
}

#[inline(always)]
unsafe fn recycle_local_block<T: Tunables>(slab: *mut SlabHeader, address: *mut u8, class_index: usize) {
    let block_size = T::SizeClasses::SIZES[class_index];
    let offset = address.addr() - slab.addr();
    let shift = unsafe { *<T::SizeClasses as SizeClassTables>::CLASS_SHIFTS.get_unchecked(class_index) };
    let block_index = if shift == u8::MAX {
        let reciprocal = unsafe { *<T::SizeClasses as SizeClassTables>::CLASS_RECIPROCALS.get_unchecked(class_index) };
        ((offset as u128 * reciprocal as u128) >> usize::BITS) as usize
    } else {
        offset >> shift
    };
    debug_assert_eq!(block_index * block_size, offset);
    let word_index = block_index / 64;
    let mask = 1_u64 << (block_index % 64);
    let word = unsafe { (*slab).recycled.get_unchecked_mut(word_index) };
    debug_assert_eq!(*word & mask, 0);
    *word |= mask;
    unsafe { (*slab).recycled_summary |= 1_u32 << word_index };
    unsafe { (*slab).free_count += 1 };
}

#[cold]
#[inline(never)]
unsafe fn drain_remote_blocks<T: Tunables>(slab: *mut SlabHeader, class_index: usize) {
    let mut block = unsafe { (*slab).remote_free.swap(ptr::null_mut(), Ordering::Acquire) };
    while !block.is_null() {
        let next = unsafe { read_free_next(block) };
        tracking::record_remote_drain();
        unsafe { (*slab).requested_bytes -= read_free_requested(block) };
        unsafe { release_free_metadata(block) };
        unsafe { recycle_local_block::<T>(slab, block, class_index) };
        block = next;
    }
}

#[cold]
#[inline(never)]
unsafe fn drain_remote_inbox<T: Tunables>(state: &mut ReusableHeapState) {
    if state.owner.is_null() {
        return;
    }

    let mut slab = unsafe { (*state.owner).remote_slabs.swap(ptr::null_mut(), Ordering::AcqRel) };
    while !slab.is_null() {
        let next = unsafe { (*slab).inbox_next };
        let marker = unsafe { (*slab).marker.load(Ordering::Acquire) };
        let normal_class = marker.wrapping_sub(SLAB_MARKER);
        let class_index = if normal_class < T::SizeClasses::SIZES.len() {
            normal_class
        } else {
            marker.wrapping_sub(CONTEXT_SLAB_MARKER)
        };
        debug_assert!(class_index < T::SizeClasses::SIZES.len());
        let was_full = unsafe { (*slab).free_count == 0 };

        unsafe { drain_remote_blocks::<T>(slab, class_index) };

        if was_full && unsafe { (*slab).free_count != 0 } {
            if slab_class_from_marker::<T>(marker).is_some() {
                let class = unsafe { state.classes.get_unchecked(class_index) };
                if class.active != slab {
                    let lists = unsafe { state.class_lists.get_unchecked_mut(class_index) };
                    unsafe { (*slab).next_partial = lists.partial };
                    lists.partial = slab;
                }
            } else {
                debug_assert!(is_context_marker::<T>(marker));
                let class = unsafe { state.context_classes.get_unchecked(class_index) };
                if class.active != slab {
                    let lists = unsafe { state.context_class_lists.get_unchecked_mut(class_index) };
                    unsafe { (*slab).next_partial = lists.partial };
                    lists.partial = slab;
                }
            }
        }

        #[cfg(test)]
        wait_at_test_cas_barrier();
        unsafe { (*slab).remote_queued.store(false, Ordering::Release) };
        if !unsafe { (*slab).remote_free.load(Ordering::Acquire) }.is_null() {
            unsafe { queue_remote_slab(slab) };
        }
        slab = next;
    }
}

#[cold]
#[inline(never)]
unsafe fn take_most_free_slab<T: Tunables>(slot: &mut *mut SlabHeader, class_index: usize) -> *mut SlabHeader {
    let mut best = ptr::null_mut::<SlabHeader>();
    let mut best_previous = ptr::null_mut::<SlabHeader>();
    let mut previous = ptr::null_mut::<SlabHeader>();
    let mut current = *slot;
    let mut scanned = 0;
    while !current.is_null() && scanned < T::PARTIAL_SLAB_SCAN_LIMIT {
        if best.is_null() || unsafe { (*current).free_count > (*best).free_count } {
            best = current;
            best_previous = previous;
        }
        previous = current;
        current = unsafe { (*current).next_partial };
        scanned += 1;
    }
    record_partial_scan(class_index, scanned, T::PARTIAL_SLAB_SCAN_LIMIT);
    if best.is_null() {
        return best;
    }

    let next = unsafe { (*best).next_partial };
    if best_previous.is_null() {
        *slot = next;
    } else {
        unsafe { (*best_previous).next_partial = next };
    }
    best
}

#[inline(always)]
unsafe fn pop_remote_block(class: &RemoteClass) -> *mut u8 {
    while class
        .popping
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }
    let mut head = class.blocks.load(Ordering::Acquire);
    while !head.is_null() {
        let next = unsafe { read_free_next(head) };
        #[cfg(test)]
        let comparison = TEST_FAIL_REMOTE_POP_CAS.with(|fail| {
            if fail.replace(false) {
                Err(class.blocks.load(Ordering::Acquire))
            } else {
                class.blocks.compare_exchange_weak(head, next, Ordering::Acquire, Ordering::Acquire)
            }
        });
        #[cfg(not(test))]
        let comparison = class.blocks.compare_exchange_weak(head, next, Ordering::Acquire, Ordering::Acquire);
        match comparison {
            Ok(_) => {
                unsafe { release_free_metadata(head) };
                class.popping.store(false, Ordering::Release);
                return head;
            }
            Err(actual) => head = actual,
        }
    }
    class.popping.store(false, Ordering::Release);
    ptr::null_mut()
}

unsafe fn push_remote_available(class: &RemoteClass, block: *mut u8) {
    let mut head = class.blocks.load(Ordering::Relaxed);
    loop {
        unsafe { write_free_next(block, head) };
        #[cfg(test)]
        let comparison = TEST_FAIL_REMOTE_PUSH_CAS.with(|fail| {
            if fail.replace(false) {
                Err(class.blocks.load(Ordering::Relaxed))
            } else {
                class
                    .blocks
                    .compare_exchange_weak(head, block, Ordering::Release, Ordering::Relaxed)
            }
        });
        #[cfg(not(test))]
        let comparison = class
            .blocks
            .compare_exchange_weak(head, block, Ordering::Release, Ordering::Relaxed);
        match comparison {
            Ok(_) => return,
            Err(actual) => head = actual,
        }
    }
}

#[derive(Clone, Copy)]
enum ClassEventKind {
    Allocation,
    TlsCacheHit,
    RecycledBatchHit,
    RecycledWordRefill,
    RecycledSingleHit,
    FreshHit,
    SlabRefill,
    LocalFree,
    BitmapSpill,
    RemoteFree,
}

#[derive(Clone, Copy)]
enum MediumEventKind {
    TlsCacheHit,
    GlobalCacheHit,
    FreshCommit,
    CachedFree,
    GlobalFree,
    PurgedSpan,
}

#[inline(always)]
fn record_class_event(class_index: usize, event: ClassEventKind) {
    #[cfg(feature = "tuning-telemetry")]
    tuning_telemetry::record_class(
        class_index,
        match event {
            ClassEventKind::Allocation => ClassEvent::Allocation,
            ClassEventKind::TlsCacheHit => ClassEvent::TlsCacheHit,
            ClassEventKind::RecycledBatchHit => ClassEvent::RecycledBatchHit,
            ClassEventKind::RecycledWordRefill => ClassEvent::RecycledWordRefill,
            ClassEventKind::RecycledSingleHit => ClassEvent::RecycledSingleHit,
            ClassEventKind::FreshHit => ClassEvent::FreshHit,
            ClassEventKind::SlabRefill => ClassEvent::SlabRefill,
            ClassEventKind::LocalFree => ClassEvent::LocalFree,
            ClassEventKind::BitmapSpill => ClassEvent::BitmapSpill,
            ClassEventKind::RemoteFree => ClassEvent::RemoteFree,
        },
    );
    #[cfg(not(feature = "tuning-telemetry"))]
    let _ = (class_index, event);
}

#[inline(always)]
fn record_partial_scan(class_index: usize, scanned: usize, limit: usize) {
    #[cfg(feature = "tuning-telemetry")]
    tuning_telemetry::record_partial_scan(class_index, scanned, limit);
    #[cfg(not(feature = "tuning-telemetry"))]
    let _ = (class_index, scanned, limit);
}

#[inline(always)]
fn record_medium_event(event: MediumEventKind, count: usize) {
    #[cfg(feature = "tuning-telemetry")]
    tuning_telemetry::record_medium(
        match event {
            MediumEventKind::TlsCacheHit => MediumEvent::TlsCacheHit,
            MediumEventKind::GlobalCacheHit => MediumEvent::GlobalCacheHit,
            MediumEventKind::FreshCommit => MediumEvent::FreshCommit,
            MediumEventKind::CachedFree => MediumEvent::CachedFree,
            MediumEventKind::GlobalFree => MediumEvent::GlobalFree,
            MediumEventKind::PurgedSpan => MediumEvent::PurgedSpan,
        },
        count,
    );
    #[cfg(not(feature = "tuning-telemetry"))]
    let _ = (event, count);
}

#[inline(always)]
unsafe fn thread_state() -> *mut ThreadState {
    let state = THREAD_STATE.with(|storage| storage.get().cast::<ThreadState>());
    if !unsafe { (*state).cleanup_registered || (*state).tearing_down } {
        THREAD_STATE_GUARD.with(|_| {});
        unsafe { (*state).cleanup_registered = true };
    }
    state
}

unsafe fn thread_token(state: *mut ThreadState) -> usize {
    let token = unsafe { (*state).token };
    if token != 0 {
        return token;
    }
    let token = NEXT_THREAD_TOKEN.fetch_add(1, Ordering::Relaxed);
    debug_assert_ne!(token, 0);
    unsafe { (*state).token = token };
    token
}

#[inline(always)]
unsafe fn current_reusable_heap(state: *mut ThreadState) -> *mut ReusableHeapState {
    let active = unsafe { (*state).active_heap };
    if active.is_null() {
        unsafe { ensure_default_heap(state) }
    } else {
        active
    }
}

#[cold]
unsafe fn ensure_default_heap(state: *mut ThreadState) -> *mut ReusableHeapState {
    let existing = unsafe { (*state).default_heap };
    if !existing.is_null() {
        return existing;
    }

    let heap = hal::map(mem::size_of::<ReusableHeapState>()).cast::<ReusableHeapState>();
    if heap.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        heap.write(ReusableHeapState::new(GeneralOptions::new(), crate::domain::default_state()));
        initialize_general_heap(heap);
        (*state).default_heap = heap;
    }
    heap
}

#[inline(always)]
unsafe fn current_initialized_reusable_heap(state: *mut ThreadState) -> *mut ReusableHeapState {
    let active = unsafe { (*state).active_heap };
    if active.is_null() {
        unsafe { (*state).default_heap }
    } else {
        active
    }
}

#[inline(always)]
unsafe fn current_remote_heap(state: *mut ThreadState) -> *mut RemoteHeapState {
    let active = unsafe { (*state).active_remote };
    if active == unsafe { (*state).remote_heap } {
        ptr::null_mut()
    } else {
        active
    }
}

unsafe fn write_header(address: *mut u8, header: *mut ExtraHeader) {
    let header_address = unsafe { hal::allocation_prefix_for_write::<*mut ExtraHeader>(address, HEADER_OFFSET) };
    unsafe { header_address.cast::<*mut ExtraHeader>().write(header) };
}

unsafe fn read_header(address: *mut u8) -> *mut ExtraHeader {
    #[cfg(miri)]
    let header_address = unsafe { hal::allocation_prefix_for_read::<*mut ExtraHeader>(address, HEADER_OFFSET) };
    #[cfg(not(miri))]
    let header_address = unsafe { address.sub(HEADER_OFFSET).cast::<*mut ExtraHeader>() };
    #[cfg(all(debug_assertions, not(coverage_nightly)))]
    {
        if header_address.is_null() {
            // SAFETY: GlobalAlloc::dealloc requires a non-null pointer previously returned by this allocator.
            unsafe { std::hint::unreachable_unchecked() }
        } else {
            unsafe { header_address.read() }
        }
    }
    #[cfg(any(not(debug_assertions), coverage_nightly))]
    unsafe {
        header_address.read()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use allocation_hints::domain::Domain;

    use super::*;

    #[cfg(not(miri))]
    crate::config!(DirectTrackingConfig { track_aggregates: true });
    crate::config!(CallerTrackingConfig {
        track_callers: true,
        caller_event_capacity: 4,
        caller_allocation_stack_frames: 0,
        caller_deallocation_stack_frames: 0,
    });

    fn new_domain() -> Domain {
        crate::initialize();
        Domain::new()
    }

    struct LateTlsAllocatorUser;

    impl Drop for LateTlsAllocatorUser {
        fn drop(&mut self) {
            let state = unsafe { thread_state() };
            assert!(unsafe { (*state).tearing_down });
            assert!(unsafe { (*state).default_heap.is_null() });

            let allocator = unsafe { Rallocator::<Standard>::new() };
            let layout = Layout::new::<[u8; 64]>();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { allocator.dealloc(address, layout) };
            assert!(unsafe { (*state).default_heap.is_null() });
        }
    }

    thread_local! {
        static LATE_TLS_ALLOCATOR_USER: LateTlsAllocatorUser = const { LateTlsAllocatorUser };
    }

    #[derive(Clone, Copy)]
    struct SendSlab(*mut SlabHeader);

    unsafe impl Send for SendSlab {}

    #[derive(Clone, Copy)]
    struct SendAddress(*mut u8);

    unsafe impl Send for SendAddress {}

    impl SendSlab {
        unsafe fn queue(self) {
            unsafe { queue_remote_slab(self.0) };
        }
    }

    #[derive(Clone, Copy)]
    struct SendRemotePush {
        slab: *mut SlabHeader,
        block: *mut u8,
    }

    unsafe impl Send for SendRemotePush {}

    impl SendRemotePush {
        unsafe fn push(self) {
            unsafe { push_remote_block(self.slab, self.block, 0, 1) };
        }

        #[cfg(not(miri))]
        unsafe fn republish_after_drain(self, ready: &AtomicBool, barrier: &std::sync::Barrier) {
            while !unsafe { (*self.slab).remote_free.load(Ordering::Acquire) }.is_null() {
                ready.store(true, Ordering::Release);
                spin_loop();
            }
            unsafe {
                write_free_next(self.block, ptr::null_mut());
                write_free_requested(self.block, 0);
                (*self.slab).remote_free.store(self.block, Ordering::Release);
            }
            barrier.wait();
        }
    }

    #[test]
    fn constructors_initialize_empty_runtime_state() {
        crate::initialize();
        let state = ThreadState::new();
        assert_eq!(state.token, 0);
        assert!(state.default_heap.is_null());
        assert!(state.remote_heap.is_null());
        assert!(state.active_heap.is_null());
        assert!(state.active_bump.is_null());
        assert!(state.active_remote.is_null());
        assert_eq!(state.bump_pool_len, 0);

        let class = RemoteClass::new();
        assert!(class.blocks.load(Ordering::Relaxed).is_null());
        assert!(!class.popping.load(Ordering::Relaxed));
        assert!(!class.refilling.load(Ordering::Relaxed));

        assert!(context_required_size(usize::MAX, 16).is_none());
        assert!(direct_mapping_size(usize::MAX, 16, 0).is_none());
    }

    #[test]
    fn remote_available_retries_compare_exchange_failures() {
        crate::initialize();
        let class = RemoteClass::new();
        let block = Box::into_raw(Box::new(0_usize)).cast::<u8>();

        fail_next_test_remote_push_cas();
        unsafe { push_remote_available(&class, block) };
        assert_eq!(class.blocks.load(Ordering::Relaxed), block);

        fail_next_test_remote_pop_cas();
        assert_eq!(unsafe { pop_remote_block(&class) }, block);
        assert!(class.blocks.load(Ordering::Relaxed).is_null());

        unsafe { drop(Box::from_raw(block.cast::<usize>())) };
    }

    #[cfg(not(miri))]
    #[test]
    fn allocator_propagates_hal_allocation_failures() {
        crate::initialize();
        hal::fail_next_map();
        assert!(create_domain().is_null());

        let mut domain = DomainState::new();
        let domain_pointer = ptr::from_mut(&mut domain);
        let published = AtomicPtr::new(ptr::null_mut());
        let mut medium = MediumState {
            regions: ptr::null_mut(),
            last_region: ptr::null_mut(),
        };
        hal::fail_next_reserve();
        assert!(unsafe { append_region(&mut medium, &published, domain_pointer) }.is_none());
        hal::fail_next_map();
        assert!(unsafe { append_region(&mut medium, &published, domain_pointer) }.is_none());

        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain_pointer);
        hal::fail_next_commit_locality_segment();
        assert!(allocator.allocate_slab(&mut heap).address.is_null());

        let mut thread = ThreadState::new();
        hal::fail_next_map();
        assert!(unsafe { ensure_default_heap(ptr::from_mut(&mut thread)) }.is_null());

        hal::fail_next_map();
        assert!(create_bump_fallback_heap(domain_pointer).is_null());

        hal::fail_next_commit_locality_segment();
        assert!(allocator.refill(0, &mut heap).is_null());
        hal::fail_next_commit_locality_segment();
        assert!(allocator.refill_context(0, &mut heap).is_null());
    }

    #[cfg(not(miri))]
    #[test]
    fn slab_initialization_and_fallback_cover_capacity_edges() {
        crate::initialize();
        let (first_block, block_count) = slab_block_layout(ConfigSizeClasses::<Standard>::SIZES[0]);
        assert!(first_block < block_count);

        let mut domain = DomainState::new();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
        let domain_pointer = ptr::from_mut(&mut domain);
        let regions = &domain.regions;
        assert!(regions.allocate_slices(domain_pointer, MEDIUM_REGION_SLICE_COUNT + 1).is_none());
        let full = regions.allocate_slices(domain_pointer, MEDIUM_REGION_SLICE_COUNT).unwrap();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        hal::fail_next_reserve();
        hal::fail_next_map();
        let fallback = allocator.allocate_slab(&mut heap);
        assert!(!fallback.address.is_null());
        assert_eq!(fallback.segment_slices, DIRECT_SLAB_SEGMENT);
        unsafe { hal::unmap(fallback.address, SLAB_SIZE) };

        let mut state = regions.state.lock();
        let region = state.regions;
        clear_region_cache();
        unsafe {
            hal::unmap((*region).base, MEDIUM_REGION_SIZE);
            hal::unmap(region.cast(), size_of::<RegionState>());
        }
        state.regions = ptr::null_mut();
        state.last_region = ptr::null_mut();
        regions.regions.store(ptr::null_mut(), Ordering::Relaxed);
        let _ = full;
    }

    #[cfg(not(miri))]
    #[test]
    fn slow_refills_reuse_active_and_partial_slabs() {
        crate::initialize();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut domain = DomainState::new();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));

        let normal = hal::map(SLAB_SIZE);
        let first = allocator.initialize_slab(
            SlabAllocation {
                address: normal,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        assert!(!first.is_null());
        assert!(!allocator.pop_or_refill_slow(0, &mut heap).is_null());
        let normal_header = normal.cast::<SlabHeader>();
        heap.classes[0].active = ptr::null_mut();
        heap.class_lists[0].partial = normal_header;
        assert!(!allocator.pop_or_refill_slow(0, &mut heap).is_null());
        unsafe {
            (*normal_header).fresh_next = ptr::null_mut();
            (*normal_header).recycled_summary = 0;
            (*normal_header).remote_free.store(ptr::null_mut(), Ordering::Relaxed);
        }
        heap.classes[0].active = ptr::null_mut();
        heap.class_lists[0].partial = normal_header;
        hal::fail_next_commit_locality_segment();
        assert!(allocator.pop_or_refill_slow(0, &mut heap).is_null());

        let contextual = hal::map(SLAB_SIZE);
        let first = allocator.initialize_slab(
            SlabAllocation {
                address: contextual,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            CONTEXT_SLAB_MARKER,
        );
        assert!(!first.is_null());
        assert!(!allocator.pop_or_refill_context_slow(0, &mut heap).is_null());
        let context_header = contextual.cast::<SlabHeader>();
        heap.context_classes[0].active = ptr::null_mut();
        heap.context_class_lists[0].partial = context_header;
        assert!(!allocator.pop_or_refill_context_slow(0, &mut heap).is_null());
        unsafe {
            (*context_header).fresh_next = ptr::null_mut();
            (*context_header).recycled_summary = 0;
            (*context_header).remote_free.store(ptr::null_mut(), Ordering::Relaxed);
        }
        heap.context_classes[0].active = context_header;
        heap.context_class_lists[0].partial = ptr::null_mut();
        hal::fail_next_commit_locality_segment();
        assert!(allocator.pop_or_refill_context(0, &mut heap).is_null());
        heap.context_class_lists[0].partial = context_header;
        hal::fail_next_commit_locality_segment();
        assert!(allocator.pop_or_refill_context_slow(0, &mut heap).is_null());
        hal::fail_next_commit_locality_segment();
        assert!(allocator.pop_or_refill_context_slow(0, &mut heap).is_null());
        assert!(heap.context_classes[0].active.is_null());

        unsafe {
            hal::unmap(normal, SLAB_SIZE);
            hal::unmap(contextual, SLAB_SIZE);
        }
    }

    #[cfg(not(miri))]
    #[test]
    fn allocator_recovers_from_commit_and_decommit_failures() {
        crate::initialize();
        let domain = new_domain();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), crate::domain::state(domain));

        hal::fail_next_commit();
        let medium_layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        assert!(allocator.allocate_medium(medium_layout, &mut heap).is_null());

        hal::fail_next_commit();
        assert!(allocate_bump_chunk(crate::domain::state(domain)).is_null());

        let bump = allocate_bump_chunk(crate::domain::state(domain));
        assert!(!bump.is_null());
        hal::fail_next_decommit();
        unsafe { release_bump_chunk(bump) };
        assert!(region_containing(bump).is_some());
        unsafe { release_bump_chunk(bump) };

        let domain_state = crate::domain::state(domain);
        let address = unsafe { domain_regions(domain_state).allocate_slices(domain_state, 1) }.unwrap();
        assert!(unsafe { hal::commit(address, MEDIUM_SLICE_SIZE) });
        hal::fail_next_decommit();
        assert!(!unsafe { release_retired_storage(address, false, MEDIUM_SLICE_SIZE, MEDIUM_SLICE_SIZE, false) });
        assert!(unsafe { release_retired_storage(address, false, MEDIUM_SLICE_SIZE, MEDIUM_SLICE_SIZE, false) });
    }

    #[cfg(not(miri))]
    #[test]
    fn locality_slab_commit_failure_preserves_the_reserved_segment() {
        crate::initialize();
        let domain = new_domain();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), crate::domain::state(domain));
        let first = allocator.allocate_slab(&mut heap);
        assert!(!first.address.is_null());
        assert!(!allocator.initialize_slab(first, 0, &mut heap, SLAB_MARKER).is_null());

        assert!(!allocator.allocate_slab(&mut heap).address.is_null());
        hal::zero_next_commit_locality_slab();
        assert!(!allocator.allocate_slab(&mut heap).address.is_null());
        hal::fully_commit_next_locality_slab();
        assert!(!allocator.allocate_slab(&mut heap).address.is_null());

        hal::fail_next_commit_locality_slab();
        assert!(allocator.allocate_slab(&mut heap).address.is_null());

        let segment = heap.locality_segment.cast::<u8>();
        let slices = unsafe { (*heap.locality_segment).segment_slices as usize };
        let committed = unsafe { (*heap.locality_segment).segment_committed_bytes };
        assert!(unsafe { hal::decommit(segment, committed) });
        unsafe { domain_regions(crate::domain::state(domain)).release_slices(segment, slices) };
    }

    #[cfg(not(miri))]
    #[test]
    fn remote_and_direct_allocation_failures_return_null() {
        crate::initialize();
        let domain = new_domain();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), crate::domain::state(domain));

        hal::fail_next_map();
        let direct_layout = Layout::from_size_align(2 * MEDIUM_REGION_SIZE, 16).unwrap();
        assert!(unsafe { allocator.allocate_direct(direct_layout, false, None, ptr::from_mut(&mut heap)) }.is_null());

        let mut remote = Box::new(RemoteHeapState {
            owner: ptr::null_mut(),
            embedded_owner: OwnerState::new(),
            owner_token: AtomicUsize::new(1),
            owner_heap: AtomicPtr::new(ptr::from_mut(&mut heap)),
            domain: crate::domain::state(domain),
            options: GeneralOptions::new(),
            usage: RemoteUsage::new(),
            classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
        });
        remote.owner = ptr::addr_of_mut!(remote.embedded_owner);
        hal::fail_next_commit();
        assert!(
            allocator
                .pop_or_refill_remote(ptr::from_mut(remote.as_mut()), 0, false, 1)
                .is_null()
        );
        assert_eq!(remote.usage.operations.load(Ordering::Relaxed), 0);

        force_next_test_remote_refill_contention();
        assert!(
            !allocator
                .pop_or_refill_remote(ptr::from_mut(remote.as_mut()), 0, false, 1)
                .is_null()
        );
        assert!(!remote.classes[0].refilling.load(Ordering::Relaxed));
        assert_eq!(remote.usage.operations.load(Ordering::Relaxed), 0);

        hal::fail_next_align_offset();
        assert!(unsafe { allocator.allocate_direct(Layout::new::<u64>(), false, None, ptr::from_mut(&mut heap),) }.is_null());

        let contextual =
            unsafe { allocator.allocate_direct(Layout::from_size_align(24, 16).unwrap(), true, None, ptr::from_mut(&mut heap)) };
        assert!(!contextual.is_null());
        unsafe {
            let header = read_header(contextual);
            allocator.deallocate_direct(
                contextual,
                Layout::from_size_align(24, 16).unwrap(),
                header.map_addr(|address| address & !TAG_MASK),
            );
        }

        let aligned_layout = Layout::new::<u64>();
        let shifted = unsafe { allocator.allocate_direct(aligned_layout, true, None, ptr::from_mut(&mut heap)) };
        assert!(!shifted.is_null());
        assert_ne!(shifted.addr() & (ConfigSizeClasses::<Standard>::SIZES[0] - 1), 0);
        unsafe {
            let header = read_header(shifted);
            allocator.deallocate_direct(shifted, aligned_layout, header.map_addr(|address| address & !TAG_MASK));
        }

        let state = ptr::from_mut(unsafe { &mut *thread_state() });
        let over_aligned = Layout::from_size_align(32, 2 * MAX_SMALL_ALIGNMENT).unwrap();
        let contextual = unsafe { allocator.allocate_with_context(over_aligned, None, state) };
        assert!(!contextual.is_null());
        unsafe { allocator.dealloc(contextual, over_aligned) };

        let mut isolated_thread = ThreadState::new();
        isolated_thread.default_heap = ptr::from_mut(&mut heap);
        hal::fail_next_commit_locality_segment();
        assert!(unsafe { allocator.allocate_with_context(Layout::new::<u8>(), None, ptr::from_mut(&mut isolated_thread),) }.is_null());
    }

    #[cfg(not(miri))]
    #[test]
    fn ordinary_alignment_direct_payload_cannot_forge_bump_ownership() {
        const OLD_BUMP_MARKER: usize = 0x5241_4C4C_4152_454E;

        crate::initialize();
        // SAFETY: DirectTrackingConfig uses the same Standard tunables and block
        // geometry as every other allocator in this test process; only aggregate
        // counter instrumentation differs.
        let allocator = unsafe { Rallocator::<DirectTrackingConfig>::new() };
        let layout = Layout::from_size_align(3 * size_of::<usize>(), align_of::<usize>()).unwrap();
        // SAFETY: the owner is null, tracking is disabled, and layout is valid.
        let address = unsafe { allocator.allocate_direct(layout, false, None, ptr::null_mut()) };
        assert!(!address.is_null());
        let unmappings = tracking::stats().unwrap().os_unmappings;

        let words = address.cast::<usize>();
        // SAFETY: the successful allocation contains three aligned usize slots.
        // All writes stay within that live allocation, and dealloc receives the
        // original pointer with the identical layout.
        unsafe {
            words.write(OLD_BUMP_MARKER);
            words.add(1).write(!OLD_BUMP_MARKER);
            words.add(2).write(1);
            allocator.dealloc(address, layout);
        }

        assert!(tracking::stats().unwrap().os_unmappings > unmappings);
    }

    #[cfg(not(miri))]
    #[test]
    fn global_allocation_falls_back_after_injected_failures() {
        crate::initialize();
        std::thread::spawn(|| {
            let allocator = unsafe { Rallocator::<Standard>::new() };
            hal::fail_next_map();
            assert!(unsafe { allocator.alloc(Layout::new::<u8>()) }.is_null());

            let state = unsafe { &mut *thread_state() };
            assert!(!unsafe { ensure_default_heap(ptr::from_mut(state)) }.is_null());
            hal::fail_next_commit();
            let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { allocator.dealloc(address, layout) };

            hal::fail_next_commit_locality_segment();
            let layout = Layout::new::<u8>();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { allocator.dealloc(address, layout) };
        })
        .join()
        .unwrap();
    }

    #[cfg(not(miri))]
    #[test]
    fn thread_heap_and_bump_chunk_report_backing_failures() {
        crate::initialize();
        std::thread::spawn(|| {
            hal::fail_next_map();
            assert!(thread_heap_state().is_none());

            let state = unsafe { &mut *thread_state() };
            assert!(!unsafe { ensure_default_heap(ptr::from_mut(state)) }.is_null());
            hal::fail_next_map();
            assert!(thread_heap_state().is_none());
        })
        .join()
        .unwrap();

        let mut domain = DomainState::new();
        let domain_pointer = ptr::from_mut(&mut domain);
        domain.regions.allocate_slices(domain_pointer, MEDIUM_REGION_SLICE_COUNT).unwrap();
        hal::fail_next_reserve();
        assert!(allocate_bump_chunk(domain_pointer).is_null());

        let regions = &domain.regions;
        let mut state = regions.state.lock();
        let region = state.regions;
        clear_region_cache();
        unsafe {
            hal::unmap((*region).base, MEDIUM_REGION_SIZE);
            hal::unmap(region.cast(), size_of::<RegionState>());
        }
        state.regions = ptr::null_mut();
        state.last_region = ptr::null_mut();
        regions.regions.store(ptr::null_mut(), Ordering::Relaxed);

        std::thread::spawn(|| {
            let domain = new_domain();
            for _ in 0..5 {
                let bump = bump::create_state(allocation_hints::heap::bump::Options::new(), crate::domain::state(domain)).unwrap();
                return_pooled_bump(bump);
            }
            assert_eq!(unsafe { (*thread_state()).bump_pool_len }, 4);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn tracking_target_creates_a_log_for_a_new_session() {
        crate::initialize();
        let _test = tracking::TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        tracking::track_callers(true);
        invalidate_tracking_cache();
        assert!(tracking_target::<Standard>(tracking::active_session()).is_some());
        tracking::track_callers(false);
        invalidate_tracking_cache();
    }

    #[test]
    fn caller_tracking_allocates_directly_from_an_active_bump() {
        crate::initialize();
        let _test = tracking::TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let domain = new_domain();
        let options = allocation_hints::heap::bump::Options::new();
        let bump_state = bump::create_state(options, crate::domain::state(domain)).unwrap();
        unsafe { bump::reset_state(bump_state, options) };
        assert!(bump::ensure_fallback_heap(bump_state));

        let state = unsafe { thread_state() };
        let saved = unsafe { ((*state).active_heap, (*state).active_bump, (*state).active_remote) };
        unsafe {
            (*state).active_heap = (*bump_state).fallback_heap;
            (*state).active_bump = bump_state;
            (*state).active_remote = ptr::null_mut();
        }
        tracking::track_callers(true);
        invalidate_tracking_cache();

        let allocator = unsafe { Rallocator::<CallerTrackingConfig>::new() };
        let layout = Layout::new::<u64>();
        let address = unsafe { allocator.alloc(layout) };
        assert!(!address.is_null());
        unsafe { allocator.dealloc(address, layout) };

        tracking::track_callers(false);
        invalidate_tracking_cache();
        let untracked = unsafe { allocator.alloc(layout) };
        assert!(!untracked.is_null());
        unsafe { allocator.dealloc(untracked, layout) };

        let fallback_layout = Layout::from_size_align(bump::BUMP_SEGMENT_SIZE + 1, 16).unwrap();
        let fallback = unsafe { allocator.alloc(fallback_layout) };
        assert!(!fallback.is_null());
        unsafe { allocator.dealloc(fallback, fallback_layout) };

        unsafe {
            ((*state).active_heap, (*state).active_bump, (*state).active_remote) = saved;
            bump::release_handle(bump_state);
        }
    }

    #[test]
    fn thread_heap_usage_rejects_a_detached_owner_heap() {
        crate::initialize();
        let state = unsafe { thread_state() };
        let token = unsafe { thread_token(state) };
        let mut remote = Box::new(RemoteHeapState {
            owner: ptr::null_mut(),
            embedded_owner: OwnerState::new(),
            owner_token: AtomicUsize::new(token),
            owner_heap: AtomicPtr::new(ptr::null_mut()),
            domain: crate::domain::default_state(),
            options: GeneralOptions::new(),
            usage: RemoteUsage::new(),
            classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
        });
        remote.owner = ptr::addr_of_mut!(remote.embedded_owner);
        assert_eq!(unsafe { thread_heap_usage(ptr::from_mut(remote.as_mut())) }, Err(()));
    }

    #[test]
    fn exhausted_bump_allocations_use_every_general_fallback_class() {
        crate::initialize();
        let domain = new_domain();
        let options = allocation_hints::heap::bump::Options::new().with_max_allocation_bytes(1);
        let bump_state = bump::create_state(options, crate::domain::state(domain)).unwrap();
        unsafe { bump::reset_state(bump_state, options) };
        assert!(bump::ensure_fallback_heap(bump_state));
        let fallback = unsafe { (*bump_state).fallback_heap };
        let state = unsafe { thread_state() };
        let saved = unsafe { ((*state).active_heap, (*state).active_bump, (*state).active_remote) };

        #[cfg(not(miri))]
        let remote = {
            let mut remote = Box::new(RemoteHeapState {
                owner: ptr::null_mut(),
                embedded_owner: OwnerState::new(),
                owner_token: AtomicUsize::new(unsafe { thread_token(state) }),
                owner_heap: AtomicPtr::new(fallback),
                domain: crate::domain::state(domain),
                options: GeneralOptions::new(),
                usage: RemoteUsage::new(),
                classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
                context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            });
            remote.owner = ptr::addr_of_mut!(remote.embedded_owner);
            Box::into_raw(remote)
        };
        unsafe {
            (*state).active_heap = fallback;
            (*state).active_bump = bump_state;
        }
        #[cfg(not(miri))]
        unsafe {
            (*state).active_remote = remote;
        }
        #[cfg(miri)]
        unsafe {
            (*state).active_remote = ptr::null_mut();
        }

        let allocator = unsafe { Rallocator::<Standard>::new() };
        for layout in [
            Layout::from_size_align(16, 16).unwrap(),
            Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap(),
            Layout::from_size_align(16, MAX_MEDIUM_ALIGNMENT * 2).unwrap(),
        ] {
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { allocator.dealloc(address, layout) };
        }

        unsafe {
            ((*state).active_heap, (*state).active_bump, (*state).active_remote) = saved;
        }
        unsafe { bump::release_handle(bump_state) };
    }

    #[test]
    fn medium_purge_releases_fixed_and_variable_spans() {
        crate::initialize();
        let domain = new_domain();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let options = GeneralOptions::from_values(MEDIUM_SLICE_SIZE, 0);
        let mut heap = ReusableHeapState::new(options, crate::domain::state(domain));
        let fixed_layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let large_layout = Layout::from_size_align((MEDIUM_MAX_SLICES + 1) * MEDIUM_SLICE_SIZE, 16).unwrap();

        let fixed = allocator.allocate_medium(fixed_layout, &mut heap);
        let large = allocator.allocate_medium(large_layout, &mut heap);
        assert!(!fixed.is_null());
        assert!(!large.is_null());
        unsafe {
            allocator.deallocate_medium(fixed, fixed_layout, ptr::from_mut(&mut heap));
            allocator.deallocate_medium(large, large_layout, ptr::from_mut(&mut heap));
        }

        let regions = unsafe { domain_regions(crate::domain::state(domain)) };
        let mut state = regions.state.lock();
        allocator.purge_medium_locked_at(&mut state, false, 0);
        assert!(!unsafe { (*state.regions).bins[0].free_list }.is_null());
        assert!(!unsafe { (*state.regions).large_free }.is_null());
        unsafe {
            (*state.regions).bins[0].purge_after = 2;
            (*state.regions).large_purge_after = 2;
        }
        allocator.purge_medium_locked_at(&mut state, false, 1);
        assert!(!unsafe { (*state.regions).bins[0].free_list }.is_null());
        assert!(!unsafe { (*state.regions).large_free }.is_null());
        allocator.purge_medium_locked_at(&mut state, false, 2);
        assert!(unsafe { (*state.regions).bins[0].free_list }.is_null());
        assert!(unsafe { (*state.regions).large_free }.is_null());
        allocator.purge_medium_locked(&mut state, true);
    }

    #[cfg(not(miri))]
    #[test]
    fn medium_purge_keeps_slices_reserved_when_decommit_fails() {
        crate::initialize();
        let domain = new_domain();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::from_values(MEDIUM_SLICE_SIZE, 0), crate::domain::state(domain));
        let fixed_layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let fixed = allocator.allocate_medium(fixed_layout, &mut heap);
        unsafe { allocator.deallocate_medium(fixed, fixed_layout, ptr::from_mut(&mut heap)) };
        let regions = unsafe { domain_regions(crate::domain::state(domain)) };
        {
            let mut state = regions.state.lock();
            unsafe { (*state.regions).bins[0].purge_after = 1 };
            hal::fail_next_decommit();
            allocator.purge_medium_locked(&mut state, false);
        }
        assert!(unsafe { hal::decommit(fixed, MEDIUM_SLICE_SIZE) });
        unsafe { regions.release_slices(fixed, 1) };

        let large_layout = Layout::from_size_align((MEDIUM_MAX_SLICES + 1) * MEDIUM_SLICE_SIZE, 16).unwrap();
        let large = allocator.allocate_medium(large_layout, &mut heap);
        unsafe { allocator.deallocate_medium(large, large_layout, ptr::from_mut(&mut heap)) };
        {
            let mut state = regions.state.lock();
            unsafe { (*state.regions).large_purge_after = 1 };
            hal::fail_next_decommit();
            allocator.purge_medium_locked(&mut state, false);
        }
        let slices = medium_slice_count(large_layout).unwrap();
        assert!(unsafe { hal::decommit(large, slices * MEDIUM_SLICE_SIZE) });
        unsafe { regions.release_slices(large, slices) };
    }

    #[test]
    fn invalid_medium_requests_and_non_region_metadata_are_rejected() {
        crate::initialize();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut domain = DomainState::new();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
        let over_aligned = Layout::from_size_align(1, 2 * MAX_MEDIUM_ALIGNMENT).unwrap();
        assert!(allocator.allocate_medium(over_aligned, &mut heap).is_null());
        unsafe { allocator.deallocate_medium(ptr::null_mut(), over_aligned, ptr::from_mut(&mut heap)) };
        let oversized = Layout::from_size_align(MEDIUM_REGION_SIZE + MEDIUM_SLICE_SIZE, 16).unwrap();
        assert_eq!(medium_slice_count(oversized), None);

        let outside = ptr::without_provenance_mut::<u8>(0x1234_5000);
        record_small_segment(outside, 0, false, ptr::null_mut(), 1, false);
        update_physical_small_live_blocks(outside, true);
        assert_eq!(allocation_segment(outside).addr(), outside.addr() & !(SLAB_SIZE - 1));

        let state = MediumState {
            regions: ptr::null_mut(),
            last_region: ptr::null_mut(),
        };
        assert!(unsafe { find_region(&state, outside) }.is_none());
        assert!(!slices_are_free(outside, 1));
    }

    #[test]
    fn ordinary_layout_medium_allocation_uses_region_deallocation_path() {
        crate::initialize();
        let domain = new_domain();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), crate::domain::state(domain));
        let layout = Layout::new::<[u8; 16]>();
        let address = allocator.allocate_medium(layout, &mut heap);
        assert!(!address.is_null());
        unsafe {
            address.cast::<usize>().write(0);
            allocator.dealloc(address, layout);
        }
    }

    #[test]
    fn topology_snapshots_cover_continuations_and_truncated_spans() {
        crate::initialize();
        let domain = new_domain();
        let domain_state = crate::domain::state(domain);
        let regions = unsafe { domain_regions(domain_state) };
        let base = regions.allocate_slices(domain_state, 12).unwrap();
        let region = region_containing(base).unwrap();
        unsafe {
            (*region).physical[0]
                .kind_and_span
                .store(PHYSICAL_SLICE_MEDIUM | (3 << PHYSICAL_SPAN_SHIFT), Ordering::Release);
            (*region).physical[1].kind_and_span.store(PHYSICAL_SLICE_SMALL, Ordering::Release);
            (*region).physical[2]
                .kind_and_span
                .store(PHYSICAL_SLICE_MEDIUM_CONTINUATION, Ordering::Release);
            (*region).physical[11]
                .kind_and_span
                .store(PHYSICAL_SLICE_MEDIUM | (2 << PHYSICAL_SPAN_SHIFT), Ordering::Release);
        }

        let snapshots = telemetry_region_snapshots();
        let snapshot = snapshots.iter().find(|snapshot| snapshot.base_address == base.addr()).unwrap();
        assert_eq!(snapshot.used_slices, 12);
        assert_eq!(snapshot.slices[2].kind, tracking::PhysicalSliceKind::MediumContinuation);
        assert_eq!(allocation_segment(unsafe { base.add(123) }).addr(), base.addr());

        unsafe { regions.release_slices(base, 12) };
    }

    #[test]
    fn direct_slab_usage_and_list_removal_cover_non_head_entries() {
        crate::initialize();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut domain = DomainState::new();
        let domain_pointer = ptr::from_mut(&mut domain);
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain_pointer);
        unsafe { initialize_general_heap(ptr::from_mut(&mut heap)) };

        let slab = hal::map(SLAB_SIZE);
        let block = allocator.initialize_slab(
            SlabAllocation {
                address: slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        assert!(!block.is_null());
        let mut remote = Box::new(RemoteHeapState {
            owner: ptr::null_mut(),
            embedded_owner: OwnerState::new(),
            owner_token: AtomicUsize::new(0),
            owner_heap: AtomicPtr::new(ptr::from_mut(&mut heap)),
            domain: domain_pointer,
            options: GeneralOptions::new(),
            usage: RemoteUsage::new(),
            classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
        });
        remote.owner = ptr::addr_of_mut!(remote.embedded_owner);
        unsafe { record_small_allocation::<crate::tunables::Standard>(block, 0, 16, ptr::from_mut(remote.as_mut())) };
        let usage = unsafe { general_heap_usage(ptr::from_mut(&mut heap), ptr::null_mut()) };
        assert_eq!(usage.reserved_bytes(), SLAB_SIZE);

        let segment = hal::map(MEDIUM_SLICE_SIZE);
        let first_segment_block = allocator.initialize_slab(
            SlabAllocation {
                address: segment,
                segment_slices: 1,
                committed_bytes: MEDIUM_SLICE_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        let second_segment_block = allocator.initialize_slab(
            SlabAllocation {
                address: unsafe { segment.add(SLAB_SIZE) },
                segment_slices: 0,
                committed_bytes: 0,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        assert!(!first_segment_block.is_null());
        assert!(!second_segment_block.is_null());
        heap.locality_segment = ptr::null_mut();
        let usage = unsafe { general_heap_usage(ptr::from_mut(&mut heap), ptr::null_mut()) };
        assert!(usage.reserved_bytes() >= MEDIUM_SLICE_SIZE + SLAB_SIZE);

        let layout = Layout::from_size_align(MEDIUM_REGION_SIZE + MEDIUM_SLICE_SIZE, 16).unwrap();
        let first = unsafe { allocator.allocate_direct(layout, false, None, ptr::from_mut(&mut heap)) };
        let second = unsafe { allocator.allocate_direct(layout, false, None, ptr::from_mut(&mut heap)) };
        assert!(!first.is_null());
        assert!(!second.is_null());
        unsafe {
            let first_extra = read_header(first).map_addr(|address| address & !TAG_MASK);
            allocator.deallocate_direct(first, layout, first_extra);
            let second_extra = read_header(second).map_addr(|address| address & !TAG_MASK);
            allocator.deallocate_direct(second, layout, second_extra);
            hal::unmap(slab, SLAB_SIZE);
            hal::unmap(segment, MEDIUM_SLICE_SIZE);
            release_owner_storage((*heap.owner).retirement);
        }
    }

    #[cfg(not(miri))]
    #[test]
    fn retirement_handles_direct_slabs_and_decommit_failures() {
        crate::initialize();
        let mut domain = DomainState::new();
        let domain_pointer = ptr::from_mut(&mut domain);
        let allocator = unsafe { Rallocator::<Standard>::new() };

        let empty_mapping = hal::map(SLAB_SIZE);
        assert!(!empty_mapping.is_null());
        assert!(!unsafe { prepare_retired_slice(empty_mapping.cast(), 0, SLAB_SIZE, true, false) });

        let empty_heap = create_bump_fallback_heap(domain_pointer);
        assert!(!empty_heap.is_null());
        let empty_slab = hal::map(SLAB_SIZE);
        let empty_block = allocator.initialize_slab(
            SlabAllocation {
                address: empty_slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            unsafe { &mut *empty_heap },
            SLAB_MARKER,
        );
        assert!(!empty_block.is_null());
        unsafe { (*empty_slab.cast::<SlabHeader>()).free_count += 1 };
        unsafe { retire_general_heap(empty_heap) };

        let live_heap = create_bump_fallback_heap(domain_pointer);
        assert!(!live_heap.is_null());
        let live_slab = hal::map(SLAB_SIZE);
        let live_block = allocator.initialize_slab(
            SlabAllocation {
                address: live_slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            unsafe { &mut *live_heap },
            SLAB_MARKER,
        );
        assert!(!live_block.is_null());
        unsafe { retire_general_heap(live_heap) };
        let mut thread = ThreadState::new();
        unsafe { push_block::<crate::tunables::Standard>(live_block, 0, 1, ptr::from_mut(&mut thread)) };

        let full_domain = new_domain();
        let full_domain_state = crate::domain::state(full_domain);
        let full_heap = create_bump_fallback_heap(full_domain_state);
        let full_regions = unsafe { domain_regions(full_domain_state) };
        let full_segment = full_regions.allocate_slices(full_domain_state, 1).unwrap();
        assert!(unsafe { hal::commit(full_segment, MEDIUM_SLICE_SIZE) });
        let first = allocator.initialize_slab(
            SlabAllocation {
                address: full_segment,
                segment_slices: 1,
                committed_bytes: MEDIUM_SLICE_SIZE,
            },
            0,
            unsafe { &mut *full_heap },
            SLAB_MARKER,
        );
        let second = allocator.initialize_slab(
            SlabAllocation {
                address: unsafe { full_segment.add(SLAB_SIZE) },
                segment_slices: 0,
                committed_bytes: 0,
            },
            0,
            unsafe { &mut *full_heap },
            SLAB_MARKER,
        );
        unsafe {
            (*full_segment.cast::<SlabHeader>()).free_count += 1;
            (*full_segment.add(SLAB_SIZE).cast::<SlabHeader>()).free_count += 1;
            (*full_heap).locality_segment = ptr::null_mut();
        }
        assert!(!first.is_null());
        assert!(!second.is_null());
        unsafe { retire_general_heap(full_heap) };

        let cached_domain = new_domain();
        let cached_heap = create_bump_fallback_heap(crate::domain::state(cached_domain));
        assert!(!cached_heap.is_null());
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let cached = allocator.allocate_medium(layout, unsafe { &mut *cached_heap });
        assert!(!cached.is_null());
        unsafe { allocator.deallocate_medium(cached, layout, cached_heap) };
        hal::fail_next_decommit();
        unsafe { retire_general_heap(cached_heap) };

        let domain = new_domain();
        let domain_state = crate::domain::state(domain);
        let regions = unsafe { domain_regions(domain_state) };
        let slice = regions.allocate_slices(domain_state, 1).unwrap();
        assert!(unsafe { hal::commit(slice, MEDIUM_SLICE_SIZE) });
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain_state);
        unsafe { initialize_general_heap(ptr::from_mut(&mut heap)) };
        let block = allocator.initialize_slab(
            SlabAllocation {
                address: slice,
                segment_slices: 1,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        assert!(!block.is_null());
        assert!(unsafe { prepare_retired_slice(slice.cast(), 1, SLAB_SIZE, false, false) });
        hal::fail_next_decommit();
        unsafe { release_retired_block(slice.cast()) };
        assert!(unsafe { hal::decommit(slice, MEDIUM_SLICE_SIZE) });
        unsafe { regions.release_slices(slice, 1) };

        let invalid = hal::map(SLAB_SIZE);
        unsafe {
            invalid.cast::<RetiredSliceState>().write(RetiredSliceState {
                marker: AtomicUsize::new(0),
                owner: ptr::null_mut(),
                remaining: AtomicUsize::new(1),
                state: ptr::null_mut(),
                ready: AtomicBool::new(false),
                released: AtomicBool::new(false),
                track_aggregates: false,
                direct_mapping: true,
                state_padding: [0; 4],
                committed_bytes: SLAB_SIZE,
                release_bytes: SLAB_SIZE,
            });
        }
        assert!(std::panic::catch_unwind(|| unsafe { release_retired_block(invalid.cast()) }).is_err());
        unsafe { hal::unmap(invalid, SLAB_SIZE) };
    }

    #[cfg(not(miri))]
    #[test]
    fn exhausted_region_and_secondary_extents_cover_search_edges() {
        crate::initialize();
        let mut domain = DomainState::new();
        let domain_pointer = ptr::from_mut(&mut domain);
        let regions = &domain.regions;
        let full = regions.allocate_slices(domain_pointer, MEDIUM_REGION_SLICE_COUNT).unwrap();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain_pointer);
        hal::fail_next_reserve();
        assert!(
            allocator
                .allocate_medium(Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap(), &mut heap,)
                .is_null()
        );
        hal::fail_next_reserve();
        assert!(regions.allocate_slices(domain_pointer, 1).is_none());

        let region = regions.state.lock().regions;
        unsafe { mark_slices(&mut (*region).used, 0, MEDIUM_REGION_SLICE_COUNT, false) };
        assert!(unsafe { hal::commit(full, 8 * MEDIUM_SLICE_SIZE) });
        let first = full.cast::<LargeFreeBlock>();
        let second = unsafe { full.add(2 * MEDIUM_SLICE_SIZE).cast::<LargeFreeBlock>() };
        unsafe {
            first.write(LargeFreeBlock {
                next: second,
                slice_count: 1,
            });
            second.write(LargeFreeBlock {
                next: ptr::null_mut(),
                slice_count: 2,
            });
            (*region).large_free = first;
        }
        assert_eq!(unsafe { take_large_extent(region, 2) }, Some(second.cast::<u8>()));

        unsafe {
            second.write(LargeFreeBlock {
                next: ptr::null_mut(),
                slice_count: 3,
            });
            (*first).next = second;
            (*region).large_free = first;
        }
        assert_eq!(unsafe { take_large_extent(region, 2) }, Some(second.cast::<u8>()));
        assert_eq!(unsafe { (*first).next }, unsafe {
            second.cast::<u8>().add(2 * MEDIUM_SLICE_SIZE).cast::<LargeFreeBlock>()
        });

        assert!(unsafe { hal::decommit(full, 8 * MEDIUM_SLICE_SIZE) });
        let mut state = regions.state.lock();
        clear_region_cache();
        unsafe {
            hal::unmap((*region).base, MEDIUM_REGION_SIZE);
            hal::unmap(region.cast(), size_of::<RegionState>());
        }
        state.regions = ptr::null_mut();
        state.last_region = ptr::null_mut();
        regions.regions.store(ptr::null_mut(), Ordering::Relaxed);
    }

    #[test]
    fn coordination_retries_and_partial_selection_cover_contended_paths() {
        crate::initialize();
        let retirement = RetirementState::new();
        retirement.operations.store(OPERATION_RETIRED, Ordering::Relaxed);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { acquire_heap_inspection(&retirement) })).is_err());

        retirement.operations.store(1, Ordering::Relaxed);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::yield_now();
                retirement.operations.store(0, Ordering::Release);
            });
            acquire_heap_retirement(&retirement);
        });
        retirement.operations.store(OPERATION_INSPECTING, Ordering::Relaxed);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::yield_now();
                retirement.operations.store(0, Ordering::Release);
            });
            assert!(unsafe { begin_heap_usage_operation(ptr::from_ref(&retirement).cast_mut()) });
        });
        unsafe { end_heap_usage_operation(ptr::from_ref(&retirement).cast_mut()) };
        unsafe { release_external_allocation(ptr::null_mut()) };

        let usage = RemoteUsage::new();
        usage.operations.store(1, Ordering::Relaxed);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::yield_now();
                usage.operations.store(0, Ordering::Release);
            });
            acquire_remote_inspection(&usage);
        });

        let remote = Box::new(RemoteHeapState {
            owner: ptr::null_mut(),
            embedded_owner: OwnerState::new(),
            owner_token: AtomicUsize::new(0),
            owner_heap: AtomicPtr::new(ptr::null_mut()),
            domain: ptr::null_mut(),
            options: GeneralOptions::new(),
            usage: RemoteUsage::new(),
            classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
        });
        remote.usage.operations.store(OPERATION_INSPECTING, Ordering::Relaxed);
        let remote_pointer = ptr::from_ref(remote.as_ref()).cast_mut();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::yield_now();
                remote.usage.operations.store(0, Ordering::Release);
            });
            unsafe { begin_remote_usage_operation(remote_pointer) };
        });
        unsafe { end_remote_usage_operation(remote_pointer) };

        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut domain = DomainState::new();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
        let first = hal::map(SLAB_SIZE);
        let second = hal::map(SLAB_SIZE);
        allocator.initialize_slab(
            SlabAllocation {
                address: first,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        allocator.initialize_slab(
            SlabAllocation {
                address: second,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        unsafe {
            (*first.cast::<SlabHeader>()).free_count = 1;
            (*first.cast::<SlabHeader>()).next_partial = second.cast();
            (*second.cast::<SlabHeader>()).free_count = 2;
            (*second.cast::<SlabHeader>()).next_partial = ptr::null_mut();
        }
        let mut list = first.cast::<SlabHeader>();
        assert_eq!(
            unsafe { take_most_free_slab::<crate::tunables::Standard>(&mut list, 0) },
            second.cast()
        );
        assert!(unsafe { (*first.cast::<SlabHeader>()).next_partial }.is_null());
        unsafe {
            hal::unmap(first, SLAB_SIZE);
            hal::unmap(second, SLAB_SIZE);
        }
    }

    #[test]
    fn publication_retries_after_deterministic_compare_exchange_contention() {
        crate::initialize();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                set_test_cas_barrier(barrier);
                assert!(!create_domain().is_null());
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut domain = DomainState::new();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
        let first = hal::map(SLAB_SIZE);
        let second = hal::map(SLAB_SIZE);
        allocator.initialize_slab(
            SlabAllocation {
                address: first,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        allocator.initialize_slab(
            SlabAllocation {
                address: second,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for slab in [SendSlab(first.cast()), SendSlab(second.cast())] {
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                set_test_cas_barrier(barrier);
                unsafe { slab.queue() };
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        unsafe {
            (*heap.owner).remote_slabs.store(ptr::null_mut(), Ordering::Relaxed);
        }

        let slab = hal::map(SLAB_SIZE);
        let first_block = allocator.initialize_slab(
            SlabAllocation {
                address: slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        let second_block = unsafe { take_local_slab_block::<crate::tunables::Standard>(slab.cast(), 0) };
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut workers = Vec::new();
        for block in [first_block, second_block] {
            let barrier = barrier.clone();
            let push = SendRemotePush { slab: slab.cast(), block };
            workers.push(std::thread::spawn(move || {
                set_test_cas_barrier(barrier);
                unsafe { push.push() };
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        unsafe {
            (*heap.owner).remote_slabs.store(ptr::null_mut(), Ordering::Relaxed);
            hal::unmap(slab, SLAB_SIZE);
            hal::unmap(first, SLAB_SIZE);
            hal::unmap(second, SLAB_SIZE);
        }
    }

    #[test]
    fn context_remote_inbox_returns_freed_blocks_to_partial_lists() {
        crate::initialize();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let mut domain = DomainState::new();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
        let normal_slab = hal::map(SLAB_SIZE);
        let normal_block = allocator.initialize_slab(
            SlabAllocation {
                address: normal_slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            SLAB_MARKER,
        );
        let normal_header = normal_slab.cast::<SlabHeader>();
        unsafe {
            (*normal_header).free_count = 0;
            (*normal_header).fresh_next = ptr::null_mut();
            write_free_next(normal_block, ptr::null_mut());
            write_free_requested(normal_block, 0);
            (*normal_header).remote_free.store(normal_block, Ordering::Relaxed);
            (*normal_header).remote_queued.store(true, Ordering::Relaxed);
            (*heap.owner).remote_slabs.store(normal_header, Ordering::Relaxed);
        }
        heap.classes[0].active = ptr::null_mut();
        assert!(!allocator.pop_or_refill_slow(0, &mut heap).is_null());
        assert_eq!(heap.classes[0].active, normal_header);

        let slab = hal::map(SLAB_SIZE);
        let block = allocator.initialize_slab(
            SlabAllocation {
                address: slab,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut heap,
            CONTEXT_SLAB_MARKER,
        );
        assert!(!block.is_null());
        let header = slab.cast::<SlabHeader>();
        unsafe {
            (*header).free_count = 0;
            (*header).fresh_next = ptr::null_mut();
            write_free_next(block, ptr::null_mut());
            write_free_requested(block, 0);
            (*header).remote_free.store(block, Ordering::Relaxed);
            (*header).remote_queued.store(true, Ordering::Relaxed);
            (*heap.owner).remote_slabs.store(header, Ordering::Relaxed);
        }
        heap.context_classes[0].active = ptr::null_mut();
        assert!(!allocator.pop_or_refill_context_slow(0, &mut heap).is_null());
        assert_eq!(heap.context_classes[0].active, header);

        #[cfg(miri)]
        unsafe {
            hal::unmap(slab, SLAB_SIZE);
        }

        #[cfg(not(miri))]
        {
            unsafe {
                drain_remote_inbox::<crate::tunables::Standard>(&mut ReusableHeapState::new(
                    GeneralOptions::new(),
                    ptr::from_mut(&mut domain),
                ));
            };

            unsafe {
                (*header).remote_queued.store(true, Ordering::Relaxed);
                queue_remote_slab(header);
            }

            let requeued_slab = hal::map(SLAB_SIZE);
            let first_remote = allocator.initialize_slab(
                SlabAllocation {
                    address: requeued_slab,
                    segment_slices: DIRECT_SLAB_SEGMENT,
                    committed_bytes: SLAB_SIZE,
                },
                0,
                &mut heap,
                SLAB_MARKER,
            );
            let second_remote = unsafe { take_local_slab_block::<crate::tunables::Standard>(requeued_slab.cast(), 0) };
            let requeued_header = requeued_slab.cast::<SlabHeader>();
            unsafe {
                write_free_next(first_remote, ptr::null_mut());
                write_free_requested(first_remote, 0);
                (*requeued_header).remote_free.store(first_remote, Ordering::Relaxed);
                (*requeued_header).remote_queued.store(true, Ordering::Relaxed);
                (*heap.owner).remote_slabs.store(requeued_header, Ordering::Relaxed);
            }
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = barrier.clone();
            let ready = std::sync::Arc::new(AtomicBool::new(false));
            let worker_ready = ready.clone();
            let republish = SendRemotePush {
                slab: requeued_header,
                block: second_remote,
            };
            let worker = std::thread::spawn(move || unsafe { republish.republish_after_drain(&worker_ready, &worker_barrier) });
            while !ready.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            set_test_cas_barrier(barrier);
            unsafe { drain_remote_inbox::<crate::tunables::Standard>(&mut heap) };
            worker.join().unwrap();
            assert_eq!(unsafe { (*heap.owner).remote_slabs.load(Ordering::Relaxed) }, requeued_header);
            unsafe {
                (*heap.owner).remote_slabs.store(ptr::null_mut(), Ordering::Relaxed);
                hal::unmap(requeued_slab, SLAB_SIZE);
                hal::unmap(slab, SLAB_SIZE);
            }

            let remote_slab = hal::map(SLAB_SIZE);
            let mut remote = Box::new(RemoteHeapState {
                owner: ptr::null_mut(),
                embedded_owner: OwnerState::new(),
                owner_token: AtomicUsize::new(usize::MAX),
                owner_heap: AtomicPtr::new(ptr::null_mut()),
                domain: ptr::from_mut(&mut domain),
                options: GeneralOptions::new(),
                usage: RemoteUsage::new(),
                classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
                context_classes: [const { RemoteClass::new() }; MAX_SIZE_CLASSES],
            });
            remote.owner = ptr::addr_of_mut!(remote.embedded_owner);
            unsafe {
                allocator.initialize_remote_slab(remote_slab, 0, remote.owner, CONTEXT_SLAB_MARKER, &remote.context_classes[0]);
            }
            let remote_block = unsafe { pop_remote_block(&remote.context_classes[0]) };
            remote.usage.live_allocations.store(1, Ordering::Relaxed);
            remote.usage.requested_bytes.store(1, Ordering::Relaxed);
            remote.usage.usable_bytes.store(16, Ordering::Relaxed);
            let mut thread = ThreadState::new();
            unsafe { push_context_block::<crate::tunables::Standard>(remote_block, 0, 1, ptr::from_mut(&mut thread)) };
            assert!(!unsafe { pop_remote_block(&remote.context_classes[0]) }.is_null());
            unsafe { hal::unmap(remote_slab, SLAB_SIZE) };

            let spill_slab = hal::map(SLAB_SIZE);
            let first = allocator.initialize_slab(
                SlabAllocation {
                    address: spill_slab,
                    segment_slices: DIRECT_SLAB_SEGMENT,
                    committed_bytes: SLAB_SIZE,
                },
                0,
                &mut heap,
                CONTEXT_SLAB_MARKER,
            );
            let second = unsafe { take_local_slab_block::<crate::tunables::Standard>(spill_slab.cast(), 0) };
            let third = unsafe { take_local_slab_block::<crate::tunables::Standard>(spill_slab.cast(), 0) };
            let spill_header = spill_slab.cast::<SlabHeader>();
            let block_size = ConfigSizeClasses::<Standard>::SIZES[0];
            unsafe {
                (*spill_header).free_count = 0;
                (*spill_header).requested_bytes = block_size * 3;
            }
            heap.context_classes[0].active = ptr::null_mut();
            let mut thread = ThreadState::new();
            thread.default_heap = ptr::from_mut(&mut heap);
            unsafe {
                push_context_block::<crate::tunables::Standard>(first, 0, block_size, ptr::from_mut(&mut thread));
                push_context_block::<crate::tunables::Standard>(second, 0, block_size, ptr::from_mut(&mut thread));
                push_context_block::<crate::tunables::Standard>(third, 0, block_size, ptr::from_mut(&mut thread));
            }
            assert_eq!(heap.context_class_lists[0].partial, spill_header);
            unsafe { hal::unmap(spill_slab, SLAB_SIZE) };
        }

        unsafe { hal::unmap(normal_slab, SLAB_SIZE) };
    }

    #[test]
    fn missing_bump_chunk_region_delegates_failure() {
        std::panic::catch_unwind(|| bump_chunk_region_or_else(ptr::without_provenance_mut(1), || panic!("injected missing bump region")))
            .unwrap_err();
    }

    #[test]
    fn bitmap_and_extent_helpers_cover_wrapping_and_coalescing() {
        crate::initialize();
        let mut used = [0; MEDIUM_REGION_BITMAP_WORDS];
        mark_slices(&mut used, 0, 2, true);
        mark_slices(&mut used, 4, 2, true);
        assert_eq!(find_free_slices(&used, 4, 2), Some(6));
        assert_eq!(
            find_free_slices_in(&used, MEDIUM_REGION_SLICE_COUNT - 1, MEDIUM_REGION_SLICE_COUNT, 2),
            None
        );
        mark_slices(&mut used, 0, 2, false);
        used.fill(u64::MAX);
        mark_slices(&mut used, 2, 2, false);
        assert_eq!(find_free_slices(&used, 4, 2), Some(2));

        let domain = new_domain();
        let domain_state = crate::domain::state(domain);
        let regions = unsafe { domain_regions(domain_state) };
        let base = regions.allocate_slices(domain_state, 8).expect("test region allocation");
        assert!(unsafe { hal::commit(base, 8 * MEDIUM_SLICE_SIZE) });
        let region = region_containing(base).unwrap();
        let first = base;
        let middle = unsafe { base.add(2 * MEDIUM_SLICE_SIZE) };
        let last = unsafe { base.add(5 * MEDIUM_SLICE_SIZE) };
        unsafe {
            insert_large_extent(region, middle, 3);
            insert_large_extent(region, first, 2);
            insert_large_extent(region, last, 3);
        }
        let whole = unsafe { take_large_extent(region, 2) }.unwrap();
        assert_eq!(whole, base);
        let tail = unsafe { take_large_extent(region, 6) }.unwrap();
        assert_eq!(tail, unsafe { base.add(2 * MEDIUM_SLICE_SIZE) });
        assert!(unsafe { take_large_extent(region, 1) }.is_none());
        assert!(unsafe { hal::decommit(base, 8 * MEDIUM_SLICE_SIZE) });
        unsafe { regions.release_slices(base, 8) };
    }

    #[test]
    fn region_manager_expands_after_a_region_is_full() {
        crate::initialize();
        let mut domain = DomainState::new();
        let domain_pointer = ptr::from_mut(&mut domain);
        let regions = &domain.regions;
        let first = regions.allocate_slices(domain_pointer, MEDIUM_REGION_SLICE_COUNT).unwrap();
        let second = regions.allocate_slices(domain_pointer, 1).unwrap();

        assert!(second.addr() < first.addr() || second.addr() >= first.addr() + MEDIUM_REGION_SIZE);

        let mut state = regions.state.lock();
        assert_eq!(unsafe { find_region(&state, second) }.unwrap(), state.last_region);
        let mut region = state.regions;
        clear_region_cache();
        while !region.is_null() {
            let next = unsafe { (*region).next.load(Ordering::Relaxed) };
            unsafe {
                hal::unmap((*region).base, MEDIUM_REGION_SIZE);
                hal::unmap(region.cast(), size_of::<RegionState>());
            }
            region = next;
        }
        state.regions = ptr::null_mut();
        state.last_region = ptr::null_mut();
        regions.regions.store(ptr::null_mut(), Ordering::Relaxed);
    }

    #[test]
    fn bump_chunks_are_slices_from_shared_regions() {
        crate::initialize();
        let domain = new_domain();
        let domain_state = crate::domain::state(domain);
        let chunk = allocate_bump_chunk(domain_state);
        assert!(!chunk.is_null());
        let region = region_containing(chunk).unwrap();
        assert_eq!(unsafe { (*region).domain }, domain_state);
        unsafe { release_bump_chunk(chunk) };
    }

    #[cfg(not(miri))]
    #[test]
    fn thread_exit_releases_empty_default_heap_slices() {
        crate::initialize();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let allocator = unsafe { Rallocator::<Standard>::new() };
            let layout = Layout::new::<[u8; 64]>();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { allocator.dealloc(address, layout) };

            let state = unsafe { &*thread_state() };
            let heap = unsafe { &*state.default_heap };
            sender
                .send((SendAddress(heap.locality_segment.cast()), heap.locality_segment_slices))
                .unwrap();
        })
        .join()
        .unwrap();

        let (SendAddress(segment), slice_count) = receiver.recv().unwrap();
        assert!(!segment.is_null());
        assert!(slices_are_free(segment, slice_count));
    }

    #[test]
    fn later_tls_destructor_can_use_allocator_after_thread_state_cleanup() {
        crate::initialize();
        std::thread::spawn(|| {
            LATE_TLS_ALLOCATOR_USER.with(|_| {});

            let allocator = unsafe { Rallocator::<Standard>::new() };
            let layout = Layout::new::<[u8; 64]>();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { allocator.dealloc(address, layout) };
        })
        .join()
        .unwrap();
    }

    #[test]
    fn escaped_thread_allocation_releases_its_slice_after_owner_exit() {
        crate::initialize();
        let (sender, receiver) = mpsc::channel();
        let address = std::thread::spawn(move || {
            let allocator = unsafe { Rallocator::<Standard>::new() };
            let layout = Layout::new::<[u8; 64]>();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            unsafe { address.write(7) };

            let state = unsafe { &*thread_state() };
            let heap = unsafe { &*state.default_heap };
            sender.send(SendAddress(heap.locality_segment.cast())).unwrap();
            SendAddress(address)
        })
        .join()
        .unwrap();

        let SendAddress(segment) = receiver.recv().unwrap();
        let SendAddress(address) = address;
        assert!(!segment.is_null());
        assert!(!slices_are_free(segment, 1));
        assert_eq!(unsafe { address.read() }, 7);

        let allocator = unsafe { Rallocator::<Standard>::new() };
        unsafe { allocator.dealloc(address, Layout::new::<[u8; 64]>()) };
        assert!(slices_are_free(segment, 1));
    }

    fn slices_are_free(address: *mut u8, count: usize) -> bool {
        let Some(containing) = region_containing(address) else {
            return false;
        };
        let regions = unsafe { domain_regions((*containing).domain) };
        let _state = regions.state.lock();
        let region = containing;
        let first = (address.addr() - unsafe { (*region).base.addr() }) / MEDIUM_SLICE_SIZE;
        (first..first + count).all(|slice| unsafe { (*region).used[slice / 64] & (1_u64 << (slice % 64)) == 0 })
    }
}
