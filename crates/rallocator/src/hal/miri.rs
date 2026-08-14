// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::spin_loop;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

const ALLOCATION_ALIGNMENT: usize = 2 * 1024 * 1024;
const FREE_METADATA_CAPACITY: usize = 1 << 12;
const PREFIX_METADATA_CAPACITY: usize = 1 << 12;
const PREFIX_EMPTY: usize = 0;
const PREFIX_TOMBSTONE: usize = 1;
pub(crate) const MEDIUM_MAX_SLICES: usize = 8;
pub(crate) const MEDIUM_REGION_SIZE: usize = 2 * 1024 * 1024;
static MONOTONIC_MILLIS: AtomicU64 = AtomicU64::new(0);
static FREE_METADATA: [FreeMetadata; FREE_METADATA_CAPACITY] = [const { FreeMetadata::new() }; FREE_METADATA_CAPACITY];
static PREFIX_METADATA_LOCK: AtomicBool = AtomicBool::new(false);
static PREFIX_METADATA: [PrefixMetadata; PREFIX_METADATA_CAPACITY] = [const { PrefixMetadata::new() }; PREFIX_METADATA_CAPACITY];

struct FreeMetadata {
    block: AtomicPtr<u8>,
    next: AtomicPtr<u8>,
    requested_bytes: AtomicUsize,
}

struct PrefixMetadata {
    payload: AtomicUsize,
    prefix: AtomicPtr<u8>,
}

