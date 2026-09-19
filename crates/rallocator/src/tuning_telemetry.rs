// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Opt-in allocator tuning counters for controlled diagnostics.
//!
//! Enabling the `tuning-telemetry` Cargo feature does **not** start recording.
//! Call [`TuningTelemetry::enable`] explicitly, observe the active session with
//! [`TuningTelemetry::snapshot_if_active`], then call [`TuningTelemetry::disable`].
//! Control applies to one linked copy of this crate: its allocator values and
//! threads share a session.
//! Starting a session replaces the previous session and resets its counters.
//!
//! Observations allocate no memory, do not pause recording or drain recorders,
//! and do not reset counters. Session identity is stabilized against concurrent
//! control operations, but counter fields are independently sampled: they are
//! neither transactional process totals nor exact allocation latency.
//!
//! Small-class counters are shared by class index across allocator personalities,
//! not partitioned by personality or layout. Public class-size labels use
//! [`StandardSizeClasses`] and are valid only when all participating personalities
//! use that layout. Custom layouts are not detected or filtered: mixing them can
//! combine different sizes under one standard label. Do not interpret public
//! small-class observations as per-size data in such a session. Medium counters
//! are not labeled by small size class.
//!
//! Active instrumentation can perturb performance. Collect these diagnostics
//! separately from timing comparisons. These are cumulative event counters, not
//! queue/bin occupancy or resident-memory measurements.

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::config::{MAX_SIZE_CLASSES, SizeClassLayout, StandardSizeClasses};

static ACTIVE_SESSION: AtomicUsize = AtomicUsize::new(0);
static TRANSITION: std::sync::Mutex<()> = std::sync::Mutex::new(());
static RECORDERS: AtomicUsize = AtomicUsize::new(RECORDING_CLOSED);
static NEXT_SESSION: AtomicUsize = AtomicUsize::new(1);
static LAST_SESSION: AtomicUsize = AtomicUsize::new(0);
static CLASS_COUNTERS: [ClassCounters; MAX_SIZE_CLASSES] = [const { ClassCounters::new() }; MAX_SIZE_CLASSES];
static MEDIUM_COUNTERS: MediumCounters = MediumCounters::new();
static PARTIAL_SCAN_LIMIT: AtomicU64 = AtomicU64::new(0);

// Closing and admission modify the SAME atomic word: an admission either
// precedes close and contributes to its drain count, or observes the closed bit
// and fails. Counter writes are released by guard departure and acquired by
// drain. This does not rely on store->load ordering across separate atomics.
const RECORDING_CLOSED: usize = 1 << (usize::BITS - 1);
const RECORDER_COUNT: usize = RECORDING_CLOSED - 1;

/// Controls explicitly enabled tuning sessions for this linked copy of the crate.
#[derive(Debug)]
pub struct TuningTelemetry;

impl TuningTelemetry {
    /// Starts a new session, resetting the previous counters, and returns its ID.
    ///
    /// # Panics
    ///
    /// Panics if a previous control operation poisoned the transition lock.
    pub fn enable() -> usize {
        let _transition = TRANSITION.lock().unwrap();
        close_recording();
        wait_for_recorders();
        reset();
        let session_id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        LAST_SESSION.store(session_id, Ordering::Release);
        open_recording(session_id);
        session_id
    }

    /// Stops recording while preserving the completed session's counters.
    ///
    /// # Panics
    ///
    /// Panics if a previous control operation poisoned the transition lock.
    pub fn disable() {
        let _transition = TRANSITION.lock().unwrap();
        close_recording();
        wait_for_recorders();
    }

    /// Returns whether tuning recording is currently enabled.
    pub fn is_enabled() -> bool {
        ACTIVE_SESSION.load(Ordering::Acquire) != 0
    }

    pub(crate) fn collect() -> TuningTelemetryReport {
        Self::collect_for::<StandardSizeClasses>()
    }

    pub(crate) fn collect_for<L: SizeClassLayout>() -> TuningTelemetryReport {
        let _transition = TRANSITION.lock().unwrap();
        Self::collect_locked::<L>()
    }

    /// Observes an explicitly enabled session without pausing or resetting it.
    /// Counters are independently read, not a transactional process snapshot.
    ///
    /// Returns `None` when inactive, rather than a zero-filled measured record.
    /// Small-class labels assume every participating personality uses
    /// [`StandardSizeClasses`]; custom layouts are neither detected nor separated.
    ///
    /// # Panics
    ///
    /// Panics if a previous control operation poisoned the transition lock.
    pub fn snapshot_if_active() -> Option<TuningTelemetryObservation> {
        let _transition = TRANSITION.lock().unwrap();
        let session_id = ACTIVE_SESSION.load(Ordering::Acquire);
        if session_id == 0 {
            return None;
        }
        Some(TuningTelemetryObservation {
            session_id,
            classes: std::array::from_fn(|class_index| {
                // Unused tail entries are private and excluded by classes().
                let block_size = StandardSizeClasses::SIZES.get(class_index).copied().unwrap_or(0);
                read_class(class_index, block_size)
            }),
            medium: read_medium(),
        })
    }

