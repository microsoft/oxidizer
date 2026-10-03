// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::struct_field_names,
    reason = "Event totals and the retained event collection are intentionally explicit in the public snapshot model"
)]

//! Allocation-safe, bounded process event recording.

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::cell::{Cell, UnsafeCell};
use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::system::SystemSlice;

/// Allocation event types.
pub mod alloc;
/// General event types.
pub mod event;
/// I/O event types.
pub mod io;
/// Runtime event types.
pub mod runtime;
/// Thread event types.
pub mod thread;

use event::{
    Address, BacktraceCapture, Event, EventClass, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, ObjectId,
    Record,
};
use thread::{ThreadId, ThreadLog};

const DEFAULT_EVENT_CAPACITY_PER_THREAD: usize = 65_536;
const MIN_EVENT_CAPACITY_PER_THREAD: usize = 64;
const MAX_EVENT_CAPACITY_PER_THREAD: usize = 1_048_576;
const MAX_EVENT_SAMPLING_ONE_IN: usize = 1_048_576;
const RECORDER_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(not(test))]
const RETIRED_RING_BUDGET_BYTES: usize = 268_435_456;
#[cfg(test)]
const RETIRED_RING_BUDGET_BYTES: usize = 32_768;

/// Validated power-of-two capacity for one thread's event buffer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EventBufferCapacity(usize);

impl EventBufferCapacity {
    /// Default event-buffer capacity.
    pub const DEFAULT: Self = Self(DEFAULT_EVENT_CAPACITY_PER_THREAD);

    /// Validates a per-thread event capacity.
    #[must_use]
    pub const fn new(value: usize) -> Option<Self> {
        if value >= MIN_EVENT_CAPACITY_PER_THREAD && value <= MAX_EVENT_CAPACITY_PER_THREAD && value.is_power_of_two() {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the event count represented by this capacity.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }

    /// Returns recorder memory used while one thread owns an event buffer of this capacity.
    #[must_use]
    pub const fn memory_bytes_per_thread(self) -> usize {
        std::mem::size_of::<ThreadRecorder>() + self.get() * std::mem::size_of::<Slot>()
    }

    const fn exponent(self) -> u32 {
        self.0.trailing_zeros()
    }

    const fn from_exponent(exponent: u32) -> Self {
        Self(1usize << exponent)
    }
}

impl Default for EventBufferCapacity {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Validated denominator for object-consistent event sampling.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EventSampling(usize);

impl EventSampling {
    /// Records every object.
    pub const ALL: Self = Self(1);

    /// Validates a one-in-`value` sampling denominator.
    #[must_use]
    pub const fn one_in(value: usize) -> Option<Self> {
        if value != 0 && value <= MAX_EVENT_SAMPLING_ONE_IN {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the sampling denominator.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }

    fn includes(self, object_id: ObjectId) -> bool {
        let mut mixed = object_id.get().wrapping_add(0x9e37_79b9_7f4a_7c15);
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^= mixed >> 31;
        let denominator = u64::try_from(self.0).unwrap_or(u64::MAX);
        if denominator.is_power_of_two() {
            mixed & (denominator - 1) == 0
        } else {
            mixed <= u64::MAX / denominator
        }
    }
}

impl Default for EventSampling {
    fn default() -> Self {
        Self::ALL
    }
}

/// Lightweight runtime recorder counters.
#[cfg(any(test, feature = "monitor"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Statistics {
    /// Threads that emitted events in the current recording session.
    pub(crate) thread_count: u64,
    /// Events emitted in the current recording session.
    pub(crate) total_events: u64,
    /// Events currently retained across thread rings.
    pub(crate) retained_events: u64,
    /// Events overwritten across thread rings.
    pub(crate) lost_events: u64,
    /// Configured event capacity for each newly active thread.
    pub(crate) event_capacity_per_thread: u64,
    /// Memory currently retained by recorder metadata and event buffers.
    pub(crate) allocated_bytes: u64,
    /// Policies used by the active recording session.
    pub(crate) recording: RecordingPolicies,
}

#[cfg(any(test, feature = "monitor"))]
pub(crate) struct Activity {
    pub(crate) statistics: Statistics,
    pub(crate) session_id: u64,
    pub(crate) class_events: [u64; 6],
    pub(crate) threads: Vec<ThreadStatistics>,
}

#[cfg(any(test, feature = "monitor"))]
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ThreadStatistics {
    pub(crate) thread_id: ThreadId,
    pub(crate) name: String,
    pub(crate) total_events: u64,
    pub(crate) retained_events: u64,
    pub(crate) lost_events: u64,
    pub(crate) event_capacity: u64,
    pub(crate) retired: bool,
}

pub(crate) const MAX_STACK_FRAMES: usize = 24;
const MAX_THREAD_NAME_LEN: usize = 64;
const RECORDING_ENABLED: u8 = 1;
const BACKTRACES_ENABLED: u8 = 1 << 1;
const SAMPLING_SHIFT: u32 = 8;
const SAMPLING_MASK: u64 = (1 << 21) - 1;

static EVENT_CAPACITY: AtomicU64 = AtomicU64::new(EventBufferCapacity::DEFAULT.exponent() as u64);
static ALLOCATION_POLICY: AtomicU64 = AtomicU64::new(0);
static GENERAL_POLICY: AtomicU64 = AtomicU64::new(0);
static ARC_DEREFERENCE_POLICY: AtomicU64 = AtomicU64::new(0);
static RUNTIME_TASK_POLICY: AtomicU64 = AtomicU64::new(0);
static IO_POLICY: AtomicU64 = AtomicU64::new(0);
static CACHE_POLICY: AtomicU64 = AtomicU64::new(0);
static RETIRED_RINGS: Mutex<RetiredRings> = Mutex::new(RetiredRings {
    rings: VecDeque::new(),
    allocated_bytes: 0,
});
static CONFIGURATION_LOCKED: AtomicBool = AtomicBool::new(false);
static ACTIVE_SESSION: AtomicU64 = AtomicU64::new(0);
static LAST_SESSION: AtomicU64 = AtomicU64::new(0);
// Stop and partially completed clears reset counters without changing the
// recording observation retained by snapshot sources.
static ACTIVITY_GENERATION: AtomicU64 = AtomicU64::new(0);
static LAST_ALLOCATION_POLICY: AtomicU64 = AtomicU64::new(0);
static LAST_GENERAL_POLICY: AtomicU64 = AtomicU64::new(0);
static LAST_ARC_DEREFERENCE_POLICY: AtomicU64 = AtomicU64::new(0);
static LAST_RUNTIME_TASK_POLICY: AtomicU64 = AtomicU64::new(0);
static LAST_IO_POLICY: AtomicU64 = AtomicU64::new(0);
static LAST_CACHE_POLICY: AtomicU64 = AtomicU64::new(0);
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);
static RECORDERS: AtomicPtr<ThreadRecorder> = AtomicPtr::new(ptr::null_mut());
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(test, miri))]
#[cfg_attr(test, mutants::skip)]
fn test_event_buffer_capacity() -> EventBufferCapacity {
    EventBufferCapacity(MIN_EVENT_CAPACITY_PER_THREAD)
}

#[cfg(all(test, not(miri)))]
#[cfg_attr(test, mutants::skip)]
fn test_event_buffer_capacity() -> EventBufferCapacity {
    EventBufferCapacity::DEFAULT
}

#[cfg(test)]
pub(crate) fn test_configuration() -> Configuration {
    Configuration {
        // Tests that care about capacity select it explicitly. Other tests need
        // only one complete recorder lifecycle, so Miri does not benefit from
        // initializing the production-sized 65,536-slot ring.
        event_capacity_per_thread: test_event_buffer_capacity(),
        ..Configuration::default()
    }
}

thread_local! {
    static SUPPRESSION_DEPTH: Cell<usize> = const { Cell::new(0) };
    static LOCAL_RECORDER: LocalRecorder = const { LocalRecorder::new() };
}

/// Runtime telemetry configuration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Configuration {
    /// Recording policy for allocation lifecycle events.
    pub allocations: RecordingPolicy,
    /// Recording policy for ordinary primitive events.
    pub general_events: RecordingPolicy,
    /// Recording policy for high-frequency Arc dereferences.
    pub arc_dereferences: RecordingPolicy,
    /// Recording policy for runtime task and scheduling events.
    pub runtime_tasks: RecordingPolicy,
    /// Recording policy for I/O primitive operations.
    pub io: RecordingPolicy,
    /// Recording policy for cache operations.
    pub cache: RecordingPolicy,
    /// Events retained by each participating thread.
    pub event_capacity_per_thread: EventBufferCapacity,
}

/// Recording controls for one event class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingPolicy {
    /// Whether events in this class are recorded.
    pub enabled: bool,
    /// Whether configured events capture instruction-pointer backtraces.
    pub capture_backtraces: bool,
    /// Records all events for approximately one in every X objects.
    pub event_sampling: EventSampling,
}

impl RecordingPolicy {
    /// Records every object and uses the supplied backtrace setting.
    #[must_use]
    pub const fn all(capture_backtraces: bool) -> Self {
        Self {
            enabled: true,
            capture_backtraces,
            event_sampling: EventSampling::ALL,
        }
    }
}

/// Recording policies for all independently selectable event classes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecordingPolicies {
    /// Allocation lifecycle policy.
    pub allocations: RecordingPolicy,
    /// Ordinary primitive-event policy.
    pub general_events: RecordingPolicy,
    /// Arc dereference policy.
    pub arc_dereferences: RecordingPolicy,
    /// Runtime task and scheduling-event policy.
    pub runtime_tasks: RecordingPolicy,
    /// I/O primitive operation policy.
    pub io: RecordingPolicy,
    /// Cache operation policy.
    pub cache: RecordingPolicy,
}

impl Default for RecordingPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            capture_backtraces: false,
            event_sampling: EventSampling::ALL,
        }
    }
}

/// Identifies one active runtime-event recording session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingSession(NonZeroU64);

// Written only under ConfigurationLock. A stopped session keeps its observation
// boundary so sources do not turn frozen state into continuously growing ages.
static SESSION_STOPPED_AT: AtomicU64 = AtomicU64::new(0);

/// Session and clock boundary used to interpret recording-only source state.
#[derive(Clone, Copy, Debug)]
pub struct RecordingObservation {
    /// Retained recording generation, including a just-stopped generation.
    pub session: RecordingSession,
    /// Latest time at which the session's source state may be interpreted.
    pub observed_at: EventTimestamp,
}

/// Returns the active generation without touching thread-local recorder state.
///
/// Sources must first use [`recording_enabled_for`] and revalidate this
/// generation after acquiring their own state synchronization.
#[doc(hidden)]
#[must_use]
pub fn active_recording_session() -> Option<RecordingSession> {
    RecordingSession::from_raw(ACTIVE_SESSION.load(Ordering::Acquire))
}

/// Reads a coherent recording observation, or `None` if configuration is busy.
#[doc(hidden)]
#[must_use]
pub fn recording_observation() -> Option<RecordingObservation> {
    let _configuration = ConfigurationLock::acquire_until(Instant::now())?;
    recording_observation_locked()
}

fn recording_observation_locked() -> Option<RecordingObservation> {
    let session = RecordingSession::from_raw(LAST_SESSION.load(Ordering::Acquire))?;
    let stopped_at = SESSION_STOPPED_AT.load(Ordering::Acquire);
    Some(RecordingObservation {
        session,
        observed_at: if stopped_at == 0 {
            EventTimestamp::now()
        } else {
            EventTimestamp::from_ticks(stopped_at)
        },
    })
}

impl RecordingSession {
    /// Returns the numeric session identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Reconstructs a recording session from a previously stored identifier.
    #[must_use]
    pub const fn from_raw(session_id: u64) -> Option<Self> {
        match NonZeroU64::new(session_id) {
            Some(session_id) => Some(Self(session_id)),
            None => None,
        }
    }
}

/// Configures runtime event recording for the process.
pub(crate) fn configure(configuration: Configuration) {
    let _lock = ConfigurationLock::acquire();
    configure_locked(configuration);
}

#[cfg(feature = "monitor")]
pub(crate) fn try_configure(configuration: Configuration) -> Result<(), crate::Error> {
    let _lock = ConfigurationLock::acquire_until(wait_deadline())
        .ok_or_else(|| crate::Error::new("seismograph recording configuration timed out waiting for another operation"))?;
    configure_locked(configuration);
    Ok(())
}

fn configure_locked(configuration: Configuration) {
    let allocation_policy = encode_policy(configuration.allocations);
    let general_policy = encode_policy(configuration.general_events);
    let arc_dereference_policy = encode_policy(configuration.arc_dereferences);
    let runtime_task_policy = encode_policy(configuration.runtime_tasks);
    let io_policy = encode_policy(configuration.io);
    let cache_policy = encode_policy(configuration.cache);
    let capacity = u64::from(configuration.event_capacity_per_thread.exponent());
    let enabled = configuration.allocations.enabled
        || configuration.general_events.enabled
        || configuration.arc_dereferences.enabled
        || configuration.runtime_tasks.enabled
        || configuration.io.enabled
        || configuration.cache.enabled;
    let changed = EVENT_CAPACITY.load(Ordering::Acquire) != capacity
        || ALLOCATION_POLICY.load(Ordering::Acquire) != allocation_policy
        || GENERAL_POLICY.load(Ordering::Acquire) != general_policy
        || ARC_DEREFERENCE_POLICY.load(Ordering::Acquire) != arc_dereference_policy
        || RUNTIME_TASK_POLICY.load(Ordering::Acquire) != runtime_task_policy
        || IO_POLICY.load(Ordering::Acquire) != io_policy
        || CACHE_POLICY.load(Ordering::Acquire) != cache_policy;
    if !changed {
        if !enabled {
            ACTIVE_SESSION.store(0, Ordering::SeqCst);
        }
        return;
    }
    if ACTIVE_SESSION.load(Ordering::Acquire) != 0 {
        SESSION_STOPPED_AT.store(EventTimestamp::now().ticks().max(1), Ordering::Release);
    }
    ACTIVE_SESSION.store(0, Ordering::SeqCst);
    EVENT_CAPACITY.store(capacity, Ordering::SeqCst);
    ALLOCATION_POLICY.store(allocation_policy, Ordering::SeqCst);
    GENERAL_POLICY.store(general_policy, Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::SeqCst);
    IO_POLICY.store(io_policy, Ordering::SeqCst);
    CACHE_POLICY.store(cache_policy, Ordering::SeqCst);
    if enabled {
        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        LAST_ALLOCATION_POLICY.store(allocation_policy, Ordering::Release);
        LAST_GENERAL_POLICY.store(general_policy, Ordering::Release);
        LAST_ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::Release);
        LAST_RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::Release);
        LAST_IO_POLICY.store(io_policy, Ordering::Release);
        LAST_CACHE_POLICY.store(cache_policy, Ordering::Release);
        LAST_SESSION.store(session, Ordering::Release);
        ACTIVITY_GENERATION.store(session, Ordering::Release);
        SESSION_STOPPED_AT.store(0, Ordering::Release);
        ACTIVE_SESSION.store(session, Ordering::Release);
    }
}

/// Returns the active runtime telemetry configuration.
#[cfg(any(test, feature = "monitor"))]
#[must_use]
pub(crate) fn configuration() -> Configuration {
    Configuration {
        allocations: decode_policy(ALLOCATION_POLICY.load(Ordering::Acquire)),
        general_events: decode_policy(GENERAL_POLICY.load(Ordering::Acquire)),
        arc_dereferences: decode_policy(ARC_DEREFERENCE_POLICY.load(Ordering::Acquire)),
        runtime_tasks: decode_policy(RUNTIME_TASK_POLICY.load(Ordering::Acquire)),
        io: decode_policy(IO_POLICY.load(Ordering::Acquire)),
        cache: decode_policy(CACHE_POLICY.load(Ordering::Acquire)),
        event_capacity_per_thread: decode_capacity(EVENT_CAPACITY.load(Ordering::Acquire)),
    }
}

