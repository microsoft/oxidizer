// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation telemetry, process-wide event tracking, and deferred stack resolution.

use std::alloc::Layout;
#[cfg(test)]
use std::cell::RefCell;
use std::cell::{Cell, UnsafeCell};
#[cfg(all(not(miri), feature = "caller-symbolization"))]
use std::ffi::c_void;
use std::hint::spin_loop;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use std::{io, ptr};

use rallocator_telemetry::callers::{
    AddressLookup as EncodedAddressLookup, AddressLookupFields as EncodedAddressLookupFields, Callers as EncodedCallers,
    CallersFields as EncodedCallersFields, Event as EncodedEvent, EventFields as EncodedEventFields, EventKind as EncodedEventKind,
    HeapKind as EncodedHeapKind, ThreadLog as EncodedThreadLog, ThreadLogFields as EncodedThreadLogFields, ThreadName as EncodedThreadName,
    ThreadNameFields as EncodedThreadNameFields,
};
use rallocator_telemetry::snapshot::{
    Domain as EncodedDomain, DomainFields as EncodedDomainFields, Estimate as EncodedEstimate, EstimateFields as EncodedEstimateFields,
    Histograms as EncodedHistograms, HistogramsFields as EncodedHistogramsFields, Region as EncodedRegion,
    RegionFields as EncodedRegionFields, SizeClass as EncodedSizeClass, SizeClassFields as EncodedSizeClassFields,
    Snapshot as EncodedSnapshot, Stats as EncodedStats, StatsFields as EncodedStatsFields, Version,
};
use rallocator_telemetry::topology::{
    Segment as EncodedSegment, SegmentFields as EncodedSegmentFields, Slice as EncodedSlice, SliceFields as EncodedSliceFields,
    SliceKind as EncodedSliceKind, TopologyRegion as EncodedTopologyRegion, TopologyRegionFields as EncodedTopologyRegionFields,
};

use crate::allocator::{enter_tracking_internal, restore_tracking_internal, tracking_target};
use crate::config::Config;
use crate::hal;
use crate::tunables::MAX_SIZE_CLASSES;

/// A value with deterministic lower and upper bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Estimate<T> {
    value: T,
    lower_bound: T,
    upper_bound: T,
}

impl<T: Copy> Estimate<T> {
    pub const fn value(&self) -> T {
        self.value
    }

    pub const fn lower_bound(&self) -> T {
        self.lower_bound
    }

    pub const fn upper_bound(&self) -> T {
        self.upper_bound
    }
}

impl<T: Copy + PartialEq> Estimate<T> {
    pub fn is_exact(&self) -> bool {
        self.lower_bound == self.upper_bound
    }
}

impl<T: Copy> Estimate<T> {
    const fn exact(value: T) -> Self {
        Self {
            value,
            lower_bound: value,
            upper_bound: value,
        }
    }

    const fn bounded(value: T, lower_bound: T, upper_bound: T) -> Self {
        Self {
            value,
            lower_bound,
            upper_bound,
        }
    }
}

/// Cheap aggregate statistics collected by a telemetry-enabled allocator.
///
/// Fields are read from independent atomic counters. A value is memory-safe
/// and individually valid, but it is not a transactional process-wide snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
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

/// Process memory totals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryStats {
    live_requested_bytes: Estimate<usize>,
    live_usable_bytes: Estimate<usize>,
    mapped_bytes: usize,
    peak_live_bytes: usize,
}

/// Process allocation operation totals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationStats {
    allocations: usize,
    deallocations: usize,
    allocated_bytes: usize,
    deallocated_bytes: usize,
}

/// Operating-system mapping and reclamation totals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReclamationStats {
    mapped_bytes: usize,
    mappings: usize,
    unmappings: usize,
}

/// Cross-thread free totals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteStats {
    frees: usize,
    pending_blocks: Estimate<usize>,
    drained_blocks: usize,
}

/// Cumulative counter changes over one sampling interval.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatsDelta {
    allocated_bytes: usize,
    deallocated_bytes: usize,
    allocations: usize,
    deallocations: usize,
    mappings: usize,
    unmappings: usize,
    remote_frees: usize,
    drained_remote_blocks: usize,
}

/// One interval sample from a [`Sampler`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    elapsed: std::time::Duration,
    current: Stats,
    delta: StatsDelta,
}

/// Converts cumulative telemetry counters into interval samples.
///
/// Samples inherit [`Stats`]'s non-transactional consistency contract.
#[derive(Clone, Copy, Debug)]
pub struct Sampler {
    sampled_at: Instant,
    previous: Stats,
}

/// A time-bounded telemetry collection.
///
/// Reports inherit [`Stats`]'s non-transactional consistency contract.
#[derive(Debug)]
pub struct Session {
    started_at: Instant,
    sampler: Sampler,
    baseline: Stats,
}

/// Final counters collected during a [`Session`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionReport {
    elapsed: std::time::Duration,
    initial: Stats,
    final_stats: Stats,
    delta: StatsDelta,
}

/// Stable category of a snapshot capture error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SnapshotErrorKind {
    /// Neither aggregate nor caller telemetry is available.
    Unavailable,
    /// The encoded snapshot length could not be calculated.
    SizingFailed,
    /// Memory for the encoded snapshot could not be mapped.
    AllocationFailed,
    /// Encoding into the mapped snapshot buffer failed.
    EncodingFailed,
}

/// An error reported while capturing a telemetry snapshot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SnapshotError {
    kind: SnapshotErrorKind,
}

impl SnapshotError {
    const fn unavailable() -> Self {
        Self {
            kind: SnapshotErrorKind::Unavailable,
        }
    }

    const fn sizing_failed() -> Self {
        Self {
            kind: SnapshotErrorKind::SizingFailed,
        }
    }

    const fn allocation_failed() -> Self {
        Self {
            kind: SnapshotErrorKind::AllocationFailed,
        }
    }

    const fn encoding_failed() -> Self {
        Self {
            kind: SnapshotErrorKind::EncodingFailed,
        }
    }

    /// Returns the stable category of this error.
    #[must_use]
    pub const fn kind(self) -> SnapshotErrorKind {
        self.kind
    }
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            SnapshotErrorKind::Unavailable => formatter.write_str("allocator telemetry is unavailable"),
            SnapshotErrorKind::SizingFailed => formatter.write_str("the telemetry snapshot length could not be calculated"),
            SnapshotErrorKind::AllocationFailed => formatter.write_str("the telemetry snapshot mapping could not be allocated"),
            SnapshotErrorKind::EncodingFailed => formatter.write_str("the telemetry snapshot could not be encoded"),
        }
    }
}

impl std::fmt::Debug for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "SnapshotError({self})")
    }
}

impl std::error::Error for SnapshotError {}

impl Stats {
    #[must_use]
    pub const fn memory(&self) -> MemoryStats {
        MemoryStats {
            live_requested_bytes: Estimate::exact(self.live_bytes),
            live_usable_bytes: Estimate::bounded(self.live_bytes, self.live_bytes, self.mapped_bytes),
            mapped_bytes: self.mapped_bytes,
            peak_live_bytes: self.peak_live_bytes,
        }
    }

    #[must_use]
    pub const fn operations(&self) -> OperationStats {
        OperationStats {
            allocations: self.allocations,
            deallocations: self.deallocations,
            allocated_bytes: self.allocated_bytes,
            deallocated_bytes: self.deallocated_bytes,
        }
    }

