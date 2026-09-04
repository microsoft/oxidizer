// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation telemetry, process-wide event tracking, and deferred stack resolution.

use std::alloc::Layout;
use std::cell::Cell;
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
#[cfg(all(not(miri), feature = "caller-symbolization"))]
use std::ffi::c_void;
use std::ptr;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Instant;

use seismograph::recorder::{alloc as runtime_alloc, event as runtime_event, thread as runtime_thread};
use seismograph_rallocator::callers::{
    AddressLookup as EncodedAddressLookup, AddressLookupFields as EncodedAddressLookupFields, Callers as EncodedCallers,
    CallersFields as EncodedCallersFields, Event as EncodedEvent, EventFields as EncodedEventFields, EventKind as EncodedEventKind,
    HeapKind as EncodedHeapKind, ThreadLog as EncodedThreadLog, ThreadLogFields as EncodedThreadLogFields, ThreadName as EncodedThreadName,
    ThreadNameFields as EncodedThreadNameFields,
};
use seismograph_rallocator::snapshot::{
    Domain as EncodedDomain, DomainFields as EncodedDomainFields, Estimate as EncodedEstimate, EstimateFields as EncodedEstimateFields,
    Histograms as EncodedHistograms, HistogramsFields as EncodedHistogramsFields, Region as EncodedRegion,
    RegionFields as EncodedRegionFields, SizeClass as EncodedSizeClass, SizeClassFields as EncodedSizeClassFields,
    Snapshot as EncodedSnapshot, Stats as EncodedStats, StatsFields as EncodedStatsFields, Version,
};
use seismograph_rallocator::topology::{
    Segment as EncodedSegment, SegmentFields as EncodedSegmentFields, Slice as EncodedSlice, SliceFields as EncodedSliceFields,
    SliceKind as EncodedSliceKind, TopologyRegion as EncodedTopologyRegion, TopologyRegionFields as EncodedTopologyRegionFields,
};

use crate::allocator::{enter_tracking_internal, restore_tracking_internal};
use crate::config::MAX_SIZE_CLASSES;
use crate::hal;

const SNAPSHOT_ARENA_CHUNK_BYTES: usize = 4 * 1024 * 1024;
static RALLOCATOR_SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    seismograph_rallocator::source::ID,
    seismograph_rallocator::source::NAME,
    seismograph_rallocator::source::SCHEMA_VERSION,
    capture_seismograph_source,
);

thread_local! {
    static ACTIVE_SNAPSHOT_ARENA: Cell<*mut SnapshotArena> = const { Cell::new(ptr::null_mut()) };
}

#[repr(C)]
struct SnapshotArenaChunk {
    previous: *mut Self,
    mapping_bytes: usize,
    cursor: usize,
    dedicated: bool,
}

struct SnapshotArena {
    head: *mut SnapshotArenaChunk,
    parent: *mut Self,
}

impl SnapshotArena {
    const fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            parent: ptr::null_mut(),
        }
    }

    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        if !self.head.is_null() && !unsafe { (*self.head).dedicated } {
            let address = unsafe { allocate_from_snapshot_chunk(self.head, size, layout.align()) };
            if !address.is_null() {
                return address;
            }
        }

        let required_bytes = snapshot_required_bytes(layout, size);
        let dedicated = required_bytes > SNAPSHOT_ARENA_CHUNK_BYTES / 2;
        let mapping_bytes = if dedicated {
            required_bytes
        } else {
            SNAPSHOT_ARENA_CHUNK_BYTES.max(required_bytes)
        };
        let mapping = hal::map(mapping_bytes);
        if mapping.is_null() {
            return ptr::null_mut();
        }

        let chunk = mapping.cast::<SnapshotArenaChunk>();
        // SAFETY: hal::map returned a writable mapping of mapping_bytes, which is
        // large enough for the header and requested allocation by construction.
        unsafe {
            chunk.write(SnapshotArenaChunk {
                previous: self.head,
                mapping_bytes,
                cursor: size_of::<SnapshotArenaChunk>(),
                dedicated,
            });
        }
        self.head = chunk;
        // SAFETY: chunk was initialized above and belongs exclusively to this arena.
        unsafe { allocate_from_snapshot_chunk(chunk, size, layout.align()) }
    }

    fn deallocate(&mut self, address: *mut u8) -> bool {
        let mut link = &raw mut self.head;
        while !unsafe { *link }.is_null() {
            let chunk = unsafe { *link };
            let start = chunk.addr();
            let end = start.saturating_add(unsafe { (*chunk).mapping_bytes });
            if address.addr() >= start && address.addr() < end {
                if unsafe { (*chunk).dedicated } {
                    unsafe {
                        *link = (*chunk).previous;
                        hal::unmap(chunk.cast(), (*chunk).mapping_bytes);
                    }
                }
                return true;
            }
            link = unsafe { &raw mut (*chunk).previous };
        }

        if self.parent.is_null() {
            false
        } else {
            // SAFETY: nested arenas are stack-scoped on this thread, so the parent
            // remains alive and is not accessed concurrently while the child is active.
            unsafe { (*self.parent).deallocate(address) }
        }
    }
}

impl Drop for SnapshotArena {
    fn drop(&mut self) {
        let mut chunk = self.head;
        while !chunk.is_null() {
            let previous = unsafe { (*chunk).previous };
            let mapping_bytes = unsafe { (*chunk).mapping_bytes };
            // SAFETY: every chunk was obtained from hal::map by this arena and has
            // not been unmapped unless it was first unlinked by deallocate.
            unsafe { hal::unmap(chunk.cast(), mapping_bytes) };
            chunk = previous;
        }
    }
}

struct SnapshotArenaActivation {
    previous: *mut SnapshotArena,
}

impl Drop for SnapshotArenaActivation {
    fn drop(&mut self) {
        let _ = ACTIVE_SNAPSHOT_ARENA.try_with(|active| active.set(self.previous));
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // A valid Layout guarantees this calculation cannot overflow.
fn snapshot_required_bytes(layout: Layout, size: usize) -> usize {
    size_of::<SnapshotArenaChunk>()
        .checked_add(layout.align() - 1)
        .and_then(|bytes| bytes.checked_add(size))
        .expect("Layout guarantees its padded size is representable")
}

unsafe fn allocate_from_snapshot_chunk(chunk: *mut SnapshotArenaChunk, size: usize, alignment: usize) -> *mut u8 {
    let cursor = unsafe { (*chunk).cursor };
    let Some(aligned) = cursor.checked_add(alignment - 1).map(|value| value & !(alignment - 1)) else {
        return ptr::null_mut();
    };
    let Some(end) = aligned.checked_add(size) else {
        return ptr::null_mut();
    };
    if end > unsafe { (*chunk).mapping_bytes } {
        return ptr::null_mut();
    }
    unsafe { (*chunk).cursor = end };
    unsafe { chunk.cast::<u8>().add(aligned) }
}

pub(crate) fn with_snapshot_arena<R>(operation: impl FnOnce() -> R) -> R {
    let mut arena = SnapshotArena::new();
    let previous = ACTIVE_SNAPSHOT_ARENA
        .try_with(|active| active.replace(ptr::from_mut(&mut arena)))
        .unwrap_or(ptr::null_mut());
    arena.parent = previous;
    let _activation = SnapshotArenaActivation { previous };
    operation()
}

pub(crate) fn snapshot_arena_allocate(layout: Layout) -> Option<*mut u8> {
    ACTIVE_SNAPSHOT_ARENA
        .try_with(|active| {
            let arena = active.get();
            (!arena.is_null()).then(|| unsafe { (*arena).allocate(layout) })
        })
        .unwrap_or(None)
}

pub(crate) fn snapshot_arena_deallocate(address: *mut u8) -> bool {
    ACTIVE_SNAPSHOT_ARENA
        .try_with(|active| {
            let arena = active.get();
            !arena.is_null() && unsafe { (*arena).deallocate(address) }
        })
        .unwrap_or(false)
}

/// A value with deterministic lower and upper bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Estimate<T> {
    value: T,
    lower_bound: T,
    upper_bound: T,
}

impl<T: Copy> Estimate<T> {
    const fn bounded(value: T, lower_bound: T, upper_bound: T) -> Self {
        Self {
            value,
            lower_bound,
            upper_bound,
        }
    }
}

/// Cheap lifetime aggregate statistics collected by the allocator.
///
/// Fields are read from independent atomic counters. A value is memory-safe
/// and individually valid, but it is not a transactional process-wide snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Stats {
    pub allocated_bytes: usize,
    pub deallocated_bytes: usize,
    pub live_bytes: usize,
    pub peak_live_bytes: usize,
    pub mapped_bytes: usize,
    pub os_mappings: usize,
    pub os_unmappings: usize,
    pub allocations: usize,
    pub deallocations: usize,
    pub remote_frees: usize,
    pub pending_remote_blocks: usize,
    remote_pushes_in_progress: usize,
    pub drained_remote_blocks: usize,
}