/// Returns whether any event class is currently enabled.
#[doc(hidden)]
#[must_use]
#[inline]
pub fn recording_enabled() -> bool {
    !is_suppressed()
        && (policy_enabled(ALLOCATION_POLICY.load(Ordering::Relaxed))
            || policy_enabled(GENERAL_POLICY.load(Ordering::Relaxed))
            || policy_enabled(ARC_DEREFERENCE_POLICY.load(Ordering::Relaxed))
            || policy_enabled(RUNTIME_TASK_POLICY.load(Ordering::Relaxed))
            || policy_enabled(IO_POLICY.load(Ordering::Relaxed))
            || policy_enabled(CACHE_POLICY.load(Ordering::Relaxed)))
}

/// Returns whether one event class is currently enabled.
#[doc(hidden)]
#[must_use]
#[inline]
pub fn recording_enabled_for(class: EventClass) -> bool {
    policy_enabled(policy_atomic(class).load(Ordering::Relaxed)) && !is_suppressed()
}

/// Selects an object for recording and binds it to the active session.
#[must_use]
#[inline]
pub fn select_object(object_id: ObjectId) -> Option<RecordingSession> {
    select_object_for(EventClass::General, object_id)
}

/// Selects an object in one event class and binds it to the active session.
#[doc(hidden)]
#[must_use]
#[inline]
pub fn select_object_for(class: EventClass, object_id: ObjectId) -> Option<RecordingSession> {
    let policy = policy_atomic(class).load(Ordering::Relaxed);
    if !policy_enabled(policy) || is_suppressed() {
        return None;
    }
    let session = ACTIVE_SESSION.load(Ordering::Relaxed);
    if !decode_sampling(policy).includes(object_id) {
        return None;
    }
    if !selection_still_current(class, policy, session) {
        return None;
    }
    RecordingSession::from_raw(session)
}

#[inline]
#[cfg_attr(coverage_nightly, coverage(off))] // State can change only in the race window between the paired loads.
fn selection_still_current(class: EventClass, policy: u64, session: u64) -> bool {
    session != 0 && policy_atomic(class).load(Ordering::Acquire) == policy && ACTIVE_SESSION.load(Ordering::Acquire) == session
}

/// Reads recorder counters without copying retained events.
#[cfg(test)]
#[must_use]
pub(crate) fn statistics() -> Statistics {
    try_statistics().unwrap_or_else(|_error| Statistics {
        event_capacity_per_thread: u64::try_from(configuration().event_capacity_per_thread.get()).unwrap_or(u64::MAX),
        recording: last_recording_policies(),
        ..Statistics::default()
    })
}

#[cfg(any(test, feature = "monitor"))]
pub(crate) fn try_statistics() -> Result<Statistics, crate::Error> {
    read_activity(false).map(|activity| activity.statistics)
}

/// Reads counters without copying ring payloads or invoking snapshot sources.
#[cfg(any(test, feature = "monitor"))]
pub(crate) fn try_activity() -> Result<Activity, crate::Error> {
    read_activity(true)
}

#[cfg(any(test, feature = "monitor"))]
fn read_activity(include_threads: bool) -> Result<Activity, crate::Error> {
    let _suppression = SuppressionGuard::enter();
    let deadline = wait_deadline();
    // Keep counters, policies, and session identity on the same side of every
    // configuration/reset boundary, without quiescing concurrent event writers.
    let _configuration = ConfigurationLock::acquire_until(deadline)
        .ok_or_else(|| crate::Error::new("seismograph recorder statistics timed out waiting for configuration"))?;
    let session = LAST_SESSION.load(Ordering::Acquire);
    let capacity = configuration().event_capacity_per_thread;
    let mut activity = Activity {
        statistics: Statistics {
            event_capacity_per_thread: u64::try_from(capacity.get()).unwrap_or(u64::MAX),
            recording: last_recording_policies(),
            ..Statistics::default()
        },
        session_id: ACTIVITY_GENERATION.load(Ordering::Acquire),
        class_events: [0; 6],
        threads: Vec::new(),
    };
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    while !recorder.is_null() {
        // SAFETY: published recorders are retained for process lifetime.
        let current = unsafe { &*recorder };
        let _ring = current
            .ring_lock_until(deadline)
            .ok_or_else(|| crate::Error::new("seismograph recorder statistics timed out waiting for an event ring"))?;
        let ring_capacity = current.ring().map_or(0, Ring::capacity);
        activity.statistics.allocated_bytes = activity.statistics.allocated_bytes.saturating_add(
            u64::try_from(std::mem::size_of::<ThreadRecorder>() + ring_capacity * std::mem::size_of::<Slot>()).unwrap_or(u64::MAX),
        );
        if session != 0 && current.session.load(Ordering::Acquire) == session {
            let total_events = u64::try_from(current.write_index.load(Ordering::Acquire)).unwrap_or(u64::MAX);
            let retained_events = total_events.min(u64::try_from(ring_capacity).unwrap_or(u64::MAX));
            let lost_events = total_events.saturating_sub(retained_events);
            activity.statistics.thread_count = activity.statistics.thread_count.saturating_add(1);
            activity.statistics.total_events = activity.statistics.total_events.saturating_add(total_events);
            activity.statistics.retained_events = activity.statistics.retained_events.saturating_add(retained_events);
            activity.statistics.lost_events = activity.statistics.lost_events.saturating_add(lost_events);
        }
        if include_threads && session != 0 && current.activity_session.load(Ordering::Acquire) == session {
            let mut total_events = 0_u64;
            for (total, count) in activity.class_events.iter_mut().zip(&current.class_events) {
                let count = count.load(Ordering::Relaxed);
                *total = total.saturating_add(count);
                total_events = total_events.saturating_add(count);
            }
            let event_capacity = u64::try_from(ring_capacity).unwrap_or(u64::MAX);
            let retained_events = total_events.min(event_capacity);
            activity.threads.push(ThreadStatistics {
                thread_id: current.thread_id,
                name: String::from_utf8_lossy(&current.thread_name[..current.thread_name_len]).into_owned(),
                total_events,
                retained_events,
                lost_events: total_events.saturating_sub(retained_events),
                event_capacity,
                retired: current.retired.load(Ordering::Acquire),
            });
        }
        recorder = current.next.load(Ordering::Acquire);
    }
    Ok(activity)
}

/// Lazily constructs and records an event in a known class.
///
/// The builder may return `None` to omit the event without initializing a recorder.
#[inline]
pub(crate) fn record(class: EventClass, event: impl FnOnce() -> Option<Record>) {
    let _ = record_session(class, event);
}

/// Lazily constructs an event and returns the session that accepted it.
///
/// A builder returning `None` intentionally omits the event and returns no session.
#[inline]
pub(crate) fn record_session(class: EventClass, event: impl FnOnce() -> Option<Record>) -> Option<RecordingSession> {
    let policy = policy_atomic(class).load(Ordering::Relaxed);
    if !policy_enabled(policy) || is_suppressed() {
        return None;
    }
    let session = ACTIVE_SESSION.load(Ordering::Relaxed);
    if session == 0 {
        return None;
    }
    let record = event()?;
    if record.class() != class {
        return None;
    }
    if !decode_sampling(policy).includes(record.sampling_object_id()) {
        return None;
    }
    let capacity = EVENT_CAPACITY.load(Ordering::Relaxed);
    record_enabled(session, class, record, policy, capacity)
        .then(|| RecordingSession::from_raw(session).expect("recording requires a nonzero active session"))
}

/// Records an event only while its originating session remains active.
///
/// A builder returning `None` intentionally omits the event and returns `false`.
#[inline]
pub(crate) fn record_in_session(session: RecordingSession, event: impl FnOnce() -> Option<Record>) -> bool {
    record_in_session_classified(session, EventClass::General, event)
}

/// Records a classified event only while its originating session remains active.
///
/// A builder returning `None` intentionally omits the event and returns `false`
/// without initializing a recorder.
#[inline]
#[doc(hidden)]
pub fn record_in_session_classified(session: RecordingSession, class: EventClass, event: impl FnOnce() -> Option<Record>) -> bool {
    let policy = policy_atomic(class).load(Ordering::Relaxed);
    if !policy_enabled(policy) || is_suppressed() || ACTIVE_SESSION.load(Ordering::Relaxed) != session.get() {
        return false;
    }
    let Some(record) = event() else {
        return false;
    };
    if record.class() != class {
        return false;
    }
    if !decode_sampling(policy).includes(record.sampling_object_id()) {
        return false;
    }
    let capacity = EVENT_CAPACITY.load(Ordering::Relaxed);
    record_enabled(session.get(), class, record, policy, capacity)
}

#[cold]
fn record_enabled(session: u64, class: EventClass, record: Record, policy: u64, capacity: u64) -> bool {
    record_enabled_with_recorder(try_local_recorder(), session, class, record, policy, capacity)
}

fn record_enabled_with_recorder(
    recorder: Option<*const ThreadRecorder>,
    session: u64,
    class: EventClass,
    record: Record,
    policy: u64,
    capacity: u64,
) -> bool {
    let Some(recorder) = recorder else {
        return false;
    };
    // SAFETY: recorders are allocated through System, published once, and
    // intentionally retained for process lifetime.
    let recorder = unsafe { &*recorder };
    recorder.writer_active.store(true, Ordering::SeqCst);
    let _writer = WriterActiveGuard { recorder };
    if policy_atomic(class).load(Ordering::SeqCst) != policy
        || EVENT_CAPACITY.load(Ordering::SeqCst) != capacity
        || ACTIVE_SESSION.load(Ordering::SeqCst) != session
    {
        return false;
    }
    let capture_backtrace = match record.backtrace {
        BacktraceCapture::Configured => policy_backtraces(policy),
        BacktraceCapture::Never => false,
        BacktraceCapture::Always => true,
    };
    let (frames, frame_count) = if capture_backtrace {
        let _suppression = SuppressionGuard::enter();
        capture_stack()
    } else {
        ([0; MAX_STACK_FRAMES], 0)
    };
    recorder.record(session, decode_capacity(capacity), record, frames, frame_count)
}

/// Captures all currently retained runtime events.
#[cfg(test)]
#[must_use]
pub(crate) fn snapshot(disposition: crate::snapshot::EventBufferDisposition) -> Option<Events> {
    try_snapshot(disposition).ok().flatten()
}

#[cfg(test)]
pub(crate) fn try_snapshot(disposition: crate::snapshot::EventBufferDisposition) -> Result<Option<Events>, crate::Error> {
    try_snapshot_with_observation(disposition).map(|(events, _)| events)
}

pub(crate) fn try_snapshot_with_observation(
    disposition: crate::snapshot::EventBufferDisposition,
) -> Result<(Option<Events>, Option<RecordingObservation>), crate::Error> {
    let _suppression = SuppressionGuard::enter();
    if disposition != crate::snapshot::EventBufferDisposition::Retain {
        return reset_event_buffers(disposition, true);
    }
    let _configuration = ConfigurationLock::acquire_until(wait_deadline())
        .ok_or_else(|| crate::Error::new("seismograph snapshot timed out waiting for configuration"))?;
    let observation = recording_observation_locked();
    let session = LAST_SESSION.load(Ordering::Acquire);
    if session == 0 {
        return Ok((None, observation));
    }
    snapshot_from_recorders_until(session, RECORDERS.load(Ordering::Acquire), wait_deadline()).map(|events| (events, observation))
}

#[cfg(test)]
fn snapshot_from_recorders(session: u64, recorder: *mut ThreadRecorder) -> Option<Events> {
    snapshot_from_recorders_until(session, recorder, wait_deadline()).ok().flatten()
}

fn snapshot_from_recorders_until(
    session: u64,
    mut recorder: *mut ThreadRecorder,
    deadline: Instant,
) -> Result<Option<Events>, crate::Error> {
    if recorder.is_null() {
        return Ok(None);
    }
    let mut snapshot = Events {
        clock: EventClock::CURRENT,
        recording: last_recording_policies(),
        ..Events::default()
    };
    while !recorder.is_null() {
        // SAFETY: published recorders are retained for process lifetime.
        let current = unsafe { &*recorder };
        if current.session.load(Ordering::Acquire) == session {
            let thread = current
                .snapshot_until(deadline)
                .ok_or_else(|| crate::Error::new("seismograph snapshot timed out waiting for an event recorder"))?;
            snapshot.total_events = snapshot.total_events.saturating_add(thread.log.total_events);
            snapshot.lost_events = snapshot.lost_events.saturating_add(thread.log.lost_events);
            snapshot.threads.push(thread.log);
            snapshot.events.extend(thread.events);
        }
        recorder = current.next.load(Ordering::Acquire);
    }
    Ok(Some(snapshot))
}

/// Returns whether telemetry-internal work is suppressed on this thread.
#[doc(hidden)]
#[must_use]
pub fn is_suppressed() -> bool {
    SUPPRESSION_DEPTH.try_with(|depth| depth.get() != 0).unwrap_or(true)
}

/// Returns the current thread's process-unique recorder identity.
///
/// Calling this function initializes the thread-local recorder when necessary.
#[must_use]
pub fn current_thread_id() -> ThreadId {
    let recorder = local_recorder();
    // SAFETY: local_recorder returns a process-lifetime recorder allocated
    // through System and owned for writes by this thread.
    unsafe { &*recorder }.thread_id
}

/// Captures a backtrace using the active recording policy.
#[doc(hidden)]
#[must_use]
pub fn capture_backtrace(policy: BacktraceCapture) -> Vec<Address> {
    let runtime_policy = RUNTIME_TASK_POLICY.load(Ordering::Relaxed);
    if !policy_enabled(runtime_policy) {
        return Vec::new();
    }
    let enabled = match policy {
        BacktraceCapture::Configured => policy_backtraces(runtime_policy),
        BacktraceCapture::Never => false,
        BacktraceCapture::Always => true,
    };
    if !enabled {
        return Vec::new();
    }

    let _suppression = SuppressionGuard::enter();
    let (frames, frame_count) = capture_stack();
    frames[..usize::from(frame_count)].iter().copied().map(Address::new).collect()
}

/// Converts a captured return address to the address used for symbol lookup.
///
/// Windows stack capture reports return addresses. Looking up the preceding
/// instruction avoids attributing a boundary return address to the next
/// function while preserving the original captured address as its identity.
#[doc(hidden)]
#[must_use]
pub const fn symbol_lookup_address(address: Address) -> Address {
    #[cfg(windows)]
    {
        Address::new(address.get().saturating_sub(1))
    }
    #[cfg(not(windows))]
    {
        address
    }
}

/// A thread-local guard that suppresses telemetry-internal operations.
#[doc(hidden)]
#[derive(Debug)]
pub struct SuppressionGuard {
    entered: bool,
}

impl SuppressionGuard {
    /// Enters a nested telemetry-suppression scope.
    #[must_use]
    pub fn enter() -> Self {
        let entered = SUPPRESSION_DEPTH.try_with(|depth| depth.set(depth.get().saturating_add(1))).is_ok();
        Self { entered }
    }
}

impl Drop for SuppressionGuard {
    fn drop(&mut self) {
        let entered = usize::from(self.entered);
        let _ = SUPPRESSION_DEPTH.try_with(|depth| depth.set(depth.get().saturating_sub(entered)));
    }
}

struct ThreadRecorder {
    next: AtomicPtr<Self>,
    session: AtomicU64,
    activity_session: AtomicU64,
    thread_id: ThreadId,
    thread_name: [u8; MAX_THREAD_NAME_LEN],
    thread_name_len: usize,
    ring_locked: AtomicBool,
    ring: UnsafeCell<Option<Ring>>,
    ring_capacity: AtomicUsize,
    writer_active: AtomicBool,
    retired: AtomicBool,
    release_on_unlock: AtomicBool,
    write_index: AtomicUsize,
    class_events: [AtomicU64; 6],
}

struct WriterActiveGuard<'a> {
    recorder: &'a ThreadRecorder,
}

impl Drop for WriterActiveGuard<'_> {
    fn drop(&mut self) {
        self.recorder.writer_active.store(false, Ordering::Release);
    }
}

// SAFETY: the owning thread is the only event writer. Snapshots, resizing, and
// retirement serialize changes to `ring` with `ring_locked`, while individual
// slot payloads provide their own synchronization.
unsafe impl Sync for ThreadRecorder {}

