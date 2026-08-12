// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump-backed allocation heaps.

use std::alloc::Layout;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, fence};

use allocation_hints::heap::bump::Options;

use crate::allocator::{DomainState, ReusableHeapState};
use crate::hal;
use crate::telemetry::{self, TrackingAllocation};

pub(crate) const BUMP_CHUNK_SIZE: usize = 64 * 1024;
pub(crate) const BUMP_SEGMENT_SIZE: usize = BUMP_CHUNK_SIZE / 2;
const SEGMENT_REFERENCE_CREDITS: usize = BUMP_SEGMENT_SIZE;
const RETENTION_DECAY_INTERVAL: usize = 8;
pub(crate) const BUMP_CHUNK_MARKER: usize = 0x5241_4C4C_4152_454E;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SegmentCredits(usize);

impl SegmentCredits {
    const NONE: Self = Self(0);
    const FULL: Self = Self(SEGMENT_REFERENCE_CREDITS);

    const fn get(self) -> usize {
        self.0
    }

    fn consume(&mut self) {
        assert!(self.0 != 0, "a bump segment must reserve one reference credit per allocation");
        self.0 -= 1;
    }

    fn reclaim(&mut self) {
        assert!(self.0 < SEGMENT_REFERENCE_CREDITS, "bump segment reference credits overflowed");
        self.0 += 1;
    }
}

#[repr(C)]
pub(crate) struct BumpChunk {
    marker: usize,
    state: *mut BumpState,
    next: *mut Self,
}

#[repr(C)]
struct BumpSegment {
    marker: usize,
    state: *mut BumpState,
}

#[repr(C, align(64))]
pub(crate) struct BumpState {
    references: AtomicUsize,
    reference_padding: [usize; 7],
    root: *mut BumpChunk,
    current: *mut BumpChunk,
    available: *mut BumpChunk,
    tail: *mut BumpChunk,
    cursor: *mut u8,
    end: *mut u8,
    allocation_count: usize,
    used_bytes: usize,
    live_requested_bytes: AtomicUsize,
    handle_released: AtomicBool,
    usage_sequence: AtomicUsize,
    chunk_count: usize,
    used_chunk_count: usize,
    retention_target: usize,
    underutilized_resets: usize,
    remaining_reference_credits: SegmentCredits,
    options: Options,
    pub(crate) domain: *mut DomainState,
    pub(crate) fallback_heap: *mut ReusableHeapState,
    pub(crate) pool_next: *mut Self,
}

#[repr(C)]
struct TrackingHeader {
    allocation: TrackingAllocation,
    previous_cursor: *mut u8,
}

#[repr(C)]
struct BumpRoot {
    chunk: BumpChunk,
    state: BumpState,
}

struct GlobalBumpPool {
    locked: AtomicBool,
    head: std::cell::UnsafeCell<*mut BumpState>,
}

// SAFETY: The global pool head is read or written only while `locked` is held.
unsafe impl Sync for GlobalBumpPool {}
// SAFETY: A BumpState may move between threads only while inactive. Its
// non-atomic allocation fields are serialized by the heap's exclusive claim;
// cross-thread deallocation uses the atomic reference count and usage seqcount.
unsafe impl Send for BumpState {}

static GLOBAL_POOL: GlobalBumpPool = GlobalBumpPool {
    locked: AtomicBool::new(false),
    head: std::cell::UnsafeCell::new(ptr::null_mut()),
};

#[cfg(test)]
static FAIL_NEXT_CHUNK_ALLOCATION: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);
#[cfg(test)]
static FAIL_NEXT_FALLBACK_ALLOCATION: std::sync::Mutex<Option<std::thread::ThreadId>> = std::sync::Mutex::new(None);

const _: () = assert!(std::mem::offset_of!(BumpState, root) == 64);

pub(crate) fn usage(state: &BumpState) -> (usize, usize, usize, usize, usize, usize) {
    usage_with_retry_hook(state, |_| {})
}

// The exclusive heap claim serializes allocation and inspection. The seqcount
// only stabilizes the non-atomic accounting fields against cross-thread frees.
fn usage_with_retry_hook(state: &BumpState, mut retry_hook: impl FnMut(&BumpState)) -> (usize, usize, usize, usize, usize, usize) {
    loop {
        let sequence = state.usage_sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            std::hint::spin_loop();
            retry_hook(state);
            continue;
        }
        let live_allocations = state.references.load(Ordering::Acquire) - 1 - state.remaining_reference_credits.get();
        let live_requested_bytes = state.live_requested_bytes.load(Ordering::Acquire);
        retry_hook(state);
        if state.usage_sequence.load(Ordering::Acquire) == sequence {
            return (
                state.chunk_count * BUMP_CHUNK_SIZE,
                state.used_bytes,
                state.allocation_count,
                live_allocations,
                live_requested_bytes,
                state.chunk_count,
            );
        }
    }
}