    // Legacy collection holds TRANSITION while quiescing recorders. The active
    // observation above deliberately does not use this pausing path.
    fn collect_locked<L: SizeClassLayout>() -> TuningTelemetryReport {
        let active_session = close_recording();
        wait_for_recorders();
        let classes = CLASS_COUNTERS
            .iter()
            .take(L::SIZES.len())
            .enumerate()
            .map(|(class_index, _)| read_class(class_index, L::SIZES[class_index]))
            .collect::<Vec<_>>();
        let medium = read_medium();
        let partial_scan_limit = PARTIAL_SCAN_LIMIT.load(Ordering::Relaxed) as usize;
        let recommendations = recommendations(&classes, &medium, partial_scan_limit);
        let report = TuningTelemetryReport {
            session_id: LAST_SESSION.load(Ordering::Acquire),
            classes,
            medium,
            recommendations,
        };
        if active_session != 0 {
            open_recording(active_session);
        }
        report
    }
}

/// Allocation-free observation of the active tuning session's existing counters.
#[derive(Clone, Eq, PartialEq)]
pub struct TuningTelemetryObservation {
    /// Identifier of the explicitly enabled session these counters belong to.
    pub session_id: usize,
    classes: [ClassTuningTelemetry; MAX_SIZE_CLASSES],
    /// Cumulative medium-span events recorded in this session.
    pub medium: MediumTuningTelemetry,
}

impl TuningTelemetryObservation {
    /// Observed class counters with the built-in standard class-size labels.
    ///
    /// Counters combine all participating personalities by class index. These
    /// labels are not valid per-size observations if any uses a custom layout.
    #[must_use]
    pub fn classes(&self) -> &[ClassTuningTelemetry] {
        &self.classes[..StandardSizeClasses::SIZES.len()]
    }
}

impl fmt::Debug for TuningTelemetryObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TuningTelemetryObservation")
            .field("session_id", &self.session_id)
            .field("classes", &self.classes())
            .field("medium", &self.medium)
            .finish()
    }
}

impl fmt::Display for TuningTelemetryObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "tuning_session_id={}", self.session_id)?;
        writeln!(
            formatter,
            "medium tls_allocation_hits={} recycled_spans={} fresh_spans={} local_cached_frees={} shared_span_returns={} purged_spans={}",
            self.medium.tls_cache_hits,
            self.medium.global_cache_hits,
            self.medium.fresh_commits,
            self.medium.cached_frees,
            self.medium.global_frees,
            self.medium.purged_spans
        )?;
        for class in self.classes() {
            writeln!(
                formatter,
                "class index={} block_bytes={} allocations={} tls_hits={} recycled_batch_hits={} recycled_word_refills={} recycled_single_hits={} fresh_hits={} slab_refills={} partial_scan_calls={} partial_slabs_scanned={} partial_limit_hits={} local_frees={} bitmap_spills={} remote_frees={}",
                class.class_index,
                class.block_size,
                class.allocations,
                class.tls_cache_hits,
                class.recycled_batch_hits,
                class.recycled_word_refills,
                class.recycled_single_hits,
                class.fresh_hits,
                class.slab_refills,
                class.partial_scan_calls,
                class.partial_slabs_scanned,
                class.partial_limit_hits,
                class.local_frees,
                class.bitmap_spills,
                class.remote_frees
            )?;
        }
        Ok(())
    }
}

fn read_class(class_index: usize, block_size: usize) -> ClassTuningTelemetry {
    let counters = &CLASS_COUNTERS[class_index];
    ClassTuningTelemetry {
        class_index,
        block_size,
        allocations: counters.allocations.load(Ordering::Relaxed),
        tls_cache_hits: counters.tls_cache_hits.load(Ordering::Relaxed),
        recycled_batch_hits: counters.recycled_batch_hits.load(Ordering::Relaxed),
        recycled_word_refills: counters.recycled_word_refills.load(Ordering::Relaxed),
        recycled_single_hits: counters.recycled_single_hits.load(Ordering::Relaxed),
        fresh_hits: counters.fresh_hits.load(Ordering::Relaxed),
        slab_refills: counters.slab_refills.load(Ordering::Relaxed),
        partial_scan_calls: counters.partial_scan_calls.load(Ordering::Relaxed),
        partial_slabs_scanned: counters.partial_slabs_scanned.load(Ordering::Relaxed),
        partial_limit_hits: counters.partial_limit_hits.load(Ordering::Relaxed),
        local_frees: counters.local_frees.load(Ordering::Relaxed),
        bitmap_spills: counters.bitmap_spills.load(Ordering::Relaxed),
        remote_frees: counters.remote_frees.load(Ordering::Relaxed),
    }
}