/// Stable category of a snapshot capture error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum SnapshotErrorKind {
    /// The encoded snapshot length could not be calculated.
    Sizing,
    /// Memory for the encoded snapshot could not be mapped.
    Allocation,
    /// Encoding into the mapped snapshot buffer failed.
    Encoding,
}

/// An error reported while capturing a telemetry snapshot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct SnapshotError {
    kind: SnapshotErrorKind,
}

impl SnapshotError {
    const fn sizing_failed() -> Self {
        Self {
            kind: SnapshotErrorKind::Sizing,
        }
    }

    const fn allocation_failed() -> Self {
        Self {
            kind: SnapshotErrorKind::Allocation,
        }
    }

    const fn encoding_failed() -> Self {
        Self {
            kind: SnapshotErrorKind::Encoding,
        }
    }
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            SnapshotErrorKind::Sizing => formatter.write_str("the telemetry snapshot length could not be calculated"),
            SnapshotErrorKind::Allocation => formatter.write_str("the telemetry snapshot mapping could not be allocated"),
            SnapshotErrorKind::Encoding => formatter.write_str("the telemetry snapshot could not be encoded"),
        }
    }
}

impl std::fmt::Debug for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "SnapshotError({self})")
    }
}

impl std::error::Error for SnapshotError {}

static NEXT_ALLOCATION_ID: AtomicUsize = AtomicUsize::new(1);
static AGGREGATES_AVAILABLE: AtomicBool = AtomicBool::new(false);
static PEAK_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static MAPPED_BYTES: AtomicUsize = AtomicUsize::new(0);
static BUMP_COMMITTED_BYTES: AtomicUsize = AtomicUsize::new(0);
static OS_MAPPINGS: AtomicUsize = AtomicUsize::new(0);
static OS_UNMAPPINGS: AtomicUsize = AtomicUsize::new(0);
static REMOTE_FREES: AtomicUsize = AtomicUsize::new(0);
static PENDING_REMOTE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
static REMOTE_PUSHES_IN_PROGRESS: AtomicUsize = AtomicUsize::new(0);
static DRAINED_REMOTE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
const HISTOGRAM_BUCKETS: usize = usize::BITS as usize + 1;
static AGGREGATE_REGISTRY: AtomicPtr<AggregateShard> = AtomicPtr::new(ptr::null_mut());
static FALLBACK_AGGREGATE_REGISTERED: AtomicBool = AtomicBool::new(false);
static FALLBACK_AGGREGATE_SHARD: AggregateShard = AggregateShard::new(true);
static SIZE_CLASS_BLOCK_BYTES: [AtomicUsize; MAX_SIZE_CLASSES] = [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES];
static SIZE_CLASS_ALLOCATIONS: [AtomicUsize; MAX_SIZE_CLASSES] = [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES];
static SIZE_CLASS_DEALLOCATIONS: [AtomicUsize; MAX_SIZE_CLASSES] = [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES];
static SIZE_CLASS_ALLOCATED_BYTES: [AtomicUsize; MAX_SIZE_CLASSES] = [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES];
static SIZE_CLASS_DEALLOCATED_BYTES: [AtomicUsize; MAX_SIZE_CLASSES] = [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES];
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());
thread_local! {
    static TELEMETRY_SUPPRESSION_DEPTH: Cell<usize> = const { Cell::new(0) };
    static AGGREGATE_SHARD: Cell<*const AggregateShard> = const { Cell::new(ptr::null()) };
    #[cfg(test)]
    static AGGREGATE_REGISTRATION_BARRIER: RefCell<Option<Arc<std::sync::Barrier>>> = const { RefCell::new(None) };
}

#[repr(C, align(64))]
pub(crate) struct AggregateShard {
    next: AtomicPtr<Self>,
    shared: bool,
    allocated_bytes: AtomicUsize,
    deallocated_bytes: AtomicUsize,
    allocations: AtomicUsize,
    deallocations: AtomicUsize,
}

impl AggregateShard {
    const fn new(shared: bool) -> Self {
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            shared,
            allocated_bytes: AtomicUsize::new(0),
            deallocated_bytes: AtomicUsize::new(0),
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
        }
    }
}

#[derive(Clone)]
struct AggregateSnapshot {
    allocated_bytes: usize,
    deallocated_bytes: usize,
    allocations: usize,
    deallocations: usize,
}

#[derive(Clone)]
struct SizeClassAggregateSnapshot {
    block_bytes: [usize; MAX_SIZE_CLASSES],
    allocations: [usize; MAX_SIZE_CLASSES],
    deallocations: [usize; MAX_SIZE_CLASSES],
    allocated_bytes: [usize; MAX_SIZE_CLASSES],
    deallocated_bytes: [usize; MAX_SIZE_CLASSES],
}

impl SizeClassAggregateSnapshot {
    const fn new() -> Self {
        Self {
            block_bytes: [0; MAX_SIZE_CLASSES],
            allocations: [0; MAX_SIZE_CLASSES],
            deallocations: [0; MAX_SIZE_CLASSES],
            allocated_bytes: [0; MAX_SIZE_CLASSES],
            deallocated_bytes: [0; MAX_SIZE_CLASSES],
        }
    }
}

impl AggregateSnapshot {
    const fn new() -> Self {
        Self {
            allocated_bytes: 0,
            deallocated_bytes: 0,
            allocations: 0,
            deallocations: 0,
        }
    }
}

/// Returns aggregate statistics when they were compiled into the allocator.
#[must_use]
#[cfg(test)]
pub(crate) fn stats() -> Option<Stats> {
    crate::allocator::flush_thread_aggregate_batch();
    aggregate_snapshot().map(|aggregates| aggregate_stats(&aggregates))
}

fn aggregate_stats(aggregates: &AggregateSnapshot) -> Stats {
    let live_bytes = aggregates.allocated_bytes.saturating_sub(aggregates.deallocated_bytes);
    let peak_live_bytes = PEAK_LIVE_BYTES.fetch_max(live_bytes, Ordering::Relaxed).max(live_bytes);
    Stats {
        allocated_bytes: aggregates.allocated_bytes,
        deallocated_bytes: aggregates.deallocated_bytes,
        live_bytes,
        peak_live_bytes,
        mapped_bytes: MAPPED_BYTES.load(Ordering::Relaxed) + BUMP_COMMITTED_BYTES.load(Ordering::Relaxed),
        os_mappings: OS_MAPPINGS.load(Ordering::Relaxed),
        os_unmappings: OS_UNMAPPINGS.load(Ordering::Relaxed),
        allocations: aggregates.allocations,
        deallocations: aggregates.deallocations,
        remote_frees: REMOTE_FREES.load(Ordering::Relaxed),
        pending_remote_blocks: PENDING_REMOTE_BLOCKS.load(Ordering::Relaxed),
        remote_pushes_in_progress: REMOTE_PUSHES_IN_PROGRESS.load(Ordering::Relaxed),
        drained_remote_blocks: DRAINED_REMOTE_BLOCKS.load(Ordering::Relaxed),
    }
}

fn aggregate_snapshot() -> Option<AggregateSnapshot> {
    if !AGGREGATES_AVAILABLE.load(Ordering::Acquire) {
        return None;
    }
    let mut snapshot = AggregateSnapshot::new();
    let mut current = AGGREGATE_REGISTRY.load(Ordering::Acquire);
    while !current.is_null() {
        let shard = unsafe { &*current };
        snapshot.allocated_bytes = snapshot.allocated_bytes.wrapping_add(shard.allocated_bytes.load(Ordering::Relaxed));
        snapshot.deallocated_bytes = snapshot
            .deallocated_bytes
            .wrapping_add(shard.deallocated_bytes.load(Ordering::Relaxed));
        snapshot.allocations = snapshot.allocations.wrapping_add(shard.allocations.load(Ordering::Relaxed));
        snapshot.deallocations = snapshot.deallocations.wrapping_add(shard.deallocations.load(Ordering::Relaxed));
        current = shard.next.load(Ordering::Acquire);
    }
    Some(snapshot)
}

fn size_class_aggregate_snapshot() -> SizeClassAggregateSnapshot {
    let mut snapshot = SizeClassAggregateSnapshot::new();
    for class_index in 0..MAX_SIZE_CLASSES {
        snapshot.block_bytes[class_index] = SIZE_CLASS_BLOCK_BYTES[class_index].load(Ordering::Relaxed);
        snapshot.allocations[class_index] = SIZE_CLASS_ALLOCATIONS[class_index].load(Ordering::Relaxed);
        snapshot.deallocations[class_index] = SIZE_CLASS_DEALLOCATIONS[class_index].load(Ordering::Relaxed);
        snapshot.allocated_bytes[class_index] = SIZE_CLASS_ALLOCATED_BYTES[class_index].load(Ordering::Relaxed);
        snapshot.deallocated_bytes[class_index] = SIZE_CLASS_DEALLOCATED_BYTES[class_index].load(Ordering::Relaxed);
    }
    snapshot
}