    #[must_use]
    pub const fn reclamation(&self) -> ReclamationStats {
        ReclamationStats {
            mapped_bytes: self.mapped_bytes,
            mappings: self.os_mappings,
            unmappings: self.os_unmappings,
        }
    }

    #[must_use]
    pub const fn remote(&self) -> RemoteStats {
        RemoteStats {
            frees: self.remote_frees,
            pending_blocks: Estimate::bounded(
                self.pending_remote_blocks,
                self.pending_remote_blocks.saturating_sub(self.remote_pushes_in_progress),
                self.pending_remote_blocks,
            ),
            drained_blocks: self.drained_remote_blocks,
        }
    }
}

impl MemoryStats {
    #[must_use]
    pub const fn live_requested_bytes(&self) -> Estimate<usize> {
        self.live_requested_bytes
    }

    #[must_use]
    pub const fn live_usable_bytes(&self) -> Estimate<usize> {
        self.live_usable_bytes
    }

    #[must_use]
    pub const fn mapped_bytes(&self) -> usize {
        self.mapped_bytes
    }

    /// Returns mapped bytes.
    ///
    /// This compatibility alias predates the more accurate [`MemoryStats::mapped_bytes`] name.
    #[must_use]
    pub const fn committed_bytes(&self) -> usize {
        self.mapped_bytes
    }

    #[must_use]
    pub const fn peak_live_bytes(&self) -> usize {
        self.peak_live_bytes
    }
}

impl OperationStats {
    #[must_use]
    pub const fn allocations(&self) -> usize {
        self.allocations
    }

    #[must_use]
    pub const fn deallocations(&self) -> usize {
        self.deallocations
    }

    #[must_use]
    pub const fn allocated_bytes(&self) -> usize {
        self.allocated_bytes
    }

    #[must_use]
    pub const fn deallocated_bytes(&self) -> usize {
        self.deallocated_bytes
    }
}

impl ReclamationStats {
    #[must_use]
    pub const fn mapped_bytes(&self) -> usize {
        self.mapped_bytes
    }

    /// Returns mapped bytes.
    ///
    /// This compatibility alias predates the more accurate
    /// [`ReclamationStats::mapped_bytes`] name.
    #[must_use]
    pub const fn committed_bytes(&self) -> usize {
        self.mapped_bytes
    }

    #[must_use]
    pub const fn mappings(&self) -> usize {
        self.mappings
    }

    #[must_use]
    pub const fn unmappings(&self) -> usize {
        self.unmappings
    }
}

impl RemoteStats {
    #[must_use]
    pub const fn frees(&self) -> usize {
        self.frees
    }

    #[must_use]
    pub const fn pending_blocks(&self) -> Estimate<usize> {
        self.pending_blocks
    }

    #[must_use]
    pub const fn drained_blocks(&self) -> usize {
        self.drained_blocks
    }
}

impl StatsDelta {
    fn between(initial: Stats, current: Stats) -> Self {
        Self {
            allocated_bytes: current.allocated_bytes.saturating_sub(initial.allocated_bytes),
            deallocated_bytes: current.deallocated_bytes.saturating_sub(initial.deallocated_bytes),
            allocations: current.allocations.saturating_sub(initial.allocations),
            deallocations: current.deallocations.saturating_sub(initial.deallocations),
            mappings: current.os_mappings.saturating_sub(initial.os_mappings),
            unmappings: current.os_unmappings.saturating_sub(initial.os_unmappings),
            remote_frees: current.remote_frees.saturating_sub(initial.remote_frees),
            drained_remote_blocks: current.drained_remote_blocks.saturating_sub(initial.drained_remote_blocks),
        }
    }

    #[must_use]
    pub const fn allocated_bytes(&self) -> usize {
        self.allocated_bytes
    }

    #[must_use]
    pub const fn deallocated_bytes(&self) -> usize {
        self.deallocated_bytes
    }

    #[must_use]
    pub const fn allocations(&self) -> usize {
        self.allocations
    }

    #[must_use]
    pub const fn deallocations(&self) -> usize {
        self.deallocations
    }

    #[must_use]
    pub const fn mappings(&self) -> usize {
        self.mappings
    }

    #[must_use]
    pub const fn unmappings(&self) -> usize {
        self.unmappings
    }

    #[must_use]
    pub const fn remote_frees(&self) -> usize {
        self.remote_frees
    }

    #[must_use]
    pub const fn drained_remote_blocks(&self) -> usize {
        self.drained_remote_blocks
    }
}

impl Sample {
    #[must_use]
    pub const fn elapsed(&self) -> std::time::Duration {
        self.elapsed
    }

    #[must_use]
    pub const fn current(&self) -> &Stats {
        &self.current
    }

    #[must_use]
    pub const fn delta(&self) -> &StatsDelta {
        &self.delta
    }
}

impl Sampler {
    #[must_use]
    pub fn new() -> Option<Self> {
        Self::new_at(Instant::now())
    }

    fn new_at(sampled_at: Instant) -> Option<Self> {
        Some(Self {
            sampled_at,
            previous: stats()?,
        })
    }

    pub fn sample(&mut self) -> Option<Sample> {
        self.sample_at(Instant::now())
    }

    fn sample_at(&mut self, sampled_at: Instant) -> Option<Sample> {
        let current = stats()?;
        let sample = Sample {
            elapsed: sampled_at.duration_since(self.sampled_at),
            current,
            delta: StatsDelta::between(self.previous, current),
        };
        self.sampled_at = sampled_at;
        self.previous = current;
        Some(sample)
    }
}

impl Session {
    #[must_use]
    pub fn start() -> Option<Self> {
        Self::start_at(Instant::now())
    }

    fn start_at(started_at: Instant) -> Option<Self> {
        let baseline = stats()?;
        Some(Self {
            started_at,
            sampler: Sampler {
                sampled_at: started_at,
                previous: baseline,
            },
            baseline,
        })
    }

    pub fn sample(&mut self) -> Option<Sample> {
        self.sampler.sample()
    }

    #[must_use]
    pub fn finish(self) -> Option<SessionReport> {
        self.finish_at(Instant::now())
    }

    fn finish_at(self, finished_at: Instant) -> Option<SessionReport> {
        let final_stats = stats()?;
        Some(SessionReport {
            elapsed: finished_at.duration_since(self.started_at),
            initial: self.baseline,
            final_stats,
            delta: StatsDelta::between(self.baseline, final_stats),
        })
    }
}

impl SessionReport {
    #[must_use]
    pub const fn elapsed(&self) -> std::time::Duration {
        self.elapsed
    }

    #[must_use]
    pub const fn initial(&self) -> &Stats {
        &self.initial
    }

    #[must_use]
    pub const fn final_stats(&self) -> &Stats {
        &self.final_stats
    }

    #[must_use]
    pub const fn delta(&self) -> &StatsDelta {
        &self.delta
    }
}

/// Maximum number of instruction pointers captured for one allocation.
pub(crate) const MAX_TRACKED_STACK_FRAMES: usize = 24;

#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn validate_config<C: Config>() {
    assert!(
        C::CALLER_EVENT_CAPACITY != 0 && C::CALLER_EVENT_CAPACITY.is_power_of_two(),
        "caller event capacity must be a nonzero power of two"
    );
    assert!(
        C::CALLER_ALLOCATION_STACK_FRAMES <= MAX_TRACKED_STACK_FRAMES,
        "caller allocation stack depth cannot exceed 24"
    );
    assert!(
        C::CALLER_DEALLOCATION_STACK_FRAMES <= MAX_TRACKED_STACK_FRAMES,
        "caller deallocation stack depth cannot exceed 24"
    );
}