fn read_medium() -> MediumTuningTelemetry {
    MediumTuningTelemetry {
        tls_cache_hits: MEDIUM_COUNTERS.tls_cache_hits.load(Ordering::Relaxed),
        global_cache_hits: MEDIUM_COUNTERS.global_cache_hits.load(Ordering::Relaxed),
        fresh_commits: MEDIUM_COUNTERS.fresh_commits.load(Ordering::Relaxed),
        cached_frees: MEDIUM_COUNTERS.cached_frees.load(Ordering::Relaxed),
        global_frees: MEDIUM_COUNTERS.global_frees.load(Ordering::Relaxed),
        purged_spans: MEDIUM_COUNTERS.purged_spans.load(Ordering::Relaxed),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TuningTelemetryReport {
    pub session_id: usize,
    pub classes: Vec<ClassTuningTelemetry>,
    pub medium: MediumTuningTelemetry,
    pub recommendations: TuningRecommendations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Cumulative small-allocation path events for one standard size class.
pub struct ClassTuningTelemetry {
    /// Index into the standard size-class table.
    pub class_index: usize,
    /// Usable block size of the standard class, in bytes.
    pub block_size: usize,
    /// Recorded allocation events.
    pub allocations: u64,
    /// Allocations served by the thread-local cache.
    pub tls_cache_hits: u64,
    /// Recorded allocations served by a recycled bitmap batch.
    pub recycled_batch_hits: u64,
    /// Recorded recycled-bitmap word refill operations.
    pub recycled_word_refills: u64,
    /// Recorded allocations served by single recycled slots.
    pub recycled_single_hits: u64,
    /// Recorded allocations served by fresh slab slots.
    pub fresh_hits: u64,
    /// Recorded slab refill operations.
    pub slab_refills: u64,
    /// Recorded partial-slab search operations.
    pub partial_scan_calls: u64,
    /// Total partial slabs visited by those searches.
    pub partial_slabs_scanned: u64,
    /// Searches that reached the configured scan limit.
    pub partial_limit_hits: u64,
    /// Recorded owner-local frees.
    pub local_frees: u64,
    /// Recorded local-cache spills into recycled bitmaps.
    pub bitmap_spills: u64,
    /// Recorded remote frees.
    pub remote_frees: u64,
}

/// Cumulative medium-span path events, not bytes, occupancy, or syscall counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediumTuningTelemetry {
    /// Allocations served from a heap-local batch.
    pub tls_cache_hits: u64,
    /// Recycled spans transferred from shared backing, including batch prefetch.
    pub global_cache_hits: u64,
    /// Fresh spans committed, including batch prefetch; not OS syscall count.
    pub fresh_commits: u64,
    /// Frees accepted by a heap-local cache, including frees that evict a batch.
    pub cached_frees: u64,
    /// Spans transferred to shared backing, including mailbox publication and
    /// local-cache eviction batches; not current bin or mailbox occupancy.
    pub global_frees: u64,
    /// Successfully reclaimed spans before coalescing; not OS syscall count.
    pub purged_spans: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TuningRecommendations {
    pub recycled_bitmap_batch_max_block_size: usize,
    pub partial_slab_scan_limit: Option<usize>,
    pub medium_purge_delay_ms: u64,
}

impl fmt::Display for TuningTelemetryReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "tuning telemetry session {}", self.session_id)?;
        writeln!(
            formatter,
            "{:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>9} {:>8} {:>8}",
            "class", "allocs", "tls", "batch", "bitmap", "fresh", "refills", "scanned", "spills", "remote"
        )?;
        for class in self
            .classes
            .iter()
            .filter(|class| class.allocations != 0 || class.local_frees != 0 || class.remote_frees != 0)
        {
            writeln!(
                formatter,
                "{:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>9} {:>8} {:>8}",
                class.block_size,
                class.allocations,
                class.tls_cache_hits,
                class.recycled_batch_hits,
                class.recycled_word_refills + class.recycled_single_hits,
                class.fresh_hits,
                class.slab_refills,
                class.partial_slabs_scanned,
                class.bitmap_spills,
                class.remote_frees
            )?;
        }
        writeln!(
            formatter,
            "medium: tls_hits={} global_hits={} commits={} cached_frees={} global_frees={} purged={}",
            self.medium.tls_cache_hits,
            self.medium.global_cache_hits,
            self.medium.fresh_commits,
            self.medium.cached_frees,
            self.medium.global_frees,
            self.medium.purged_spans
        )?;
        write!(
            formatter,
            "suggested: batch_max={} partial_scan=",
            self.recommendations.recycled_bitmap_batch_max_block_size
        )?;
        if let Some(limit) = self.recommendations.partial_slab_scan_limit {
            write!(formatter, "{limit}")?;
        } else {
            write!(formatter, "no-data")?;
        }
        writeln!(formatter, " medium_purge={}ms", self.recommendations.medium_purge_delay_ms)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ClassEvent {
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
pub(crate) enum MediumEvent {
    TlsCacheHit,
    GlobalCacheHit,
    FreshCommit,
    CachedFree,
    GlobalFree,
    PurgedSpan,
}

pub(crate) fn record_class(class_index: usize, event: ClassEvent) {
    let Some(_recording) = RecordingGuard::begin() else {
        return;
    };
    let counters = CLASS_COUNTERS
        .get(class_index)
        .expect("size-class lookup must return an index below MAX_SIZE_CLASSES");
    let counter = match event {
        ClassEvent::Allocation => &counters.allocations,
        ClassEvent::TlsCacheHit => &counters.tls_cache_hits,
        ClassEvent::RecycledBatchHit => &counters.recycled_batch_hits,
        ClassEvent::RecycledWordRefill => &counters.recycled_word_refills,
        ClassEvent::RecycledSingleHit => &counters.recycled_single_hits,
        ClassEvent::FreshHit => &counters.fresh_hits,
        ClassEvent::SlabRefill => &counters.slab_refills,
        ClassEvent::LocalFree => &counters.local_frees,
        ClassEvent::BitmapSpill => &counters.bitmap_spills,
        ClassEvent::RemoteFree => &counters.remote_frees,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_partial_scan(class_index: usize, scanned: usize, limit: usize) {
    let Some(_recording) = RecordingGuard::begin() else {
        return;
    };
    PARTIAL_SCAN_LIMIT.store(limit as u64, Ordering::Relaxed);
    if scanned == 0 {
        return;
    }
    let counters = CLASS_COUNTERS
        .get(class_index)
        .expect("size-class lookup must return an index below MAX_SIZE_CLASSES");
    counters.partial_scan_calls.fetch_add(1, Ordering::Relaxed);
    counters.partial_slabs_scanned.fetch_add(scanned as u64, Ordering::Relaxed);
    if scanned == limit {
        counters.partial_limit_hits.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_medium(event: MediumEvent, count: usize) {
    let Some(_recording) = RecordingGuard::begin() else {
        return;
    };
    let counter = match event {
        MediumEvent::TlsCacheHit => &MEDIUM_COUNTERS.tls_cache_hits,
        MediumEvent::GlobalCacheHit => &MEDIUM_COUNTERS.global_cache_hits,
        MediumEvent::FreshCommit => &MEDIUM_COUNTERS.fresh_commits,
        MediumEvent::CachedFree => &MEDIUM_COUNTERS.cached_frees,
        MediumEvent::GlobalFree => &MEDIUM_COUNTERS.global_frees,
        MediumEvent::PurgedSpan => &MEDIUM_COUNTERS.purged_spans,
    };
    counter.fetch_add(count as u64, Ordering::Relaxed);
}

struct RecordingGuard;

impl RecordingGuard {
    fn begin() -> Option<Self> {
        Self::begin_with(|| {})
    }

    fn begin_with(after_start: impl FnOnce()) -> Option<Self> {
        let session = ACTIVE_SESSION.load(Ordering::Acquire);
        if session == 0 {
            return None;
        }
        RECORDERS.fetch_update(Ordering::AcqRel, Ordering::Acquire, admitted_count).ok()?;
        after_start();
        if ACTIVE_SESSION.load(Ordering::Acquire) != session {
            RECORDERS.fetch_sub(1, Ordering::Release);
            return None;
        }
        Some(Self)
    }
}

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        RECORDERS.fetch_sub(1, Ordering::Release);
    }
}

fn admitted_count(state: usize) -> Option<usize> {
    // Exhaustion is a rejected diagnostic event, never a carry into the gate.
    if state & RECORDING_CLOSED != 0 || state == RECORDER_COUNT {
        None
    } else {
        Some(state + 1)
    }
}

fn close_recording() -> usize {
    RECORDERS.fetch_or(RECORDING_CLOSED, Ordering::AcqRel);
    ACTIVE_SESSION.swap(0, Ordering::AcqRel)
}

fn open_recording(session: usize) {
    // TRANSITION is held and the closed gate has drained. Publish the session
    // and any reset counters before permitting a successful admission CAS.
    ACTIVE_SESSION.store(session, Ordering::Release);
    RECORDERS.store(0, Ordering::Release);
}

fn wait_for_recorders() {
    wait_for_recorders_with(|| {});
}

fn wait_for_recorders_with(mut on_wait: impl FnMut()) {
    while RECORDERS.load(Ordering::Acquire) & RECORDER_COUNT != 0 {
        on_wait();
        std::hint::spin_loop();
    }
}

struct ClassCounters {
    allocations: AtomicU64,
    tls_cache_hits: AtomicU64,
    recycled_batch_hits: AtomicU64,
    recycled_word_refills: AtomicU64,
    recycled_single_hits: AtomicU64,
    fresh_hits: AtomicU64,
    slab_refills: AtomicU64,
    partial_scan_calls: AtomicU64,
    partial_slabs_scanned: AtomicU64,
    partial_limit_hits: AtomicU64,
    local_frees: AtomicU64,
    bitmap_spills: AtomicU64,
    remote_frees: AtomicU64,
}

impl ClassCounters {
    const fn new() -> Self {
        Self {
            allocations: AtomicU64::new(0),
            tls_cache_hits: AtomicU64::new(0),
            recycled_batch_hits: AtomicU64::new(0),
            recycled_word_refills: AtomicU64::new(0),
            recycled_single_hits: AtomicU64::new(0),
            fresh_hits: AtomicU64::new(0),
            slab_refills: AtomicU64::new(0),
            partial_scan_calls: AtomicU64::new(0),
            partial_slabs_scanned: AtomicU64::new(0),
            partial_limit_hits: AtomicU64::new(0),
            local_frees: AtomicU64::new(0),
            bitmap_spills: AtomicU64::new(0),
            remote_frees: AtomicU64::new(0),
        }
    }

    fn reset(&self) {
        self.allocations.store(0, Ordering::Relaxed);
        self.tls_cache_hits.store(0, Ordering::Relaxed);
        self.recycled_batch_hits.store(0, Ordering::Relaxed);
        self.recycled_word_refills.store(0, Ordering::Relaxed);
        self.recycled_single_hits.store(0, Ordering::Relaxed);
        self.fresh_hits.store(0, Ordering::Relaxed);
        self.slab_refills.store(0, Ordering::Relaxed);
        self.partial_scan_calls.store(0, Ordering::Relaxed);
        self.partial_slabs_scanned.store(0, Ordering::Relaxed);
        self.partial_limit_hits.store(0, Ordering::Relaxed);
        self.local_frees.store(0, Ordering::Relaxed);
        self.bitmap_spills.store(0, Ordering::Relaxed);
        self.remote_frees.store(0, Ordering::Relaxed);
    }
}

struct MediumCounters {
    tls_cache_hits: AtomicU64,
    global_cache_hits: AtomicU64,
    fresh_commits: AtomicU64,
    cached_frees: AtomicU64,
    global_frees: AtomicU64,
    purged_spans: AtomicU64,
}

impl MediumCounters {
    const fn new() -> Self {
        Self {
            tls_cache_hits: AtomicU64::new(0),
            global_cache_hits: AtomicU64::new(0),
            fresh_commits: AtomicU64::new(0),
            cached_frees: AtomicU64::new(0),
            global_frees: AtomicU64::new(0),
            purged_spans: AtomicU64::new(0),
        }
    }

    fn reset(&self) {
        self.tls_cache_hits.store(0, Ordering::Relaxed);
        self.global_cache_hits.store(0, Ordering::Relaxed);
        self.fresh_commits.store(0, Ordering::Relaxed);
        self.cached_frees.store(0, Ordering::Relaxed);
        self.global_frees.store(0, Ordering::Relaxed);
        self.purged_spans.store(0, Ordering::Relaxed);
    }
}

fn reset() {
    for counters in &CLASS_COUNTERS {
        counters.reset();
    }
    MEDIUM_COUNTERS.reset();
    PARTIAL_SCAN_LIMIT.store(0, Ordering::Relaxed);
}

fn recommendations(classes: &[ClassTuningTelemetry], medium: &MediumTuningTelemetry, partial_scan_limit: usize) -> TuningRecommendations {
    let recycled_bitmap_batch_max_block_size = classes
        .iter()
        .filter(|class| {
            let recycled = class.recycled_batch_hits + class.recycled_word_refills + class.recycled_single_hits;
            recycled >= 32 && recycled >= class.fresh_hits / 10
        })
        .map(|class| class.block_size)
        .max()
        .unwrap_or(0);
    let scan_calls: u64 = classes.iter().map(|class| class.partial_scan_calls).sum();
    let limit_hits: u64 = classes.iter().map(|class| class.partial_limit_hits).sum();
    let partial_slab_scan_limit = if partial_scan_limit == 0 || scan_calls == 0 {
        None
    } else if limit_hits.saturating_mul(4) > scan_calls {
        Some(partial_scan_limit.saturating_mul(2))
    } else {
        Some(partial_scan_limit)
    };
    let total_medium_hits = medium.tls_cache_hits + medium.global_cache_hits;
    let medium_purge_delay_ms = if medium.purged_spans != 0 && total_medium_hits > medium.fresh_commits {
        5_000
    } else {
        1_000
    };
    TuningRecommendations {
        recycled_bitmap_batch_max_block_size,
        partial_slab_scan_limit,
        medium_purge_delay_ms,
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout};

    use super::*;
    use crate::Rallocator;
    use crate::config::Standard;
    use crate::telemetry::TEST_LOCK;

    struct FailAfter {
        remaining: usize,
    }

    impl fmt::Write for FailAfter {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            if value.len() > self.remaining {
                return Err(fmt::Error);
            }
            self.remaining -= value.len();
            Ok(())
        }
    }

    struct AllSizeClasses;

    impl SizeClassLayout for AllSizeClasses {
        const SIZES: &'static [usize] = &[16; MAX_SIZE_CLASSES];
    }

    fn class(block_size: usize) -> ClassTuningTelemetry {
        ClassTuningTelemetry {
            class_index: 0,
            block_size,
            allocations: 0,
            tls_cache_hits: 0,
            recycled_batch_hits: 0,
            recycled_word_refills: 0,
            recycled_single_hits: 0,
            fresh_hits: 0,
            slab_refills: 0,
            partial_scan_calls: 0,
            partial_slabs_scanned: 0,
            partial_limit_hits: 0,
            local_frees: 0,
            bitmap_spills: 0,
            remote_frees: 0,
        }
    }

    fn indexed_class(class_index: usize, block_size: usize) -> ClassTuningTelemetry {
        ClassTuningTelemetry {
            class_index,
            ..class(block_size)
        }
    }

    fn medium() -> MediumTuningTelemetry {
        MediumTuningTelemetry {
            tls_cache_hits: 0,
            global_cache_hits: 0,
            fresh_commits: 0,
            cached_frees: 0,
            global_frees: 0,
            purged_spans: 0,
        }
    }

    fn observation() -> TuningTelemetryObservation {
        TuningTelemetryObservation {
            session_id: 7,
            classes: std::array::from_fn(|index| indexed_class(index, 64)),
            medium: medium(),
        }
    }

    #[test]
    fn observation_debug_includes_session_and_visible_counters() {
        let observation = observation();
        let classes = &observation.classes[..StandardSizeClasses::SIZES.len()];
        let medium = observation.medium;
        assert_eq!(
            format!("{observation:?}"),
            format!("TuningTelemetryObservation {{ session_id: 7, classes: {classes:?}, medium: {medium:?} }}")
        );
    }

    #[test]
    fn observation_display_propagates_each_write_failure() {
        let observation = observation();
        let rendered = observation.to_string();
        let boundaries = [0, rendered.find("medium ").unwrap(), rendered.find("class index=").unwrap()];
        assert_eq!(
            boundaries.map(|remaining| fmt::write(&mut FailAfter { remaining }, format_args!("{observation}"))),
            [Err(fmt::Error); 3]
        );
    }

    #[test]
    fn allocator_paths_produce_tuning_recommendations() {
        let _test = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        TuningTelemetry::disable();
        let session_id = TuningTelemetry::enable();
        let allocator = unsafe { Rallocator::<Standard>::new() };
        let layout = Layout::from_size_align(256, 16).unwrap();

        let mut addresses = Vec::with_capacity(128);
        for _ in 0..128 {
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            addresses.push(address);
        }
        for address in addresses.drain(..).rev() {
            unsafe { allocator.dealloc(address, layout) };
        }
        for _ in 0..128 {
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            addresses.push(address);
        }
        for address in addresses {
            unsafe { allocator.dealloc(address, layout) };
        }
        TuningTelemetry::disable();

        let report = TuningTelemetry::collect();
        let class = report.classes.iter().find(|class| class.block_size == 256).unwrap();
        assert_eq!(report.session_id, session_id);
        assert!(class.allocations >= 256);
        assert!(class.fresh_hits != 0);
        assert!(class.recycled_batch_hits + class.recycled_word_refills != 0);
        assert_eq!(report.recommendations.partial_slab_scan_limit, None);
        assert!(report.to_string().contains("partial_scan=no-data"));
    }

    #[test]
    fn active_snapshot_preserves_session_and_does_not_activate_or_reset_counters() {
        let _test = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        TuningTelemetry::disable();
        assert!(TuningTelemetry::snapshot_if_active().is_none());
        assert!(!TuningTelemetry::is_enabled());

        let session = TuningTelemetry::enable();
        {
            let _transition = TRANSITION.lock().unwrap();
            record_medium(MediumEvent::GlobalCacheHit, 7);
        }
        let first = TuningTelemetry::snapshot_if_active().unwrap();
        assert_eq!(first.session_id, session);
        assert_eq!(first.classes().len(), StandardSizeClasses::SIZES.len());
        assert!(first.medium.global_cache_hits >= 7);
        let rendered = first.to_string();
        assert!(rendered.contains(&format!("tuning_session_id={session}")));
        assert!(rendered.contains("fresh_spans="));
        assert!(rendered.contains("shared_span_returns="));
        {
            let _transition = TRANSITION.lock().unwrap();
            assert!(TuningTelemetry::is_enabled());
            record_medium(MediumEvent::GlobalCacheHit, 11);
        }
        let second = TuningTelemetry::snapshot_if_active().unwrap();
        assert_eq!(second.session_id, session);
        assert!(second.medium.global_cache_hits >= first.medium.global_cache_hits + 11);
        {
            let _transition = TRANSITION.lock().unwrap();
            assert!(TuningTelemetry::is_enabled());
        }

        TuningTelemetry::disable();
        assert!(TuningTelemetry::snapshot_if_active().is_none());
        assert!(!TuningTelemetry::is_enabled());
    }

    #[test]
    fn active_snapshot_does_not_drain_in_flight_recorders() {
        let _test = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = TuningTelemetry::enable();
        let recording = RecordingGuard::begin().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(TuningTelemetry::snapshot_if_active().map(|observation| observation.session_id))
                .unwrap();
        });
        let observed = receiver.recv_timeout(std::time::Duration::from_secs(2));
        // Release the guard before joining even if a regression made observation
        // wait for quiescence, so that failure is bounded rather than a deadlock.
        drop(recording);
        worker.join().unwrap();
        TuningTelemetry::disable();
        assert_eq!(observed.unwrap(), Some(session));
    }

    #[test]
    fn reports_the_selected_size_class_layout() {
        let _test = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        TuningTelemetry::disable();
        TuningTelemetry::enable();
        TuningTelemetry::disable();

        let report = TuningTelemetry::collect_for::<StandardSizeClasses>();
        assert!(report.classes.iter().any(|class| class.block_size == 4_384));
        assert_eq!(report.classes.len(), StandardSizeClasses::SIZES.len());
    }

    #[test]
    fn records_every_event_and_restores_collection_state() {
        const TEST_CLASS: usize = MAX_SIZE_CLASSES - 1;

        let _test = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        TuningTelemetry::disable();
        reset();
        record_class(TEST_CLASS, ClassEvent::Allocation);
        record_partial_scan(TEST_CLASS, 1, 4);
        record_medium(MediumEvent::TlsCacheHit, 1);
        let disabled = TuningTelemetry::collect_for::<AllSizeClasses>();
        assert_eq!(disabled.classes[TEST_CLASS], indexed_class(TEST_CLASS, 16));
        assert_eq!(disabled.medium, medium());
        assert!(!TuningTelemetry::is_enabled());

        let first_session = TuningTelemetry::enable();
        assert!(TuningTelemetry::is_enabled());
        for event in [
            ClassEvent::Allocation,
            ClassEvent::TlsCacheHit,
            ClassEvent::RecycledBatchHit,
            ClassEvent::RecycledWordRefill,
            ClassEvent::RecycledSingleHit,
            ClassEvent::FreshHit,
            ClassEvent::SlabRefill,
            ClassEvent::LocalFree,
            ClassEvent::BitmapSpill,
            ClassEvent::RemoteFree,
        ] {
            record_class(TEST_CLASS, event);
        }
        record_partial_scan(TEST_CLASS, 0, 4);
        record_partial_scan(TEST_CLASS, 2, 4);
        record_partial_scan(TEST_CLASS, 4, 4);
        for event in [
            MediumEvent::TlsCacheHit,
            MediumEvent::GlobalCacheHit,
            MediumEvent::FreshCommit,
            MediumEvent::CachedFree,
            MediumEvent::GlobalFree,
            MediumEvent::PurgedSpan,
        ] {
            record_medium(event, 2);
        }

        let report = TuningTelemetry::collect_for::<AllSizeClasses>();
        assert!(TuningTelemetry::is_enabled());
        assert_eq!(report.session_id, first_session);
        assert_eq!(
            report.classes[TEST_CLASS],
            ClassTuningTelemetry {
                allocations: 1,
                tls_cache_hits: 1,
                recycled_batch_hits: 1,
                recycled_word_refills: 1,
                recycled_single_hits: 1,
                fresh_hits: 1,
                slab_refills: 1,
                partial_scan_calls: 2,
                partial_slabs_scanned: 6,
                partial_limit_hits: 1,
                local_frees: 1,
                bitmap_spills: 1,
                remote_frees: 1,
                ..indexed_class(TEST_CLASS, 16)
            }
        );
        assert!(report.medium.tls_cache_hits >= 2);
        assert!(report.medium.global_cache_hits >= 2);
        assert!(report.medium.fresh_commits >= 2);
        assert!(report.medium.cached_frees >= 2);
        assert!(report.medium.global_frees >= 2);
        assert!(report.medium.purged_spans >= 2);

        let second_session = TuningTelemetry::enable();
        assert_eq!(second_session, first_session + 1);
        let reset_report = TuningTelemetry::collect();
        assert_eq!(reset_report.session_id, second_session);
        assert_eq!(
            reset_report.classes.iter().copied().find(|item| item.class_index == 0),
            Some(class(StandardSizeClasses::SIZES[0]))
        );
        TuningTelemetry::disable();
        let inactive_report = TuningTelemetry::collect_for::<AllSizeClasses>();
        assert_eq!(inactive_report.session_id, second_session);
        assert!(!TuningTelemetry::is_enabled());
    }

    #[test]
    fn counter_constructors_and_resets_clear_every_field() {
        let class_counters = ClassCounters::new();
        class_counters.allocations.store(1, Ordering::Relaxed);
        class_counters.tls_cache_hits.store(1, Ordering::Relaxed);
        class_counters.recycled_batch_hits.store(1, Ordering::Relaxed);
        class_counters.recycled_word_refills.store(1, Ordering::Relaxed);
        class_counters.recycled_single_hits.store(1, Ordering::Relaxed);
        class_counters.fresh_hits.store(1, Ordering::Relaxed);
        class_counters.slab_refills.store(1, Ordering::Relaxed);
        class_counters.partial_scan_calls.store(1, Ordering::Relaxed);
        class_counters.partial_slabs_scanned.store(1, Ordering::Relaxed);
        class_counters.partial_limit_hits.store(1, Ordering::Relaxed);
        class_counters.local_frees.store(1, Ordering::Relaxed);
        class_counters.bitmap_spills.store(1, Ordering::Relaxed);
        class_counters.remote_frees.store(1, Ordering::Relaxed);
        class_counters.reset();
        assert_eq!(class_counters.allocations.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.tls_cache_hits.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.recycled_batch_hits.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.recycled_word_refills.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.recycled_single_hits.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.fresh_hits.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.slab_refills.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.partial_scan_calls.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.partial_slabs_scanned.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.partial_limit_hits.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.local_frees.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.bitmap_spills.load(Ordering::Relaxed), 0);
        assert_eq!(class_counters.remote_frees.load(Ordering::Relaxed), 0);

        let medium_counters = MediumCounters::new();
        medium_counters.tls_cache_hits.store(1, Ordering::Relaxed);
        medium_counters.global_cache_hits.store(1, Ordering::Relaxed);
        medium_counters.fresh_commits.store(1, Ordering::Relaxed);
        medium_counters.cached_frees.store(1, Ordering::Relaxed);
        medium_counters.global_frees.store(1, Ordering::Relaxed);
        medium_counters.purged_spans.store(1, Ordering::Relaxed);
        medium_counters.reset();
        assert_eq!(medium_counters.tls_cache_hits.load(Ordering::Relaxed), 0);
        assert_eq!(medium_counters.global_cache_hits.load(Ordering::Relaxed), 0);
        assert_eq!(medium_counters.fresh_commits.load(Ordering::Relaxed), 0);
        assert_eq!(medium_counters.cached_frees.load(Ordering::Relaxed), 0);
        assert_eq!(medium_counters.global_frees.load(Ordering::Relaxed), 0);
        assert_eq!(medium_counters.purged_spans.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn recorder_transition_and_wait_retries_are_deterministic() {
        let _test = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        RECORDERS.store(0, Ordering::Release);
        ACTIVE_SESSION.store(1, Ordering::Release);
        assert!(
            RecordingGuard::begin_with(|| {
                ACTIVE_SESSION.store(0, Ordering::Release);
            })
            .is_none()
        );
        assert_eq!(RECORDERS.load(Ordering::Acquire), 0);

        RECORDERS.store(1, Ordering::Release);
        let mut waits = 0;
        wait_for_recorders_with(|| {
            waits += 1;
            RECORDERS.store(0, Ordering::Release);
        });
        assert_eq!(waits, 1);
    }

    #[test]
    fn admission_never_carries_into_or_clears_the_closed_bit() {
        assert_eq!(admitted_count(0), Some(1));
        assert_eq!(admitted_count(RECORDER_COUNT - 1), Some(RECORDER_COUNT));
        assert_eq!(admitted_count(RECORDER_COUNT), None);
        assert_eq!(admitted_count(RECORDING_CLOSED), None);
        assert_eq!(admitted_count(usize::MAX), None);
    }

    #[test]
    fn display_includes_active_classes_and_both_scan_recommendations() {
        let visible = ClassTuningTelemetry {
            allocations: 1,
            tls_cache_hits: 2,
            recycled_batch_hits: 3,
            recycled_word_refills: 4,
            recycled_single_hits: 5,
            fresh_hits: 6,
            slab_refills: 7,
            partial_slabs_scanned: 8,
            bitmap_spills: 9,
            remote_frees: 10,
            ..class(64)
        };
        let hidden = class(128);
        let mut report = TuningTelemetryReport {
            session_id: 7,
            classes: vec![visible, hidden],
            medium: MediumTuningTelemetry {
                tls_cache_hits: 11,
                global_cache_hits: 12,
                fresh_commits: 13,
                cached_frees: 14,
                global_frees: 15,
                purged_spans: 16,
            },
            recommendations: TuningRecommendations {
                recycled_bitmap_batch_max_block_size: 256,
                partial_slab_scan_limit: Some(8),
                medium_purge_delay_ms: 5_000,
            },
        };

        let with_limit = report.to_string();
        assert!(with_limit.contains("tuning telemetry session 7"));
        assert!(with_limit.contains("     64"));
        assert!(!with_limit.contains("    128"));
        assert!(with_limit.contains("partial_scan=8 medium_purge=5000ms"));

        report.recommendations.partial_slab_scan_limit = None;
        assert!(report.to_string().contains("partial_scan=no-data"));
    }

    #[test]
    fn display_propagates_errors_from_every_write() {
        let mut report = TuningTelemetryReport {
            session_id: 7,
            classes: vec![ClassTuningTelemetry {
                allocations: 1,
                ..class(64)
            }],
            medium: medium(),
            recommendations: TuningRecommendations {
                recycled_bitmap_batch_max_block_size: 256,
                partial_slab_scan_limit: Some(8),
                medium_purge_delay_ms: 1_000,
            },
        };
        let rendered = report.to_string();
        let limit = rendered.find("partial_scan=8").unwrap() + "partial_scan=".len();
        let boundaries = [
            0,
            rendered.find("  class").unwrap(),
            rendered.find("     64").unwrap(),
            rendered.find("medium:").unwrap(),
            rendered.find("suggested:").unwrap(),
            limit,
            limit + 1,
        ];
        for remaining in boundaries {
            assert!(fmt::write(&mut FailAfter { remaining }, format_args!("{report}")).is_err());
        }

        report.recommendations.partial_slab_scan_limit = None;
        let rendered = report.to_string();
        let remaining = rendered.find("no-data").unwrap();
        assert!(fmt::write(&mut FailAfter { remaining }, format_args!("{report}")).is_err());
    }

    #[test]
    fn recommendations_cover_threshold_scan_and_purge_branches() {
        let mut classes = vec![class(64), class(256), class(512)];
        classes[0].recycled_batch_hits = 31;
        classes[1].recycled_word_refills = 32;
        classes[1].fresh_hits = 321;
        classes[2].recycled_single_hits = 40;
        classes[2].fresh_hits = 400;

        let no_scan_data = recommendations(&classes, &medium(), 0);
        assert_eq!(no_scan_data.recycled_bitmap_batch_max_block_size, 512);
        assert_eq!(no_scan_data.partial_slab_scan_limit, None);
        assert_eq!(no_scan_data.medium_purge_delay_ms, 1_000);

        classes[0].partial_scan_calls = 4;
        classes[0].partial_limit_hits = 2;
        let doubled = recommendations(
            &classes,
            &MediumTuningTelemetry {
                tls_cache_hits: 2,
                global_cache_hits: 1,
                fresh_commits: 2,
                purged_spans: 1,
                ..medium()
            },
            4,
        );
        assert_eq!(doubled.partial_slab_scan_limit, Some(8));
        assert_eq!(doubled.medium_purge_delay_ms, 5_000);

        classes[0].partial_limit_hits = 1;
        let unchanged = recommendations(&classes, &medium(), 4);
        assert_eq!(unchanged.partial_slab_scan_limit, Some(4));
    }

    #[test]
    fn recommendations_use_bitmap_refills_and_report_missing_scan_data() {
        let mut classes = StandardSizeClasses::SIZES
            .iter()
            .enumerate()
            .map(|(class_index, &block_size)| ClassTuningTelemetry {
                class_index,
                block_size,
                allocations: 0,
                tls_cache_hits: 0,
                recycled_batch_hits: 0,
                recycled_word_refills: 0,
                recycled_single_hits: 0,
                fresh_hits: 0,
                slab_refills: 0,
                partial_scan_calls: 0,
                partial_slabs_scanned: 0,
                partial_limit_hits: 0,
                local_frees: 0,
                bitmap_spills: 0,
                remote_frees: 0,
            })
            .collect::<Vec<_>>();
        let class = classes.iter_mut().find(|class| class.block_size == 256).unwrap();
        class.recycled_word_refills = 32;

        let recommendation = recommendations(
            &classes,
            &MediumTuningTelemetry {
                tls_cache_hits: 0,
                global_cache_hits: 0,
                fresh_commits: 0,
                cached_frees: 0,
                global_frees: 0,
                purged_spans: 0,
            },
            4,
        );

        assert_eq!(recommendation.recycled_bitmap_batch_max_block_size, 256);
        assert_eq!(recommendation.partial_slab_scan_limit, None);
    }
}