fn size_class_snapshots(before: &SizeClassAggregateSnapshot, after: &SizeClassAggregateSnapshot) -> Vec<SizeClassSnapshot> {
    let mut classes = Vec::new();
    for class_index in 0..MAX_SIZE_CLASSES {
        let block_bytes = before.block_bytes[class_index].max(after.block_bytes[class_index]);
        if block_bytes == 0 {
            continue;
        }
        let allocations_before = before.allocations[class_index];
        let deallocations_before = before.deallocations[class_index];
        let allocations_after = after.allocations[class_index];
        let deallocations_after = after.deallocations[class_index];
        let allocated_before = before.allocated_bytes[class_index];
        let deallocated_before = before.deallocated_bytes[class_index];
        let allocated_after = after.allocated_bytes[class_index];
        let deallocated_after = after.deallocated_bytes[class_index];
        let live_allocations = Estimate::bounded(
            allocations_after.saturating_sub(deallocations_after),
            allocations_before.saturating_sub(deallocations_after),
            allocations_after.saturating_sub(deallocations_before),
        );
        let requested_bytes = Estimate::bounded(
            allocated_after.saturating_sub(deallocated_after),
            allocated_before.saturating_sub(deallocated_after),
            allocated_after.saturating_sub(deallocated_before),
        );
        classes.push(SizeClassSnapshot {
            class_index,
            block_bytes,
            live_allocations,
            requested_bytes,
            usable_bytes: Estimate::bounded(
                live_allocations.value.saturating_mul(block_bytes),
                live_allocations.lower_bound.saturating_mul(block_bytes),
                live_allocations.upper_bound.saturating_mul(block_bytes),
            ),
        });
    }
    classes
}

fn try_snapshot_with_runtime_events(
    runtime_events: Option<&runtime_event::Events>,
    include_runtime_events: bool,
) -> Result<Snapshot, SnapshotError> {
    with_snapshot_arena(|| {
        with_telemetry_suppressed(|| {
            let started_at = Instant::now();
            crate::allocator::flush_thread_aggregate_batch();
            let size_classes_before = size_class_aggregate_snapshot();
            let regions = crate::allocator::telemetry_region_snapshots();
            let domains = crate::allocator::telemetry_domain_snapshots();
            let aggregates_after = aggregate_snapshot();
            let stats = aggregates_after.as_ref().map(aggregate_stats);
            let size_classes_after = size_class_aggregate_snapshot();
            let size_classes = size_class_snapshots(&size_classes_before, &size_classes_after);
            let callers = caller_snapshot_from_runtime(runtime_events);
            let stats = snapshot_stats(stats);
            let mut encoded = EncodedSnapshot::new(producer_version());
            encoded.stats = encode_stats(stats);
            encoded.size_classes = size_classes.iter().map(encode_size_class).collect();
            encoded.regions = regions.iter().map(encode_region).collect();
            encoded.topology = regions.iter().map(encode_topology_region).collect();
            encoded.domains = encode_domains(&domains, &regions);
            encoded.callers = callers.as_ref().map(encode_callers);
            encoded.runtime_events = include_runtime_events.then(|| runtime_events.cloned()).flatten();
            encoded.histograms = encode_histograms();
            encoded.addresses = resolve_addresses(callers.as_ref(), encoded.runtime_events.as_ref());
            encoded.metadata.capture_duration_nanos = u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX);

            let len = seismograph_rallocator::encoded_len(&encoded).map_err(|_error| SnapshotError::sizing_failed())?;
            let address = NonNull::new(crate::hal::map(len)).ok_or(SnapshotError::allocation_failed())?;
            let mut mapping = MappedBytes { address, len };
            seismograph_rallocator::encode(&encoded, mapping.as_mut_slice()).map_err(|_error| SnapshotError::encoding_failed())?;
            Ok(Snapshot { mapping })
        })
    })
}

pub(crate) fn register_seismograph_source() {
    seismograph::snapshot::register_source(&RALLOCATOR_SOURCE);
}

fn capture_seismograph_source(
    context: seismograph::snapshot::SnapshotContext<'_>,
) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let snapshot = try_snapshot_with_runtime_events(Some(context.events()), false)
        .map_err(|_error| seismograph::Error::new("rallocator snapshot capture failed"))?;
    seismograph::snapshot::SourceData::copy_from(snapshot.as_bytes())
}

fn snapshot_stats(stats: Option<Stats>) -> Stats {
    stats.unwrap_or_default()
}

#[cfg(test)]
fn encode_snapshot_with(
    encoded: &EncodedSnapshot,
    encoded_len: fn(&EncodedSnapshot) -> Option<usize>,
    map: fn(usize) -> *mut u8,
    encode: fn(&EncodedSnapshot, &mut [u8]) -> bool,
) -> Option<Snapshot> {
    let len = encoded_len(encoded)?;
    let address = NonNull::new(map(len))?;
    let mut mapping = MappedBytes { address, len };
    if !encode(encoded, mapping.as_mut_slice()) {
        return None;
    }
    Some(Snapshot { mapping })
}

fn producer_version() -> Version {
    Version::new(
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap(),
    )
}

fn encode_stats(stats: Stats) -> EncodedStats {
    EncodedStats::from_fields(EncodedStatsFields {
        allocated_bytes: stats.allocated_bytes as u64,
        deallocated_bytes: stats.deallocated_bytes as u64,
        live_bytes: stats.live_bytes as u64,
        peak_live_bytes: stats.peak_live_bytes as u64,
        mapped_bytes: stats.mapped_bytes as u64,
        os_mappings: stats.os_mappings as u64,
        os_unmappings: stats.os_unmappings as u64,
        allocations: stats.allocations as u64,
        deallocations: stats.deallocations as u64,
        remote_frees: stats.remote_frees as u64,
        pending_remote_blocks: stats.pending_remote_blocks as u64,
        remote_pushes_in_progress: stats.remote_pushes_in_progress as u64,
        drained_remote_blocks: stats.drained_remote_blocks as u64,
    })
}

fn encode_estimate(estimate: Estimate<usize>) -> EncodedEstimate {
    EncodedEstimate::from_fields(EncodedEstimateFields {
        value: estimate.value as u64,
        lower_bound: estimate.lower_bound as u64,
        upper_bound: estimate.upper_bound as u64,
    })
}

fn encode_size_class(class: &SizeClassSnapshot) -> EncodedSizeClass {
    EncodedSizeClass::from_fields(EncodedSizeClassFields {
        class_index: class.class_index as u32,
        block_bytes: class.block_bytes as u64,
        live_allocations: encode_estimate(class.live_allocations),
        requested_bytes: encode_estimate(class.requested_bytes),
        usable_bytes: encode_estimate(class.usable_bytes),
    })
}

fn encode_region(region: &RegionSnapshot) -> EncodedRegion {
    EncodedRegion::from_fields(EncodedRegionFields {
        region_index: region.region_index as u32,
        reserved_bytes: region.reserved_bytes as u64,
        used_slices: region.used_slices as u64,
        free_slices: region.free_slices as u64,
    })
}

fn encode_domains(domains: &[DomainSnapshot], regions: &[RegionSnapshot]) -> Vec<EncodedDomain> {
    domains
        .iter()
        .map(|domain| {
            let matching = regions.iter().filter(|region| region.domain_id == domain.domain_id);
            let mut region_indices = Vec::new();
            let mut region_count = 0_u64;
            let mut reserved_bytes = 0_u64;
            let mut used_slices = 0_u64;
            let mut free_slices = 0_u64;
            let mut small_slices = 0_u64;
            let mut medium_slices = 0_u64;
            let mut bump_slices = 0_u64;
            let mut unknown_slices = 0_u64;
            for region in matching {
                region_indices.push(region.region_index as u32);
                region_count += 1;
                reserved_bytes += region.reserved_bytes as u64;
                used_slices += region.used_slices as u64;
                free_slices += region.free_slices as u64;
                for slice in &region.slices {
                    match slice.kind {
                        PhysicalSliceKind::Small => small_slices += 1,
                        PhysicalSliceKind::Medium | PhysicalSliceKind::MediumContinuation => medium_slices += 1,
                        PhysicalSliceKind::Bump => bump_slices += 1,
                        PhysicalSliceKind::Unknown => unknown_slices += 1,
                    }
                }
            }
            EncodedDomain::from_fields(EncodedDomainFields {
                domain_id: domain.domain_id as u64,
                is_default: domain.is_default,
                region_count,
                reserved_bytes,
                used_slices,
                free_slices,
                small_slices,
                medium_slices,
                bump_slices,
                unknown_slices,
                region_indices,
            })
        })
        .collect()
}