static ACTIVE_SESSION: AtomicUsize = AtomicUsize::new(0);
static LAST_SESSION: AtomicUsize = AtomicUsize::new(0);
static NEXT_SESSION: AtomicUsize = AtomicUsize::new(1);
static AGGREGATES_AVAILABLE: AtomicBool = AtomicBool::new(false);
static CALLERS_AVAILABLE: AtomicBool = AtomicBool::new(false);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
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
static NEXT_THREAD_LOG_ID: AtomicUsize = AtomicUsize::new(1);
static REGISTRY: OnceLock<Mutex<Vec<Arc<TrackingState>>>> = OnceLock::new();
static AGGREGATE_REGISTRY: AtomicPtr<AggregateShard> = AtomicPtr::new(ptr::null_mut());
static FALLBACK_AGGREGATE_REGISTERED: AtomicBool = AtomicBool::new(false);
static FALLBACK_AGGREGATE_SHARD: AggregateShard = AggregateShard::new(true);
static THREAD_NAMES: OnceLock<Mutex<Vec<ThreadName>>> = OnceLock::new();
static CONTROL: Mutex<()> = Mutex::new(());
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());
thread_local! {
    static TELEMETRY_SUPPRESSION_DEPTH: Cell<usize> = const { Cell::new(0) };
    static AGGREGATE_SHARD: Cell<*const AggregateShard> = const { Cell::new(ptr::null()) };
    #[cfg(test)]
    static AGGREGATE_REGISTRATION_BARRIER: RefCell<Option<Arc<std::sync::Barrier>>> = const { RefCell::new(None) };
}

#[repr(C, align(64))]
struct AggregateShard {
    next: AtomicPtr<Self>,
    shared: bool,
    allocated_bytes: AtomicUsize,
    deallocated_bytes: AtomicUsize,
    allocations: AtomicUsize,
    deallocations: AtomicUsize,
    class_block_bytes: [AtomicUsize; MAX_SIZE_CLASSES],
    class_allocations: [AtomicUsize; MAX_SIZE_CLASSES],
    class_deallocations: [AtomicUsize; MAX_SIZE_CLASSES],
    class_allocated_bytes: [AtomicUsize; MAX_SIZE_CLASSES],
    class_deallocated_bytes: [AtomicUsize; MAX_SIZE_CLASSES],
    size_allocations: [AtomicUsize; HISTOGRAM_BUCKETS],
    size_live: [AtomicIsize; HISTOGRAM_BUCKETS],
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
            class_block_bytes: [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES],
            class_allocations: [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES],
            class_deallocations: [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES],
            class_allocated_bytes: [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES],
            class_deallocated_bytes: [const { AtomicUsize::new(0) }; MAX_SIZE_CLASSES],
            size_allocations: [const { AtomicUsize::new(0) }; HISTOGRAM_BUCKETS],
            size_live: [const { AtomicIsize::new(0) }; HISTOGRAM_BUCKETS],
        }
    }
}

#[derive(Clone)]
struct AggregateSnapshot {
    allocated_bytes: usize,
    deallocated_bytes: usize,
    allocations: usize,
    deallocations: usize,
    class_block_bytes: [usize; MAX_SIZE_CLASSES],
    class_allocations: [usize; MAX_SIZE_CLASSES],
    class_deallocations: [usize; MAX_SIZE_CLASSES],
    class_allocated_bytes: [usize; MAX_SIZE_CLASSES],
    class_deallocated_bytes: [usize; MAX_SIZE_CLASSES],
    size_allocations: [usize; HISTOGRAM_BUCKETS],
    size_live: [isize; HISTOGRAM_BUCKETS],
}

impl AggregateSnapshot {
    const fn new() -> Self {
        Self {
            allocated_bytes: 0,
            deallocated_bytes: 0,
            allocations: 0,
            deallocations: 0,
            class_block_bytes: [0; MAX_SIZE_CLASSES],
            class_allocations: [0; MAX_SIZE_CLASSES],
            class_deallocations: [0; MAX_SIZE_CLASSES],
            class_allocated_bytes: [0; MAX_SIZE_CLASSES],
            class_deallocated_bytes: [0; MAX_SIZE_CLASSES],
            size_allocations: [0; HISTOGRAM_BUCKETS],
            size_live: [0; HISTOGRAM_BUCKETS],
        }
    }
}

/// Returns aggregate statistics when they were compiled into the allocator.
#[must_use]
pub fn stats() -> Option<Stats> {
    aggregate_snapshot().map(|aggregates| aggregate_stats(&aggregates))
}

fn aggregate_stats(aggregates: &AggregateSnapshot) -> Stats {
    Stats {
        allocated_bytes: aggregates.allocated_bytes,
        deallocated_bytes: aggregates.deallocated_bytes,
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
        peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed),
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
        for class_index in 0..MAX_SIZE_CLASSES {
            snapshot.class_block_bytes[class_index] =
                snapshot.class_block_bytes[class_index].max(shard.class_block_bytes[class_index].load(Ordering::Relaxed));
            snapshot.class_allocations[class_index] =
                snapshot.class_allocations[class_index].wrapping_add(shard.class_allocations[class_index].load(Ordering::Relaxed));
            snapshot.class_deallocations[class_index] =
                snapshot.class_deallocations[class_index].wrapping_add(shard.class_deallocations[class_index].load(Ordering::Relaxed));
            snapshot.class_allocated_bytes[class_index] =
                snapshot.class_allocated_bytes[class_index].wrapping_add(shard.class_allocated_bytes[class_index].load(Ordering::Relaxed));
            snapshot.class_deallocated_bytes[class_index] = snapshot.class_deallocated_bytes[class_index]
                .wrapping_add(shard.class_deallocated_bytes[class_index].load(Ordering::Relaxed));
        }
        for bucket in 0..HISTOGRAM_BUCKETS {
            snapshot.size_allocations[bucket] =
                snapshot.size_allocations[bucket].wrapping_add(shard.size_allocations[bucket].load(Ordering::Relaxed));
            snapshot.size_live[bucket] = snapshot.size_live[bucket].wrapping_add(shard.size_live[bucket].load(Ordering::Relaxed));
        }
        current = shard.next.load(Ordering::Acquire);
    }
    Some(snapshot)
}

/// Globally enables or disables caller tracking for the process.
///
/// Caller tracking has no effect unless the global allocator's [`crate::config::Config`]
/// sets `TRACK_CALLERS` to `true`. The toggle affects every thread, not only the
/// caller. Enabling after a disable starts a new session; the previously
/// completed session remains available to [`snapshot`] until another session
/// starts. Threads observe a toggle at their next allocation, while an
/// allocation racing the toggle may belong to either adjacent session.
pub fn track_callers(enabled: bool) {
    let _control = lock_control();
    with_telemetry_suppressed(|| {
        if enabled {
            if ACTIVE_SESSION.load(Ordering::Acquire) != 0 {
                return;
            }
            let _ = registry();
            let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
            lock_registry().clear();
            LAST_SESSION.store(session, Ordering::Release);
            ACTIVE_SESSION.store(session, Ordering::Release);
        } else {
            ACTIVE_SESSION.store(0, Ordering::Release);
        }
        crate::allocator::invalidate_tracking_cache();
    });
}