pub(crate) fn options(state: &BumpState) -> Options {
    state.options
}

#[inline(always)]
pub(crate) unsafe fn allocate(state: *mut BumpState, layout: Layout) -> *mut u8 {
    let size = layout.size().max(1);
    let options = unsafe { (*state).options };
    if size > options.max_allocation_bytes() || layout.align() > options.max_alignment() || !can_fit_in_chunk_segment(layout) {
        return ptr::null_mut();
    }

    loop {
        let aligned = match align_up(unsafe { (*state).cursor.addr() }, layout.align()) {
            Some(address) => address,
            None => return ptr::null_mut(),
        };
        let end = unsafe { (*state).end };
        if aligned <= end.addr() && end.addr() - aligned >= size {
            let address = unsafe { (*state).cursor.with_addr(aligned) };
            unsafe {
                (*state).cursor = address.add(size);
                (*state).allocation_count += 1;
                (*state).used_bytes += size;
                (*state).live_requested_bytes.fetch_add(layout.size(), Ordering::Relaxed);
                (*state).remaining_reference_credits.consume();
            }
            return address;
        }

        if !unsafe { advance_chunk(state) } {
            return ptr::null_mut();
        }
    }
}

#[inline(always)]
pub(crate) unsafe fn allocate_tracked(state: *mut BumpState, layout: Layout) -> *mut u8 {
    let size = layout.size().max(1);
    let options = unsafe { (*state).options };
    if size > options.max_allocation_bytes() || layout.align() > options.max_alignment() || !can_fit_tracked_in_chunk_segment(layout) {
        return ptr::null_mut();
    }

    loop {
        let cursor = unsafe { (*state).cursor };
        let alignment = layout.align().max(align_of::<TrackingHeader>());
        let user = match align_up(cursor.addr().saturating_add(size_of::<TrackingHeader>()), alignment) {
            Some(address) => address,
            None => return ptr::null_mut(),
        };
        let end = unsafe { (*state).end };
        if user <= end.addr() && end.addr() - user >= size {
            let address = cursor.with_addr(user);
            let header = unsafe { hal::allocation_prefix_for_write::<TrackingHeader>(address, size_of::<TrackingHeader>()) };
            unsafe {
                header.write(TrackingHeader {
                    allocation: TrackingAllocation::NONE,
                    previous_cursor: cursor,
                });
                (*state).cursor = address.add(size);
                (*state).allocation_count += 1;
                (*state).used_bytes += size;
                (*state).live_requested_bytes.fetch_add(layout.size(), Ordering::Relaxed);
                (*state).remaining_reference_credits.consume();
            }
            return address;
        }

        if !unsafe { advance_chunk(state) } {
            return ptr::null_mut();
        }
    }
}

pub(crate) unsafe fn set_tracking(address: *mut u8, allocation: TrackingAllocation) {
    let header = unsafe { address.sub(size_of::<TrackingHeader>()).cast::<TrackingHeader>() };
    unsafe { (*header).allocation = allocation };
}

/// Returns the bump state recorded in an allocator-owned segment prefix.
///
/// # Safety
///
/// `segment` must point to readable allocator metadata preceding the
/// allocation. The allocation must not begin at `segment`, because such an
/// allocation owns the marker and state bytes itself.
#[inline(always)]
pub(crate) unsafe fn state_for_allocation(segment: *mut u8, marker: usize) -> Option<*mut BumpState> {
    if marker != BUMP_CHUNK_MARKER {
        return None;
    }
    let state = unsafe { (*segment.cast::<BumpSegment>()).state };
    debug_assert!(!state.is_null());
    (!state.is_null()).then_some(state)
}

#[inline(always)]
pub(crate) unsafe fn deallocate(state: *mut BumpState, address: *mut u8, layout: Layout, reclaim_tail: bool) {
    let size = layout.size().max(1);
    unsafe {
        finish_deallocation(
            state,
            layout.size(),
            size,
            address.add(size),
            // The headerless path can reclaim the allocation itself without
            // recovering padding that preceded it.
            address,
            reclaim_tail,
        );
    };
}

unsafe fn finish_deallocation(
    state: *mut BumpState,
    requested_bytes: usize,
    used_bytes: usize,
    allocation_end: *mut u8,
    rewind_to: *mut u8,
    reclaim_tail: bool,
) {
    unsafe {
        finish_deallocation_with_retry_hook(state, requested_bytes, used_bytes, allocation_end, rewind_to, reclaim_tail, || {});
    }
}