fn encode_topology_region(region: &RegionSnapshot) -> EncodedTopologyRegion {
    let slices = region
        .slices
        .iter()
        .map(|slice| {
            let kind = match slice.kind {
                PhysicalSliceKind::Unknown => EncodedSliceKind::Unknown,
                PhysicalSliceKind::Small => EncodedSliceKind::Small,
                PhysicalSliceKind::Medium => EncodedSliceKind::Medium,
                PhysicalSliceKind::MediumContinuation => EncodedSliceKind::MediumContinuation,
                PhysicalSliceKind::Bump => EncodedSliceKind::Bump,
            };
            let segments = slice
                .segments
                .iter()
                .map(|segment| {
                    EncodedSegment::from_fields(EncodedSegmentFields {
                        segment_index: segment.segment_index as u8,
                        class_index: segment.class_index as u32,
                        context: segment.context,
                        live_blocks: segment.live_blocks as u32,
                        usable_blocks: segment.usable_blocks as u32,
                        utilization_tracked: segment.utilization_tracked,
                    })
                })
                .collect();
            EncodedSlice::from_fields(EncodedSliceFields {
                slice_index: slice.slice_index as u32,
                kind,
                span_slices: slice.span_slices as u32,
                owner: slice.owner as u64,
                requested_bytes: slice.requested_bytes as u64,
                usable_bytes: slice.usable_bytes as u64,
                segments,
            })
        })
        .collect();
    EncodedTopologyRegion::from_fields(EncodedTopologyRegionFields {
        region_index: region.region_index as u32,
        base_address: region.base_address as u64,
        region_bytes: region.reserved_bytes as u64,
        slice_bytes: region.slice_bytes as u64,
        used_bitmap: region.used_bitmap.clone(),
        slices,
    })
}

fn encode_callers(callers: &CallerSnapshot) -> EncodedCallers {
    let threads = callers
        .threads
        .iter()
        .map(|thread| {
            EncodedThreadLog::from_fields(EncodedThreadLogFields {
                thread_log_id: thread.thread_log_id as u64,
                total_events: thread.total_events as u64,
                lost_events: thread.lost_events as u64,
                allocated_histogram: thread.allocated_histogram.iter().map(|count| *count as u64).collect(),
                live_histogram: thread.live_histogram.iter().map(|count| *count as u64).collect(),
            })
        })
        .collect();
    let events = callers
        .events
        .iter()
        .map(|event| {
            let kind = match event.kind {
                EventKind::Allocated => EncodedEventKind::Allocated,
                EventKind::Deallocated => EncodedEventKind::Deallocated,
            };
            let heap_kind = match event.heap_kind {
                HeapKind::General => EncodedHeapKind::General,
                HeapKind::Bump => EncodedHeapKind::Bump,
                HeapKind::Thread => EncodedHeapKind::Thread,
            };
            EncodedEvent::from_fields(EncodedEventFields {
                thread_log_id: event.thread_log_id as u64,
                event_thread_id: event.event_thread_id as u64,
                sequence: event.sequence as u64,
                allocation_id: event.allocation_id as u64,
                kind,
                heap_id: event.heap_id as u64,
                heap_kind,
                freed_after_heap_release: event.freed_after_heap_release,
                address: event.address as u64,
                size: event.size as u64,
                align: event.align as u64,
                call_stack: event.call_stack.iter().map(|ip| ip.0 as u64).collect(),
            })
        })
        .collect();
    let thread_names = callers
        .thread_names
        .iter()
        .map(|thread| {
            EncodedThreadName::from_fields(EncodedThreadNameFields {
                thread_id: thread.thread_id as u64,
                name: thread.name.clone(),
            })
        })
        .collect();
    EncodedCallers::from_fields(EncodedCallersFields {
        session_id: callers.session_id as u64,
        total_events: callers.total_events as u64,
        lost_events: callers.lost_events as u64,
        threads,
        events,
        thread_names,
    })
}

fn encode_histograms() -> EncodedHistograms {
    EncodedHistograms::from_fields(EncodedHistogramsFields {
        allocated: Vec::new(),
        live: Vec::new(),
    })
}

fn resolve_addresses(callers: Option<&CallerSnapshot>, runtime_events: Option<&runtime_event::Events>) -> Vec<EncodedAddressLookup> {
    let mut addresses = callers
        .into_iter()
        .flat_map(|callers| callers.events.iter())
        .flat_map(|event| event.call_stack.iter())
        .map(|ip| ip.0)
        .chain(
            runtime_events
                .into_iter()
                .flat_map(|events| events.events.iter())
                .flat_map(|event| event.call_stack.iter())
                .map(|address| address.get() as usize),
        )
        .filter(|address| *address != 0)
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();

    addresses
        .into_iter()
        .map(|address| {
            let lookup = EncodedAddressLookup::from_fields(EncodedAddressLookupFields {
                address: address as u64,
                symbol: None,
                filename: None,
                line: None,
                column: None,
            });
            #[cfg(all(not(miri), feature = "caller-symbolization"))]
            {
                let mut lookup = lookup;
                let lookup_address = runtime_event::Address::new(address as u64);
                let lookup_address = seismograph::recorder::symbol_lookup_address(lookup_address);
                backtrace::resolve(lookup_address.get() as *mut c_void, |symbol| {
                    merge_address_lookup(
                        &mut lookup,
                        symbol.name().map(|name| name.to_string()),
                        symbol.filename().map(|path| path.to_string_lossy().into_owned()),
                        symbol.lineno(),
                        symbol.colno(),
                    );
                });
                lookup
            }
            #[cfg(any(miri, not(feature = "caller-symbolization")))]
            {
                lookup
            }
        })
        .collect()
}

#[cfg(any(test, all(not(miri), feature = "caller-symbolization")))]
fn merge_address_lookup(
    lookup: &mut EncodedAddressLookup,
    symbol: Option<String>,
    filename: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
) {
    lookup.symbol = lookup.symbol.take().or(symbol);
    lookup.filename = lookup.filename.take().or(filename);
    lookup.line = lookup.line.or(line);
    lookup.column = lookup.column.or(column);
}

fn caller_snapshot_from_runtime(runtime: Option<&runtime_event::Events>) -> Option<CallerSnapshot> {
    let runtime = runtime?;
    let deallocated = runtime
        .events
        .iter()
        .filter(|event| event.kind == runtime_event::EventKind::Deallocation)
        .filter_map(|event| event.allocation().map(|allocation| allocation.allocation_id))
        .collect::<HashSet<_>>();
    let allocation_threads = runtime
        .events
        .iter()
        .filter(|event| event.kind == runtime_event::EventKind::Allocation)
        .filter_map(|event| event.allocation().map(|allocation| (allocation.allocation_id, event.thread_id)))
        .collect::<HashMap<_, _>>();
    let thread_counts = runtime
        .threads
        .iter()
        .map(|thread| (thread.thread_id, (thread.total_events as usize, thread.lost_events as usize)))
        .collect::<HashMap<_, _>>();
    let mut allocated_histograms = HashMap::<runtime_thread::ThreadId, Vec<usize>>::new();
    let mut live_histograms = HashMap::<runtime_thread::ThreadId, Vec<usize>>::new();
    let mut events = Vec::new();

    for event in &runtime.events {
        let Some(allocation) = event.allocation() else {
            continue;
        };
        let thread_log_id = allocation_threads
            .get(&allocation.allocation_id)
            .copied()
            .unwrap_or(event.thread_id);
        let bucket = histogram_bucket(allocation.size as usize);
        if event.kind == runtime_event::EventKind::Allocation {
            allocated_histograms
                .entry(thread_log_id)
                .or_insert_with(|| vec![0; HISTOGRAM_BUCKETS])[bucket] += 1;
            if !deallocated.contains(&allocation.allocation_id) {
                live_histograms.entry(thread_log_id).or_insert_with(|| vec![0; HISTOGRAM_BUCKETS])[bucket] += 1;
            }
        }
        events.extend(decode_runtime_heap_kind(allocation.heap_kind).map(|heap_kind| Event {
            thread_log_id: thread_log_id.get() as usize,
            event_thread_id: allocation.event_thread_id.get() as usize,
            sequence: event.sequence.get() as usize,
            allocation_id: allocation.allocation_id.get() as usize,
            kind: if event.kind == runtime_event::EventKind::Allocation {
                EventKind::Allocated
            } else {
                EventKind::Deallocated
            },
            heap_id: allocation.heap_id.get() as usize,
            heap_kind,
            freed_after_heap_release: allocation.freed_after_heap_release,
            address: allocation.address.get() as usize,
            size: allocation.size as usize,
            align: allocation.alignment as usize,
            call_stack: event.call_stack.iter().map(|address| Ip(address.get() as usize)).collect(),
        }));
    }
    if events.is_empty() {
        return None;
    }
    events.sort_unstable_by_key(|event| (event.thread_log_id, event.sequence));
    let threads = thread_counts
        .into_iter()
        .map(|(thread_id, (total_events, lost_events))| ThreadLog {
            thread_log_id: thread_id.get() as usize,
            total_events,
            lost_events,
            allocated_histogram: allocated_histograms
                .remove(&thread_id)
                .unwrap_or_else(|| vec![0; HISTOGRAM_BUCKETS]),
            live_histogram: live_histograms.remove(&thread_id).unwrap_or_else(|| vec![0; HISTOGRAM_BUCKETS]),
        })
        .collect();
    let recorder_names = runtime
        .threads
        .iter()
        .map(|thread| (thread.thread_id, thread.name.as_str()))
        .collect::<HashMap<_, _>>();
    let thread_names = runtime
        .events
        .iter()
        .filter_map(|event| {
            let allocation = event.allocation()?;
            Some((allocation.event_thread_id, recorder_names.get(&event.thread_id).copied()?))
        })
        .collect::<HashMap<_, _>>()
        .into_iter()
        .map(|(thread_id, name)| ThreadName {
            thread_id: thread_id.get() as usize,
            name: name.to_owned(),
        })
        .collect();
    Some(CallerSnapshot {
        session_id: 1,
        total_events: runtime.total_events as usize,
        lost_events: runtime.lost_events as usize,
        threads,
        events,
        thread_names,
    })
}