/// Captures process statistics, allocator structure, and retained caller events.
#[must_use]
pub fn snapshot() -> Option<Snapshot> {
    try_snapshot().ok()
}

/// Captures process statistics, allocator structure, and retained caller events.
///
/// # Errors
///
/// Returns a typed error when telemetry is unavailable or snapshot allocation,
/// sizing, or encoding fails.
pub fn try_snapshot() -> Result<Snapshot, SnapshotError> {
    with_telemetry_suppressed(|| {
        let started_at = Instant::now();
        let aggregates_before = aggregate_snapshot();
        let regions = crate::allocator::telemetry_region_snapshots();
        let domains = crate::allocator::telemetry_domain_snapshots();
        let aggregates_after = aggregate_snapshot();
        let stats = aggregates_after.as_ref().map(aggregate_stats);
        let size_classes = match (&aggregates_before, &aggregates_after) {
            (Some(before), Some(after)) => size_class_snapshots(before, after),
            _ => Vec::new(),
        };
        let callers = caller_snapshot();
        let stats = snapshot_stats(stats, callers.is_some()).ok_or(SnapshotError::unavailable())?;
        let mut encoded = EncodedSnapshot::new(producer_version());
        encoded.stats = encode_stats(stats);
        encoded.size_classes = size_classes.iter().map(encode_size_class).collect();
        encoded.regions = regions.iter().map(encode_region).collect();
        encoded.topology = regions.iter().map(encode_topology_region).collect();
        encoded.domains = encode_domains(&domains, &regions);
        encoded.callers = callers.as_ref().map(encode_callers);
        encoded.histograms = encode_histograms(aggregates_after.as_ref());
        encoded.addresses = callers.as_ref().map(resolve_addresses).unwrap_or_default();
        encoded.metadata.capture_duration_nanos = u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX);

        let len = rallocator_telemetry::encoded_len(&encoded).map_err(|_error| SnapshotError::sizing_failed())?;
        let address = NonNull::new(crate::hal::map(len)).ok_or(SnapshotError::allocation_failed())?;
        let mut mapping = MappedBytes { address, len };
        rallocator_telemetry::encode(&encoded, mapping.as_mut_slice()).map_err(|_error| SnapshotError::encoding_failed())?;
        Ok(Snapshot { mapping })
    })
}