unsafe fn finish_deallocation_with_retry_hook(
    state: *mut BumpState,
    requested_bytes: usize,
    used_bytes: usize,
    allocation_end: *mut u8,
    rewind_to: *mut u8,
    reclaim_tail: bool,
    mut retry_hook: impl FnMut(),
) {
    let sequence = loop {
        let sequence = unsafe { (*state).usage_sequence.load(Ordering::Acquire) };
        if sequence & 1 == 0
            && unsafe {
                (*state)
                    .usage_sequence
                    .compare_exchange_weak(sequence, sequence + 1, Ordering::AcqRel, Ordering::Acquire)
            }
            .is_ok()
        {
            break sequence;
        }
        std::hint::spin_loop();
        retry_hook();
    };
    unsafe { (*state).live_requested_bytes.fetch_sub(requested_bytes, Ordering::Relaxed) };
    let reclaimed = reclaim_tail && unsafe { (*state).cursor == allocation_end };
    if reclaimed {
        unsafe {
            (*state).cursor = rewind_to;
            (*state).used_bytes -= used_bytes;
            (*state).remaining_reference_credits.reclaim();
        }
    }
    let previous = if reclaimed {
        0
    } else {
        let previous = unsafe { (*state).references.fetch_sub(1, Ordering::Release) };
        debug_assert_ne!(previous, 0);
        previous
    };
    unsafe { (*state).usage_sequence.store(sequence.wrapping_add(2), Ordering::Release) };
    if !reclaimed && previous == 1 {
        fence(Ordering::Acquire);
        unsafe { adapt_and_trim_state(state) };
        crate::allocator::return_pooled_bump(state);
    }
}

#[inline(always)]
pub(crate) unsafe fn deallocate_tracked(state: *mut BumpState, address: *mut u8, layout: Layout, reclaim_tail: bool) {
    #[cfg(miri)]
    let header = unsafe { hal::allocation_prefix_for_read::<TrackingHeader>(address, size_of::<TrackingHeader>()) };
    #[cfg(not(miri))]
    let header = unsafe { address.sub(size_of::<TrackingHeader>()).cast::<TrackingHeader>() };
    let allocation = unsafe { (*header).allocation };
    let previous_cursor = unsafe { (*header).previous_cursor };
    let released = unsafe { (*state).handle_released.load(Ordering::Acquire) };
    telemetry::record_deallocation(allocation, address, layout, released);
    let size = layout.size().max(1);
    unsafe { finish_deallocation(state, layout.size(), size, address.add(size), previous_cursor, reclaim_tail) };
}

pub(crate) fn take_global(domain: *mut DomainState) -> Option<*mut BumpState> {
    lock_global_pool();
    let current = unsafe { take_matching_state(&mut *GLOBAL_POOL.head.get(), domain) };
    GLOBAL_POOL.locked.store(false, Ordering::Release);
    (!current.is_null()).then_some(current)
}

unsafe fn take_matching_state(head: &mut *mut BumpState, domain: *mut DomainState) -> *mut BumpState {
    let mut previous = ptr::null_mut::<BumpState>();
    let mut current = *head;
    while !current.is_null() && unsafe { (*current).domain } != domain {
        previous = current;
        current = unsafe { (*current).pool_next };
    }
    if !current.is_null() {
        let next = unsafe { (*current).pool_next };
        if previous.is_null() {
            *head = next;
        } else {
            unsafe { (*previous).pool_next = next };
        }
    }
    current
}

pub(crate) unsafe fn return_global(state: *mut BumpState) {
    lock_global_pool();
    unsafe {
        (*state).pool_next = *GLOBAL_POOL.head.get();
        *GLOBAL_POOL.head.get() = state;
    }
    GLOBAL_POOL.locked.store(false, Ordering::Release);
}

pub(crate) fn create_state(options: Options, domain: *mut DomainState) -> Option<*mut BumpState> {
    let mapping = allocate_bump_chunk(domain);
    if mapping.is_null() {
        return None;
    }
    debug_assert!(mapping.addr().is_multiple_of(BUMP_CHUNK_SIZE));

    let fallback_heap = create_bump_fallback_heap(domain);
    if fallback_heap.is_null() {
        unsafe { crate::allocator::release_bump_chunk(mapping) };
        return None;
    }
    let root = mapping.cast::<BumpRoot>();
    let state = unsafe { ptr::addr_of_mut!((*root).state) };
    let chunk = mapping.cast::<BumpChunk>();
    unsafe {
        root.write(BumpRoot {
            chunk: BumpChunk {
                marker: BUMP_CHUNK_MARKER,
                state,
                next: ptr::null_mut(),
            },
            state: BumpState {
                references: AtomicUsize::new(0),
                reference_padding: [0; 7],
                root: chunk,
                current: chunk,
                available: ptr::null_mut(),
                tail: chunk,
                cursor: ptr::null_mut(),
                end: mapping.add(BUMP_CHUNK_SIZE),
                allocation_count: 0,
                used_bytes: 0,
                live_requested_bytes: AtomicUsize::new(0),
                handle_released: AtomicBool::new(false),
                usage_sequence: AtomicUsize::new(0),
                chunk_count: 1,
                used_chunk_count: 1,
                retention_target: options.retained_chunks(),
                underutilized_resets: 0,
                remaining_reference_credits: SegmentCredits::NONE,
                options,
                domain,
                fallback_heap,
                pool_next: ptr::null_mut(),
            },
        });
        write_second_segment(chunk, state);
    }
    crate::allocator::register_bump_chunk(mapping, state);
    Some(state)
}