impl ThreadRecorder {
    fn new() -> Self {
        let mut thread_name = [0; MAX_THREAD_NAME_LEN];
        let thread_name_len = std::thread::current().name().map_or(0, |name| {
            let bytes = name.as_bytes();
            let len = bytes.len().min(MAX_THREAD_NAME_LEN);
            thread_name[..len].copy_from_slice(&bytes[..len]);
            len
        });
        Self {
            next: AtomicPtr::new(ptr::null_mut()),
            session: AtomicU64::new(0),
            activity_session: AtomicU64::new(0),
            thread_id: ThreadId::new(NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed)),
            thread_name,
            thread_name_len,
            ring_locked: AtomicBool::new(false),
            ring: UnsafeCell::new(None),
            ring_capacity: AtomicUsize::new(0),
            writer_active: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            release_on_unlock: AtomicBool::new(false),
            write_index: AtomicUsize::new(0),
            class_events: [const { AtomicU64::new(0) }; 6],
        }
    }

    fn record(
        &self,
        session: u64,
        capacity: EventBufferCapacity,
        record: Record,
        frames: [u64; MAX_STACK_FRAMES],
        frame_count: u8,
    ) -> bool {
        let deadline = wait_deadline();
        let new_session = self.session.load(Ordering::Relaxed) != session;
        if new_session && !self.begin_session_until(session, capacity, deadline) {
            return false;
        }
        let index = self.write_index.fetch_add(1, Ordering::Relaxed);
        // Match write_index's accepted sequence attempts, including slot timeouts.
        // Only this thread increments; reset paths either belong to this writer,
        // quiesce it with writer_active, or operate after retirement. Readers use
        // atomics because ring_lock does not exclude live counter updates.
        let count = &self.class_events[class_index(record.class())];
        count.store(count.load(Ordering::Relaxed).saturating_add(1), Ordering::Relaxed);
        let ring = self.ring().expect("begin_session installs an event ring");
        let slot = &ring.slots[index & ring.mask];
        let Some(_slot) = slot.lock_until(deadline) else {
            return false;
        };
        // SAFETY: the slot lock provides exclusive access to its payload.
        unsafe {
            slot.data.get().write(EventData {
                sequence: index as u64 + 1,
                timestamp: record.timestamp,
                kind: record.kind,
                payload: record.payload,
                frame_count,
                frames,
            });
        }
        true
    }

    #[cfg(test)]
    fn snapshot(&self) -> ThreadSnapshot {
        self.snapshot_until(wait_deadline())
            .unwrap_or_else(|| panic!("event recorder snapshot did not complete within {RECORDER_WAIT_TIMEOUT:?}"))
    }

    fn snapshot_until(&self, deadline: Instant) -> Option<ThreadSnapshot> {
        let _ring = self.ring_lock_until(deadline)?;
        let total_events = self.write_index.load(Ordering::Acquire);
        let Some(ring) = self.ring() else {
            return Some(ThreadSnapshot {
                log: ThreadLog {
                    thread_id: self.thread_id,
                    total_events: 0,
                    lost_events: 0,
                    name: String::from_utf8_lossy(&self.thread_name[..self.thread_name_len]).into_owned(),
                },
                events: Vec::new(),
            });
        };
        let first = total_events.saturating_sub(ring.capacity());
        let mut events = Vec::with_capacity(total_events - first);
        for index in first..total_events {
            let slot = &ring.slots[index & ring.mask];
            let _slot = slot.lock_until(deadline)?;
            // SAFETY: the slot lock prevents the writer from modifying the
            // payload while it is copied.
            let data = unsafe { *slot.data.get() };
            if data.sequence == index as u64 + 1 {
                events.push(Event {
                    thread_id: self.thread_id,
                    sequence: EventSequence::new(data.sequence),
                    timestamp: data.timestamp,
                    kind: data.kind,
                    payload: data.payload,
                    call_stack: data.frames[..usize::from(data.frame_count)]
                        .iter()
                        .copied()
                        .map(Address::new)
                        .collect(),
                });
            }
        }
        let snapshot = ThreadSnapshot {
            log: ThreadLog {
                thread_id: self.thread_id,
                total_events: total_events as u64,
                lost_events: first as u64,
                name: String::from_utf8_lossy(&self.thread_name[..self.thread_name_len]).into_owned(),
            },
            events,
        };
        if self.retired.load(Ordering::Acquire) {
            self.release_on_unlock.store(true, Ordering::Release);
            forget_retired_ring(self);
        }
        Some(snapshot)
    }

    #[cfg(test)]
    fn begin_session(&self, session: u64, capacity: EventBufferCapacity) {
        assert!(
            self.begin_session_until(session, capacity, wait_deadline()),
            "event recorder session did not begin within {RECORDER_WAIT_TIMEOUT:?}"
        );
    }

    fn begin_session_until(&self, session: u64, capacity: EventBufferCapacity, deadline: Instant) -> bool {
        let Some(_ring) = self.ring_lock_until(deadline) else {
            return false;
        };
        if self.ring().is_some_and(|ring| ring.capacity() == capacity.get()) {
            self.reset_counts();
            self.activity_session.store(session, Ordering::Release);
            self.session.store(session, Ordering::Release);
            self.ring_capacity.store(capacity.get(), Ordering::Release);
            return true;
        }
        let replacement = Ring::new(capacity);
        // SAFETY: the owning writer is the only caller that replaces a live
        // ring, and ring_lock excludes snapshots and retirement.
        let previous = unsafe { (&mut *self.ring.get()).replace(replacement) };
        self.reset_counts();
        self.activity_session.store(session, Ordering::Release);
        self.session.store(session, Ordering::Release);
        self.ring_capacity.store(capacity.get(), Ordering::Release);
        drop(previous);
        true
    }

    fn clear(&self, release: bool, deadline: Instant) -> bool {
        let Some(_ring) = self.ring_lock_until(deadline) else {
            return false;
        };
        self.reset_counts();
        self.session.store(0, Ordering::Release);
        if release {
            self.release_ring_locked();
        }
        true
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        let ring_bytes = self
            .ring_capacity
            .load(Ordering::Acquire)
            .saturating_mul(std::mem::size_of::<Slot>());
        if ring_bytes != 0 {
            register_retired_ring(self, ring_bytes);
        }
    }

    #[cfg(test)]
    fn ring_lock(&self) -> RingLock<'_> {
        self.ring_lock_until(wait_deadline())
            .unwrap_or_else(|| panic!("event ring lock was not released within {RECORDER_WAIT_TIMEOUT:?}"))
    }

    fn ring_lock_until(&self, deadline: Instant) -> Option<RingLock<'_>> {
        while self
            .ring_locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if wait_expired(deadline) {
                return None;
            }
            std::hint::spin_loop();
        }
        Some(RingLock { recorder: self })
    }

    fn ring(&self) -> Option<&Ring> {
        // SAFETY: callers either own this recorder's writer context or hold
        // ring_lock. Ring replacement only occurs under those conditions.
        unsafe { (&*self.ring.get()).as_ref() }
    }

    fn release_ring_locked(&self) {
        forget_retired_ring(self);
        // Natural retirement releases only the ring. Activity counts and their
        // session identity remain in process-lifetime metadata for stable rates.
        self.write_index.store(0, Ordering::Relaxed);
        self.session.store(0, Ordering::Release);
        self.ring_capacity.store(0, Ordering::Release);
        // SAFETY: the caller holds ring_lock, excluding snapshots, resizing,
        // and retirement from accessing the ring concurrently.
        let ring = unsafe { (&mut *self.ring.get()).take() };
        drop(ring);
    }

    fn reset_counts(&self) {
        self.write_index.store(0, Ordering::Relaxed);
        self.activity_session.store(0, Ordering::Release);
        for count in &self.class_events {
            count.store(0, Ordering::Relaxed);
        }
    }

    #[cfg_attr(test, mutants::skip)] // Eviction release is covered with a held reader; removing it strands retired rings and blocks later tests.
    fn release_retired_ring(&self) {
        self.release_on_unlock.store(true, Ordering::Release);
        forget_retired_ring(self);
        let _ring = self.ring_lock_until(wait_deadline());
    }
}

const fn class_index(class: EventClass) -> usize {
    match class {
        EventClass::Allocation => 0,
        EventClass::General => 1,
        EventClass::ArcDereference => 2,
        EventClass::RuntimeTask => 3,
        EventClass::Io => 4,
        EventClass::Cache => 5,
    }
}

struct RetiredRings {
    rings: VecDeque<(usize, usize)>,
    allocated_bytes: usize,
}

fn retired_rings() -> std::sync::MutexGuard<'static, RetiredRings> {
    RETIRED_RINGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn register_retired_ring(recorder: &ThreadRecorder, ring_bytes: usize) {
    let address = ptr::from_ref(recorder) as usize;
    let limit = RETIRED_RING_BUDGET_BYTES.max(ring_bytes);
    {
        let mut retired = retired_rings();
        retired.rings.push_back((address, ring_bytes));
        retired.allocated_bytes = retired.allocated_bytes.saturating_add(ring_bytes);
    }

    loop {
        let evicted = {
            let mut retired = retired_rings();
            if retired.allocated_bytes <= limit {
                None
            } else {
                retired.rings.pop_front().map(|(address, bytes)| {
                    retired.allocated_bytes = retired.allocated_bytes.saturating_sub(bytes);
                    address
                })
            }
        };
        let Some(evicted) = evicted else {
            break;
        };
        // SAFETY: published recorders are retained for process lifetime.
        unsafe { &*(evicted as *const ThreadRecorder) }.release_retired_ring();
    }
}

fn forget_retired_ring(recorder: &ThreadRecorder) {
    let address = ptr::from_ref(recorder) as usize;
    let mut retired = retired_rings();
    if let Some(index) = retired
        .rings
        .iter()
        .position(|(candidate, _)| retired_ring_matches(*candidate, address))
        && let Some((_, bytes)) = retired.rings.remove(index)
    {
        retired.allocated_bytes = retired.allocated_bytes.saturating_sub(bytes);
    }
}

#[cfg_attr(test, mutants::skip)] // Inverting identity releases another recorder's allocation.
const fn retired_ring_matches(candidate: usize, address: usize) -> bool {
    candidate == address
}

struct Ring {
    slots: SystemSlice<Slot>,
    mask: usize,
}

impl Ring {
    fn new(capacity: EventBufferCapacity) -> Self {
        Self {
            slots: SystemSlice::from_fn(capacity.get(), |_| Slot::new()),
            mask: capacity.get() - 1,
        }
    }

    const fn capacity(&self) -> usize {
        self.mask + 1
    }
}

struct RingLock<'a> {
    recorder: &'a ThreadRecorder,
}

impl Drop for RingLock<'_> {
    fn drop(&mut self) {
        if self.recorder.release_on_unlock.load(Ordering::Acquire) {
            self.recorder.release_ring_locked();
        }
        self.recorder.ring_locked.store(false, Ordering::Release);
    }
}

struct ThreadSnapshot {
    log: ThreadLog,
    events: Vec<Event>,
}

struct Slot {
    locked: AtomicBool,
    data: UnsafeCell<EventData>,
}

// SAFETY: all access to the UnsafeCell payload is serialized by `locked`.
unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(EventData::EMPTY),
        }
    }

    #[cfg(test)]
    fn lock(&self) {
        let guard = self
            .lock_until(wait_deadline())
            .unwrap_or_else(|| panic!("event slot lock was not released within {RECORDER_WAIT_TIMEOUT:?}"));
        std::mem::forget(guard);
    }

    fn lock_until(&self, deadline: Instant) -> Option<SlotLock<'_>> {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if wait_expired(deadline) {
                return None;
            }
            std::hint::spin_loop();
        }
        Some(SlotLock { slot: self })
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

struct SlotLock<'a> {
    slot: &'a Slot,
}

impl Drop for SlotLock<'_> {
    fn drop(&mut self) {
        self.slot.unlock();
    }
}

#[derive(Clone, Copy)]
struct EventData {
    sequence: u64,
    timestamp: event::EventTimestamp,
    kind: EventKind,
    payload: EventPayload,
    frame_count: u8,
    frames: [u64; MAX_STACK_FRAMES],
}

impl EventData {
    const EMPTY: Self = Self {
        sequence: 0,
        timestamp: event::EventTimestamp::from_ticks(0),
        kind: EventKind::ArcDeref,
        payload: EventPayload::Object(ObjectId::new(0)),
        frame_count: 0,
        frames: [0; MAX_STACK_FRAMES],
    };
}

struct LocalRecorder {
    recorder: Cell<*const ThreadRecorder>,
}

impl LocalRecorder {
    const fn new() -> Self {
        Self {
            recorder: Cell::new(ptr::null()),
        }
    }
}

impl Drop for LocalRecorder {
    fn drop(&mut self) {
        let recorder = self.recorder.get();
        if recorder.is_null() {
            return;
        }
        // SAFETY: this TLS owner is the recorder's only writer, and TLS
        // destruction begins only after that thread has stopped recording.
        unsafe { &*recorder }.retire();
    }
}

struct ConfigurationLock;