fn decode_runtime_heap_kind(kind: runtime_alloc::HeapKind) -> Option<HeapKind> {
    [
        (runtime_alloc::HeapKind::General, HeapKind::General),
        (runtime_alloc::HeapKind::Bump, HeapKind::Bump),
        (runtime_alloc::HeapKind::Thread, HeapKind::Thread),
    ]
    .into_iter()
    .find_map(|(candidate, decoded)| (kind == candidate).then_some(decoded))
}

fn histogram_bucket(size: usize) -> usize {
    if size == 0 {
        0
    } else {
        usize::BITS as usize - size.leading_zeros() as usize
    }
}

pub(crate) fn record_mapping(size: usize) {
    AGGREGATES_AVAILABLE.store(true, Ordering::Release);
    MAPPED_BYTES.fetch_add(size, Ordering::Relaxed);
    OS_MAPPINGS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_unmapping(size: usize) {
    MAPPED_BYTES.fetch_sub(size, Ordering::Relaxed);
    OS_UNMAPPINGS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_bump_commit(size: usize) {
    BUMP_COMMITTED_BYTES.fetch_add(size, Ordering::Relaxed);
}

pub(crate) fn record_bump_decommit(size: usize) {
    BUMP_COMMITTED_BYTES.fetch_sub(size, Ordering::Relaxed);
}

#[inline(always)]
fn add_owner(shard: &AggregateShard, counter: &AtomicUsize, value: usize) {
    if shard.shared {
        counter.fetch_add(value, Ordering::Relaxed);
    } else {
        counter.store(counter.load(Ordering::Relaxed).wrapping_add(value), Ordering::Relaxed);
    }
}

#[inline(always)]
fn register_aggregate_shard(shard: *mut AggregateShard) {
    let mut head = AGGREGATE_REGISTRY.load(Ordering::Acquire);
    loop {
        unsafe { (*shard).next.store(head, Ordering::Relaxed) };
        #[cfg(test)]
        wait_at_aggregate_registration_barrier();
        match AGGREGATE_REGISTRY.compare_exchange_weak(head, shard, Ordering::Release, Ordering::Acquire) {
            Ok(_) => return,
            Err(current) => head = current,
        }
    }
}

#[cfg(all(test, not(miri)))]
fn set_aggregate_registration_barrier(barrier: Arc<std::sync::Barrier>) {
    AGGREGATE_REGISTRATION_BARRIER.with(|slot| *slot.borrow_mut() = Some(barrier));
}

#[cfg(test)]
fn wait_at_aggregate_registration_barrier() {
    AGGREGATE_REGISTRATION_BARRIER.with(|slot| {
        if let Some(barrier) = slot.borrow_mut().take() {
            barrier.wait();
        }
    });
}

#[cold]
fn initialize_aggregate_shard() -> *const AggregateShard {
    let _internal = TelemetrySuppressionGuard::enter();
    let address = crate::hal::map(std::mem::size_of::<AggregateShard>()).cast::<AggregateShard>();
    if !address.is_null() {
        unsafe { address.write(AggregateShard::new(false)) };
        register_aggregate_shard(address);
    } else if !FALLBACK_AGGREGATE_REGISTERED.swap(true, Ordering::AcqRel) {
        register_aggregate_shard((&raw const FALLBACK_AGGREGATE_SHARD).cast_mut());
    }
    AGGREGATES_AVAILABLE.store(true, Ordering::Release);
    if address.is_null() {
        &raw const FALLBACK_AGGREGATE_SHARD
    } else {
        address
    }
}

#[inline(always)]
fn aggregate_shard() -> &'static AggregateShard {
    AGGREGATE_SHARD.with(|storage| {
        let mut address = storage.get();
        if address.is_null() {
            address = initialize_aggregate_shard();
            storage.set(address);
        }
        unsafe { &*address }
    })
}

pub(crate) fn aggregate_shard_pointer() -> *const AggregateShard {
    ptr::from_ref(aggregate_shard())
}

#[inline(always)]
fn record_allocation_in(shard: &AggregateShard, size: usize) {
    add_owner(shard, &shard.allocated_bytes, size);
    add_owner(shard, &shard.allocations, 1);
}

#[inline(always)]
fn record_deallocation_in(shard: &AggregateShard, size: usize) {
    add_owner(shard, &shard.deallocated_bytes, size);
    add_owner(shard, &shard.deallocations, 1);
}

pub(crate) fn record_allocation(size: usize) {
    if telemetry_suppressed() {
        return;
    }
    record_allocation_in(aggregate_shard(), size);
}

pub(crate) fn record_deallocation_stats(size: usize) {
    if telemetry_suppressed() {
        return;
    }
    record_deallocation_in(aggregate_shard(), size);
}

#[cfg(test)]
pub(crate) fn record_small_allocation(_class_index: usize, _block_bytes: usize, requested_bytes: usize) {
    if telemetry_suppressed() {
        return;
    }
    record_allocation_in(aggregate_shard(), requested_bytes);
}

#[cfg(test)]
pub(crate) fn record_small_deallocation(_class_index: usize, requested_bytes: usize) {
    if telemetry_suppressed() {
        return;
    }
    record_deallocation_in(aggregate_shard(), requested_bytes);
}

pub(crate) fn publish_aggregate_batch(
    shard: *const AggregateShard,
    allocated_bytes: usize,
    deallocated_bytes: usize,
    allocations: usize,
    deallocations: usize,
) {
    let shard = unsafe { &*shard };
    add_owner(shard, &shard.allocated_bytes, allocated_bytes);
    add_owner(shard, &shard.deallocated_bytes, deallocated_bytes);
    add_owner(shard, &shard.allocations, allocations);
    add_owner(shard, &shard.deallocations, deallocations);
}

pub(crate) fn publish_size_class_batch(
    class_index: usize,
    block_bytes: usize,
    allocations: usize,
    deallocations: usize,
    allocated_bytes: usize,
    deallocated_bytes: usize,
) {
    SIZE_CLASS_BLOCK_BYTES[class_index].store(block_bytes, Ordering::Relaxed);
    SIZE_CLASS_ALLOCATIONS[class_index].fetch_add(allocations, Ordering::Relaxed);
    SIZE_CLASS_DEALLOCATIONS[class_index].fetch_add(deallocations, Ordering::Relaxed);
    SIZE_CLASS_ALLOCATED_BYTES[class_index].fetch_add(allocated_bytes, Ordering::Relaxed);
    SIZE_CLASS_DEALLOCATED_BYTES[class_index].fetch_add(deallocated_bytes, Ordering::Relaxed);
}

pub(crate) fn begin_remote_free() {
    if !AGGREGATES_AVAILABLE.load(Ordering::Relaxed) {
        return;
    }
    REMOTE_PUSHES_IN_PROGRESS.fetch_add(1, Ordering::Relaxed);
    REMOTE_FREES.fetch_add(1, Ordering::Relaxed);
    PENDING_REMOTE_BLOCKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn finish_remote_free() {
    if AGGREGATES_AVAILABLE.load(Ordering::Relaxed) {
        REMOTE_PUSHES_IN_PROGRESS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_remote_retired_free() {
    if !telemetry_suppressed() && AGGREGATES_AVAILABLE.load(Ordering::Relaxed) {
        REMOTE_FREES.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_remote_drain() {
    if !AGGREGATES_AVAILABLE.load(Ordering::Relaxed) {
        return;
    }
    PENDING_REMOTE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
    DRAINED_REMOTE_BLOCKS.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventKind {
    Allocated,
    Deallocated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeapKind {
    General,
    Bump,
    Thread,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Ip(usize);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Event {
    thread_log_id: usize,
    event_thread_id: usize,
    sequence: usize,
    allocation_id: usize,
    kind: EventKind,
    heap_id: usize,
    heap_kind: HeapKind,
    freed_after_heap_release: bool,
    address: usize,
    size: usize,
    align: usize,
    call_stack: Vec<Ip>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThreadLog {
    thread_log_id: usize,
    total_events: usize,
    lost_events: usize,
    allocated_histogram: Vec<usize>,
    live_histogram: Vec<usize>,
}

/// An opaque, self-contained binary telemetry snapshot.
pub(crate) struct Snapshot {
    mapping: MappedBytes,
}

struct MappedBytes {
    address: NonNull<u8>,
    len: usize,
}

// The owned mapping is immutable after encoding and may be unmapped on any thread.
unsafe impl Send for MappedBytes {}
unsafe impl Sync for MappedBytes {}

impl MappedBytes {
    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.address.as_ptr(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.address.as_ptr(), self.len) }
    }
}

impl Drop for MappedBytes {
    fn drop(&mut self) {
        unsafe { crate::hal::unmap(self.address.as_ptr(), self.len) };
    }
}

impl Snapshot {
    /// Returns the complete encoded snapshot.
    ///
    /// Decode these bytes with [`seismograph_rallocator::decode`].
    #[must_use]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.mapping.as_slice()
    }
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Snapshot")
            .field("bytes", &self.mapping.len)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SizeClassSnapshot {
    class_index: usize,
    block_bytes: usize,
    live_allocations: Estimate<usize>,
    requested_bytes: Estimate<usize>,
    usable_bytes: Estimate<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegionSnapshot {
    pub(crate) domain_id: usize,
    pub(crate) region_index: usize,
    pub(crate) base_address: usize,
    pub(crate) reserved_bytes: usize,
    pub(crate) slice_bytes: usize,
    pub(crate) used_slices: usize,
    pub(crate) free_slices: usize,
    pub(crate) used_bitmap: Vec<u64>,
    pub(crate) slices: Vec<PhysicalSliceSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DomainSnapshot {
    pub(crate) domain_id: usize,
    pub(crate) is_default: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PhysicalSliceKind {
    Unknown,
    Small,
    Medium,
    MediumContinuation,
    Bump,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PhysicalSliceSnapshot {
    pub(crate) slice_index: usize,
    pub(crate) kind: PhysicalSliceKind,
    pub(crate) span_slices: usize,
    pub(crate) owner: usize,
    pub(crate) requested_bytes: usize,
    pub(crate) usable_bytes: usize,
    pub(crate) segments: Vec<PhysicalSegmentSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhysicalSegmentSnapshot {
    pub(crate) segment_index: usize,
    pub(crate) class_index: usize,
    pub(crate) context: bool,
    pub(crate) live_blocks: usize,
    pub(crate) usable_blocks: usize,
    pub(crate) utilization_tracked: bool,
}

#[derive(Clone, Debug)]
struct CallerSnapshot {
    session_id: usize,
    total_events: usize,
    lost_events: usize,
    threads: Vec<ThreadLog>,
    events: Vec<Event>,
    thread_names: Vec<ThreadName>,
}

#[derive(Clone, Debug)]
struct ThreadName {
    thread_id: usize,
    name: String,
}

#[derive(Clone, Copy)]
pub(crate) struct TrackingAllocation {
    allocation_id: usize,
    recording_session: Option<seismograph::recorder::RecordingSession>,
    heap_id: usize,
    heap_kind: HeapKind,
}

impl TrackingAllocation {
    pub(crate) const NONE: Self = Self {
        allocation_id: 0,
        recording_session: None,
        heap_id: 0,
        heap_kind: HeapKind::General,
    };

    pub(crate) const fn allocation_id(self) -> usize {
        self.allocation_id
    }

    pub(crate) const fn recording_session(self) -> Option<seismograph::recorder::RecordingSession> {
        self.recording_session
    }

    pub(crate) const fn from_parts(
        allocation_id: usize,
        recording_session: Option<seismograph::recorder::RecordingSession>,
        heap_id: usize,
        heap_kind: HeapKind,
    ) -> Self {
        Self {
            allocation_id,
            recording_session,
            heap_id,
            heap_kind,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PendingTracking {
    allocation_id: usize,
    recording_session: seismograph::recorder::RecordingSession,
}

#[cfg(test)]
pub(crate) fn pending_tracking_for_test() -> PendingTracking {
    PendingTracking {
        allocation_id: usize::MAX,
        recording_session: seismograph::recorder::RecordingSession::from_raw(u64::MAX).unwrap(),
    }
}

#[inline(always)]
pub(crate) fn begin_allocation() -> Option<PendingTracking> {
    if !seismograph::recorder::recording_enabled_for(runtime_event::EventClass::Allocation) || telemetry_suppressed() {
        return None;
    }
    let allocation_id = NEXT_ALLOCATION_ID.fetch_add(1, Ordering::Relaxed);
    let recording_session = seismograph::recorder::select_object_for(
        runtime_event::EventClass::Allocation,
        runtime_event::ObjectId::new(allocation_id as u64),
    )?;
    Some(PendingTracking {
        allocation_id,
        recording_session,
    })
}

impl PendingTracking {
    pub(crate) fn commit(self, address: *mut u8, layout: Layout, heap_id: usize, heap_kind: HeapKind) -> TrackingAllocation {
        let recorded = seismograph::record_in_session_classified(self.recording_session, runtime_event::EventClass::Allocation, || {
            runtime_event::Record::allocation(runtime_alloc::Allocation {
                allocation_id: runtime_alloc::AllocationId::new(self.allocation_id as u64),
                event_thread_id: runtime_alloc::EventThreadId::new(crate::allocator::tracking_thread_token() as u64),
                heap_id: runtime_alloc::HeapId::new(heap_id as u64),
                heap_kind: encode_runtime_heap_kind(heap_kind),
                freed_after_heap_release: false,
                address: runtime_event::Address::from_ptr(address),
                size: layout.size() as u64,
                alignment: layout.align() as u64,
            })
        });
        if !recorded {
            return TrackingAllocation::NONE;
        }
        TrackingAllocation {
            allocation_id: self.allocation_id,
            recording_session: Some(self.recording_session),
            heap_id,
            heap_kind,
        }
    }
}

pub(crate) fn record_deallocation(allocation: TrackingAllocation, address: *mut u8, layout: Layout, freed_after_heap_release: bool) {
    if allocation.allocation_id == 0 {
        return;
    }
    let Some(recording_session) = allocation.recording_session else {
        return;
    };
    let _recorded = seismograph::record_in_session_classified(recording_session, runtime_event::EventClass::Allocation, || {
        runtime_event::Record::deallocation(runtime_alloc::Allocation {
            allocation_id: runtime_alloc::AllocationId::new(allocation.allocation_id as u64),
            event_thread_id: runtime_alloc::EventThreadId::new(crate::allocator::tracking_thread_token() as u64),
            heap_id: runtime_alloc::HeapId::new(allocation.heap_id as u64),
            heap_kind: encode_runtime_heap_kind(allocation.heap_kind),
            freed_after_heap_release,
            address: runtime_event::Address::from_ptr(address),
            size: layout.size() as u64,
            alignment: layout.align() as u64,
        })
    });
}

const fn encode_runtime_heap_kind(kind: HeapKind) -> runtime_alloc::HeapKind {
    match kind {
        HeapKind::General => runtime_alloc::HeapKind::General,
        HeapKind::Bump => runtime_alloc::HeapKind::Bump,
        HeapKind::Thread => runtime_alloc::HeapKind::Thread,
    }
}

struct TelemetrySuppressionGuard {
    previous: bool,
    depth_entered: bool,
}

impl TelemetrySuppressionGuard {
    fn enter() -> Self {
        let depth_entered = TELEMETRY_SUPPRESSION_DEPTH.try_with(|depth| depth.set(depth.get() + 1)).is_ok();
        Self {
            previous: enter_tracking_internal(),
            depth_entered,
        }
    }
}

impl Drop for TelemetrySuppressionGuard {
    fn drop(&mut self) {
        restore_tracking_internal(self.previous);
        if self.depth_entered {
            restore_suppression_depth();
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn restore_suppression_depth() {
    let _ = TELEMETRY_SUPPRESSION_DEPTH.try_with(|depth| depth.set(depth.get() - 1));
}

fn telemetry_suppressed() -> bool {
    seismograph::recorder::is_suppressed() || TELEMETRY_SUPPRESSION_DEPTH.try_with(|depth| depth.get() != 0).unwrap_or(true)
}

fn with_telemetry_suppressed<R>(operation: impl FnOnce() -> R) -> R {
    let _guard = TelemetrySuppressionGuard::enter();
    operation()
}

#[cfg(test)]
mod tests {
    use std::sync::{Barrier, PoisonError};
    use std::thread;

    use super::*;

    fn sample_stats() -> Stats {
        Stats {
            allocated_bytes: 101,
            deallocated_bytes: 41,
            live_bytes: 60,
            peak_live_bytes: 80,
            mapped_bytes: 256,
            os_mappings: 7,
            os_unmappings: 3,
            allocations: 11,
            deallocations: 5,
            remote_frees: 4,
            pending_remote_blocks: 3,
            remote_pushes_in_progress: 2,
            drained_remote_blocks: 1,
        }
    }

    fn no_encoded_len(_: &EncodedSnapshot) -> Option<usize> {
        None
    }

    fn one_encoded_byte(_: &EncodedSnapshot) -> Option<usize> {
        Some(1)
    }

    fn null_mapping(_: usize) -> *mut u8 {
        ptr::null_mut()
    }

    fn one_byte_mapping(len: usize) -> *mut u8 {
        crate::hal::map(len)
    }

    fn encode_successfully(_: &EncodedSnapshot, bytes: &mut [u8]) -> bool {
        bytes[0] = 0;
        true
    }

    fn fail_encoding(_: &EncodedSnapshot, _: &mut [u8]) -> bool {
        false
    }

    #[cfg(not(miri))]
    #[test]
    fn snapshot_arena_handles_chunk_reuse_dedicated_allocations_and_nested_deallocation() {
        let mut arena = SnapshotArena::new();
        hal::fail_next_map();
        assert!(arena.allocate(Layout::new::<u64>()).is_null());

        let small_layout = Layout::from_size_align(64, 64).unwrap();
        let first = arena.allocate(small_layout);
        let second = arena.allocate(small_layout);
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert_eq!(first.addr() % small_layout.align(), 0);
        assert_eq!(second.addr() % small_layout.align(), 0);

        let dedicated_layout = Layout::from_size_align(SNAPSHOT_ARENA_CHUNK_BYTES, 4_096).unwrap();
        let dedicated = arena.allocate(dedicated_layout);
        assert!(!dedicated.is_null());
        assert!(arena.deallocate(first));
        assert!(arena.deallocate(dedicated));
        assert!(!arena.deallocate(dedicated));
        assert!(!arena.deallocate(ptr::without_provenance_mut(1)));

        let mut child = SnapshotArena::new();
        child.parent = ptr::from_mut(&mut arena);
        let child_address = child.allocate(Layout::new::<u32>());
        assert!(!child_address.is_null());
        assert!(child.deallocate(second));
        assert!(child.deallocate(child_address));

        let chunk = arena.head;
        let original_cursor = unsafe { (*chunk).cursor };
        unsafe { (*chunk).cursor = usize::MAX };
        assert!(unsafe { allocate_from_snapshot_chunk(chunk, 1, 2) }.is_null());
        unsafe { (*chunk).cursor = usize::MAX - 1 };
        assert!(unsafe { allocate_from_snapshot_chunk(chunk, 2, 1) }.is_null());
        unsafe { (*chunk).cursor = (*chunk).mapping_bytes };
        assert!(unsafe { allocate_from_snapshot_chunk(chunk, 1, 1) }.is_null());
        unsafe { (*chunk).cursor = original_cursor };
    }

    #[test]
    fn active_snapshot_arena_routes_allocations_and_restores_nested_arenas() {
        let layout = Layout::new::<u64>();
        assert!(snapshot_arena_allocate(layout).is_none());
        assert!(!snapshot_arena_deallocate(ptr::without_provenance_mut(1)));

        with_snapshot_arena(|| {
            let outer = snapshot_arena_allocate(layout).unwrap();
            with_snapshot_arena(|| assert!(snapshot_arena_deallocate(outer)));
            let inner = with_snapshot_arena(|| snapshot_arena_allocate(layout).unwrap());
            assert!(!snapshot_arena_deallocate(inner));
        });

        assert!(snapshot_arena_allocate(layout).is_none());
    }

    #[test]
    fn snapshot_errors_report_stable_kinds_and_messages() {
        let errors = [
            (
                SnapshotError::sizing_failed(),
                SnapshotErrorKind::Sizing,
                "the telemetry snapshot length could not be calculated",
            ),
            (
                SnapshotError::allocation_failed(),
                SnapshotErrorKind::Allocation,
                "the telemetry snapshot mapping could not be allocated",
            ),
            (
                SnapshotError::encoding_failed(),
                SnapshotErrorKind::Encoding,
                "the telemetry snapshot could not be encoded",
            ),
        ];

        for (error, kind, message) in errors {
            assert_eq!(error.kind, kind);
            assert_eq!(error.to_string(), message);
            assert_eq!(format!("{error:?}"), format!("SnapshotError({message})"));
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn private_snapshot_encoders_cover_every_topology_kind() {
        let class = SizeClassSnapshot {
            class_index: 2,
            block_bytes: 64,
            live_allocations: Estimate::bounded(3, 2, 4),
            requested_bytes: Estimate::bounded(150, 120, 180),
            usable_bytes: Estimate::bounded(192, 128, 256),
        };
        assert_eq!(encode_estimate(class.live_allocations).value, 3);
        assert_eq!(encode_size_class(&class).block_bytes, 64);
        assert_eq!(encode_stats(sample_stats()).allocations, 11);
        assert_eq!(producer_version(), Version::new(0, 1, 0));

        let kinds = [
            PhysicalSliceKind::Unknown,
            PhysicalSliceKind::Small,
            PhysicalSliceKind::Medium,
            PhysicalSliceKind::MediumContinuation,
            PhysicalSliceKind::Bump,
        ];
        let region = RegionSnapshot {
            domain_id: 9,
            region_index: 4,
            base_address: 0x1000,
            reserved_bytes: 0x20_000,
            slice_bytes: 0x1_000,
            used_slices: kinds.len(),
            free_slices: 2,
            used_bitmap: vec![0x1f],
            slices: kinds
                .into_iter()
                .enumerate()
                .map(|(slice_index, kind)| PhysicalSliceSnapshot {
                    slice_index,
                    kind,
                    span_slices: 1,
                    owner: 17,
                    requested_bytes: 20,
                    usable_bytes: 32,
                    segments: vec![PhysicalSegmentSnapshot {
                        segment_index: 1,
                        class_index: 2,
                        context: true,
                        live_blocks: 3,
                        usable_blocks: 4,
                        utilization_tracked: true,
                    }],
                })
                .collect(),
        };
        assert_eq!(encode_region(&region).region_index, 4);
        let topology = encode_topology_region(&region);
        assert_eq!(topology.slices.len(), 5);
        assert_eq!(topology.slices[0].kind, EncodedSliceKind::Unknown);
        assert_eq!(topology.slices[1].kind, EncodedSliceKind::Small);
        assert_eq!(topology.slices[2].kind, EncodedSliceKind::Medium);
        assert_eq!(topology.slices[3].kind, EncodedSliceKind::MediumContinuation);
        assert_eq!(topology.slices[4].kind, EncodedSliceKind::Bump);
        assert_eq!(topology.slices[0].segments[0].live_blocks, 3);

        let domains = encode_domains(
            &[
                DomainSnapshot {
                    domain_id: 9,
                    is_default: true,
                },
                DomainSnapshot {
                    domain_id: 10,
                    is_default: false,
                },
            ],
            &[region],
        );
        assert_eq!(domains[0].small_slices, 1);
        assert_eq!(domains[0].medium_slices, 2);
        assert_eq!(domains[0].bump_slices, 1);
        assert_eq!(domains[0].unknown_slices, 1);
        assert_eq!(domains[1].region_count, 0);
    }

    #[test]
    fn caller_encoding_and_address_details_are_deterministic() {
        static STACK_MARKER: [u8; 1] = [0];
        static RUNTIME_STACK_MARKER: [u8; 1] = [1];
        let stack_address = (&raw const STACK_MARKER).addr();
        let runtime_stack_address = (&raw const RUNTIME_STACK_MARKER).addr();
        let callers = CallerSnapshot {
            session_id: 3,
            total_events: 2,
            lost_events: 0,
            threads: vec![ThreadLog {
                thread_log_id: 7,
                total_events: 2,
                lost_events: 0,
                allocated_histogram: vec![0, 1],
                live_histogram: vec![0, 0],
            }],
            events: vec![
                Event {
                    thread_log_id: 7,
                    event_thread_id: 7,
                    sequence: 1,
                    allocation_id: 8,
                    kind: EventKind::Allocated,
                    heap_id: 4,
                    heap_kind: HeapKind::General,
                    freed_after_heap_release: false,
                    address: 0x1000,
                    size: 16,
                    align: 8,
                    call_stack: vec![Ip(0), Ip(stack_address)],
                },
                Event {
                    thread_log_id: 7,
                    event_thread_id: 9,
                    sequence: 2,
                    allocation_id: 8,
                    kind: EventKind::Deallocated,
                    heap_id: 4,
                    heap_kind: HeapKind::General,
                    freed_after_heap_release: false,
                    address: 0x1000,
                    size: 16,
                    align: 8,
                    call_stack: Vec::new(),
                },
            ],
            thread_names: vec![
                ThreadName {
                    thread_id: 7,
                    name: "allocator".to_owned(),
                },
                ThreadName {
                    thread_id: 9,
                    name: "reclaimer".to_owned(),
                },
            ],
        };
        let encoded = encode_callers(&callers);
        assert_eq!(encoded.events[0].kind, EncodedEventKind::Allocated);
        assert_eq!(encoded.events[1].kind, EncodedEventKind::Deallocated);
        assert_eq!(encoded.events[0].call_stack.len(), 2);

        let runtime_events = runtime_event::Events {
            clock: runtime_event::EventClock::ProcessMonotonic,
            total_events: 1,
            lost_events: 0,
            recording: seismograph::recorder::RecordingPolicies::default(),
            threads: Vec::new(),
            events: vec![runtime_event::Event {
                thread_id: runtime_thread::ThreadId::new(7),
                sequence: runtime_event::EventSequence::new(1),
                timestamp: runtime_event::EventTimestamp::from_ticks(1),
                kind: runtime_event::EventKind::ArcDeref,
                payload: runtime_event::EventPayload::Object(runtime_event::ObjectId::new(0x1000)),
                call_stack: vec![runtime_event::Address::new(runtime_stack_address as u64)],
            }],
        };
        let addresses = resolve_addresses(Some(&callers), Some(&runtime_events));
        assert_eq!(addresses.len(), 2);
        assert!(addresses.iter().any(|lookup| lookup.address == stack_address as u64));
        assert!(addresses.iter().any(|lookup| lookup.address == runtime_stack_address as u64));

        let mut lookup = EncodedAddressLookup::from_fields(EncodedAddressLookupFields {
            address: 0,
            symbol: None,
            filename: None,
            line: None,
            column: None,
        });
        merge_address_lookup(&mut lookup, Some("first".into()), Some("first.rs".into()), Some(10), Some(20));
        merge_address_lookup(&mut lookup, Some("second".into()), Some("second.rs".into()), Some(30), Some(40));
        assert_eq!(lookup.symbol.as_deref(), Some("first"));
        assert_eq!(lookup.filename.as_deref(), Some("first.rs"));
        assert_eq!(lookup.line, Some(10));
        assert_eq!(lookup.column, Some(20));
    }

    #[test]
    fn suppression_and_counter_recorders_cover_disabled_paths() {
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(!telemetry_suppressed());
        with_telemetry_suppressed(|| {
            assert!(telemetry_suppressed());
            record_allocation(1);
            record_deallocation_stats(1);
            record_small_allocation(0, 8, 1);
            record_small_deallocation(0, 1);
            record_remote_retired_free();
        });
        assert!(!telemetry_suppressed());

        AGGREGATES_AVAILABLE.store(false, Ordering::Release);
        assert!(stats().is_none());
        let _ = try_snapshot_with_runtime_events(None, true);
        assert_eq!(snapshot_stats(None), Stats::default());
        assert_eq!(snapshot_stats(Some(sample_stats())), sample_stats());
        assert_eq!(histogram_bucket(0), 0);

        record_deallocation(
            TrackingAllocation {
                allocation_id: 1,
                recording_session: None,
                heap_id: 0,
                heap_kind: HeapKind::General,
            },
            ptr::without_provenance_mut(1),
            Layout::new::<u8>(),
            false,
        );
    }

    #[test]
    fn allocation_tracking_is_preselected_by_object_sampling() {
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy {
                enabled: true,
                event_sampling: seismograph::recorder::EventSampling::one_in(100).unwrap(),
                ..Default::default()
            },
            ..Default::default()
        });

        let selected = (0..10_000).filter(|_| begin_allocation().is_some()).count();

        assert!((70..=130).contains(&selected), "selected {selected} allocations");
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }

    #[test]
    fn pending_allocation_is_discarded_when_its_recording_session_ends() {
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let pending = begin_allocation().unwrap();
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy {
                enabled: true,
                event_sampling: seismograph::recorder::EventSampling::one_in(20).unwrap(),
                ..Default::default()
            },
            ..Default::default()
        });

        let tracking = pending.commit(ptr::without_provenance_mut(16), Layout::new::<u64>(), 7, HeapKind::General);

        assert_eq!(tracking.allocation_id(), 0);
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }

    #[test]
    fn deallocation_is_not_recorded_after_its_recording_session_ends() {
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let layout = Layout::new::<u64>();
        let address = ptr::without_provenance_mut(16);
        let tracking = begin_allocation().unwrap().commit(address, layout, 7, HeapKind::General);
        assert_ne!(tracking.allocation_id(), 0);
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy {
                enabled: true,
                event_sampling: seismograph::recorder::EventSampling::one_in(20).unwrap(),
                ..Default::default()
            },
            ..Default::default()
        });

        record_deallocation(tracking, address, layout, false);
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions {
            event_buffers: seismograph::snapshot::EventBufferDisposition::Release,
        })
        .unwrap();
        let events = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap().events;

        assert!(events.events.is_empty());
        seismograph::recorder(seismograph::recorder::Configuration::default());
        begin_remote_free();
        finish_remote_free();
        record_remote_drain();

        record_mapping(8);
        record_bump_commit(4);
        record_bump_decommit(4);
        record_allocation(3);
        record_deallocation_stats(3);
        record_small_allocation(0, 8, 3);
        record_small_deallocation(0, 3);
        begin_remote_free();
        finish_remote_free();
        record_remote_retired_free();
        record_remote_drain();
        record_unmapping(8);
        assert!(stats().is_some());
        assert!(aggregate_snapshot().is_some());
    }

    #[cfg(not(miri))]
    #[test]
    fn aggregate_helpers_cover_shared_fallback_and_registration_retry_paths() {
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let shared = AggregateShard::new(true);
        add_owner(&shared, &shared.allocations, 2);
        assert_eq!(shared.allocations.load(Ordering::Relaxed), 2);

        hal::fail_next_map();
        assert!(ptr::eq(initialize_aggregate_shard(), &raw const FALLBACK_AGGREGATE_SHARD));

        let barrier = Arc::new(Barrier::new(2));
        let workers = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    set_aggregate_registration_barrier(barrier);
                    register_aggregate_shard(Box::into_raw(Box::new(AggregateShard::new(false))));
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn aggregate_shards_merge_cross_thread_deallocations() {
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let size = 2_048;
        let stats_before = stats().unwrap_or_default();

        record_allocation(size);
        std::thread::spawn(move || record_deallocation_stats(size)).join().unwrap();

        let stats_after = stats().unwrap();
        assert_eq!(stats_after.allocated_bytes, stats_before.allocated_bytes + size);
        assert_eq!(stats_after.deallocated_bytes, stats_before.deallocated_bytes + size);
        assert_eq!(stats_after.live_bytes, stats_before.live_bytes);
    }

    #[test]
    fn snapshot_debug_reports_encoded_size() {
        let encoded = EncodedSnapshot::new(producer_version());
        let missing_len = encode_snapshot_with(&encoded, no_encoded_len, null_mapping, encode_successfully);
        assert!(missing_len.is_none());
        let failed_map = encode_snapshot_with(&encoded, one_encoded_byte, null_mapping, encode_successfully);
        assert!(failed_map.is_none());
        let failed_encode = encode_snapshot_with(&encoded, one_encoded_byte, one_byte_mapping, fail_encoding);
        assert!(failed_encode.is_none());

        let snapshot = encode_snapshot_with(&encoded, one_encoded_byte, one_byte_mapping, encode_successfully).unwrap();
        assert_eq!(snapshot.as_bytes().len(), 1);
        assert_eq!(format!("{snapshot:?}"), "Snapshot { bytes: 1, .. }");
        #[cfg(not(miri))]
        {
            let path = "telemetry-unit-snapshot.bin";
            std::fs::write(path, snapshot.as_bytes()).unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }
}