pub(crate) unsafe fn reset_state(state: *mut BumpState, options: Options) {
    debug_assert_eq!(unsafe { (*state).references.load(Ordering::Relaxed) }, 0);
    debug_assert_eq!(unsafe { (*state).remaining_reference_credits }, SegmentCredits::NONE);
    debug_assert!(!unsafe { (*state).fallback_heap.is_null() });
    let retention_target = unsafe { (*state).retention_target }.clamp(options.retained_chunks(), options.max_retained_chunks());
    unsafe { trim_state(state, retention_target) };
    let state_ref = unsafe { &mut *state };
    state_ref.options = options;
    state_ref.retention_target = retention_target;
    state_ref.references.store(1, Ordering::Relaxed);
    state_ref.current = state_ref.root;
    state_ref.available = unsafe { (*state_ref.root).next };
    state_ref.cursor = root_start(state_ref.root);
    state_ref.end = unsafe { state_ref.root.cast::<u8>().add(BUMP_SEGMENT_SIZE) };
    state_ref.allocation_count = 0;
    state_ref.used_bytes = 0;
    state_ref.live_requested_bytes.store(0, Ordering::Relaxed);
    state_ref.handle_released.store(false, Ordering::Relaxed);
    state_ref.usage_sequence.store(0, Ordering::Relaxed);
    state_ref.used_chunk_count = 1;
    state_ref.pool_next = ptr::null_mut();
    unsafe { reserve_segment_references(state) };
}

pub(crate) fn ensure_fallback_heap(state: *mut BumpState) -> bool {
    if !unsafe { (*state).fallback_heap.is_null() } {
        return true;
    }
    let fallback_heap = create_bump_fallback_heap(unsafe { (*state).domain });
    if fallback_heap.is_null() {
        return false;
    }
    unsafe { (*state).fallback_heap = fallback_heap };
    true
}

pub(crate) unsafe fn take_fallback_heap(state: *mut BumpState) -> *mut ReusableHeapState {
    unsafe { std::mem::replace(&mut (*state).fallback_heap, ptr::null_mut()) }
}

unsafe fn advance_chunk(state: *mut BumpState) -> bool {
    let second_segment = unsafe { (*state).current.cast::<u8>().add(BUMP_SEGMENT_SIZE) };
    if unsafe { (*state).end } == second_segment {
        unsafe {
            release_unused_references(state);
            (*state).cursor = second_segment_start((*state).current);
            (*state).end = (*state).current.cast::<u8>().add(BUMP_CHUNK_SIZE);
            reserve_segment_references(state);
        }
        return true;
    }

    let chunk = if unsafe { (*state).available.is_null() } {
        let mapping = allocate_bump_chunk(unsafe { (*state).domain });
        if mapping.is_null() {
            return false;
        }
        debug_assert!(mapping.addr().is_multiple_of(BUMP_CHUNK_SIZE));
        let chunk = mapping.cast::<BumpChunk>();
        unsafe {
            chunk.write(BumpChunk {
                marker: BUMP_CHUNK_MARKER,
                state,
                next: ptr::null_mut(),
            });
            write_second_segment(chunk, state);
            (*(*state).tail).next = chunk;
        }
        crate::allocator::register_bump_chunk(mapping, state);
        unsafe {
            (*state).tail = chunk;
            (*state).chunk_count += 1;
        }
        chunk
    } else {
        let chunk = unsafe { (*state).available };
        unsafe { (*state).available = (*chunk).next };
        chunk
    };

    unsafe {
        release_unused_references(state);
        (*state).current = chunk;
        (*state).cursor = chunk_start(chunk);
        (*state).end = chunk.cast::<u8>().add(BUMP_SEGMENT_SIZE);
        (*state).used_chunk_count += 1;
        reserve_segment_references(state);
    }
    true
}

pub(crate) unsafe fn release_handle(state: *mut BumpState) {
    unsafe { (*state).handle_released.store(true, Ordering::Release) };
    unsafe { release_unused_references(state) };
    unsafe { release_reference(state) };
}

fn can_fit_tracked_in_chunk_segment(layout: Layout) -> bool {
    layout
        .size()
        .max(1)
        .checked_add(size_of::<TrackingHeader>())
        .and_then(|size| size.checked_add(layout.align().max(align_of::<TrackingHeader>()) - 1))
        .is_some_and(|size| size <= BUMP_SEGMENT_SIZE)
}

unsafe fn release_reference(state: *mut BumpState) {
    let previous = unsafe { (*state).references.fetch_sub(1, Ordering::Release) };
    debug_assert_ne!(previous, 0);
    if previous == 1 {
        fence(Ordering::Acquire);
        unsafe { adapt_and_trim_state(state) };
        crate::allocator::return_pooled_bump(state);
    }
}