impl ConfigurationLock {
    fn acquire() -> Self {
        loop {
            if let Some(lock) = Self::acquire_until(wait_deadline()) {
                return lock;
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Lock timeout and reacquisition are tested directly; returning None unconditionally deadlocks configuration.
    fn acquire_until(deadline: Instant) -> Option<Self> {
        while CONFIGURATION_LOCKED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if wait_expired(deadline) {
                return None;
            }
            std::hint::spin_loop();
        }
        Some(Self)
    }
}

impl Drop for ConfigurationLock {
    fn drop(&mut self) {
        CONFIGURATION_LOCKED.store(false, Ordering::Release);
    }
}

const fn encode_policy(policy: RecordingPolicy) -> u64 {
    let mut flags = 0;
    if policy.enabled {
        flags |= RECORDING_ENABLED;
    }
    if policy.capture_backtraces {
        flags |= BACKTRACES_ENABLED;
    }
    (flags as u64) + ((policy.event_sampling.get() as u64) << SAMPLING_SHIFT)
}

const fn decode_policy(policy: u64) -> RecordingPolicy {
    RecordingPolicy {
        enabled: policy_enabled(policy),
        capture_backtraces: policy_backtraces(policy),
        event_sampling: decode_sampling(policy),
    }
}

const fn policy_enabled(policy: u64) -> bool {
    policy & (RECORDING_ENABLED as u64) != 0
}

const fn policy_backtraces(policy: u64) -> bool {
    policy & (BACKTRACES_ENABLED as u64) != 0
}

const fn policy_atomic(class: EventClass) -> &'static AtomicU64 {
    match class {
        EventClass::Allocation => &ALLOCATION_POLICY,
        EventClass::General => &GENERAL_POLICY,
        EventClass::ArcDereference => &ARC_DEREFERENCE_POLICY,
        EventClass::RuntimeTask => &RUNTIME_TASK_POLICY,
        EventClass::Io => &IO_POLICY,
        EventClass::Cache => &CACHE_POLICY,
    }
}

const fn decode_capacity(capacity: u64) -> EventBufferCapacity {
    EventBufferCapacity::from_exponent((capacity & 0x3f) as u32)
}

const fn decode_sampling(policy: u64) -> EventSampling {
    let denominator = ((policy >> SAMPLING_SHIFT) & SAMPLING_MASK) as usize;
    if denominator == 0 {
        EventSampling::ALL
    } else {
        EventSampling(denominator)
    }
}

fn last_recording_policies() -> RecordingPolicies {
    RecordingPolicies {
        allocations: decode_policy(LAST_ALLOCATION_POLICY.load(Ordering::Acquire)),
        general_events: decode_policy(LAST_GENERAL_POLICY.load(Ordering::Acquire)),
        arc_dereferences: decode_policy(LAST_ARC_DEREFERENCE_POLICY.load(Ordering::Acquire)),
        runtime_tasks: decode_policy(LAST_RUNTIME_TASK_POLICY.load(Ordering::Acquire)),
        io: decode_policy(LAST_IO_POLICY.load(Ordering::Acquire)),
        cache: decode_policy(LAST_CACHE_POLICY.load(Ordering::Acquire)),
    }
}

#[cfg(test)]
fn destructive_snapshot(disposition: crate::snapshot::EventBufferDisposition) -> Result<Option<Events>, crate::Error> {
    reset_event_buffers(disposition, true).map(|(events, _)| events)
}

/// Empties all event rings without capturing a snapshot or invoking snapshot sources.
///
/// Active-thread allocations and all recording policies are preserved. Exited-thread
/// allocations are released; process-lifetime recorder metadata remains registered.
///
/// # Errors
///
/// Returns an error if configuration, writers, or ring readers do not quiesce in
/// time. Policies are restored on failure; some rings may already have been emptied.
pub fn clear_event_buffers() -> Result<(), crate::Error> {
    let _suppression = SuppressionGuard::enter();
    reset_event_buffers(crate::snapshot::EventBufferDisposition::Clear, false).map(|_| ())
}

fn reset_event_buffers(
    disposition: crate::snapshot::EventBufferDisposition,
    capture: bool,
) -> Result<(Option<Events>, Option<RecordingObservation>), crate::Error> {
    let deadline = wait_deadline();
    let _configuration = ConfigurationLock::acquire_until(deadline)
        .ok_or_else(|| crate::Error::new("seismograph recorder timed out waiting for configuration"))?;
    let stop = disposition == crate::snapshot::EventBufferDisposition::Stop;
    let allocation_policy = ALLOCATION_POLICY.load(Ordering::SeqCst);
    let general_policy = GENERAL_POLICY.load(Ordering::SeqCst);
    let arc_dereference_policy = ARC_DEREFERENCE_POLICY.load(Ordering::SeqCst);
    let runtime_task_policy = RUNTIME_TASK_POLICY.load(Ordering::SeqCst);
    let io_policy = IO_POLICY.load(Ordering::SeqCst);
    let cache_policy = CACHE_POLICY.load(Ordering::SeqCst);
    let previous_stopped_at = SESSION_STOPPED_AT.load(Ordering::Acquire);
    if ACTIVE_SESSION.load(Ordering::Acquire) != 0 {
        SESSION_STOPPED_AT.store(EventTimestamp::now().ticks().max(1), Ordering::Release);
    }
    let was_enabled = ACTIVE_SESSION.swap(0, Ordering::SeqCst) != 0;
    ALLOCATION_POLICY.store(disabled_policy(allocation_policy), Ordering::SeqCst);
    GENERAL_POLICY.store(disabled_policy(general_policy), Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(disabled_policy(arc_dereference_policy), Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(disabled_policy(runtime_task_policy), Ordering::SeqCst);
    IO_POLICY.store(disabled_policy(io_policy), Ordering::SeqCst);
    CACHE_POLICY.store(disabled_policy(cache_policy), Ordering::SeqCst);
    let observation = recording_observation_locked();

    if !wait_for_writers_until(deadline) {
        SESSION_STOPPED_AT.store(previous_stopped_at, Ordering::Release);
        restore_recording_policies(
            allocation_policy,
            general_policy,
            arc_dereference_policy,
            runtime_task_policy,
            io_policy,
            cache_policy,
            was_enabled.then(|| LAST_SESSION.load(Ordering::Acquire)),
        );
        return Err(crate::Error::new("seismograph snapshot timed out waiting for active event writers"));
    }
    let captured = if capture {
        snapshot_session_until(LAST_SESSION.load(Ordering::Acquire), deadline)
    } else {
        Ok(None)
    };
    let snapshot = match captured {
        Ok(snapshot) => snapshot,
        Err(error) => {
            SESSION_STOPPED_AT.store(previous_stopped_at, Ordering::Release);
            restore_recording_policies(
                allocation_policy,
                general_policy,
                arc_dereference_policy,
                runtime_task_policy,
                io_policy,
                cache_policy,
                was_enabled.then(|| LAST_SESSION.load(Ordering::Acquire)),
            );
            return Err(error);
        }
    };
    let release = stop || disposition == crate::snapshot::EventBufferDisposition::Release;
    // Publish a fresh rate boundary before any destructive mutation, including
    // a clear that fails after resetting only some of the registered recorders.
    let next_session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    ACTIVITY_GENERATION.store(next_session, Ordering::Release);
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    while !recorder.is_null() {
        // SAFETY: recorders remain registered for process lifetime.
        let current = unsafe { &*recorder };
        if !current.clear(release || current.retired.load(Ordering::Acquire), deadline) {
            if !stop {
                SESSION_STOPPED_AT.store(previous_stopped_at, Ordering::Release);
                restore_recording_policies(
                    allocation_policy,
                    general_policy,
                    arc_dereference_policy,
                    runtime_task_policy,
                    io_policy,
                    cache_policy,
                    was_enabled.then(|| LAST_SESSION.load(Ordering::Acquire)),
                );
            }
            return Err(crate::Error::new("seismograph snapshot timed out clearing an event recorder"));
        }
        recorder = current.next.load(Ordering::Acquire);
    }

    if stop {
        return Ok((snapshot, observation));
    }
    // Clear must invalidate recording-only source state even while stopped.
    LAST_SESSION.store(next_session, Ordering::Release);
    if was_enabled {
        SESSION_STOPPED_AT.store(0, Ordering::Release);
        ACTIVE_SESSION.store(next_session, Ordering::SeqCst);
    }
    ALLOCATION_POLICY.store(allocation_policy, Ordering::SeqCst);
    GENERAL_POLICY.store(general_policy, Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::SeqCst);
    IO_POLICY.store(io_policy, Ordering::SeqCst);
    CACHE_POLICY.store(cache_policy, Ordering::SeqCst);
    Ok((snapshot, observation))
}

const fn disabled_policy(policy: u64) -> u64 {
    policy & !(RECORDING_ENABLED as u64)
}

#[cfg(test)]
fn wait_for_writers() {
    assert!(
        wait_for_writers_until(wait_deadline()),
        "event writers did not quiesce within {RECORDER_WAIT_TIMEOUT:?}"
    );
}

fn wait_for_writers_until(deadline: Instant) -> bool {
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    while !recorder.is_null() {
        // SAFETY: recorders remain registered for process lifetime.
        let current = unsafe { &*recorder };
        while current.writer_active.load(Ordering::SeqCst) {
            if wait_expired(deadline) {
                return false;
            }
            std::hint::spin_loop();
        }
        recorder = current.next.load(Ordering::Acquire);
    }
    true
}

#[cfg(test)]
fn snapshot_session(session: u64) -> Option<Events> {
    snapshot_session_until(session, wait_deadline()).ok().flatten()
}

fn snapshot_session_until(session: u64, deadline: Instant) -> Result<Option<Events>, crate::Error> {
    if session == 0 {
        return Ok(None);
    }
    snapshot_from_recorders_until(session, RECORDERS.load(Ordering::Acquire), deadline)
}

fn restore_recording_policies(
    allocation_policy: u64,
    general_policy: u64,
    arc_dereference_policy: u64,
    runtime_task_policy: u64,
    io_policy: u64,
    cache_policy: u64,
    session: Option<u64>,
) {
    ALLOCATION_POLICY.store(allocation_policy, Ordering::SeqCst);
    GENERAL_POLICY.store(general_policy, Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::SeqCst);
    IO_POLICY.store(io_policy, Ordering::SeqCst);
    CACHE_POLICY.store(cache_policy, Ordering::SeqCst);
    ACTIVE_SESSION.store(session.unwrap_or(0), Ordering::SeqCst);
}

fn wait_deadline() -> Instant {
    Instant::now().checked_add(RECORDER_WAIT_TIMEOUT).unwrap_or_else(Instant::now)
}

#[cfg_attr(test, mutants::skip)] // Equality with a freshly sampled Instant is nondeterministic under mutation scheduling.
fn wait_expired(deadline: Instant) -> bool {
    if Instant::now() < deadline {
        std::thread::yield_now();
        false
    } else {
        true
    }
}

fn local_recorder() -> *const ThreadRecorder {
    LOCAL_RECORDER.with(initialize_local_recorder)
}

fn try_local_recorder() -> Option<*const ThreadRecorder> {
    LOCAL_RECORDER.try_with(initialize_local_recorder).ok()
}

#[cold]
fn initialize_local_recorder(local: &LocalRecorder) -> *const ThreadRecorder {
    let existing = local.recorder.get();
    if !existing.is_null() {
        return existing;
    }

    let _suppression = SuppressionGuard::enter();
    let layout = Layout::new::<ThreadRecorder>();
    let allocated = allocate_thread_recorder(layout);
    // SAFETY: allocated is properly aligned writable storage for one value.
    unsafe { allocated.write(ThreadRecorder::new()) };
    RECORDERS
        .fetch_update(Ordering::Release, Ordering::Acquire, |head| {
            // SAFETY: allocated points to the initialized recorder owned by this
            // thread until this atomic update publishes it.
            unsafe { (*allocated).next.store(head, Ordering::Relaxed) };
            Some(allocated)
        })
        .expect("the recorder registry update closure always returns Some");
    local.recorder.set(allocated);
    allocated
}

#[cfg_attr(coverage_nightly, coverage(off))] // System allocator OOM aborts the process and cannot be exercised by a unit test.
#[expect(
    clippy::cast_ptr_alignment,
    reason = "System allocation uses Layout::new::<ThreadRecorder>(), which guarantees the target alignment"
)]
fn allocate_thread_recorder(layout: Layout) -> *mut ThreadRecorder {
    // SAFETY: layout describes one ThreadRecorder and System bypasses the
    // process global allocator.
    let allocated = unsafe { System.alloc(layout) }.cast::<ThreadRecorder>();
    if allocated.is_null() {
        handle_alloc_error(layout);
    }
    allocated
}

fn capture_stack() -> ([u64; MAX_STACK_FRAMES], u8) {
    let mut frames = [0; MAX_STACK_FRAMES];
    let frame_count = capture_platform_stack(&mut frames);
    (
        frames,
        u8::try_from(frame_count).expect("frame count is bounded by the 24-element capture buffer"),
    )
}

fn capture_platform_stack(frames: &mut [u64]) -> usize {
    #[cfg(all(target_os = "windows", not(miri)))]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;

        let mut addresses = [0usize; MAX_STACK_FRAMES];
        // SAFETY: addresses is writable for the requested number of entries.
        let count = unsafe {
            RtlCaptureStackBackTrace(
                4,
                u32::try_from(addresses.len()).expect("the fixed frame buffer fits in u32"),
                addresses.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            )
        } as usize;
        for (destination, address) in frames.iter_mut().zip(addresses).take(count) {
            *destination = address as u64;
        }
        count
    }

    #[cfg(all(target_os = "linux", not(miri)))]
    {
        const SKIPPED_FRAMES: usize = 4;
        const CAPACITY: usize = 28;
        let mut addresses = [0usize; CAPACITY];
        let capacity = i32::try_from(CAPACITY).expect("the fixed frame buffer fits in i32");
        // SAFETY: addresses is writable for CAPACITY pointers.
        let count = unsafe { libc::backtrace(addresses.as_mut_ptr().cast(), capacity) }.max(0);
        let count = usize::try_from(count).expect("the nonnegative frame count fits in usize");
        let retained = count.saturating_sub(SKIPPED_FRAMES).min(frames.len());
        for (destination, address) in frames.iter_mut().zip(addresses.into_iter().skip(SKIPPED_FRAMES)).take(retained) {
            *destination = address as u64;
        }
        retained
    }