fn snapshot_stats(stats: Option<Stats>, has_callers: bool) -> Option<Stats> {
    match stats {
        Some(stats) => Some(stats),
        None if has_callers => Some(Stats::default()),
        None => None,
    }
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

fn encode_histograms(aggregates: Option<&AggregateSnapshot>) -> EncodedHistograms {
    let empty = AggregateSnapshot::new();
    let aggregates = aggregates.unwrap_or(&empty);
    EncodedHistograms::from_fields(EncodedHistogramsFields {
        allocated: aggregates.size_allocations.iter().map(|count| *count as u64).collect(),
        live: aggregates.size_live.iter().map(|count| (*count).max(0) as u64).collect(),
    })
}

fn resolve_addresses(callers: &CallerSnapshot) -> Vec<EncodedAddressLookup> {
    let mut addresses = callers
        .events
        .iter()
        .flat_map(|event| event.call_stack.iter())
        .map(|ip| ip.0)
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
                backtrace::resolve(address as *mut c_void, |symbol| {
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

fn caller_snapshot() -> Option<CallerSnapshot> {
    if !CALLERS_AVAILABLE.load(Ordering::Acquire) {
        return None;
    }
    let _control = lock_control();
    let session_id = LAST_SESSION.load(Ordering::Acquire);
    Some(with_telemetry_suppressed(|| {
        let logs = lock_registry();
        let mut threads = Vec::new();
        let mut events = Vec::new();
        let mut total_events = 0;
        let mut lost_events = 0;

        for log in logs.iter().filter(|log| log.session_id == session_id) {
            let snapshot = log.snapshot();
            total_events += snapshot.total_events;
            lost_events += snapshot.lost_events;
            events.extend(snapshot.events);
            threads.push(ThreadLog {
                thread_log_id: log.thread_log_id,
                total_events: snapshot.total_events,
                lost_events: snapshot.lost_events,
                allocated_histogram: snapshot.allocated_histogram,
                live_histogram: snapshot.live_histogram,
            });
        }

        events.sort_unstable_by_key(|event| (event.thread_log_id, event.sequence));
        let event_thread_ids = events
            .iter()
            .map(|event| event.event_thread_id)
            .collect::<std::collections::HashSet<_>>();
        let thread_names = lock_thread_names()
            .iter()
            .filter(|thread| event_thread_ids.contains(&thread.thread_id))
            .cloned()
            .collect();
        CallerSnapshot {
            session_id,
            total_events,
            lost_events,
            threads,
            events,
            thread_names,
        }
    }))
}

fn histogram_bucket(size: usize) -> usize {
    if size == 0 {
        0
    } else {
        usize::BITS as usize - size.leading_zeros() as usize
    }
}

fn size_class_snapshots(before: &AggregateSnapshot, after: &AggregateSnapshot) -> Vec<SizeClassSnapshot> {
    let mut classes = Vec::new();
    for class_index in 0..MAX_SIZE_CLASSES {
        let block_bytes = before.class_block_bytes[class_index].max(after.class_block_bytes[class_index]);
        if block_bytes == 0 {
            continue;
        }
        let allocations_before = before.class_allocations[class_index];
        let deallocations_before = before.class_deallocations[class_index];
        let allocated_before = before.class_allocated_bytes[class_index];
        let deallocated_before = before.class_deallocated_bytes[class_index];
        let allocations_after = after.class_allocations[class_index];
        let deallocations_after = after.class_deallocations[class_index];
        let allocated_after = after.class_allocated_bytes[class_index];
        let deallocated_after = after.class_deallocated_bytes[class_index];
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
        let usable_lower = live_allocations.lower_bound().saturating_mul(block_bytes);
        let usable_value = live_allocations.value().saturating_mul(block_bytes);
        let usable_upper = live_allocations.upper_bound().saturating_mul(block_bytes);
        classes.push(SizeClassSnapshot {
            class_index,
            block_bytes,
            live_allocations,
            requested_bytes,
            usable_bytes: Estimate::bounded(usable_value, usable_lower, usable_upper),
        });
    }
    classes
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
fn add_owner_signed(shard: &AggregateShard, counter: &AtomicIsize, value: isize) {
    if shard.shared {
        counter.fetch_add(value, Ordering::Relaxed);
    } else {
        counter.store(counter.load(Ordering::Relaxed).wrapping_add(value), Ordering::Relaxed);
    }
}

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

#[inline(always)]
fn record_allocation_in(shard: &AggregateShard, size: usize) {
    add_owner(shard, &shard.allocated_bytes, size);
    add_owner(shard, &shard.allocations, 1);
    let bucket = histogram_bucket(size);
    add_owner(shard, &shard.size_allocations[bucket], 1);
    add_owner_signed(shard, &shard.size_live[bucket], 1);
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    if live > PEAK_LIVE_BYTES.load(Ordering::Relaxed) {
        PEAK_LIVE_BYTES.fetch_max(live, Ordering::Relaxed);
    }
}

#[inline(always)]
fn record_deallocation_in(shard: &AggregateShard, size: usize) {
    add_owner(shard, &shard.deallocated_bytes, size);
    add_owner(shard, &shard.deallocations, 1);
    add_owner_signed(shard, &shard.size_live[histogram_bucket(size)], -1);
    LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
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

pub(crate) fn record_small_allocation(class_index: usize, block_bytes: usize, requested_bytes: usize) {
    if telemetry_suppressed() {
        return;
    }
    let shard = aggregate_shard();
    record_allocation_in(shard, requested_bytes);
    if shard.class_block_bytes[class_index].load(Ordering::Relaxed) != block_bytes {
        shard.class_block_bytes[class_index].store(block_bytes, Ordering::Relaxed);
    }
    add_owner(shard, &shard.class_allocations[class_index], 1);
    add_owner(shard, &shard.class_allocated_bytes[class_index], requested_bytes);
}

pub(crate) fn record_small_deallocation(class_index: usize, requested_bytes: usize) {
    if telemetry_suppressed() {
        return;
    }
    let shard = aggregate_shard();
    record_deallocation_in(shard, requested_bytes);
    add_owner(shard, &shard.class_deallocations[class_index], 1);
    add_owner(shard, &shard.class_deallocated_bytes[class_index], requested_bytes);
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

impl Ip {
    fn from_address(address: usize) -> Self {
        Self(address)
    }
}

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
pub struct Snapshot {
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
    /// Decode these bytes with [`rallocator_telemetry::decode`].
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.mapping.as_slice()
    }

    /// Writes the complete encoded snapshot to a file.
    ///
    /// Decode the resulting file contents with [`rallocator_telemetry::decode`].
    pub fn write_file(&self, path: impl AsRef<Path>) -> io::Result<()> {
        with_telemetry_suppressed(|| std::fs::write(path, self.as_bytes()))
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
    state: *const TrackingState,
    allocation_id: usize,
    heap_id: usize,
    heap_kind: HeapKind,
}

impl TrackingAllocation {
    pub(crate) const NONE: Self = Self {
        state: ptr::null(),
        allocation_id: 0,
        heap_id: 0,
        heap_kind: HeapKind::General,
    };
}

pub(crate) struct PendingTracking {
    state: *const TrackingState,
    allocation_id: usize,
    frame_count: usize,
    frames: [usize; MAX_TRACKED_STACK_FRAMES],
}

#[inline(always)]
pub(crate) fn begin_allocation<C: Config>() -> Option<PendingTracking> {
    let session = ACTIVE_SESSION.load(Ordering::Acquire);
    if session == 0 {
        return None;
    }

    let state = tracking_target::<C>(session)?;
    CALLERS_AVAILABLE.store(true, Ordering::Release);
    let state_ref = unsafe { &*state };
    let _internal = TelemetrySuppressionGuard::enter();
    let allocation_id = state_ref.next_allocation_id.fetch_add(1, Ordering::Relaxed);
    let mut frames = [0; MAX_TRACKED_STACK_FRAMES];
    let frame_count = hal::capture_stack(&mut frames, state_ref.allocation_stack_frames);
    Some(PendingTracking {
        state,
        allocation_id,
        frame_count,
        frames,
    })
}

impl PendingTracking {
    pub(crate) fn commit(self, address: *mut u8, layout: Layout, heap_id: usize, heap_kind: HeapKind) -> TrackingAllocation {
        let _internal = TelemetrySuppressionGuard::enter();
        let state = unsafe { &*self.state };
        state.record(
            TrackingRecord {
                kind: EventKind::Allocated,
                allocation_id: self.allocation_id,
                event_thread_id: crate::allocator::tracking_thread_token(),
                heap_id,
                heap_kind,
                freed_after_heap_release: false,
                address: address as usize,
                layout,
            },
            &self.frames[..self.frame_count],
        );
        unsafe { Arc::increment_strong_count(self.state) };
        TrackingAllocation {
            state: self.state,
            allocation_id: self.allocation_id,
            heap_id,
            heap_kind,
        }
    }
}

pub(crate) fn record_deallocation(allocation: TrackingAllocation, address: *mut u8, layout: Layout, freed_after_heap_release: bool) {
    if allocation.state.is_null() {
        return;
    }

    let _internal = TelemetrySuppressionGuard::enter();
    let state = unsafe { &*allocation.state };
    let mut frames = [0; MAX_TRACKED_STACK_FRAMES];
    let frame_count = hal::capture_stack(&mut frames, state.deallocation_stack_frames);
    state.record(
        TrackingRecord {
            kind: EventKind::Deallocated,
            allocation_id: allocation.allocation_id,
            event_thread_id: crate::allocator::tracking_thread_token(),
            heap_id: allocation.heap_id,
            heap_kind: allocation.heap_kind,
            freed_after_heap_release,
            address: address as usize,
            layout,
        },
        &frames[..frame_count],
    );
    unsafe { Arc::decrement_strong_count(allocation.state) };
}

struct TrackingRecord {
    kind: EventKind,
    allocation_id: usize,
    event_thread_id: usize,
    heap_id: usize,
    heap_kind: HeapKind,
    freed_after_heap_release: bool,
    address: usize,
    layout: Layout,
}

pub(crate) struct TrackingState {
    session_id: usize,
    thread_log_id: usize,
    slots: Box<[TrackingSlot]>,
    slot_mask: usize,
    stacks: StackTable,
    allocation_stack_frames: usize,
    deallocation_stack_frames: usize,
    track_threads: bool,
    track_heap_lifetimes: bool,
    allocated_histogram: [AtomicUsize; HISTOGRAM_BUCKETS],
    live_histogram: [AtomicUsize; HISTOGRAM_BUCKETS],
    write_index: AtomicUsize,
    next_allocation_id: AtomicUsize,
}

pub(crate) fn create_thread_log<C: Config>(session_id: usize) -> *const TrackingState {
    let _internal = TelemetrySuppressionGuard::enter();
    let event_capacity = C::CALLER_EVENT_CAPACITY;
    let state = Arc::new(TrackingState {
        session_id,
        thread_log_id: NEXT_THREAD_LOG_ID.fetch_add(1, Ordering::Relaxed),
        slots: (0..event_capacity)
            .map(|_| TrackingSlot::new())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        slot_mask: event_capacity - 1,
        stacks: StackTable::new(event_capacity),
        allocation_stack_frames: C::CALLER_ALLOCATION_STACK_FRAMES,
        deallocation_stack_frames: C::CALLER_DEALLOCATION_STACK_FRAMES,
        track_threads: C::CALLER_TRACK_THREADS,
        track_heap_lifetimes: C::CALLER_TRACK_HEAP_LIFETIMES,
        allocated_histogram: [const { AtomicUsize::new(0) }; HISTOGRAM_BUCKETS],
        live_histogram: [const { AtomicUsize::new(0) }; HISTOGRAM_BUCKETS],
        write_index: AtomicUsize::new(0),
        next_allocation_id: AtomicUsize::new(1),
    });
    let pointer = Arc::into_raw(Arc::clone(&state));
    lock_registry().push(state);
    pointer
}

pub(crate) unsafe fn release_thread_log(state: *const TrackingState) {
    if !state.is_null() {
        let _internal = TelemetrySuppressionGuard::enter();
        drop(unsafe { Arc::from_raw(state) });
    }
}

pub(crate) fn register_thread_identity(thread_id: usize) {
    let current = std::thread::current();
    let Some(name) = current.name() else {
        return;
    };
    let mut names = lock_thread_names();
    if names.iter().any(|thread| thread.thread_id == thread_id) {
        return;
    }
    names.push(ThreadName {
        thread_id,
        name: name.to_owned(),
    });
}

#[cfg(test)]
pub(crate) fn active_session() -> usize {
    ACTIVE_SESSION.load(Ordering::Acquire)
}

impl TrackingState {
    fn record(&self, record: TrackingRecord, call_stack: &[usize]) {
        let bucket = histogram_bucket(record.layout.size());
        match record.kind {
            EventKind::Allocated => {
                self.allocated_histogram[bucket].fetch_add(1, Ordering::Relaxed);
                self.live_histogram[bucket].fetch_add(1, Ordering::Relaxed);
            }
            EventKind::Deallocated => {
                self.live_histogram[bucket].fetch_sub(1, Ordering::Relaxed);
            }
        }
        let index = self.write_index.fetch_add(1, Ordering::Relaxed);
        let sequence = index + 1;
        let slot = &self.slots[index & self.slot_mask];
        slot.lock();
        let previous_stack_id = unsafe { (*slot.data.get()).stack_id };
        let stack_id = self.stacks.replace(previous_stack_id, call_stack);
        unsafe {
            slot.data.get().write(TrackingSlotData {
                sequence,
                allocation_id: record.allocation_id,
                event_thread_id: usize::from(self.track_threads) * record.event_thread_id,
                heap_id: usize::from(self.track_heap_lifetimes) * record.heap_id,
                heap_kind: record.heap_kind,
                freed_after_heap_release: self.track_heap_lifetimes && record.freed_after_heap_release,
                address: record.address,
                size: record.layout.size(),
                align: record.layout.align(),
                stack_id,
                kind: record.kind,
            });
        }
        slot.unlock();
    }

    fn snapshot(&self) -> ThreadSnapshot {
        let total_events = self.write_index.load(Ordering::Acquire);
        let first_index = total_events.saturating_sub(self.slots.len());
        let mut events = Vec::with_capacity(total_events - first_index);

        for index in first_index..total_events {
            let expected_sequence = index + 1;
            let slot = &self.slots[index & self.slot_mask];
            slot.lock();
            let data = unsafe { *slot.data.get() };
            if data.sequence == expected_sequence {
                let call_stack = self.stacks.resolve(data.stack_id).into_iter().map(Ip::from_address).collect();
                events.push(Event {
                    thread_log_id: self.thread_log_id,
                    event_thread_id: data.event_thread_id,
                    sequence: expected_sequence,
                    allocation_id: data.allocation_id,
                    kind: data.kind,
                    heap_id: data.heap_id,
                    heap_kind: data.heap_kind,
                    freed_after_heap_release: data.freed_after_heap_release,
                    address: data.address,
                    size: data.size,
                    align: data.align,
                    call_stack,
                });
            }
            slot.unlock();
        }

        ThreadSnapshot {
            total_events,
            lost_events: total_events - events.len(),
            events,
            allocated_histogram: self.allocated_histogram.iter().map(|count| count.load(Ordering::Relaxed)).collect(),
            live_histogram: self.live_histogram.iter().map(|count| count.load(Ordering::Relaxed)).collect(),
        }
    }
}

struct ThreadSnapshot {
    total_events: usize,
    lost_events: usize,
    events: Vec<Event>,
    allocated_histogram: Vec<usize>,
    live_histogram: Vec<usize>,
}

struct TrackingSlot {
    locked: AtomicBool,
    data: UnsafeCell<TrackingSlotData>,
}

// SAFETY: TrackingSlot's data is accessed only while its atomic lock is held.
unsafe impl Sync for TrackingSlot {}

#[derive(Clone, Copy)]
struct TrackingSlotData {
    sequence: usize,
    allocation_id: usize,
    event_thread_id: usize,
    heap_id: usize,
    heap_kind: HeapKind,
    freed_after_heap_release: bool,
    address: usize,
    size: usize,
    align: usize,
    stack_id: usize,
    kind: EventKind,
}

impl TrackingSlot {
    fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(TrackingSlotData {
                sequence: 0,
                allocation_id: 0,
                event_thread_id: 0,
                heap_id: 0,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: 0,
                size: 0,
                align: 0,
                stack_id: 0,
                kind: EventKind::Allocated,
            }),
        }
    }

    fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

struct StackTable {
    locked: AtomicBool,
    entries: UnsafeCell<Box<[StackEntry]>>,
    mask: usize,
}

// SAFETY: StackTable entries are accessed only through its lock guard, which
// provides exclusive access to the UnsafeCell payload.
unsafe impl Sync for StackTable {}

#[derive(Clone, Copy)]
struct StackEntry {
    hash: u64,
    references: usize,
    frame_count: u8,
    frames: [usize; MAX_TRACKED_STACK_FRAMES],
}

impl StackEntry {
    const EMPTY: Self = Self {
        hash: 0,
        references: 0,
        frame_count: 0,
        frames: [0; MAX_TRACKED_STACK_FRAMES],
    };

    fn matches(&self, hash: u64, frames: &[usize]) -> bool {
        self.references != 0 && self.hash == hash && self.frame_count as usize == frames.len() && self.frames[..frames.len()] == *frames
    }
}

impl StackTable {
    fn new(event_capacity: usize) -> Self {
        Self {
            locked: AtomicBool::new(false),
            entries: UnsafeCell::new(vec![StackEntry::EMPTY; event_capacity].into_boxed_slice()),
            mask: event_capacity - 1,
        }
    }

    fn replace(&self, previous_stack_id: usize, frames: &[usize]) -> usize {
        let mut table = self.guard();
        let entries = table.entries();
        if previous_stack_id != 0 {
            let previous = &mut entries[previous_stack_id - 1];
            previous.references -= 1;
        }
        if frames.is_empty() {
            return 0;
        }

        let hash = stack_hash(frames);
        let mut index = hash as usize & self.mask;
        let mut available = None;
        for _ in 0..entries.len() {
            let entry = &mut entries[index];
            if entry.matches(hash, frames) {
                entry.references += 1;
                return index + 1;
            }
            if entry.references == 0 {
                available.get_or_insert(index);
                if entry.hash == 0 {
                    break;
                }
            }
            index = (index + 1) & self.mask;
        }

        let index = available.expect("stack table has one entry per retained event");
        let entry = &mut entries[index];
        entry.hash = hash;
        entry.references = 1;
        entry.frame_count = frames.len() as u8;
        entry.frames[..frames.len()].copy_from_slice(frames);
        index + 1
    }

    fn resolve(&self, stack_id: usize) -> Vec<usize> {
        if stack_id == 0 {
            return Vec::new();
        }

        let mut table = self.guard();
        let entry = &table.entries()[stack_id - 1];
        entry.frames[..entry.frame_count as usize].to_vec()
    }

    fn guard(&self) -> StackTableGuard<'_> {
        self.lock();
        StackTableGuard { table: self }
    }

    fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

struct StackTableGuard<'a> {
    table: &'a StackTable,
}

impl StackTableGuard<'_> {
    fn entries(&mut self) -> &mut [StackEntry] {
        unsafe { &mut *self.table.entries.get() }
    }
}

impl Drop for StackTableGuard<'_> {
    fn drop(&mut self) {
        self.table.unlock();
    }
}

fn stack_hash(frames: &[usize]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &frame in frames {
        hash ^= frame as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash.max(1)
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
    TELEMETRY_SUPPRESSION_DEPTH.try_with(|depth| depth.get() != 0).unwrap_or(true)
}

fn registry() -> &'static Mutex<Vec<Arc<TrackingState>>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

fn thread_names() -> &'static Mutex<Vec<ThreadName>> {
    THREAD_NAMES.get_or_init(|| Mutex::new(Vec::new()))
}

fn lock_registry() -> std::sync::MutexGuard<'static, Vec<Arc<TrackingState>>> {
    registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_thread_names() -> std::sync::MutexGuard<'static, Vec<ThreadName>> {
    thread_names().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_control() -> std::sync::MutexGuard<'static, ()> {
    CONTROL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_telemetry_suppressed<R>(operation: impl FnOnce() -> R) -> R {
    let _guard = TelemetrySuppressionGuard::enter();
    operation()
}

#[cfg(test)]
mod tests {
    use std::sync::{Barrier, PoisonError};
    use std::thread;
    use std::time::Duration;

    use super::*;

    crate::config!(TestCallerConfig {
        track_callers: true,
        caller_event_capacity: 4,
        caller_allocation_stack_frames: 2,
        caller_deallocation_stack_frames: 2,
    });
    crate::config!(ZeroCapacityConfig { caller_event_capacity: 0 });
    crate::config!(NonPowerOfTwoCapacityConfig { caller_event_capacity: 3 });
    crate::config!(ExcessiveAllocationStackConfig {
        caller_allocation_stack_frames: MAX_TRACKED_STACK_FRAMES + 1,
    });
    crate::config!(ExcessiveDeallocationStackConfig {
        caller_deallocation_stack_frames: MAX_TRACKED_STACK_FRAMES + 1,
    });

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

    #[test]
    fn telemetry_views_expose_values_and_sampling_progress() {
        crate::initialize();
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let exact = Estimate::exact(7);
        assert_eq!(exact.value(), 7);
        assert_eq!(exact.lower_bound(), 7);
        assert_eq!(exact.upper_bound(), 7);
        assert!(exact.is_exact());
        assert!(!Estimate::bounded(7, 6, 8).is_exact());

        let expected = sample_stats();
        let memory = expected.memory();
        assert_eq!(memory.live_requested_bytes(), Estimate::exact(60));
        assert_eq!(memory.live_usable_bytes(), Estimate::bounded(60, 60, 256));
        assert_eq!(memory.mapped_bytes(), 256);
        assert_eq!(memory.committed_bytes(), 256);
        assert_eq!(memory.peak_live_bytes(), 80);

        let operations = expected.operations();
        assert_eq!(operations.allocations(), 11);
        assert_eq!(operations.deallocations(), 5);
        assert_eq!(operations.allocated_bytes(), 101);
        assert_eq!(operations.deallocated_bytes(), 41);

        let reclamation = expected.reclamation();
        assert_eq!(reclamation.mapped_bytes(), 256);
        assert_eq!(reclamation.committed_bytes(), 256);
        assert_eq!(reclamation.mappings(), 7);
        assert_eq!(reclamation.unmappings(), 3);

        let remote = expected.remote();
        assert_eq!(remote.frees(), 4);
        assert_eq!(remote.pending_blocks(), Estimate::bounded(3, 1, 3));
        assert_eq!(remote.drained_blocks(), 1);

        let errors = [
            (
                SnapshotError::unavailable(),
                SnapshotErrorKind::Unavailable,
                "allocator telemetry is unavailable",
            ),
            (
                SnapshotError::sizing_failed(),
                SnapshotErrorKind::SizingFailed,
                "the telemetry snapshot length could not be calculated",
            ),
            (
                SnapshotError::allocation_failed(),
                SnapshotErrorKind::AllocationFailed,
                "the telemetry snapshot mapping could not be allocated",
            ),
            (
                SnapshotError::encoding_failed(),
                SnapshotErrorKind::EncodingFailed,
                "the telemetry snapshot could not be encoded",
            ),
        ];
        for (error, kind, message) in errors {
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), message);
            assert_eq!(format!("{error:?}"), format!("SnapshotError({message})"));
        }

        let delta = StatsDelta::between(sample_stats(), Stats::default());
        assert_eq!(delta.allocated_bytes(), 0);
        assert_eq!(delta.deallocated_bytes(), 0);
        assert_eq!(delta.allocations(), 0);
        assert_eq!(delta.deallocations(), 0);
        assert_eq!(delta.mappings(), 0);
        assert_eq!(delta.unmappings(), 0);
        assert_eq!(delta.remote_frees(), 0);
        assert_eq!(delta.drained_remote_blocks(), 0);

        AGGREGATES_AVAILABLE.store(true, Ordering::Release);
        let started_at = Instant::now();
        let mut sampler = Sampler::new_at(started_at).unwrap();
        let sample = sampler.sample_at(started_at + Duration::from_millis(25)).unwrap();
        assert_eq!(sample.elapsed(), Duration::from_millis(25));
        assert_eq!(sample.current(), &stats().unwrap());
        assert_eq!(sample.delta(), &StatsDelta::between(sampler.previous, sampler.previous));

        let mut session = Session::start_at(started_at).unwrap();
        assert!(session.sample().is_some());
        let report = session.finish_at(started_at + Duration::from_millis(40)).unwrap();
        assert_eq!(report.elapsed(), Duration::from_millis(40));
        assert_eq!(report.initial(), report.final_stats());
        assert_eq!(report.delta(), &StatsDelta::between(*report.initial(), *report.final_stats()));
    }

    #[test]
    fn caller_tracking_configuration_rejects_invalid_shapes() {
        validate_config::<TestCallerConfig>();
        std::hint::black_box((
            ZeroCapacityConfig,
            NonPowerOfTwoCapacityConfig,
            ExcessiveAllocationStackConfig,
            ExcessiveDeallocationStackConfig,
        ));
        assert!(std::panic::catch_unwind(validate_config::<ZeroCapacityConfig>).is_err());
        assert!(std::panic::catch_unwind(validate_config::<NonPowerOfTwoCapacityConfig>).is_err());
        assert!(std::panic::catch_unwind(validate_config::<ExcessiveAllocationStackConfig>).is_err());
        assert!(std::panic::catch_unwind(validate_config::<ExcessiveDeallocationStackConfig>).is_err());
    }

    #[test]
    fn private_snapshot_encoders_cover_every_topology_kind() {
        crate::initialize();
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
        static STACK_MARKER: () = ();

        crate::initialize();
        let stack_address = (&raw const STACK_MARKER).addr();
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
                    call_stack: vec![Ip::from_address(0), Ip::from_address(stack_address)],
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

        let addresses = resolve_addresses(&callers);
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].address, stack_address as u64);

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
    fn tracking_ring_and_stack_table_handle_reuse_and_contention() {
        crate::initialize();
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        {
            let _control = lock_control();
            ACTIVE_SESSION.store(0, Ordering::Release);
            assert!(begin_allocation::<TestCallerConfig>().is_none());
        }
        record_deallocation(
            TrackingAllocation::NONE,
            ptr::null_mut(),
            Layout::from_size_align(1, 1).unwrap(),
            false,
        );
        let table = Arc::new(StackTable::new(4));
        let first = table.replace(0, &[1, 2]);
        let shared = table.replace(0, &[1, 2]);
        assert_eq!(first, shared);
        assert_eq!(table.resolve(first), vec![1, 2]);
        assert_eq!(table.replace(first, &[]), 0);
        assert!(table.resolve(0).is_empty());

        let colliding = (3..100)
            .find(|candidate| stack_hash(&[1]) as usize & 3 == stack_hash(&[*candidate]) as usize & 3)
            .unwrap();
        let occupied = table.replace(shared, &[1]);
        let next = table.replace(0, &[colliding]);
        assert_ne!(occupied, next);

        let tombstones = StackTable::new(4);
        let retired = tombstones.replace(0, &[10]);
        assert_eq!(tombstones.replace(retired, &[]), 0);
        let retired_bucket = stack_hash(&[10]) as usize & 3;
        let reusing = (11..100)
            .find(|candidate| stack_hash(&[*candidate]) as usize & 3 == retired_bucket)
            .unwrap();
        assert_ne!(tombstones.replace(0, &[reusing]), 0);

        let barrier = Arc::new(Barrier::new(2));
        table.locked.store(true, Ordering::Release);
        let released = Arc::clone(&table);
        let released_barrier = Arc::clone(&barrier);
        let worker = thread::spawn(move || {
            released_barrier.wait();
            thread::sleep(Duration::from_millis(1));
            released.unlock();
        });
        barrier.wait();
        table.lock();
        table.unlock();
        worker.join().unwrap();

        let slot = Arc::new(TrackingSlot::new());
        slot.locked.store(true, Ordering::Release);
        let released = Arc::clone(&slot);
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(1));
            released.unlock();
        });
        slot.lock();
        slot.unlock();
        worker.join().unwrap();

        let state = TrackingState {
            session_id: 1,
            thread_log_id: 2,
            slots: vec![TrackingSlot::new(), TrackingSlot::new()].into_boxed_slice(),
            slot_mask: 1,
            stacks: StackTable::new(2),
            allocation_stack_frames: 2,
            deallocation_stack_frames: 2,
            track_threads: true,
            track_heap_lifetimes: true,
            allocated_histogram: std::array::from_fn(|_| AtomicUsize::new(0)),
            live_histogram: std::array::from_fn(|_| AtomicUsize::new(0)),
            write_index: AtomicUsize::new(0),
            next_allocation_id: AtomicUsize::new(1),
        };
        let layout = Layout::from_size_align(8, 8).unwrap();
        state.record(
            TrackingRecord {
                kind: EventKind::Allocated,
                allocation_id: 1,
                event_thread_id: 2,
                heap_id: 3,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: 0x1000,
                layout,
            },
            &[4, 5],
        );
        state.record(
            TrackingRecord {
                kind: EventKind::Deallocated,
                allocation_id: 1,
                event_thread_id: 4,
                heap_id: 3,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: 0x1000,
                layout,
            },
            &[],
        );
        state.record(
            TrackingRecord {
                kind: EventKind::Allocated,
                allocation_id: 2,
                event_thread_id: 2,
                heap_id: 5,
                heap_kind: HeapKind::Bump,
                freed_after_heap_release: false,
                address: 0x2000,
                layout,
            },
            &[6],
        );
        let snapshot = state.snapshot();
        assert_eq!(snapshot.total_events, 3);
        assert_eq!(snapshot.lost_events, 1);
        assert_eq!(snapshot.events.len(), 2);

        unsafe {
            (*state.slots[1].data.get()).sequence = 0;
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.lost_events, 2);

        let registered = create_thread_log::<TestCallerConfig>(123);
        let mut frames = [0; MAX_TRACKED_STACK_FRAMES];
        frames[..2].copy_from_slice(&[7, 8]);
        let mut tracked_address = 0_u8;
        let tracked_address = ptr::from_mut(&mut tracked_address);
        let allocation = PendingTracking {
            state: registered,
            allocation_id: 99,
            frame_count: 2,
            frames,
        }
        .commit(tracked_address, layout, 6, HeapKind::General);
        record_deallocation(allocation, tracked_address, layout, false);
        let registered_snapshot = unsafe { &*registered }.snapshot();
        assert_eq!(registered_snapshot.events.len(), 2);
        LAST_SESSION.store(123, Ordering::Release);
        CALLERS_AVAILABLE.store(true, Ordering::Release);
        let callers = caller_snapshot().unwrap();
        assert_eq!(callers.events.len(), 2);
        CALLERS_AVAILABLE.store(false, Ordering::Release);
        lock_registry().clear();
        unsafe { release_thread_log(registered) };
    }

    #[test]
    fn suppression_and_counter_recorders_cover_disabled_paths() {
        crate::initialize();
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
        let _ = snapshot();
        assert!(Sampler::new().is_none());
        let now = Instant::now();
        let mut sampler = Sampler {
            sampled_at: now,
            previous: Stats::default(),
        };
        assert!(sampler.sample().is_none());
        assert!(Session::start().is_none());
        let session = Session {
            started_at: now,
            sampler,
            baseline: Stats::default(),
        };
        assert!(session.finish().is_none());
        assert!(snapshot_stats(None, false).is_none());
        assert_eq!(snapshot_stats(None, true), Some(Stats::default()));
        assert_eq!(snapshot_stats(Some(sample_stats()), false), Some(sample_stats()));
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
        let before = aggregate_snapshot().unwrap();
        let after = aggregate_snapshot().unwrap();
        assert!(!size_class_snapshots(&before, &after).is_empty());

        CALLERS_AVAILABLE.store(false, Ordering::Release);
        assert!(caller_snapshot().is_none());
        track_callers(true);
        let session = active_session();
        assert_ne!(session, 0);
        track_callers(true);
        assert_eq!(active_session(), session);
        track_callers(false);
    }

    #[cfg(not(miri))]
    #[test]
    fn aggregate_helpers_cover_shared_fallback_and_registration_retry_paths() {
        crate::initialize();
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(histogram_bucket(0), 0);

        let shared = AggregateShard::new(true);
        add_owner(&shared, &shared.allocations, 2);
        add_owner_signed(&shared, &shared.size_live[0], -1);
        assert_eq!(shared.allocations.load(Ordering::Relaxed), 2);
        assert_eq!(shared.size_live[0].load(Ordering::Relaxed), -1);

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

        thread::Builder::new()
            .name("rallocator-telemetry-test".to_owned())
            .spawn(|| {
                register_thread_identity(usize::MAX);
                register_thread_identity(usize::MAX);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn aggregate_shards_merge_cross_thread_deallocations() {
        crate::initialize();
        let _test = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let size = 2_048;
        let bucket = histogram_bucket(size);
        let before = aggregate_snapshot().unwrap_or_else(AggregateSnapshot::new);
        let stats_before = stats().unwrap_or_default();

        record_allocation(size);
        std::thread::spawn(move || record_deallocation_stats(size)).join().unwrap();

        let after = aggregate_snapshot().unwrap();
        let stats_after = stats().unwrap();
        assert_eq!(after.size_allocations[bucket], before.size_allocations[bucket] + 1);
        assert_eq!(after.size_live[bucket], before.size_live[bucket]);
        assert_eq!(stats_after.allocated_bytes, stats_before.allocated_bytes + size);
        assert_eq!(stats_after.deallocated_bytes, stats_before.deallocated_bytes + size);
        assert_eq!(stats_after.live_bytes, stats_before.live_bytes);
    }

    #[test]
    fn snapshot_debug_reports_encoded_size() {
        crate::initialize();
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
            snapshot.write_file(path).unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }
}