unsafe fn reserve_segment_references(state: *mut BumpState) {
    debug_assert_eq!(unsafe { (*state).remaining_reference_credits }, SegmentCredits::NONE);
    unsafe { &(*state).references }.fetch_add(SEGMENT_REFERENCE_CREDITS, Ordering::Relaxed);
    unsafe { (*state).remaining_reference_credits = SegmentCredits::FULL };
}

unsafe fn release_unused_references(state: *mut BumpState) {
    let remaining = unsafe { (*state).remaining_reference_credits.get() };
    if remaining == 0 {
        return;
    }
    let previous = unsafe { &(*state).references }.fetch_sub(remaining, Ordering::Release);
    debug_assert!(previous > remaining);
    unsafe { (*state).remaining_reference_credits = SegmentCredits::NONE };
}

unsafe fn trim_state(state: *mut BumpState, retained_chunks: usize) {
    let state_ref = unsafe { &mut *state };
    if state_ref.chunk_count <= retained_chunks {
        return;
    }

    let mut retained_tail = state_ref.root;
    for _ in 1..retained_chunks {
        retained_tail = unsafe { (*retained_tail).next };
    }
    let mut chunk = unsafe { (*retained_tail).next };
    unsafe { (*retained_tail).next = ptr::null_mut() };
    while !chunk.is_null() {
        let next = unsafe { (*chunk).next };
        unsafe { crate::allocator::release_bump_chunk(chunk.cast()) };
        chunk = next;
    }
    state_ref.tail = retained_tail;
    state_ref.chunk_count = retained_chunks;
}

unsafe fn adapt_and_trim_state(state: *mut BumpState) {
    let retention_target = {
        let state_ref = unsafe { &mut *state };
        let minimum = state_ref.options.retained_chunks();
        let maximum = state_ref.options.max_retained_chunks();
        let used = state_ref.used_chunk_count;

        if used > state_ref.retention_target {
            state_ref.retention_target = used.min(maximum);
            state_ref.underutilized_resets = 0;
        } else if used.saturating_mul(2) <= state_ref.retention_target && state_ref.retention_target > minimum {
            state_ref.underutilized_resets += 1;
            if state_ref.underutilized_resets >= RETENTION_DECAY_INTERVAL {
                let reduction = (state_ref.retention_target / 4).max(1);
                state_ref.retention_target = state_ref.retention_target.saturating_sub(reduction).max(used).max(minimum);
                state_ref.underutilized_resets = 0;
            }
        } else {
            state_ref.underutilized_resets = 0;
        }

        state_ref.retention_target
    };
    unsafe { trim_state(state, retention_target) };
}

fn root_start(chunk: *mut BumpChunk) -> *mut u8 {
    let start = chunk.addr() + size_of::<BumpRoot>();
    chunk.cast::<u8>().with_addr(align_up(start, 16).unwrap())
}

fn chunk_start(chunk: *mut BumpChunk) -> *mut u8 {
    let start = chunk.addr() + size_of::<BumpChunk>();
    chunk.cast::<u8>().with_addr(align_up(start, 16).unwrap())
}

fn second_segment_start(chunk: *mut BumpChunk) -> *mut u8 {
    let start = chunk.addr() + BUMP_SEGMENT_SIZE + size_of::<BumpSegment>();
    chunk.cast::<u8>().with_addr(align_up(start, 16).unwrap())
}

/// Writes the allocator-owned prefix for a chunk's second segment.
///
/// # Safety
///
/// `chunk` must identify a live, writable [`BUMP_CHUNK_SIZE`]-byte bump chunk,
/// and `state` must remain valid while allocations from that chunk are live.
unsafe fn write_second_segment(chunk: *mut BumpChunk, state: *mut BumpState) {
    let segment = unsafe { chunk.cast::<u8>().add(BUMP_SEGMENT_SIZE).cast::<BumpSegment>() };
    unsafe {
        segment.write(BumpSegment {
            marker: BUMP_CHUNK_MARKER,
            state,
        });
    }
}

fn align_up(address: usize, alignment: usize) -> Option<usize> {
    address.checked_add(alignment - 1).map(|address| address & !(alignment - 1))
}

fn can_fit_in_chunk_segment(layout: Layout) -> bool {
    let start = align_up(size_of::<BumpChunk>(), layout.align()).unwrap();
    start <= BUMP_SEGMENT_SIZE && BUMP_SEGMENT_SIZE - start >= layout.size().max(1)
}

fn allocate_bump_chunk(domain: *mut DomainState) -> *mut u8 {
    if fail_next_chunk_allocation() {
        return ptr::null_mut();
    }
    crate::allocator::allocate_bump_chunk(domain)
}

fn create_bump_fallback_heap(domain: *mut DomainState) -> *mut ReusableHeapState {
    if fail_next_fallback_allocation() {
        return ptr::null_mut();
    }
    crate::allocator::create_bump_fallback_heap(domain)
}