impl PrefixMetadata {
    const fn new() -> Self {
        Self {
            payload: AtomicUsize::new(PREFIX_EMPTY),
            prefix: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

struct PrefixMetadataGuard;

impl Drop for PrefixMetadataGuard {
    fn drop(&mut self) {
        PREFIX_METADATA_LOCK.store(false, Ordering::Release);
    }
}

impl FreeMetadata {
    const fn new() -> Self {
        Self {
            block: AtomicPtr::new(ptr::null_mut()),
            next: AtomicPtr::new(ptr::null_mut()),
            requested_bytes: AtomicUsize::new(0),
        }
    }
}

pub(crate) fn map(size: usize) -> *mut u8 {
    unsafe { allocate(size) }
}

pub(crate) fn reserve(size: usize) -> *mut u8 {
    unsafe { allocate(size) }
}

pub(crate) unsafe fn commit(address: *mut u8, size: usize) -> bool {
    // Clear the allocation marker that distinguishes slabs from medium spans.
    if size >= std::mem::size_of::<usize>() {
        unsafe { address.cast::<usize>().write(0) };
    }
    true
}

pub(crate) unsafe fn commit_locality_segment(address: *mut u8, _segment_size: usize, slab_size: usize) -> Option<usize> {
    unsafe { commit(address, slab_size) }.then_some(slab_size)
}

pub(crate) unsafe fn commit_locality_slab(address: *mut u8, slab_size: usize) -> Option<usize> {
    unsafe { commit(address, slab_size) };
    Some(0)
}

pub(crate) unsafe fn decommit(_address: *mut u8, _size: usize) -> bool {
    true
}

pub(crate) unsafe fn unmap(address: *mut u8, size: usize) {
    if address.is_null() {
        return;
    }
    unsafe { System.dealloc(address, allocation_layout(size)) };
}

pub(crate) fn monotonic_millis() -> u64 {
    MONOTONIC_MILLIS.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn capture_stack(_frames: &mut [usize], _limit: usize) -> usize {
    0
}

#[inline(always)]
pub(crate) unsafe fn allocation_prefix_for_write<T>(address: *mut u8, offset: usize) -> *mut T {
    let prefix = unsafe { address.sub(offset).cast::<T>() };
    let _guard = lock_prefix_metadata();
    let payload = address.addr();
    let start = prefix_metadata_index(payload);
    let mut tombstone = None;
    for distance in 0..PREFIX_METADATA_CAPACITY {
        let entry = &PREFIX_METADATA[(start + distance) & (PREFIX_METADATA_CAPACITY - 1)];
        match entry.payload.load(Ordering::Relaxed) {
            PREFIX_EMPTY => {
                let entry = tombstone.unwrap_or(entry);
                entry.prefix.store(prefix.cast(), Ordering::Relaxed);
                entry.payload.store(payload, Ordering::Relaxed);
                return prefix;
            }
            PREFIX_TOMBSTONE => {
                tombstone.get_or_insert(entry);
            }
            existing => assert_ne!(existing, payload, "allocation prefix metadata must be registered once"),
        }
    }
    if let Some(entry) = tombstone {
        entry.prefix.store(prefix.cast(), Ordering::Relaxed);
        entry.payload.store(payload, Ordering::Relaxed);
        return prefix;
    }
    panic!("Miri allocation prefix metadata capacity exhausted");
}

#[inline(always)]
pub(crate) unsafe fn allocation_prefix_for_read<T>(address: *mut u8, offset: usize) -> *mut T {
    let _guard = lock_prefix_metadata();
    let payload = address.addr();
    let start = prefix_metadata_index(payload);
    for distance in 0..PREFIX_METADATA_CAPACITY {
        let entry = &PREFIX_METADATA[(start + distance) & (PREFIX_METADATA_CAPACITY - 1)];
        match entry.payload.load(Ordering::Relaxed) {
            PREFIX_EMPTY => break,
            existing if existing == payload => {
                let prefix = entry.prefix.swap(ptr::null_mut(), Ordering::Relaxed).cast::<T>();
                entry.payload.store(PREFIX_TOMBSTONE, Ordering::Relaxed);
                debug_assert_eq!(prefix.addr().checked_add(offset), Some(payload));
                return prefix;
            }
            _ => {}
        }
    }
    panic!("allocator metadata prefix must remain registered until deallocation");
}

fn lock_prefix_metadata() -> PrefixMetadataGuard {
    while PREFIX_METADATA_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }
    PrefixMetadataGuard
}

fn prefix_metadata_index(payload: usize) -> usize {
    (payload >> 4) & (PREFIX_METADATA_CAPACITY - 1)
}

#[inline]
pub(crate) fn align_down<F>(address: *mut u8, alignment: usize, provenance: F) -> *mut u8
where
    F: FnOnce(usize) -> *mut u8,
{
    let aligned = address.addr() & !(alignment - 1);
    provenance(aligned)
}

pub(crate) unsafe fn initialize_storage<T>(_embedded: *mut T, value: T) -> *mut T {
    let storage = map(std::mem::size_of::<T>()).cast::<T>();
    assert!(!storage.is_null(), "failed to allocate Miri HAL storage");
    unsafe { storage.write(value) };
    storage
}

pub(crate) unsafe fn release_storage<T>(storage: *mut T, embedded: *mut T) {
    debug_assert_ne!(storage, embedded);
    unsafe { unmap(storage.cast(), std::mem::size_of::<T>()) };
}

#[inline(always)]
pub(crate) unsafe fn write_free_next(block: *mut u8, next: *mut u8) {
    let metadata = free_metadata(block, true);
    metadata.next.store(next, Ordering::Relaxed);
}

#[inline(always)]
pub(crate) unsafe fn read_free_next(block: *mut u8) -> *mut u8 {
    free_metadata(block, false).next.load(Ordering::Relaxed)
}

#[inline(always)]
pub(crate) unsafe fn write_free_requested(block: *mut u8, requested_bytes: usize) {
    free_metadata(block, false)
        .requested_bytes
        .store(requested_bytes, Ordering::Relaxed);
}

#[inline(always)]
pub(crate) unsafe fn read_free_requested(block: *mut u8) -> usize {
    free_metadata(block, false).requested_bytes.load(Ordering::Relaxed)
}

#[inline(always)]
pub(crate) unsafe fn release_free_metadata(block: *mut u8) {
    free_metadata(block, false).block.store(ptr::null_mut(), Ordering::Release);
}

#[inline(always)]
pub(crate) unsafe fn peek_free_requested(block: *mut u8) -> usize {
    free_metadata(block, false).requested_bytes.load(Ordering::Relaxed)
}

unsafe fn allocate(size: usize) -> *mut u8 {
    unsafe { System.alloc_zeroed(allocation_layout(size)) }
}

fn allocation_layout(size: usize) -> Layout {
    Layout::from_size_align(size.max(1), ALLOCATION_ALIGNMENT).expect("Miri HAL allocation layout must be valid")
}

fn free_metadata(block: *mut u8, insert: bool) -> &'static FreeMetadata {
    let start = (block.addr() >> 4) & (FREE_METADATA_CAPACITY - 1);
    for offset in 0..FREE_METADATA_CAPACITY {
        let metadata = &FREE_METADATA[(start + offset) & (FREE_METADATA_CAPACITY - 1)];
        let current = metadata.block.load(Ordering::Acquire);
        if current == block {
            return metadata;
        }
        if insert
            && current.is_null()
            && metadata
                .block
                .compare_exchange(ptr::null_mut(), block, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            return metadata;
        }
    }
    panic!("Miri remote-free metadata table is full or missing an entry");
}