    #[cfg(any(miri, not(any(target_os = "windows", target_os = "linux"))))]
    {
        let _ = frames;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVITY_CLASSES: [(EventClass, EventKind); 6] = [
        (EventClass::Allocation, EventKind::Allocation),
        (EventClass::General, EventKind::MutexAccess),
        (EventClass::ArcDereference, EventKind::ArcDeref),
        (EventClass::RuntimeTask, EventKind::TaskSpawned),
        (EventClass::Io, EventKind::IoReadStarted),
        (EventClass::Cache, EventKind::CacheHit),
    ];

    #[test]
    fn activity_attributes_all_classes_and_excludes_suppressed_and_disabled_events() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        for (index, (class, kind)) in ACTIVITY_CLASSES.into_iter().enumerate() {
            for _ in 0..=index {
                record(class, || Some(Record::object(kind, ObjectId::new(1))));
            }
            let _suppression = SuppressionGuard::enter();
            record(class, || panic!("suppressed events must not be constructed"));
        }
        let before = try_activity().unwrap();
        configure(Configuration::default());
        for (class, _) in ACTIVITY_CLASSES {
            record(class, || panic!("disabled events must not be constructed"));
        }
        let after = try_activity().unwrap();
        assert_eq!(
            (
                before.class_events,
                before.statistics.total_events,
                after.class_events,
                after.threads
            ),
            ([1, 2, 3, 4, 5, 6], 21, [1, 2, 3, 4, 5, 6], before.threads)
        );
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_excludes_sampled_out_events_in_every_class() {
        let _test = TEST_LOCK.lock().unwrap();
        let sampling = EventSampling::one_in(2).unwrap();
        let policy = RecordingPolicy {
            event_sampling: sampling,
            ..RecordingPolicy::all(false)
        };
        configure(Configuration {
            allocations: policy,
            general_events: policy,
            arc_dereferences: policy,
            runtime_tasks: policy,
            io: policy,
            cache: policy,
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
        });
        clear_event_buffers().unwrap();
        let selected = (1..100).map(ObjectId::new).find(|id| sampling.includes(*id)).unwrap();
        let skipped = (1..100).map(ObjectId::new).find(|id| !sampling.includes(*id)).unwrap();
        for (class, kind) in ACTIVITY_CLASSES {
            record(class, || Some(Record::object(kind, skipped)));
        }
        let excluded = try_activity().unwrap();
        for (class, kind) in ACTIVITY_CLASSES {
            record(class, || Some(Record::object(kind, selected)));
        }
        record(EventClass::General, || Some(Record::object(EventKind::ArcDeref, selected)));
        let included = try_activity().unwrap();
        assert_eq!(
            (
                excluded.class_events,
                excluded.threads.len(),
                included.class_events,
                included.statistics.total_events
            ),
            ([0; 6], 0, [1; 6], 6)
        );
        configure(Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_reports_actual_ring_fill_before_and_after_wrap() {
        let _test = TEST_LOCK.lock().unwrap();
        for capacity in [64, 128] {
            configure(Configuration {
                general_events: RecordingPolicy::all(false),
                event_capacity_per_thread: EventBufferCapacity::new(capacity).unwrap(),
                ..Default::default()
            });
            clear_event_buffers().unwrap();
            let thread_id = current_thread_id();
            for count in 1..=capacity + 3 {
                record(EventClass::General, || {
                    Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
                });
                if [31, capacity, capacity + 3].contains(&count) {
                    let activity = try_activity().unwrap();
                    let thread = &activity.threads[0];
                    assert_eq!(
                        (
                            thread.thread_id,
                            thread.total_events,
                            thread.retained_events,
                            thread.lost_events,
                            thread.event_capacity,
                            thread.retired,
                            activity.class_events,
                        ),
                        (
                            thread_id,
                            count as u64,
                            count.min(capacity) as u64,
                            count.saturating_sub(capacity) as u64,
                            capacity as u64,
                            false,
                            [0, count as u64, 0, 0, 0, 0],
                        )
                    );
                }
            }
            configure(Configuration {
                event_capacity_per_thread: EventBufferCapacity::new(capacity * 2).unwrap(),
                ..Default::default()
            });
            let stopped = try_activity().unwrap();
            assert_eq!(
                (stopped.statistics.event_capacity_per_thread, stopped.threads[0].event_capacity),
                ((capacity * 2) as u64, capacity as u64)
            );
        }
        configure(Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_resets_with_clear_release_stop_and_restart() {
        let _test = TEST_LOCK.lock().unwrap();
        let enabled = timeout_configuration();
        configure(enabled);
        clear_event_buffers().unwrap();
        for disposition in [
            crate::snapshot::EventBufferDisposition::Clear,
            crate::snapshot::EventBufferDisposition::Release,
            crate::snapshot::EventBufferDisposition::Stop,
        ] {
            record(EventClass::Cache, || Some(Record::object(EventKind::CacheHit, ObjectId::new(1))));
            let before = try_activity().unwrap();
            if disposition == crate::snapshot::EventBufferDisposition::Clear {
                clear_event_buffers().unwrap();
            } else {
                try_snapshot(disposition).unwrap();
            }
            let after = try_activity().unwrap();
            assert_eq!(
                (after.class_events, after.statistics.total_events, after.threads.len()),
                ([0; 6], 0, 0)
            );
            assert_ne!(after.session_id, before.session_id);
            if disposition == crate::snapshot::EventBufferDisposition::Stop {
                configure(enabled);
            } else {
                assert_eq!(configuration(), enabled);
            }
            record(EventClass::Cache, || Some(Record::object(EventKind::CacheMiss, ObjectId::new(1))));
            let restarted = try_activity().unwrap();
            assert_ne!(restarted.session_id, before.session_id);
            assert_eq!((restarted.class_events, restarted.threads[0].total_events), ([0, 0, 0, 0, 0, 1], 1));
            clear_event_buffers().unwrap();
        }
        configure(Configuration {
            event_capacity_per_thread: EventBufferCapacity::new(128).unwrap(),
            ..enabled
        });
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        let resized = try_activity().unwrap();
        assert_eq!((resized.class_events, resized.threads[0].event_capacity), ([0, 1, 0, 0, 0, 0], 128));
        configure(Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_does_not_read_locked_event_slots_or_change_recorder_state() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        // SAFETY: this thread owns the process-lifetime recorder and its writer context.
        let recorder = unsafe { &*local_recorder() };
        let before = try_activity().unwrap();
        let ring_address = recorder.ring().unwrap().slots.as_ptr();
        let slot = recorder.ring().unwrap().slots[0].lock_until(wait_deadline()).unwrap();
        let after = try_activity().unwrap();
        assert_eq!(
            (after.statistics, after.session_id, after.class_events, after.threads),
            (before.statistics, before.session_id, before.class_events, before.threads)
        );
        assert_eq!(recorder.ring().unwrap().slots.as_ptr(), ring_address);
        drop(slot);
        configure(Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_retains_exited_thread_counts_after_ring_release() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("activity-worker".into())
            .spawn(move || {
                record(EventClass::General, || {
                    Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
                });
                ready_tx.send(current_thread_id()).unwrap();
                exit_rx.recv().unwrap();
            })
            .unwrap();
        let thread_id = ready_rx.recv().unwrap();
        let active = try_activity().unwrap();
        assert_eq!(
            active.threads,
            vec![ThreadStatistics {
                thread_id,
                name: "activity-worker".into(),
                total_events: 1,
                retained_events: 1,
                lost_events: 0,
                event_capacity: 64,
                retired: false,
            }]
        );
        exit_tx.send(()).unwrap();
        worker.join().unwrap();
        let retired = try_activity().unwrap();
        let reread = try_activity().unwrap();
        assert_eq!((retired.threads[0].retired, retired.class_events), (true, [0, 1, 0, 0, 0, 0]));
        assert_eq!(
            (reread.statistics, reread.threads, reread.class_events),
            (retired.statistics, retired.threads, retired.class_events)
        );
        let captured = try_snapshot(crate::snapshot::EventBufferDisposition::Retain).unwrap().unwrap();
        let released = try_activity().unwrap();
        assert_eq!(
            (
                captured.events.len(),
                released.session_id,
                released.statistics.total_events,
                released.class_events,
                released.threads,
            ),
            (
                1,
                active.session_id,
                0,
                [0, 1, 0, 0, 0, 0],
                vec![ThreadStatistics {
                    thread_id,
                    name: "activity-worker".into(),
                    total_events: 1,
                    retained_events: 0,
                    lost_events: 1,
                    event_capacity: 0,
                    retired: true,
                }],
            )
        );
        configure(Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_class_totals_survive_retired_ring_eviction_with_concurrent_growth() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        let emit_classes = || {
            std::thread::spawn(|| {
                for (class, kind) in ACTIVITY_CLASSES {
                    record(class, || Some(Record::object(kind, ObjectId::new(1))));
                }
                current_thread_id()
            })
            .join()
            .unwrap()
        };
        let evicted_id = emit_classes();
        let before = try_activity().unwrap();
        assert_eq!(before.class_events, [1; 6]);
        let retained_id = emit_classes();
        for _ in 0..10 {
            record(EventClass::General, || {
                Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
            });
        }
        let after = try_activity().unwrap();
        let evicted = after.threads.iter().find(|thread| thread.thread_id == evicted_id).unwrap();
        let retained = after.threads.iter().find(|thread| thread.thread_id == retained_id).unwrap();
        assert_eq!(
            (
                after.session_id,
                after.class_events,
                after.statistics.total_events,
                (
                    evicted.total_events,
                    evicted.event_capacity,
                    evicted.retained_events,
                    evicted.lost_events
                ),
                (retained.total_events, retained.event_capacity, retained.retained_events),
            ),
            (before.session_id, [2, 12, 2, 2, 2, 2], 16, (6, 0, 0, 6), (6, 64, 6))
        );
        try_snapshot(crate::snapshot::EventBufferDisposition::Retain).unwrap();
        let captured = try_activity().unwrap();
        assert_eq!(
            (
                captured.session_id,
                captured.class_events,
                captured.statistics.total_events,
                captured.threads.len()
            ),
            (before.session_id, [2, 12, 2, 2, 2, 2], 10, 3)
        );
        clear_event_buffers().unwrap();
        let cleared = try_activity().unwrap();
        assert_ne!(cleared.session_id, before.session_id);
        assert_eq!((cleared.class_events, cleared.threads.len()), ([0; 6], 0));
        configure(Configuration::default());
    }

    #[test]
    fn activity_partial_clear_failure_changes_the_rate_generation() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        std::thread::spawn(|| {
            record(EventClass::General, || {
                Some(Record::object(EventKind::MutexAccess, ObjectId::new(2)))
            });
        })
        .join()
        .unwrap();
        let before = try_activity().unwrap();
        let source_session = recording_observation().unwrap().session;
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        let ring = recorder.ring_lock();
        let failed = clear_event_buffers().is_err();
        drop(ring);
        let after = try_activity().unwrap();
        assert_eq!(
            (
                failed,
                before.class_events,
                after.class_events,
                recording_observation().unwrap().session
            ),
            (true, [0, 2, 0, 0, 0, 0], [0, 1, 0, 0, 0, 0], source_session)
        );
        assert_ne!(after.session_id, before.session_id);
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(3)))
        });
        let resumed = try_activity().unwrap();
        assert_eq!((resumed.session_id, resumed.class_events), (after.session_id, [0, 2, 0, 0, 0, 0]));
        configure(Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn activity_read_cannot_cross_a_clear_session_boundary() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        let before = try_activity().unwrap();
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        let ring = recorder.ring_lock();
        let reader = std::thread::spawn(|| try_activity().unwrap());
        let deadline = wait_deadline();
        while !CONFIGURATION_LOCKED.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "activity reader did not acquire the configuration lock");
            std::thread::yield_now();
        }
        let reset = std::thread::spawn(clear_event_buffers);
        drop(ring);
        let observed = reader.join().unwrap();
        reset.join().unwrap().unwrap();
        let cleared = try_activity().unwrap();
        assert_eq!(
            (observed.session_id, observed.class_events),
            (before.session_id, [0, 1, 0, 0, 0, 0])
        );
        assert_ne!(cleared.session_id, observed.session_id);
        assert_eq!((cleared.class_events, cleared.threads.len()), ([0; 6], 0));
        configure(Configuration::default());
    }

    #[test]
    fn contended_ring_rejects_a_new_session_without_changing_the_old_one() {
        let recorder = ThreadRecorder::new();
        let capacity = EventBufferCapacity::new(64).unwrap();
        let event = Record::object(EventKind::MutexAccess, ObjectId::new(1));
        assert!(recorder.record(1, capacity, event, [0; MAX_STACK_FRAMES], 0));
        let ring = recorder.ring_lock();
        let accepted = recorder.record(2, capacity, event, [0; MAX_STACK_FRAMES], 0);
        drop(ring);
        let captured = recorder.snapshot();
        assert_eq!(
            (
                accepted,
                recorder.session.load(Ordering::Acquire),
                captured.log.total_events,
                captured.events.len()
            ),
            (false, 1, 1, 1)
        );
    }

    #[test]
    fn contended_slot_drops_an_event_and_releases_the_writer() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        });
        let event = Record::object(EventKind::MutexAccess, ObjectId::new(1));
        let session = record_session(EventClass::General, || Some(event)).unwrap();
        // SAFETY: this thread owns the process-lifetime recorder and its writer context.
        let recorder = unsafe { &*local_recorder() };
        let slot = recorder.ring().unwrap().slots[1].lock_until(wait_deadline()).unwrap();
        let accepted = record_in_session(session, || Some(event));
        drop(slot);
        let writer_active = recorder.writer_active.load(Ordering::Acquire);
        let resumed = record_in_session(session, || Some(event));
        let activity = try_activity().unwrap();
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();
        configure(Configuration::default());
        assert_eq!(
            (
                accepted,
                writer_active,
                resumed,
                activity.class_events,
                captured.events.iter().map(|event| event.sequence.get()).collect::<Vec<_>>()
            ),
            (false, false, true, [0, 3, 0, 0, 0, 0], vec![1, 3])
        );
    }

    #[test]
    fn expired_snapshot_releases_ring_lock_when_a_slot_is_busy() {
        let recorder = ThreadRecorder::new();
        let capacity = EventBufferCapacity::new(64).unwrap();
        assert!(recorder.record(
            1,
            capacity,
            Record::object(EventKind::MutexAccess, ObjectId::new(1)),
            [0; MAX_STACK_FRAMES],
            0
        ));
        let slot = recorder.ring().unwrap().slots[0].lock_until(wait_deadline()).unwrap();
        let timed_out = recorder.snapshot_until(Instant::now()).is_none();
        let ring_released = !recorder.ring_locked.load(Ordering::Acquire);
        drop(slot);
        assert_eq!((timed_out, ring_released, recorder.snapshot().events.len()), (true, true, 1));
    }

    #[test]
    fn contended_statistics_returns_error_and_legacy_fallback() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        });
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        let ring = recorder.ring_lock();
        let error = try_statistics().unwrap_err().to_string();
        let fallback = statistics();
        drop(ring);
        let expected = Statistics {
            event_capacity_per_thread: 64,
            recording: last_recording_policies(),
            ..Statistics::default()
        };
        configure(Configuration::default());
        assert_eq!(
            (error.as_str(), fallback),
            ("seismograph recorder statistics timed out waiting for an event ring", expected)
        );
    }

    fn timeout_configuration() -> Configuration {
        Configuration {
            allocations: RecordingPolicy::all(false),
            general_events: RecordingPolicy::all(false),
            arc_dereferences: RecordingPolicy::all(false),
            runtime_tasks: RecordingPolicy::all(false),
            io: RecordingPolicy::all(false),
            cache: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
        }
    }

    #[test]
    fn destructive_snapshot_restores_recording_after_writer_timeout() {
        let _test = TEST_LOCK.lock().unwrap();
        let expected = timeout_configuration();
        configure(expected);
        let session = ACTIVE_SESSION.load(Ordering::Acquire);
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        recorder.writer_active.store(true, Ordering::SeqCst);
        let writer = WriterActiveGuard { recorder };
        let error = destructive_snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap_err();
        drop(writer);
        let restored = (configuration(), ACTIVE_SESSION.load(Ordering::Acquire));
        let resumed = record_session(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        configure(Configuration::default());
        assert_eq!(
            (error.to_string(), restored, resumed.map(RecordingSession::get)),
            (
                "seismograph snapshot timed out waiting for active event writers".into(),
                (expected, session),
                Some(session)
            )
        );
    }

    #[test]
    fn destructive_snapshot_restores_recording_after_snapshot_timeout() {
        let _test = TEST_LOCK.lock().unwrap();
        let expected = timeout_configuration();
        configure(expected);
        let event = Record::object(EventKind::MutexAccess, ObjectId::new(1));
        let session = record_session(EventClass::General, || Some(event)).unwrap();
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        let ring = recorder.ring_lock();
        let error = destructive_snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap_err();
        drop(ring);
        let restored = (configuration(), ACTIVE_SESSION.load(Ordering::Acquire));
        let resumed = record_in_session(session, || Some(event));
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();
        configure(Configuration::default());
        assert_eq!(
            (error.to_string(), restored, resumed, captured.events.len()),
            (
                "seismograph snapshot timed out waiting for an event recorder".into(),
                (expected, session.get()),
                true,
                2
            )
        );
    }

    #[test]
    fn destructive_snapshot_restores_recording_when_clearing_an_older_ring_times_out() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration::default());
        // A recorder with no events in the new session is skipped during capture,
        // but its retained ring still needs clearing.
        let recorder = local_recorder();
        let expected = timeout_configuration();
        configure(expected);
        let session = ACTIVE_SESSION.load(Ordering::Acquire);
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*recorder };
        let ring = recorder.ring_lock();
        let error = destructive_snapshot(crate::snapshot::EventBufferDisposition::Clear).unwrap_err();
        drop(ring);
        let restored = (configuration(), ACTIVE_SESSION.load(Ordering::Acquire));
        let resumed = record_session(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        configure(Configuration::default());
        assert_eq!(
            (error.to_string(), restored, resumed.map(RecordingSession::get)),
            (
                "seismograph snapshot timed out clearing an event recorder".into(),
                (expected, session),
                Some(session)
            )
        );
    }

    #[test]
    fn expired_configuration_lock_can_be_acquired_after_release() {
        let _test = TEST_LOCK.lock().unwrap();
        let lock = ConfigurationLock::acquire();
        let blocked = ConfigurationLock::acquire_until(Instant::now()).is_none();
        drop(lock);
        let acquired = ConfigurationLock::acquire_until(Instant::now()).is_some();
        assert_eq!((blocked, acquired), (true, true));
    }

    #[test]
    fn blocking_configuration_acquisition_retries_after_its_bounded_wait() {
        let _test = TEST_LOCK.lock().unwrap();
        let lock = ConfigurationLock::acquire();
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (finished_sender, finished_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_sender.send(()).unwrap();
            let _lock = ConfigurationLock::acquire();
            finished_sender.send(()).unwrap();
        });
        started_receiver.recv().unwrap();
        let blocked = finished_receiver.recv_timeout(RECORDER_WAIT_TIMEOUT + Duration::from_millis(100));
        drop(lock);
        let completed = finished_receiver.recv_timeout(Duration::from_secs(5));
        worker.join().unwrap();
        assert_eq!((blocked, completed), (Err(std::sync::mpsc::RecvTimeoutError::Timeout), Ok(())));
    }

    #[cfg(feature = "monitor")]
    fn monitor_connection() -> (crate::monitor::Monitor, std::net::TcpStream) {
        use seismograph_protocol::message::{Request, Response};

        let monitor = crate::monitor::Monitor::builder().name("recorder-timeout").start().unwrap();
        let mut stream = std::net::TcpStream::connect(monitor.descriptor().socket_address()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        seismograph_protocol::write_request(
            &mut stream,
            1,
            &Request::Hello {
                authentication: monitor.descriptor().authentication,
            },
        )
        .unwrap();
        assert!(matches!(
            seismograph_protocol::read_response(&mut stream).unwrap(),
            (1, Response::Hello { .. })
        ));
        (monitor, stream)
    }

    #[cfg(feature = "monitor")]
    #[cfg_attr(miri, ignore)]
    #[test]
    fn monitor_reports_busy_statistics_and_recovers_on_the_same_connection() {
        use seismograph_protocol::message::{Request, Response};

        let _test = TEST_LOCK.lock().unwrap();
        let (_monitor, mut stream) = monitor_connection();
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        let ring = recorder.ring_lock();
        seismograph_protocol::write_request(&mut stream, 2, &Request::ReadRecorderStatistics).unwrap();
        let response = seismograph_protocol::read_response(&mut stream).unwrap();
        drop(ring);
        seismograph_protocol::write_request(&mut stream, 3, &Request::ReadRecorderStatistics).unwrap();
        let resumed = seismograph_protocol::read_response(&mut stream).unwrap();
        assert_eq!(
            (response, matches!(resumed, (3, Response::RecorderStatistics(_)))),
            (
                (
                    2,
                    Response::Error("seismograph recorder statistics timed out waiting for an event ring".into())
                ),
                true
            )
        );
    }

    #[cfg(feature = "monitor")]
    #[cfg_attr(miri, ignore)]
    #[test]
    fn monitor_rejects_configuration_changes_while_locked_without_changing_policies() {
        use seismograph_protocol::message::{RecordingConfiguration, Request, Response};

        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration::default());
        let (_monitor, mut stream) = monitor_connection();
        let lock = ConfigurationLock::acquire();
        let requested = seismograph_protocol::message::RecordingPolicy {
            enabled: true,
            ..Default::default()
        };
        seismograph_protocol::write_request(&mut stream, 2, &Request::SetCacheRecording(requested)).unwrap();
        let cache_response = seismograph_protocol::read_response(&mut stream).unwrap();
        seismograph_protocol::write_request(
            &mut stream,
            3,
            &Request::SetRecording(RecordingConfiguration {
                general_events: requested,
                ..Default::default()
            }),
        )
        .unwrap();
        let general_response = seismograph_protocol::read_response(&mut stream).unwrap();
        let unchanged = configuration();
        drop(lock);
        seismograph_protocol::write_request(&mut stream, 4, &Request::SetCacheRecording(requested)).unwrap();
        let resumed = seismograph_protocol::read_response(&mut stream).unwrap();
        configure(Configuration::default());
        let expected_error = Response::Error("seismograph recording configuration timed out waiting for another operation".into());
        assert_eq!(
            (cache_response, general_response, unchanged, resumed),
            (
                (2, expected_error.clone()),
                (3, expected_error),
                Configuration::default(),
                (4, Response::Acknowledged)
            )
        );
    }

    #[test]
    fn zero_initialized_policy_uses_default_sampling() {
        assert_eq!(decode_sampling(0), EventSampling::ALL);
    }

    #[test]
    fn symbol_lookup_address_preserves_recorded_identity_semantics() {
        let address = Address::new(0x1000);
        #[cfg(windows)]
        assert_eq!(symbol_lookup_address(address), Address::new(0x0fff));
        #[cfg(not(windows))]
        assert_eq!(symbol_lookup_address(address), address);
        assert_eq!(symbol_lookup_address(Address::new(0)), Address::new(0));
    }

    struct LateTelemetryUser;

    impl Drop for LateTelemetryUser {
        fn drop(&mut self) {
            record(EventClass::Allocation, || {
                Some(Record::object(EventKind::Deallocation, ObjectId::new(1)))
            });
        }
    }

    thread_local! {
        static LATE_TELEMETRY_USER: LateTelemetryUser = const { LateTelemetryUser };
    }

    #[test]
    fn omitted_event_builders_do_not_initialize_recorders_and_allow_later_events() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        let before = try_activity().unwrap().statistics;
        std::thread::spawn(move || {
            let registered = RECORDERS.load(Ordering::Acquire);
            let constructed = Cell::new(0);
            record(EventClass::General, || {
                constructed.set(constructed.get() + 1);
                None
            });
            let omitted_session = record_session(EventClass::General, || {
                constructed.set(constructed.get() + 1);
                None
            });
            let after = try_activity().unwrap();
            assert_eq!(
                (
                    constructed.get(),
                    omitted_session,
                    LOCAL_RECORDER.with(|local| local.recorder.get().is_null()),
                    RECORDERS.load(Ordering::Acquire),
                    after.statistics,
                    after.class_events,
                    after.threads.len(),
                ),
                (2, None, true, registered, before, [0; 6], 0)
            );

            record(EventClass::General, || {
                Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
            });
            assert_eq!(
                record_session(EventClass::General, || Some(Record::object(
                    EventKind::MutexAccess,
                    ObjectId::new(2)
                ))),
                active_recording_session()
            );
        })
        .join()
        .unwrap();
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();
        configure(Configuration::default());
        assert_eq!(
            captured.events.iter().filter_map(Event::object_id).collect::<Vec<_>>(),
            vec![ObjectId::new(1), ObjectId::new(2)]
        );
    }

    #[test]
    fn omitted_session_bound_builders_do_not_initialize_recorders_and_allow_later_events() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        clear_event_buffers().unwrap();
        let session = active_recording_session().unwrap();
        let before = try_activity().unwrap().statistics;
        std::thread::spawn(move || {
            let registered = RECORDERS.load(Ordering::Acquire);
            let constructed = Cell::new(0);
            let omitted = record_in_session(session, || {
                constructed.set(constructed.get() + 1);
                None
            });
            let omitted_classified = record_in_session_classified(session, EventClass::Cache, || {
                constructed.set(constructed.get() + 1);
                None
            });
            let after = try_activity().unwrap();
            assert_eq!(
                (
                    constructed.get(),
                    omitted,
                    omitted_classified,
                    LOCAL_RECORDER.with(|local| local.recorder.get().is_null()),
                    RECORDERS.load(Ordering::Acquire),
                    after.statistics,
                    after.class_events,
                    after.threads.len(),
                ),
                (2, false, false, true, registered, before, [0; 6], 0)
            );

            assert!(record_in_session(session, || {
                Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
            }));
            assert!(record_in_session_classified(session, EventClass::Cache, || {
                Some(Record::object(EventKind::CacheHit, ObjectId::new(2)))
            }));
        })
        .join()
        .unwrap();
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();
        configure(Configuration::default());
        assert_eq!(
            captured.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![EventKind::MutexAccess, EventKind::CacheHit]
        );
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // The closures are deliberately asserted not to execute.
    fn disabled_recording_does_not_construct_events() {
        let _test = TEST_LOCK.lock().unwrap();
        let constructed = AtomicUsize::new(0);
        configure(Configuration::default());
        record(EventClass::ArcDereference, || {
            constructed.fetch_add(1, Ordering::Relaxed);
            Some(Record::object(EventKind::ArcDeref, ObjectId::new(42)))
        });
        GENERAL_POLICY.store(encode_policy(RecordingPolicy::all(false)), Ordering::Release);
        ACTIVE_SESSION.store(0, Ordering::Release);
        record(EventClass::General, || {
            constructed.fetch_add(1, Ordering::Relaxed);
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(42)))
        });
        configure(Configuration::default());
        assert_eq!(constructed.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn event_classes_are_enabled_independently_before_record_construction() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            arc_dereferences: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..test_configuration()
        });
        record(EventClass::General, || Some(Record::object(EventKind::ArcClone, ObjectId::new(42))));
        record(EventClass::ArcDereference, || {
            Some(Record::object(EventKind::ArcDeref, ObjectId::new(42)))
        });

        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();

        assert_eq!(
            captured.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![EventKind::ArcDeref]
        );
        configure(Configuration::default());
    }

    #[test]
    fn record_rejects_events_from_a_different_class() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap(),
            ..Default::default()
        });
        let session = RecordingSession::from_raw(ACTIVE_SESSION.load(Ordering::Acquire)).unwrap();

        record(EventClass::General, || {
            Some(Record::runtime(
                EventTimestamp::from_ticks(1),
                EventKind::TaskSpawned,
                runtime::RuntimeEvent {
                    runtime_id: runtime::RuntimeId::from_raw(1).unwrap(),
                    worker_id: None,
                    subject_id: 1,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                BacktraceCapture::Never,
            ))
        });
        assert!(!record_in_session_classified(session, EventClass::General, || {
            Some(Record::runtime(
                EventTimestamp::from_ticks(2),
                EventKind::TaskSpawned,
                runtime::RuntimeEvent {
                    runtime_id: runtime::RuntimeId::from_raw(1).unwrap(),
                    worker_id: None,
                    subject_id: 2,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                BacktraceCapture::Never,
            ))
        }));

        assert!(snapshot(crate::snapshot::EventBufferDisposition::Release).is_none_or(|captured| captured.events.is_empty()));
        configure(Configuration::default());
    }

    #[test]
    fn recording_during_tls_teardown_ignores_destroyed_local_recorder() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            allocations: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        drop(LateTelemetryUser);

        std::thread::spawn(|| {
            assert!(!is_suppressed());
            LATE_TELEMETRY_USER.with(|_| {});
            let _ = current_thread_id();
        })
        .join()
        .unwrap();

        configure(Configuration::default());
    }

    #[test]
    fn runtime_events_round_trip_through_snapshot_model() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            arc_dereferences: RecordingPolicy {
                enabled: true,
                capture_backtraces: !cfg!(miri),
                ..Default::default()
            },
            ..test_configuration()
        });
        record(EventClass::ArcDereference, || {
            Some(Record::object(EventKind::ArcDeref, ObjectId::new(42)))
        });

        let snapshot = snapshot(crate::snapshot::EventBufferDisposition::Retain).unwrap();
        let event = snapshot
            .events
            .iter()
            .rev()
            .find(|event| event.object_id() == Some(ObjectId::new(42)) && event.kind == EventKind::ArcDeref)
            .unwrap();
        assert_eq!(event.payload, EventPayload::Object(ObjectId::new(42)));
        #[cfg(not(miri))]
        assert!(!event.call_stack.is_empty());

        configure(Configuration::default());
    }

    #[cfg(not(miri))]
    #[test]
    fn runtime_events_can_override_global_backtrace_capture() {
        let _test = TEST_LOCK.lock().unwrap();
        let runtime_id = runtime::RuntimeId::from_raw(1).unwrap();
        configure(Configuration {
            runtime_tasks: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                ..Default::default()
            },
            ..Default::default()
        });
        record(EventClass::RuntimeTask, || {
            Some(Record::runtime(
                event::EventTimestamp::now(),
                EventKind::TaskPollStarted,
                runtime::RuntimeEvent {
                    runtime_id,
                    worker_id: None,
                    subject_id: 1,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                BacktraceCapture::Never,
            ))
        });
        record(EventClass::RuntimeTask, || {
            Some(Record::runtime(
                event::EventTimestamp::now(),
                EventKind::TaskSpawned,
                runtime::RuntimeEvent {
                    runtime_id,
                    worker_id: None,
                    subject_id: 2,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                BacktraceCapture::Always,
            ))
        });

        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();
        let poll = captured
            .events
            .iter()
            .find(|event| event.kind == EventKind::TaskPollStarted)
            .unwrap();
        let spawn = captured.events.iter().find(|event| event.kind == EventKind::TaskSpawned).unwrap();
        assert_eq!((poll.call_stack.is_empty(), spawn.call_stack.is_empty()), (true, false));
        configure(Configuration::default());
    }

    #[test]
    fn current_thread_id_is_stable() {
        let _test = TEST_LOCK.lock().unwrap();
        let current = current_thread_id();
        let other = std::thread::spawn(current_thread_id).join().unwrap();

        assert_eq!((current_thread_id(), current != other, current.get() != 0), (current, true, true));
    }

    #[test]
    fn capacities_require_bounded_powers_of_two() {
        assert_eq!(
            (
                EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD),
                EventBufferCapacity::new(1_000),
                EventBufferCapacity::new(MAX_EVENT_CAPACITY_PER_THREAD.saturating_mul(2)),
            ),
            (Some(EventBufferCapacity(MIN_EVENT_CAPACITY_PER_THREAD)), None, None)
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "statistical sampling stress test requires native execution")]
    fn object_sampling_selects_approximately_one_in_x_objects() {
        let sampling = EventSampling::one_in(100).unwrap();
        let population = if cfg!(miri) { 4_096 } else { 65_536 };
        let selected = (0..population)
            .map(ObjectId::new)
            .filter(|object_id| sampling.includes(*object_id))
            .count();

        let expected_range = if cfg!(miri) { 30..=50 } else { 560..=750 };
        assert!(expected_range.contains(&selected), "selected {selected} objects from {population}");
    }

    #[test]
    fn object_sampling_accepts_bounded_arbitrary_denominators() {
        assert_eq!(
            (
                EventSampling::one_in(1),
                EventSampling::one_in(20),
                EventSampling::one_in(100),
                EventSampling::one_in(0),
                EventSampling::one_in(MAX_EVENT_SAMPLING_ONE_IN + 1),
            ),
            (
                Some(EventSampling(1)),
                Some(EventSampling(20)),
                Some(EventSampling(100)),
                None,
                None
            )
        );
    }

    #[test]
    fn object_sampling_hash_has_stable_decisions() {
        let decisions = [2, 3, 7, 16].map(|denominator| {
            let sampling = EventSampling::one_in(denominator).unwrap();
            (1..=32)
                .filter(|value| sampling.includes(ObjectId::new(*value)))
                .collect::<Vec<_>>()
        });

        assert_eq!(
            decisions,
            [
                vec![2, 4, 5, 6, 8, 9, 10, 14, 18, 19, 20, 22, 23, 24, 26, 27, 28, 29, 30, 31],
                vec![3, 10, 11, 18, 20, 21],
                vec![3, 10, 18, 21],
                vec![6, 29],
            ]
        );
    }

    #[test]
    fn configuration_changes_rotate_sessions_and_preserve_exact_policies() {
        let _test = TEST_LOCK.lock().unwrap();
        let base_policy = RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            event_sampling: EventSampling::one_in(3).unwrap(),
        };
        let base = Configuration {
            general_events: base_policy,
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        };
        configure(base);
        let unchanged_session = ACTIVE_SESSION.load(Ordering::Acquire);
        configure(base);
        assert_eq!(ACTIVE_SESSION.load(Ordering::Acquire), unchanged_session);
        configure(Configuration::default());
        configure(Configuration::default());
        assert_eq!(ACTIVE_SESSION.load(Ordering::Acquire), 0);

        let variants = [
            Configuration {
                allocations: RecordingPolicy::all(false),
                ..base
            },
            Configuration {
                general_events: RecordingPolicy {
                    capture_backtraces: true,
                    ..base_policy
                },
                ..base
            },
            Configuration {
                arc_dereferences: RecordingPolicy::all(false),
                ..base
            },
            Configuration {
                runtime_tasks: RecordingPolicy::all(false),
                ..base
            },
            Configuration {
                io: RecordingPolicy::all(false),
                ..base
            },
            Configuration {
                cache: RecordingPolicy::all(false),
                ..base
            },
            Configuration {
                event_capacity_per_thread: EventBufferCapacity::new(128).unwrap(),
                ..base
            },
        ];
        for variant in variants {
            configure(base);
            let before = ACTIVE_SESSION.load(Ordering::Acquire);
            configure(variant);
            assert_ne!(ACTIVE_SESSION.load(Ordering::Acquire), before);
            assert_eq!(configuration(), variant);
        }

        configure(Configuration::default());
        assert_eq!(ACTIVE_SESSION.load(Ordering::Acquire), 0);
    }

    #[test]
    fn recording_observations_freeze_at_configuration_and_snapshot_stops() {
        let _test = TEST_LOCK.lock().unwrap();
        let enabled = timeout_configuration();
        configure(enabled);
        let active = recording_observation().unwrap();
        assert_eq!(SESSION_STOPPED_AT.load(Ordering::Acquire), 0);
        assert_eq!(Some(active.session), active_recording_session());

        configure(Configuration::default());
        let configured_stop = SESSION_STOPPED_AT.load(Ordering::Acquire);
        let first = recording_observation().unwrap();
        std::thread::sleep(Duration::from_millis(1));
        let second = recording_observation().unwrap();
        assert_eq!(
            (configured_stop, first.observed_at.ticks(), second.observed_at.ticks()),
            (configured_stop, configured_stop, configured_stop)
        );
        assert_ne!(configured_stop, 0);

        configure(enabled);
        let (_, observation) = reset_event_buffers(crate::snapshot::EventBufferDisposition::Stop, false).unwrap();
        let snapshot_stop = SESSION_STOPPED_AT.load(Ordering::Acquire);
        assert_eq!(observation.unwrap().observed_at.ticks(), snapshot_stop);
        assert_ne!(snapshot_stop, 0);
        configure(Configuration::default());
    }

    #[test]
    fn every_recording_class_controls_global_and_class_enablement() {
        let _test = TEST_LOCK.lock().unwrap();
        let cases = [
            (
                EventClass::Allocation,
                Configuration {
                    allocations: RecordingPolicy::all(false),
                    ..Default::default()
                },
            ),
            (
                EventClass::General,
                Configuration {
                    general_events: RecordingPolicy::all(false),
                    ..Default::default()
                },
            ),
            (
                EventClass::ArcDereference,
                Configuration {
                    arc_dereferences: RecordingPolicy::all(false),
                    ..Default::default()
                },
            ),
            (
                EventClass::RuntimeTask,
                Configuration {
                    runtime_tasks: RecordingPolicy::all(false),
                    ..Default::default()
                },
            ),
            (
                EventClass::Io,
                Configuration {
                    io: RecordingPolicy::all(false),
                    ..Default::default()
                },
            ),
            (
                EventClass::Cache,
                Configuration {
                    cache: RecordingPolicy::all(false),
                    ..Default::default()
                },
            ),
        ];

        for (enabled_class, configuration) in cases {
            configure(configuration);
            assert!(recording_enabled());
            assert!(select_object_for(enabled_class, ObjectId::new(1)).is_some());
            for (class, _) in cases {
                assert_eq!(recording_enabled_for(class), class == enabled_class);
            }
        }
        ACTIVE_SESSION.store(0, Ordering::Release);
        assert!(select_object_for(EventClass::Cache, ObjectId::new(1)).is_none());
        configure(Configuration::default());
    }

    #[test]
    fn object_sampling_keeps_or_drops_complete_object_histories() {
        let _test = TEST_LOCK.lock().unwrap();
        let sampling = EventSampling::one_in(20).unwrap();
        let sampled = (1..10_000)
            .map(ObjectId::new)
            .find(|object_id| sampling.includes(*object_id))
            .unwrap();
        let skipped = (1..10_000)
            .map(ObjectId::new)
            .find(|object_id| !sampling.includes(*object_id))
            .unwrap();
        configure(Configuration {
            general_events: RecordingPolicy {
                enabled: true,
                event_sampling: sampling,
                ..Default::default()
            },
            event_capacity_per_thread: EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap(),
            ..Default::default()
        });
        let thread_count = statistics().thread_count;
        std::thread::spawn(move || record(EventClass::General, || Some(Record::object(EventKind::ArcClone, skipped))))
            .join()
            .unwrap();
        assert_eq!(statistics().thread_count, thread_count);

        record(EventClass::General, || Some(Record::object(EventKind::ArcClone, sampled)));
        record(EventClass::General, || Some(Record::object(EventKind::ArcDrop, sampled)));
        record(EventClass::General, || Some(Record::object(EventKind::ArcClone, skipped)));
        record(EventClass::General, || Some(Record::object(EventKind::ArcDrop, skipped)));
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();

        assert_eq!(
            (
                captured.recording.general_events.event_sampling.get(),
                captured.events.iter().filter_map(Event::object_id).collect::<Vec<_>>(),
            ),
            (20, vec![sampled, sampled])
        );
        configure(Configuration::default());
    }

    #[test]
    fn runtime_sampling_uses_the_subject_identity() {
        let _test = TEST_LOCK.lock().unwrap();
        let sampling = EventSampling::one_in(20).unwrap();
        let sampled = (1..10_000)
            .find(|subject_id| sampling.includes(ObjectId::new(*subject_id)))
            .unwrap();
        let skipped = (1..10_000)
            .find(|subject_id| !sampling.includes(ObjectId::new(*subject_id)))
            .unwrap();
        configure(Configuration {
            runtime_tasks: RecordingPolicy {
                enabled: true,
                event_sampling: sampling,
                ..Default::default()
            },
            event_capacity_per_thread: EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap(),
            ..Default::default()
        });

        for subject_id in [sampled, skipped] {
            record(EventClass::RuntimeTask, || {
                Some(Record::runtime(
                    EventTimestamp::from_ticks(subject_id),
                    EventKind::TaskSpawned,
                    runtime::RuntimeEvent {
                        runtime_id: runtime::RuntimeId::from_raw(1).unwrap(),
                        worker_id: None,
                        subject_id,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    },
                    BacktraceCapture::Never,
                ))
            });
        }
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();

        assert_eq!(
            captured
                .events
                .iter()
                .filter_map(Event::runtime)
                .map(|event| event.subject_id)
                .collect::<Vec<_>>(),
            vec![sampled]
        );
        configure(Configuration::default());
    }

    #[test]
    fn destructive_snapshots_clear_or_release_buffers() {
        let _test = TEST_LOCK.lock().unwrap();
        let capacity = EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap();
        configure(Configuration {
            arc_dereferences: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            event_capacity_per_thread: capacity,
            ..Default::default()
        });
        let _initial = snapshot(crate::snapshot::EventBufferDisposition::Release);

        record(EventClass::ArcDereference, || {
            Some(Record::object(EventKind::ArcDeref, ObjectId::new(1)))
        });
        let active_bytes = statistics().allocated_bytes;
        let cleared = snapshot(crate::snapshot::EventBufferDisposition::Clear).unwrap();
        let cleared_statistics = statistics();
        record(EventClass::ArcDereference, || {
            Some(Record::object(EventKind::ArcDeref, ObjectId::new(2)))
        });
        let released = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();
        let released_statistics = statistics();

        assert_eq!(
            (
                cleared.events.len(),
                cleared_statistics.retained_events,
                cleared_statistics.allocated_bytes,
                released.events.len(),
                released_statistics.retained_events,
                released_statistics.allocated_bytes < active_bytes,
            ),
            (1, 0, active_bytes, 1, 0, true)
        );
        configure(Configuration::default());
    }

    #[test]
    fn stop_captures_events_disables_every_class_and_releases_every_ring() {
        let _test = TEST_LOCK.lock().unwrap();
        let expected = timeout_configuration();
        configure(expected);
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        let captured = try_snapshot(crate::snapshot::EventBufferDisposition::Stop).unwrap().unwrap();
        let stopped = configuration();
        assert_eq!(captured.events.len(), 1);
        assert_eq!(
            [
                stopped.allocations,
                stopped.general_events,
                stopped.arc_dereferences,
                stopped.runtime_tasks,
                stopped.io,
                stopped.cache
            ],
            [RecordingPolicy::default(); 6]
        );
        assert_eq!(ACTIVE_SESSION.load(Ordering::Acquire), 0);
        let mut pointer = RECORDERS.load(Ordering::Acquire);
        while !pointer.is_null() {
            // SAFETY: registered recorders live for the process lifetime.
            let recorder = unsafe { &*pointer };
            assert_eq!(recorder.ring_capacity.load(Ordering::Acquire), 0);
            pointer = recorder.next.load(Ordering::Acquire);
        }
        for class in [
            EventClass::Allocation,
            EventClass::General,
            EventClass::ArcDereference,
            EventClass::RuntimeTask,
            EventClass::Io,
            EventClass::Cache,
        ] {
            assert!(!recording_enabled_for(class));
            assert!(record_session(class, || panic!("stopped classes must not construct events")).is_none());
        }
        configure(Configuration::default());
    }

    #[test]
    fn continue_preserves_buffers_and_policy_even_when_disabled() {
        let _test = TEST_LOCK.lock().unwrap();
        let enabled = timeout_configuration();
        configure(enabled);
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        let before = statistics().allocated_bytes;
        assert_eq!(
            try_snapshot(crate::snapshot::EventBufferDisposition::Retain)
                .unwrap()
                .unwrap()
                .events
                .len(),
            1
        );
        assert_eq!((configuration(), statistics().allocated_bytes), (enabled, before));
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(2)))
        });
        configure(Configuration::default());
        assert_eq!(
            try_snapshot(crate::snapshot::EventBufferDisposition::Retain)
                .unwrap()
                .unwrap()
                .events
                .len(),
            2
        );
        assert_eq!(configuration(), Configuration::default());
        clear_event_buffers().unwrap();
    }

    #[test]
    fn standalone_clear_reuses_rings_and_preserves_enabled_and_disabled_policies() {
        let _test = TEST_LOCK.lock().unwrap();
        let enabled = Configuration {
            allocations: RecordingPolicy {
                enabled: false,
                capture_backtraces: true,
                event_sampling: EventSampling::one_in(4).unwrap(),
            },
            cache: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: EventSampling::one_in(8).unwrap(),
            },
            ..timeout_configuration()
        };
        configure(enabled);
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        let before = statistics().allocated_bytes;
        clear_event_buffers().unwrap();
        assert_eq!(
            (configuration(), statistics().retained_events, statistics().allocated_bytes),
            (enabled, 0, before)
        );
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(2)))
        });
        assert_eq!(statistics().retained_events, 1);
        configure(Configuration::default());
        clear_event_buffers().unwrap();
        assert_eq!(
            (configuration(), statistics().retained_events, statistics().allocated_bytes),
            (Configuration::default(), 0, before)
        );
    }

    #[test]
    fn stop_and_clear_restore_policy_on_writer_timeout() {
        let _test = TEST_LOCK.lock().unwrap();
        let expected = timeout_configuration();
        configure(expected);
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        for capture in [true, false] {
            recorder.writer_active.store(true, Ordering::SeqCst);
            let writer = WriterActiveGuard { recorder };
            let result = if capture {
                try_snapshot(crate::snapshot::EventBufferDisposition::Stop).map(|_| ())
            } else {
                clear_event_buffers()
            };
            drop(writer);
            assert!(result.unwrap_err().to_string().contains("timed out"));
            assert_eq!(configuration(), expected);
        }
        configure(Configuration::default());
    }

    #[test]
    fn stop_release_timeout_keeps_all_recording_disabled() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(timeout_configuration());
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        });
        configure(Configuration::default());
        // Start a new session without touching this ring, so capture skips it and
        // only the release phase encounters its reader lock.
        configure(timeout_configuration());
        // SAFETY: this thread owns the process-lifetime recorder.
        let recorder = unsafe { &*local_recorder() };
        let ring = recorder.ring_lock();
        let error = try_snapshot(crate::snapshot::EventBufferDisposition::Stop).unwrap_err();
        drop(ring);
        assert!(error.to_string().contains("clearing an event recorder"));
        assert!(!recording_enabled());
        assert_ne!(recorder.ring_capacity.load(Ordering::Acquire), 0);
        try_snapshot(crate::snapshot::EventBufferDisposition::Stop).unwrap();
        assert_eq!(recorder.ring_capacity.load(Ordering::Acquire), 0);
        configure(Configuration::default());
    }

    #[test]
    fn lifecycle_operations_wait_for_inflight_writers() {
        let _test = TEST_LOCK.lock().unwrap();
        for stop in [false, true] {
            configure(timeout_configuration());
            record(EventClass::General, || {
                Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
            });
            // SAFETY: this thread owns the process-lifetime recorder.
            let recorder = unsafe { &*local_recorder() };
            recorder.writer_active.store(true, Ordering::SeqCst);
            let writer = WriterActiveGuard { recorder };
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let result = if stop {
                        try_snapshot(crate::snapshot::EventBufferDisposition::Stop).map(|events| events.unwrap().events.len())
                    } else {
                        clear_event_buffers().map(|()| 0)
                    };
                    sender.send(result).unwrap();
                });
                assert_eq!(
                    receiver.recv_timeout(Duration::from_millis(20)).unwrap_err(),
                    std::sync::mpsc::RecvTimeoutError::Timeout
                );
                drop(writer);
                assert_eq!(receiver.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(), usize::from(stop));
            });
            assert_eq!(recording_enabled(), !stop);
        }
        configure(Configuration::default());
    }

    #[test]
    fn bounded_ring_wraparound_preserves_generation_and_thread_metadata() {
        let _test = TEST_LOCK.lock().unwrap();
        let capacity = EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap();
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: capacity,
            ..Default::default()
        });
        let _initial = snapshot(crate::snapshot::EventBufferDisposition::Release);
        let baseline_allocated_bytes = statistics().allocated_bytes;

        let thread = std::thread::Builder::new()
            .name("seismograph-wraparound".into())
            .spawn(|| {
                for object_id in 1..=70 {
                    record(EventClass::General, || {
                        Some(Record::object(EventKind::MutexAccess, ObjectId::new(object_id)))
                    });
                }
            })
            .unwrap();
        thread.join().unwrap();

        let statistics = statistics();
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Retain).unwrap();
        let retained = captured
            .events
            .iter()
            .map(|event| (event.sequence.get(), event.object_id().unwrap().get()))
            .collect::<Vec<_>>();
        assert_eq!(
            (
                statistics.thread_count,
                statistics.total_events,
                statistics.retained_events,
                statistics.lost_events,
                statistics.event_capacity_per_thread,
                statistics.allocated_bytes - baseline_allocated_bytes,
            ),
            (
                1,
                70,
                64,
                6,
                64,
                (std::mem::size_of::<ThreadRecorder>() + 64 * std::mem::size_of::<Slot>()) as u64,
            )
        );
        assert_eq!(
            statistics.recording,
            RecordingPolicies {
                general_events: RecordingPolicy::all(false),
                ..Default::default()
            }
        );
        assert_eq!(
            (
                captured.clock,
                captured.total_events,
                captured.lost_events,
                captured.threads.len(),
                captured.threads[0].name.as_str(),
                captured.threads[0].thread_id,
                retained,
            ),
            (
                EventClock::CURRENT,
                70,
                6,
                1,
                "seismograph-wraparound",
                captured.events[0].thread_id,
                (7..=70).map(|value| (value, value)).collect(),
            )
        );
        configure(Configuration::default());
    }

    #[test]
    fn destructive_snapshot_restores_policies_and_starts_a_new_generation() {
        let _test = TEST_LOCK.lock().unwrap();
        let policy = |sampling, capture_backtraces| RecordingPolicy {
            enabled: true,
            capture_backtraces,
            event_sampling: EventSampling::one_in(sampling).unwrap(),
        };
        let expected_configuration = Configuration {
            allocations: policy(2, false),
            general_events: policy(3, true),
            arc_dereferences: policy(4, false),
            runtime_tasks: policy(5, true),
            io: policy(6, false),
            cache: policy(7, true),
            event_capacity_per_thread: EventBufferCapacity::new(128).unwrap(),
        };
        configure(expected_configuration);
        let before = ACTIVE_SESSION.load(Ordering::Acquire);
        record(EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(3)))
        });

        let captured = snapshot(crate::snapshot::EventBufferDisposition::Clear).unwrap();
        let after = ACTIVE_SESSION.load(Ordering::Acquire);

        assert_eq!(
            (
                captured.recording,
                captured.events.len(),
                configuration(),
                after != 0 && after != before,
                decode_policy(disabled_policy(encode_policy(expected_configuration.general_events))),
            ),
            (
                RecordingPolicies {
                    allocations: expected_configuration.allocations,
                    general_events: expected_configuration.general_events,
                    arc_dereferences: expected_configuration.arc_dereferences,
                    runtime_tasks: expected_configuration.runtime_tasks,
                    io: expected_configuration.io,
                    cache: expected_configuration.cache,
                },
                1,
                expected_configuration,
                true,
                RecordingPolicy {
                    enabled: false,
                    ..expected_configuration.general_events
                },
            )
        );
        configure(Configuration::default());
    }

    #[test]
    fn recorder_rejects_each_stale_state_dimension() {
        let _test = TEST_LOCK.lock().unwrap();
        let configuration = Configuration {
            general_events: RecordingPolicy {
                enabled: true,
                event_sampling: EventSampling::one_in(3).unwrap(),
                ..Default::default()
            },
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        };
        configure(configuration);
        let session = ACTIVE_SESSION.load(Ordering::Acquire);
        let policy = GENERAL_POLICY.load(Ordering::Acquire);
        let capacity = EVENT_CAPACITY.load(Ordering::Acquire);
        let recorder = local_recorder();
        let selected = (1..100)
            .map(ObjectId::new)
            .find(|object| configuration.general_events.event_sampling.includes(*object))
            .unwrap();
        let record = || Record::object(EventKind::MutexAccess, selected);

        assert_eq!(
            (
                record_enabled_with_recorder(
                    Some(recorder),
                    session,
                    EventClass::General,
                    record(),
                    policy ^ u64::from(RECORDING_ENABLED),
                    capacity
                ),
                record_enabled_with_recorder(Some(recorder), session, EventClass::General, record(), policy, capacity + 1),
                record_enabled_with_recorder(Some(recorder), session + 1, EventClass::General, record(), policy, capacity),
                record_enabled_with_recorder(None, session, EventClass::General, record(), policy, capacity),
            ),
            (false, false, false, false)
        );
        // SAFETY: local_recorder returns this thread's process-lifetime recorder.
        assert!(!unsafe { &*recorder }.writer_active.load(Ordering::Acquire));
        configure(Configuration::default());
    }

    #[cfg(all(any(target_os = "windows", target_os = "linux"), not(miri)))]
    #[test]
    fn stack_capture_returns_real_frames_within_the_fixed_capacity() {
        let (frames, count) = capture_stack();
        let count = usize::from(count);

        assert!(count > 1);
        assert!(count <= MAX_STACK_FRAMES);
        assert!(frames[..count].iter().all(|frame| *frame != 0));
        assert!(frames[count..].iter().all(|frame| *frame == 0));
    }

    #[test]
    fn lock_guards_and_slots_publish_each_state_transition() {
        let _test = TEST_LOCK.lock().unwrap();
        let _ = local_recorder();
        let previous_general_policy = LAST_GENERAL_POLICY.swap(
            encode_policy(RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: EventSampling::one_in(3).unwrap(),
            }),
            Ordering::AcqRel,
        );
        let statistics = statistics();
        let expected_recording = last_recording_policies();
        let mut registered = RECORDERS.load(Ordering::Acquire);
        while !registered.is_null() {
            // SAFETY: registered recorders are retained for process lifetime.
            let current = unsafe { &*registered };
            current.ring_locked.store(false, Ordering::Release);
            registered = current.next.load(Ordering::Acquire);
        }
        assert_eq!(
            (statistics.event_capacity_per_thread, statistics.recording),
            (
                u64::try_from(configuration().event_capacity_per_thread.get()).unwrap(),
                expected_recording
            )
        );

        let snapshot_recorder = ThreadRecorder::new();
        snapshot_recorder.session.store(1, Ordering::Release);
        let events = snapshot_from_recorders(1, ptr::from_ref(&snapshot_recorder).cast_mut()).unwrap();
        snapshot_recorder.ring_locked.store(false, Ordering::Release);
        assert_eq!((events.clock, events.recording), (EventClock::CURRENT, expected_recording));

        let recorder = ThreadRecorder::new();
        assert!(!recorder.ring_locked.load(Ordering::Acquire));
        {
            let _guard = recorder.ring_lock();
            assert!(recorder.ring_locked.load(Ordering::Acquire));
        }
        let ring_unlocked = !recorder.ring_locked.load(Ordering::Acquire);
        recorder.ring_locked.store(false, Ordering::Release);
        assert!(ring_unlocked);

        let slot = Slot::new();
        assert!(!slot.locked.load(Ordering::Acquire));
        slot.lock();
        assert!(slot.locked.load(Ordering::Acquire));
        slot.unlock();
        let slot_unlocked = !slot.locked.load(Ordering::Acquire);
        slot.locked.store(false, Ordering::Release);
        assert!(slot_unlocked);

        assert!(!CONFIGURATION_LOCKED.load(Ordering::Acquire));
        {
            let _guard = ConfigurationLock::acquire();
            assert!(CONFIGURATION_LOCKED.load(Ordering::Acquire));
        }
        let configuration_unlocked = !CONFIGURATION_LOCKED.load(Ordering::Acquire);
        CONFIGURATION_LOCKED.store(false, Ordering::Release);
        LAST_GENERAL_POLICY.store(previous_general_policy, Ordering::Release);
        assert!(configuration_unlocked);
    }

    #[test]
    fn destructive_snapshot_waits_until_an_active_writer_finishes() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap(),
            ..Default::default()
        });
        let recorder = local_recorder();
        // SAFETY: local_recorder returns this thread's process-lifetime recorder.
        let recorder = unsafe { &*recorder };
        recorder.writer_active.store(true, Ordering::Release);
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (finished_sender, finished_receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                started_sender.send(()).unwrap();
                let captured = destructive_snapshot(crate::snapshot::EventBufferDisposition::Clear);
                finished_sender.send(captured).unwrap();
            });
            started_receiver.recv().unwrap();
            assert_eq!(
                finished_receiver.recv_timeout(std::time::Duration::from_millis(20)).unwrap_err(),
                std::sync::mpsc::RecvTimeoutError::Timeout
            );
            recorder.writer_active.store(false, Ordering::Release);
            let _captured = finished_receiver.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        });
        configure(Configuration::default());
    }

    #[test]
    fn writer_activity_is_cleared_during_panic_unwinding() {
        let recorder = ThreadRecorder::new();
        recorder.writer_active.store(true, Ordering::Release);

        let _panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _writer = WriterActiveGuard { recorder: &recorder };
            panic!("injected recording failure");
        }));

        assert!(!recorder.writer_active.load(Ordering::Acquire));
    }

    #[test]
    fn equal_capacity_reuses_the_existing_ring_between_sessions() {
        let recorder = ThreadRecorder::new();
        let capacity = EventBufferCapacity::new(64).unwrap();
        recorder.begin_session(1, capacity);
        let first = recorder.ring().unwrap().slots.as_ptr();
        recorder.begin_session(2, capacity);
        let second = recorder.ring().unwrap().slots.as_ptr();

        assert_eq!((first, second, recorder.session.load(Ordering::Acquire)), (first, first, 2));
    }

    #[test]
    fn snapshot_releases_a_retired_thread_ring_after_capturing_it() {
        let _test = TEST_LOCK.lock().unwrap();
        let capacity = EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap();
        configure(Configuration {
            arc_dereferences: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            event_capacity_per_thread: capacity,
            ..Default::default()
        });
        let _initial = snapshot(crate::snapshot::EventBufferDisposition::Release);
        let (active_sender, active_receiver) = std::sync::mpsc::channel();
        let (exit_sender, exit_receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            record(EventClass::ArcDereference, || {
                Some(Record::object(EventKind::ArcDeref, ObjectId::new(3)))
            });
            active_sender.send(statistics().allocated_bytes).unwrap();
            exit_receiver.recv().unwrap();
        });
        let active_bytes = active_receiver.recv().unwrap();
        exit_sender.send(()).unwrap();
        thread.join().unwrap();
        let retained_bytes = statistics().allocated_bytes;
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Clear).unwrap();
        let released_bytes = statistics().allocated_bytes;

        assert_eq!(retained_bytes, active_bytes);
        assert_eq!(captured.events.len(), 1);
        assert!(released_bytes < retained_bytes);
        configure(Configuration::default());
    }

    #[test]
    fn retired_thread_rings_are_bounded_and_evict_the_oldest_history() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            arc_dereferences: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            event_capacity_per_thread: EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap(),
            ..Default::default()
        });
        let _initial = snapshot(crate::snapshot::EventBufferDisposition::Release);

        for object_id in [1, 2] {
            std::thread::spawn(move || {
                record(EventClass::ArcDereference, || {
                    Some(Record::object(EventKind::ArcDeref, ObjectId::new(object_id)))
                });
            })
            .join()
            .unwrap();
        }

        assert!(retired_rings().allocated_bytes <= RETIRED_RING_BUDGET_BYTES);
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Retain).unwrap();
        assert_eq!(
            captured
                .events
                .iter()
                .filter_map(Event::object_id)
                .map(ObjectId::get)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(retired_rings().allocated_bytes, 0);
        configure(Configuration::default());
    }

    #[test]
    fn evicted_retired_ring_is_released_after_an_existing_reader() {
        let recorder = ThreadRecorder::new();
        recorder.begin_session(1, EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap());
        let ring = recorder.ring_lock();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                recorder.retire();
                recorder.release_retired_ring();
            });
            while !recorder.release_on_unlock.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            assert!(recorder.ring().is_some());
            drop(ring);
        });

        assert!(recorder.ring().is_none());
    }

    #[test]
    fn release_snapshot_quiesces_concurrent_recording() {
        let _test = TEST_LOCK.lock().unwrap();
        let capacity = EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap();
        configure(Configuration {
            arc_dereferences: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            event_capacity_per_thread: capacity,
            ..Default::default()
        });
        let running = std::sync::Arc::new(AtomicBool::new(true));
        let worker_running = std::sync::Arc::clone(&running);
        let event = Record::object(EventKind::ArcDeref, ObjectId::new(4));
        let session = ACTIVE_SESSION.load(Ordering::Acquire);
        let policy = ARC_DEREFERENCE_POLICY.load(Ordering::Acquire);
        let encoded_capacity = EVENT_CAPACITY.load(Ordering::Acquire);
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut started_sender = Some(started_sender);
            while worker_running.load(Ordering::Relaxed) {
                record_enabled(session, EventClass::ArcDereference, event, policy, encoded_capacity);
                if let Some(sender) = started_sender.take() {
                    sender.send(()).unwrap();
                }
            }
        });
        started_receiver.recv().unwrap();

        let snapshots = if cfg!(miri) { 4 } else { 32 };
        for _ in 0..snapshots {
            let _captured = snapshot(crate::snapshot::EventBufferDisposition::Release);
        }
        running.store(false, Ordering::Relaxed);
        worker.join().unwrap();

        assert_eq!(configuration().event_capacity_per_thread, capacity);
        configure(Configuration::default());
    }

    #[test]
    fn capacity_sampling_sessions_and_suppression_helpers_cover_boundaries() {
        let _test = TEST_LOCK.lock().unwrap();
        let capacity = EventBufferCapacity::default();
        assert_eq!(
            capacity.memory_bytes_per_thread(),
            std::mem::size_of::<ThreadRecorder>() + capacity.get() * std::mem::size_of::<Slot>()
        );
        assert_eq!(EventSampling::default(), EventSampling::ALL);
        assert_eq!(RecordingSession::from_raw(0), None);

        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            ..Default::default()
        });
        assert!(recording_enabled());
        assert!(recording_enabled_for(EventClass::General));
        let session = select_object(ObjectId::new(1)).unwrap();
        let constructed = Cell::new(0);
        let event = || {
            constructed.set(constructed.get() + 1);
            Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
        };
        assert!(record_in_session_classified(session, EventClass::General, event));
        assert_eq!(constructed.get(), 1);
        {
            let _suppression = SuppressionGuard::enter();
            assert!(!recording_enabled());
            assert!(!recording_enabled_for(EventClass::General));
            assert_eq!(select_object(ObjectId::new(1)), None);
            assert!(!record_in_session(session, event));
            assert!(!record_in_session_classified(session, EventClass::General, event));
        }
        configure(Configuration::default());
        assert!(!recording_enabled());
        assert_eq!(select_object(ObjectId::new(1)), None);
        assert!(!record_in_session(session, event));
        assert!(!record_in_session_classified(session, EventClass::General, event));
        assert_eq!(constructed.get(), 1);
    }

    #[test]
    fn unsampled_and_stale_sessions_are_rejected() {
        let _test = TEST_LOCK.lock().unwrap();
        let sampling = EventSampling::one_in(2).unwrap();
        let skipped = (1..100).map(ObjectId::new).find(|object| !sampling.includes(*object)).unwrap();
        configure(Configuration {
            general_events: RecordingPolicy {
                enabled: true,
                event_sampling: sampling,
                ..Default::default()
            },
            event_capacity_per_thread: EventBufferCapacity::new(MIN_EVENT_CAPACITY_PER_THREAD).unwrap(),
            ..Default::default()
        });
        assert_eq!(select_object(skipped), None);
        let selected = (1..100).map(ObjectId::new).find(|object| sampling.includes(*object)).unwrap();
        let session = select_object(selected).unwrap();
        let skipped_event = Record::object(EventKind::MutexAccess, skipped);
        assert!(!record_in_session(session, || Some(skipped_event)));
        let local = local_recorder();
        let policy = GENERAL_POLICY.load(Ordering::Relaxed);
        let capacity = EVENT_CAPACITY.load(Ordering::Relaxed);
        assert!(!record_enabled(
            session.get() + 1,
            EventClass::General,
            Record::object(EventKind::MutexAccess, selected),
            policy,
            capacity,
        ));
        // SAFETY: local_recorder returns this thread's process-lifetime recorder.
        assert!(!unsafe { &*local }.writer_active.load(Ordering::Acquire));
        let constructed = Cell::new(0);
        let event = || {
            constructed.set(constructed.get() + 1);
            Some(Record::object(EventKind::MutexAccess, selected))
        };
        assert!(record_in_session_classified(session, EventClass::General, event));
        assert_eq!(constructed.get(), 1);
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(128).unwrap(),
            ..Default::default()
        });
        assert!(!record_in_session(session, event));
        assert!(!record_in_session_classified(session, EventClass::General, event));
        assert_eq!(constructed.get(), 1);
        configure(Configuration::default());
    }

    #[test]
    fn selection_revalidation_rejects_each_stale_dimension() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            ..Default::default()
        });
        let policy = GENERAL_POLICY.load(Ordering::Relaxed);
        let session = ACTIVE_SESSION.load(Ordering::Relaxed);

        assert!(selection_still_current(EventClass::General, policy, session));
        assert!(!selection_still_current(EventClass::General, policy, 0));
        assert!(!selection_still_current(
            EventClass::General,
            policy ^ u64::from(RECORDING_ENABLED),
            session,
        ));
        assert!(!selection_still_current(EventClass::General, policy, session + 1));

        configure(Configuration::default());
    }

    #[test]
    fn backtrace_and_empty_recorder_paths_are_explicit() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration::default());
        assert_eq!(capture_backtrace(BacktraceCapture::Configured), Vec::new());
        configure(Configuration {
            runtime_tasks: RecordingPolicy::all(false),
            ..Default::default()
        });
        assert_eq!(capture_backtrace(BacktraceCapture::Configured), Vec::new());
        configure(Configuration {
            runtime_tasks: RecordingPolicy::all(true),
            ..Default::default()
        });
        assert_eq!(capture_backtrace(BacktraceCapture::Never), Vec::new());
        let captured = capture_backtrace(BacktraceCapture::Always);
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(miri)))]
        assert!(!captured.is_empty());
        assert!(captured.len() <= MAX_STACK_FRAMES);

        let recorder = ThreadRecorder::new();
        let snapshot = recorder.snapshot();
        assert_eq!((snapshot.log.total_events, snapshot.events.len()), (0, 0));
        assert_eq!(snapshot_from_recorders(1, ptr::null_mut()), None);
        assert_eq!(snapshot_session(0), None);
        assert!(!record_enabled_with_recorder(
            None,
            1,
            EventClass::General,
            Record::object(EventKind::MutexAccess, ObjectId::new(1)),
            0,
            0,
        ));

        let local = LocalRecorder::new();
        drop(local);
        configure(Configuration::default());
    }

    #[test]
    fn cache_policy_participates_in_global_enablement() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration::default());
        configure(Configuration {
            cache: RecordingPolicy::all(false),
            ..Default::default()
        });

        assert_eq!((recording_enabled(), recording_enabled_for(EventClass::Cache)), (true, true));
        configure(Configuration::default());
    }

    #[test]
    fn lock_spin_paths_complete_after_contention() {
        let _test = TEST_LOCK.lock().unwrap();
        let recorder = std::sync::Arc::new(ThreadRecorder::new());
        recorder.ring_locked.store(true, Ordering::Release);
        let release = std::sync::Arc::clone(&recorder);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            release.ring_locked.store(false, Ordering::Release);
        });
        drop(recorder.ring_lock());
        thread.join().unwrap();

        let slot = std::sync::Arc::new(Slot::new());
        slot.locked.store(true, Ordering::Release);
        let release = std::sync::Arc::clone(&slot);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            release.unlock();
        });
        slot.lock();
        slot.unlock();
        thread.join().unwrap();

        CONFIGURATION_LOCKED.store(true, Ordering::Release);
        let thread = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            CONFIGURATION_LOCKED.store(false, Ordering::Release);
        });
        drop(ConfigurationLock::acquire());
        thread.join().unwrap();
        CONFIGURATION_LOCKED.store(false, Ordering::Release);

        let recorder = local_recorder();
        // SAFETY: local_recorder returns this thread's process-lifetime recorder.
        let recorder = unsafe { &*recorder };
        recorder.writer_active.store(true, Ordering::Release);
        std::thread::scope(|scope| {
            let thread = scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                recorder.writer_active.store(false, Ordering::Release);
            });
            wait_for_writers();
            thread.join().unwrap();
        });
    }
}