#[cfg(not(test))]
fn fail_next_chunk_allocation() -> bool {
    false
}

#[cfg(test)]
fn fail_next_chunk_allocation() -> bool {
    take_failure_for_current_thread(&FAIL_NEXT_CHUNK_ALLOCATION)
}

#[cfg(not(test))]
fn fail_next_fallback_allocation() -> bool {
    false
}

#[cfg(test)]
fn fail_next_fallback_allocation() -> bool {
    take_failure_for_current_thread(&FAIL_NEXT_FALLBACK_ALLOCATION)
}

#[cfg(test)]
fn take_failure_for_current_thread(failure: &std::sync::Mutex<Option<std::thread::ThreadId>>) -> bool {
    let mut failure = failure.lock().unwrap();
    if failure.as_ref() == Some(&std::thread::current().id()) {
        failure.take();
        true
    } else {
        false
    }
}

#[cfg(test)]
fn inject_failure(failure: &std::sync::Mutex<Option<std::thread::ThreadId>>) {
    *failure.lock().unwrap() = Some(std::thread::current().id());
}

fn lock_global_pool() {
    lock_global_pool_with_retry_hook(|| {});
}

fn lock_global_pool_with_retry_hook(mut retry_hook: impl FnMut()) {
    while GLOBAL_POOL
        .locked
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
        retry_hook();
    }
}

#[cfg(test)]
mod tests {
    use allocation_hints::domain::Domain;

    use super::*;

    fn default_domain_state() -> *mut crate::allocator::DomainState {
        crate::initialize();
        crate::domain::state(Domain::default())
    }

    static USAGE_RETRY_HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn state(options: Options) -> *mut BumpState {
        let state = create_state(options, default_domain_state()).unwrap();
        unsafe { reset_state(state, options) };
        state
    }

    #[test]
    fn usage_retries_while_a_writer_is_active_or_changes_sequence() {
        crate::initialize();
        let state = state(Options::new());
        let state_ref = unsafe { &*state };
        assert_eq!(options(state_ref), Options::new());
        state_ref.usage_sequence.store(1, Ordering::Relaxed);
        USAGE_RETRY_HOOK_CALLS.store(0, Ordering::Relaxed);
        let snapshot = usage_with_retry_hook(state_ref, |state| {
            let call = USAGE_RETRY_HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
            assert!(call < 4, "usage retry loop did not converge");
            state.usage_sequence.store(if call == 0 { 2 } else { 4 }, Ordering::Release);
        });
        assert_eq!(snapshot.0, BUMP_CHUNK_SIZE);
        assert!(USAGE_RETRY_HOOK_CALLS.load(Ordering::Relaxed) >= 3);
        assert_eq!(usage(state_ref).0, BUMP_CHUNK_SIZE);
        unsafe { release_handle(state) };
    }

    #[test]
    fn allocation_rejects_address_overflow_and_exhausted_backing() {
        crate::initialize();
        let state = state(Options::new());
        let original_cursor = unsafe { (*state).cursor };
        unsafe { (*state).cursor = ptr::without_provenance_mut(usize::MAX) };
        let aligned = Layout::from_size_align(1, 2).unwrap();
        assert!(unsafe { allocate(state, aligned) }.is_null());
        assert!(unsafe { allocate_tracked(state, aligned) }.is_null());
        unsafe { (*state).cursor = original_cursor };

        let layout = Layout::from_size_align(BUMP_SEGMENT_SIZE / 2, 16).unwrap();
        let first = unsafe { allocate(state, layout) };
        let second = unsafe { allocate(state, layout) };
        assert!(!first.is_null());
        assert!(!second.is_null());
        inject_failure(&FAIL_NEXT_CHUNK_ALLOCATION);
        assert!(unsafe { allocate(state, layout) }.is_null());

        unsafe { deallocate(state, first, layout, false) };
        unsafe { deallocate(state, second, layout, false) };

        let tracked_too_large = Layout::from_size_align(BUMP_SEGMENT_SIZE + 1, 16).unwrap();
        assert!(unsafe { allocate_tracked(state, tracked_too_large) }.is_null());
        let tracked_layout = Layout::from_size_align(BUMP_SEGMENT_SIZE / 2, 16).unwrap();
        let first = unsafe { allocate_tracked(state, tracked_layout) };
        let second = unsafe { allocate_tracked(state, tracked_layout) };
        assert!(!first.is_null());
        assert!(!second.is_null());
        inject_failure(&FAIL_NEXT_CHUNK_ALLOCATION);
        assert!(unsafe { allocate_tracked(state, tracked_layout) }.is_null());
        unsafe {
            deallocate_tracked(state, first, tracked_layout, false);
            deallocate_tracked(state, second, tracked_layout, false);
        }
        unsafe { release_handle(state) };
    }

    #[test]
    fn deallocation_waits_for_an_in_progress_usage_update() {
        crate::initialize();
        let state = state(Options::new());
        let layout = Layout::new::<u64>();
        let address = unsafe { allocate(state, layout) };
        assert!(!address.is_null());
        unsafe { (*state).usage_sequence.store(1, Ordering::Release) };
        let mut retries = 0;
        unsafe {
            finish_deallocation_with_retry_hook(
                state,
                layout.size(),
                layout.size(),
                address.add(layout.size()),
                address,
                false,
                || {
                    retries += 1;
                    assert!(retries <= 64, "deallocation retry loop did not converge");
                    (*state).usage_sequence.store(2, Ordering::Release);
                },
            );
        }
        assert!(retries >= 1);
        unsafe { release_handle(state) };
    }

    #[test]
    fn contiguous_latest_allocations_rewind_in_reverse_order() {
        crate::initialize();
        let state = state(Options::new());
        let layout = Layout::new::<u64>();
        let first = unsafe { allocate(state, layout) };
        let second = unsafe { allocate(state, layout) };
        let third = unsafe { allocate(state, layout) };

        unsafe {
            deallocate(state, third, layout, true);
            deallocate(state, second, layout, true);
        }
        assert_eq!(unsafe { (*state).cursor }, second);
        assert_eq!(unsafe { (*state).used_bytes }, layout.size());

        assert_eq!(unsafe { allocate(state, layout) }, second);
        assert_eq!(unsafe { allocate(state, layout) }, third);

        unsafe {
            deallocate(state, third, layout, true);
            deallocate(state, second, layout, true);
            deallocate(state, first, layout, true);
            release_handle(state);
        }
    }

    #[test]
    fn tracked_latest_allocations_restore_alignment_padding() {
        crate::initialize();
        let state = state(Options::new());
        let first_layout = Layout::from_size_align(3, 1).unwrap();
        let second_layout = Layout::from_size_align(5, 8).unwrap();
        let third_layout = Layout::from_size_align(7, 32).unwrap();
        let first = unsafe { allocate_tracked(state, first_layout) };
        let second = unsafe { allocate_tracked(state, second_layout) };
        let third = unsafe { allocate_tracked(state, third_layout) };

        unsafe {
            deallocate_tracked(state, third, third_layout, true);
            deallocate_tracked(state, second, second_layout, true);
        }
        assert_eq!(unsafe { allocate_tracked(state, second_layout) }, second);
        assert_eq!(unsafe { allocate_tracked(state, third_layout) }, third);

        unsafe {
            deallocate_tracked(state, third, third_layout, true);
            deallocate_tracked(state, second, second_layout, true);
            deallocate_tracked(state, first, first_layout, true);
            release_handle(state);
        }
    }

    #[test]
    fn non_latest_allocation_does_not_rewind_the_cursor() {
        crate::initialize();
        let state = state(Options::new());
        let layout = Layout::new::<u64>();
        let first = unsafe { allocate(state, layout) };
        let second = unsafe { allocate(state, layout) };
        let cursor = unsafe { (*state).cursor };

        unsafe { deallocate(state, first, layout, true) };
        assert_eq!(unsafe { (*state).cursor }, cursor);

        unsafe {
            deallocate(state, second, layout, true);
            release_handle(state);
        }
    }

    #[test]
    fn headerless_alignment_padding_stops_the_rewind_chain_safely() {
        crate::initialize();
        let state = state(Options::new());
        let first_layout = Layout::from_size_align(1, 1).unwrap();
        let second_layout = Layout::from_size_align(1, 64).unwrap();
        let first = unsafe { allocate(state, first_layout) };
        let second = unsafe { allocate(state, second_layout) };
        assert_ne!(unsafe { first.add(first_layout.size()) }, second);

        unsafe { deallocate(state, second, second_layout, true) };
        assert_eq!(unsafe { (*state).cursor }, second);
        unsafe { deallocate(state, first, first_layout, true) };
        assert_eq!(unsafe { (*state).cursor }, second);

        unsafe { release_handle(state) };
    }

    #[test]
    fn final_allocation_release_returns_state_without_a_handle() {
        crate::initialize();
        let state = state(Options::new());
        let layout = Layout::new::<u64>();
        let address = unsafe { allocate(state, layout) };
        assert!(!address.is_null());
        unsafe {
            release_handle(state);
            deallocate(state, address, layout, false);
        }
        assert_eq!(crate::allocator::take_pooled_bump(default_domain_state()), Some(state));
    }

    #[test]
    fn matching_state_removal_handles_a_non_head_domain() {
        crate::initialize();
        let first = state(Options::new());
        let second = state(Options::new());
        let first_domain = ptr::without_provenance_mut::<DomainState>(std::mem::align_of::<DomainState>());
        let second_domain = ptr::without_provenance_mut::<DomainState>(std::mem::align_of::<DomainState>() * 2);
        unsafe {
            (*first).domain = first_domain;
            (*second).domain = second_domain;
            (*first).pool_next = second;
            (*second).pool_next = ptr::null_mut();
        }
        let mut head = first;
        assert_eq!(unsafe { take_matching_state(&mut head, second_domain) }, second);
        assert_eq!(head, first);
        assert!(unsafe { (*first).pool_next }.is_null());
    }

    #[test]
    fn state_creation_and_fallback_restoration_report_allocation_failures() {
        crate::initialize();
        let domain = default_domain_state();
        inject_failure(&FAIL_NEXT_CHUNK_ALLOCATION);
        assert!(create_state(Options::new(), domain).is_none());

        inject_failure(&FAIL_NEXT_FALLBACK_ALLOCATION);
        assert!(create_state(Options::new(), domain).is_none());

        let fallback_state = state(Options::new());
        let fallback = unsafe { take_fallback_heap(fallback_state) };
        inject_failure(&FAIL_NEXT_FALLBACK_ALLOCATION);
        assert!(!ensure_fallback_heap(fallback_state));
        unsafe { (*fallback_state).fallback_heap = fallback };
        unsafe { release_handle(fallback_state) };

        let credit_state = state(Options::new());
        let layout = Layout::from_size_align(BUMP_SEGMENT_SIZE / 2, 16).unwrap();
        let first = unsafe { allocate(credit_state, layout) };
        let second = unsafe { allocate(credit_state, layout) };
        assert!(!first.is_null());
        assert!(!second.is_null());

        let credits = unsafe { (*credit_state).remaining_reference_credits };
        let references = unsafe { (*credit_state).references.load(Ordering::Relaxed) };
        let cursor = unsafe { (*credit_state).cursor };
        let end = unsafe { (*credit_state).end };
        inject_failure(&FAIL_NEXT_CHUNK_ALLOCATION);
        assert!(unsafe { allocate(credit_state, layout) }.is_null());
        assert_eq!(unsafe { (*credit_state).remaining_reference_credits }, credits);
        assert_eq!(unsafe { (*credit_state).references.load(Ordering::Relaxed) }, references);
        assert_eq!(unsafe { (*credit_state).cursor }, cursor);
        assert_eq!(unsafe { (*credit_state).end }, end);

        unsafe {
            deallocate(credit_state, first, layout, false);
            deallocate(credit_state, second, layout, false);
            release_handle(credit_state);
        }
    }

    #[test]
    fn state_thresholds_and_retention_cover_direct_lifecycle_paths() {
        crate::initialize();
        let options = Options::new().with_retained_chunks(1).with_max_retained_chunks(4);
        let state = state(options);
        assert!(ensure_fallback_heap(state));
        assert!(unsafe { allocate(state, Layout::from_size_align(BUMP_SEGMENT_SIZE + 1, 1).unwrap(),) }.is_null());

        unsafe {
            (*state).retention_target = 4;
            (*state).used_chunk_count = 1;
            for _ in 0..RETENTION_DECAY_INTERVAL {
                adapt_and_trim_state(state);
            }
            assert_eq!((*state).retention_target, 3);
            release_unused_references(state);
            release_reference(state);
        }
        assert_eq!(crate::allocator::take_pooled_bump(default_domain_state()), Some(state));
    }

    #[test]
    fn reset_reuses_available_chunks_and_zero_credits_need_no_release() {
        crate::initialize();
        let options = Options::new().with_retained_chunks(2);
        let state = state(options);
        let layout = Layout::from_size_align(BUMP_SEGMENT_SIZE / 2, 16).unwrap();
        let mut allocations = Vec::new();
        for _ in 0..5 {
            let address = unsafe { allocate(state, layout) };
            assert!(!address.is_null());
            allocations.push(address);
        }
        for address in allocations {
            unsafe { deallocate(state, address, layout, false) };
        }
        unsafe { release_handle(state) };

        let state = crate::allocator::take_pooled_bump(default_domain_state()).unwrap();
        assert!(ensure_fallback_heap(state));
        unsafe { reset_state(state, options) };
        let first = unsafe { allocate(state, layout) };
        let second = unsafe { allocate(state, layout) };
        let third = unsafe { allocate(state, layout) };
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert!(!third.is_null());
        unsafe {
            release_unused_references(state);
            release_unused_references(state);
            deallocate(state, first, layout, false);
            deallocate(state, second, layout, false);
            deallocate(state, third, layout, false);
            release_handle(state);
        }
    }

    #[test]
    fn global_pool_lock_spins_until_the_owner_releases_it() {
        crate::initialize();
        GLOBAL_POOL.locked.store(true, Ordering::Relaxed);
        let mut retries = 0;
        lock_global_pool_with_retry_hook(|| {
            retries += 1;
            assert!(retries <= 64, "global-pool retry loop did not converge");
            GLOBAL_POOL.locked.store(false, Ordering::Release);
        });
        assert!(retries >= 1);
        GLOBAL_POOL.locked.store(false, Ordering::Release);
    }
}
