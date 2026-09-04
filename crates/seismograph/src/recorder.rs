// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::struct_field_names,
    reason = "Event totals and the retained event collection are intentionally explicit in the public snapshot model"
)]

//! Allocation-safe, bounded process event recording.

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::cell::{Cell, UnsafeCell};
use std::num::NonZeroU64;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

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

use event::{Address, BacktraceCapture, Event, EventClass, EventClock, EventKind, EventPayload, EventSequence, Events, ObjectId, Record};
use thread::{ThreadId, ThreadLog};

const DEFAULT_EVENT_CAPACITY_PER_THREAD: usize = 65_536;
const MIN_EVENT_CAPACITY_PER_THREAD: usize = 64;
const MAX_EVENT_CAPACITY_PER_THREAD: usize = 1_048_576;
const MAX_EVENT_SAMPLING_ONE_IN: usize = 1_048_576;

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
            u128::from(mixed) * u128::from(denominator) < 1_u128 << 64
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
static CONFIGURATION_LOCKED: AtomicBool = AtomicBool::new(false);
static ACTIVE_SESSION: AtomicU64 = AtomicU64::new(0);
static LAST_SESSION: AtomicU64 = AtomicU64::new(0);
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
    if changed || !enabled {
        ACTIVE_SESSION.store(0, Ordering::SeqCst);
    }
    EVENT_CAPACITY.store(capacity, Ordering::SeqCst);
    ALLOCATION_POLICY.store(allocation_policy, Ordering::SeqCst);
    GENERAL_POLICY.store(general_policy, Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::SeqCst);
    IO_POLICY.store(io_policy, Ordering::SeqCst);
    CACHE_POLICY.store(cache_policy, Ordering::SeqCst);
    if enabled && changed {
        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        LAST_ALLOCATION_POLICY.store(allocation_policy, Ordering::Release);
        LAST_GENERAL_POLICY.store(general_policy, Ordering::Release);
        LAST_ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::Release);
        LAST_RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::Release);
        LAST_IO_POLICY.store(io_policy, Ordering::Release);
        LAST_CACHE_POLICY.store(cache_policy, Ordering::Release);
        LAST_SESSION.store(session, Ordering::Release);
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
    if session == 0
        || !decode_sampling(policy).includes(object_id)
        || policy_atomic(class).load(Ordering::Acquire) != policy
        || ACTIVE_SESSION.load(Ordering::Acquire) != session
    {
        return None;
    }
    RecordingSession::from_raw(session)
}

/// Reads recorder counters without copying retained events.
#[cfg(any(test, feature = "monitor"))]
#[must_use]
pub(crate) fn statistics() -> Statistics {
    let session = LAST_SESSION.load(Ordering::Acquire);
    let capacity = configuration().event_capacity_per_thread;
    let mut statistics = Statistics {
        event_capacity_per_thread: u64::try_from(capacity.get()).unwrap_or(u64::MAX),
        recording: last_recording_policies(),
        ..Statistics::default()
    };
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    while !recorder.is_null() {
        // SAFETY: published recorders are retained for process lifetime.
        let current = unsafe { &*recorder };
        let _ring = current.ring_lock();
        let ring_capacity = current.ring().map_or(0, Ring::capacity);
        statistics.allocated_bytes = statistics.allocated_bytes.saturating_add(
            u64::try_from(std::mem::size_of::<ThreadRecorder>() + ring_capacity * std::mem::size_of::<Slot>()).unwrap_or(u64::MAX),
        );
        if session != 0 && current.session.load(Ordering::Acquire) == session {
            let total_events = u64::try_from(current.write_index.load(Ordering::Acquire)).unwrap_or(u64::MAX);
            let retained_events = total_events.min(u64::try_from(ring_capacity).unwrap_or(u64::MAX));
            statistics.thread_count = statistics.thread_count.saturating_add(1);
            statistics.total_events = statistics.total_events.saturating_add(total_events);
            statistics.retained_events = statistics.retained_events.saturating_add(retained_events);
            statistics.lost_events = statistics.lost_events.saturating_add(total_events.saturating_sub(retained_events));
        }
        recorder = current.next.load(Ordering::Acquire);
    }
    statistics
}

/// Lazily constructs and records an event in a known class.
#[inline]
pub(crate) fn record(class: EventClass, event: impl FnOnce() -> Record) {
    let policy = policy_atomic(class).load(Ordering::Relaxed);
    if !policy_enabled(policy) || is_suppressed() {
        return;
    }
    let session = ACTIVE_SESSION.load(Ordering::Relaxed);
    if session == 0 {
        return;
    }
    let record = event();
    debug_assert_eq!(record.class(), class);
    if record
        .sampling_object_id()
        .is_some_and(|object_id| !decode_sampling(policy).includes(object_id))
    {
        return;
    }
    let capacity = EVENT_CAPACITY.load(Ordering::Relaxed);
    record_enabled(session, class, record, policy, capacity);
}

/// Records an event only while its originating session remains active.
#[inline]
pub(crate) fn record_in_session(session: RecordingSession, event: impl FnOnce() -> Record) -> bool {
    record_in_session_classified(session, EventClass::General, event)
}

/// Records a classified event only while its originating session remains active.
#[inline]
pub(crate) fn record_in_session_classified(session: RecordingSession, class: EventClass, event: impl FnOnce() -> Record) -> bool {
    let policy = policy_atomic(class).load(Ordering::Relaxed);
    if !policy_enabled(policy) || is_suppressed() || ACTIVE_SESSION.load(Ordering::Relaxed) != session.get() {
        return false;
    }
    let record = event();
    debug_assert_eq!(record.class(), class);
    if record
        .sampling_object_id()
        .is_some_and(|object_id| !decode_sampling(policy).includes(object_id))
    {
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
    if policy_atomic(class).load(Ordering::SeqCst) != policy
        || EVENT_CAPACITY.load(Ordering::SeqCst) != capacity
        || ACTIVE_SESSION.load(Ordering::SeqCst) != session
    {
        recorder.writer_active.store(false, Ordering::Release);
        return false;
    }
    let capture_backtrace = match record.backtrace {
        BacktraceCapture::Configured => policy_backtraces(policy),
        BacktraceCapture::Never => false,
        BacktraceCapture::Always => true,
    };
    let (frames, frame_count) = if capture_backtrace {
        capture_stack()
    } else {
        ([0; MAX_STACK_FRAMES], 0)
    };
    recorder.record(session, decode_capacity(capacity), record, frames, frame_count);
    recorder.writer_active.store(false, Ordering::Release);
    true
}

/// Captures all currently retained runtime events.
#[must_use]
pub(crate) fn snapshot(disposition: crate::snapshot::EventBufferDisposition) -> Option<Events> {
    let _suppression = SuppressionGuard::enter();
    if disposition != crate::snapshot::EventBufferDisposition::Retain {
        return destructive_snapshot(disposition);
    }
    let session = LAST_SESSION.load(Ordering::Acquire);
    if session == 0 {
        return None;
    }
    snapshot_from_recorders(session, RECORDERS.load(Ordering::Acquire))
}

fn snapshot_from_recorders(session: u64, mut recorder: *mut ThreadRecorder) -> Option<Events> {
    if recorder.is_null() {
        return None;
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
            let thread = current.snapshot();
            snapshot.total_events = snapshot.total_events.saturating_add(thread.log.total_events);
            snapshot.lost_events = snapshot.lost_events.saturating_add(thread.log.lost_events);
            snapshot.threads.push(thread.log);
            snapshot.events.extend(thread.events);
        }
        recorder = current.next.load(Ordering::Acquire);
    }
    Some(snapshot)
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
    thread_id: ThreadId,
    thread_name: [u8; MAX_THREAD_NAME_LEN],
    thread_name_len: usize,
    ring_locked: AtomicBool,
    ring: UnsafeCell<Option<Ring>>,
    writer_active: AtomicBool,
    retired: AtomicBool,
    write_index: AtomicUsize,
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
            thread_id: ThreadId::new(NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed)),
            thread_name,
            thread_name_len,
            ring_locked: AtomicBool::new(false),
            ring: UnsafeCell::new(None),
            writer_active: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            write_index: AtomicUsize::new(0),
        }
    }

    fn record(&self, session: u64, capacity: EventBufferCapacity, record: Record, frames: [u64; MAX_STACK_FRAMES], frame_count: u8) {
        let new_session = self.session.load(Ordering::Relaxed) != session;
        if new_session {
            self.begin_session(session, capacity);
        }
        let index = self.write_index.fetch_add(1, Ordering::Relaxed);
        let ring = self.ring().expect("begin_session installs an event ring");
        let slot = &ring.slots[index & ring.mask];
        slot.lock();
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
        slot.unlock();
    }

    fn snapshot(&self) -> ThreadSnapshot {
        let _ring = self.ring_lock();
        let total_events = self.write_index.load(Ordering::Acquire);
        let Some(ring) = self.ring() else {
            return ThreadSnapshot {
                log: ThreadLog {
                    thread_id: self.thread_id,
                    total_events: 0,
                    lost_events: 0,
                    name: String::from_utf8_lossy(&self.thread_name[..self.thread_name_len]).into_owned(),
                },
                events: Vec::new(),
            };
        };
        let first = total_events.saturating_sub(ring.capacity());
        let mut events = Vec::with_capacity(total_events - first);
        for index in first..total_events {
            let slot = &ring.slots[index & ring.mask];
            slot.lock();
            // SAFETY: the slot lock prevents the writer from modifying the
            // payload while it is copied.
            let data = unsafe { *slot.data.get() };
            slot.unlock();
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
        ThreadSnapshot {
            log: ThreadLog {
                thread_id: self.thread_id,
                total_events: total_events as u64,
                lost_events: first as u64,
                name: String::from_utf8_lossy(&self.thread_name[..self.thread_name_len]).into_owned(),
            },
            events,
        }
    }

    fn begin_session(&self, session: u64, capacity: EventBufferCapacity) {
        let _ring = self.ring_lock();
        if self.ring().is_some_and(|ring| ring.capacity() == capacity.get()) {
            self.write_index.store(0, Ordering::Relaxed);
            self.session.store(session, Ordering::Release);
            return;
        }
        let replacement = Ring::new(capacity);
        // SAFETY: the owning writer is the only caller that replaces a live
        // ring, and ring_lock excludes snapshots and retirement.
        let previous = unsafe { (&mut *self.ring.get()).replace(replacement) };
        self.write_index.store(0, Ordering::Relaxed);
        self.session.store(session, Ordering::Release);
        drop(previous);
    }

    fn clear(&self, release: bool) {
        let _ring = self.ring_lock();
        self.write_index.store(0, Ordering::Relaxed);
        self.session.store(0, Ordering::Release);
        if release {
            // SAFETY: destructive snapshots first quiesce every writer, and
            // ring_lock excludes concurrent snapshots and retirement.
            let ring = unsafe { (&mut *self.ring.get()).take() };
            drop(ring);
        }
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
    }

    fn ring_lock(&self) -> RingLock<'_> {
        while self
            .ring_locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        RingLock { recorder: self }
    }

    fn ring(&self) -> Option<&Ring> {
        // SAFETY: callers either own this recorder's writer context or hold
        // ring_lock. Ring replacement only occurs under those conditions.
        unsafe { (&*self.ring.get()).as_ref() }
    }
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

    fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
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
        while CONFIGURATION_LOCKED
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        Self
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
    (flags as u64) | ((policy.event_sampling.get() as u64) << SAMPLING_SHIFT)
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

fn destructive_snapshot(disposition: crate::snapshot::EventBufferDisposition) -> Option<Events> {
    let _configuration = ConfigurationLock::acquire();
    let allocation_policy = ALLOCATION_POLICY.load(Ordering::SeqCst);
    let general_policy = GENERAL_POLICY.load(Ordering::SeqCst);
    let arc_dereference_policy = ARC_DEREFERENCE_POLICY.load(Ordering::SeqCst);
    let runtime_task_policy = RUNTIME_TASK_POLICY.load(Ordering::SeqCst);
    let io_policy = IO_POLICY.load(Ordering::SeqCst);
    let cache_policy = CACHE_POLICY.load(Ordering::SeqCst);
    let was_enabled = policy_enabled(allocation_policy)
        || policy_enabled(general_policy)
        || policy_enabled(arc_dereference_policy)
        || policy_enabled(runtime_task_policy)
        || policy_enabled(io_policy)
        || policy_enabled(cache_policy);
    ALLOCATION_POLICY.store(allocation_policy & !u64::from(RECORDING_ENABLED), Ordering::SeqCst);
    GENERAL_POLICY.store(general_policy & !u64::from(RECORDING_ENABLED), Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(arc_dereference_policy & !u64::from(RECORDING_ENABLED), Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(runtime_task_policy & !u64::from(RECORDING_ENABLED), Ordering::SeqCst);
    IO_POLICY.store(io_policy & !u64::from(RECORDING_ENABLED), Ordering::SeqCst);
    CACHE_POLICY.store(cache_policy & !u64::from(RECORDING_ENABLED), Ordering::SeqCst);
    ACTIVE_SESSION.store(0, Ordering::SeqCst);

    wait_for_writers();
    let snapshot = snapshot_session(LAST_SESSION.load(Ordering::Acquire));
    let release = disposition == crate::snapshot::EventBufferDisposition::Release;
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    while !recorder.is_null() {
        // SAFETY: recorders remain registered for process lifetime.
        let current = unsafe { &*recorder };
        current.clear(release || current.retired.load(Ordering::Acquire));
        recorder = current.next.load(Ordering::Acquire);
    }

    if was_enabled {
        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        LAST_SESSION.store(session, Ordering::Release);
        ACTIVE_SESSION.store(session, Ordering::SeqCst);
    }
    ALLOCATION_POLICY.store(allocation_policy, Ordering::SeqCst);
    GENERAL_POLICY.store(general_policy, Ordering::SeqCst);
    ARC_DEREFERENCE_POLICY.store(arc_dereference_policy, Ordering::SeqCst);
    RUNTIME_TASK_POLICY.store(runtime_task_policy, Ordering::SeqCst);
    IO_POLICY.store(io_policy, Ordering::SeqCst);
    CACHE_POLICY.store(cache_policy, Ordering::SeqCst);
    snapshot
}

fn wait_for_writers() {
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    while !recorder.is_null() {
        // SAFETY: recorders remain registered for process lifetime.
        let current = unsafe { &*recorder };
        while current.writer_active.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
        recorder = current.next.load(Ordering::Acquire);
    }
}

fn snapshot_session(session: u64) -> Option<Events> {
    if session == 0 {
        return None;
    }
    let mut recorder = RECORDERS.load(Ordering::Acquire);
    if recorder.is_null() {
        return None;
    }
    let mut snapshot = Events {
        recording: last_recording_policies(),
        ..Events::default()
    };
    while !recorder.is_null() {
        // SAFETY: published recorders are retained for process lifetime.
        let current = unsafe { &*recorder };
        if current.session.load(Ordering::Acquire) == session {
            let thread = current.snapshot();
            snapshot.total_events = snapshot.total_events.saturating_add(thread.log.total_events);
            snapshot.lost_events = snapshot.lost_events.saturating_add(thread.log.lost_events);
            snapshot.threads.push(thread.log);
            snapshot.events.extend(thread.events);
        }
        recorder = current.next.load(Ordering::Acquire);
    }
    Some(snapshot)
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
    let frame_count = platform::capture_stack(&mut frames);
    (
        frames,
        u8::try_from(frame_count).expect("frame count is bounded by the 24-element capture buffer"),
    )
}

#[cfg(all(target_os = "windows", not(miri)))]
mod platform {
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;

    pub(super) fn capture_stack(frames: &mut [u64]) -> usize {
        let mut addresses = [0usize; super::MAX_STACK_FRAMES];
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
}

#[cfg(all(target_os = "linux", not(miri)))]
mod platform {
    pub(super) fn capture_stack(frames: &mut [u64]) -> usize {
        const SKIPPED_FRAMES: usize = 4;
        const CAPACITY: usize = super::MAX_STACK_FRAMES + SKIPPED_FRAMES;
        let mut addresses = [0usize; CAPACITY];
        // SAFETY: addresses is writable for CAPACITY pointers.
        let count = unsafe { libc::backtrace(addresses.as_mut_ptr().cast(), CAPACITY as i32) }.max(0) as usize;
        let retained = count.saturating_sub(SKIPPED_FRAMES).min(frames.len());
        for (destination, address) in frames.iter_mut().zip(addresses.into_iter().skip(SKIPPED_FRAMES)).take(retained) {
            *destination = address as u64;
        }
        retained
    }
}

#[cfg(any(miri, not(any(target_os = "windows", target_os = "linux"))))]
mod platform {
    pub(super) fn capture_stack(_frames: &mut [u64]) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            record(EventClass::Allocation, || Record::object(EventKind::Deallocation, ObjectId::new(1)));
        }
    }

    thread_local! {
        static LATE_TELEMETRY_USER: LateTelemetryUser = const { LateTelemetryUser };
    }

    #[test]
    fn disabled_recording_does_not_construct_events() {
        let _test = TEST_LOCK.lock().unwrap();
        let constructed = AtomicUsize::new(0);
        configure(Configuration::default());
        record(EventClass::ArcDereference, || {
            constructed.fetch_add(1, Ordering::Relaxed);
            Record::object(EventKind::ArcDeref, ObjectId::new(42))
        });
        GENERAL_POLICY.store(encode_policy(RecordingPolicy::all(false)), Ordering::Release);
        ACTIVE_SESSION.store(0, Ordering::Release);
        record(EventClass::General, || {
            constructed.fetch_add(1, Ordering::Relaxed);
            Record::object(EventKind::MutexAccess, ObjectId::new(42))
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
            ..Default::default()
        });
        record(EventClass::General, || Record::object(EventKind::ArcClone, ObjectId::new(42)));
        record(EventClass::ArcDereference, || {
            Record::object(EventKind::ArcDeref, ObjectId::new(42))
        });

        let captured = snapshot(crate::snapshot::EventBufferDisposition::Release).unwrap();

        assert_eq!(
            captured.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![EventKind::ArcDeref]
        );
        configure(Configuration::default());
    }

    #[test]
    fn recording_during_tls_teardown_ignores_destroyed_local_recorder() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            general_events: RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });

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
            ..Default::default()
        });
        record(EventClass::ArcDereference, || {
            Record::object(EventKind::ArcDeref, ObjectId::new(42))
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
            Record::runtime(
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
            )
        });
        record(EventClass::RuntimeTask, || {
            Record::runtime(
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
            )
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

        assert_eq!(current_thread_id(), current_thread_id());
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
    fn object_sampling_selects_approximately_one_in_x_objects() {
        let sampling = EventSampling::one_in(100).unwrap();
        let selected = (0..65_536)
            .map(ObjectId::new)
            .filter(|object_id| sampling.includes(*object_id))
            .count();

        assert!((560..=750).contains(&selected), "selected {selected} objects");
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
        std::thread::spawn(move || record(EventClass::General, || Record::object(EventKind::ArcClone, skipped)))
            .join()
            .unwrap();
        assert_eq!(statistics().thread_count, thread_count);

        record(EventClass::General, || Record::object(EventKind::ArcClone, sampled));
        record(EventClass::General, || Record::object(EventKind::ArcDrop, sampled));
        record(EventClass::General, || Record::object(EventKind::ArcClone, skipped));
        record(EventClass::General, || Record::object(EventKind::ArcDrop, skipped));
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

        record(EventClass::ArcDereference, || Record::object(EventKind::ArcDeref, ObjectId::new(1)));
        let active_bytes = statistics().allocated_bytes;
        let cleared = snapshot(crate::snapshot::EventBufferDisposition::Clear).unwrap();
        let cleared_statistics = statistics();
        record(EventClass::ArcDereference, || Record::object(EventKind::ArcDeref, ObjectId::new(2)));
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
    fn destructive_snapshot_releases_retired_thread_ring() {
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
            record(EventClass::ArcDereference, || Record::object(EventKind::ArcDeref, ObjectId::new(3)));
            active_sender.send(statistics().allocated_bytes).unwrap();
            exit_receiver.recv().unwrap();
        });
        let active_bytes = active_receiver.recv().unwrap();
        exit_sender.send(()).unwrap();
        thread.join().unwrap();
        let retained_bytes = statistics().allocated_bytes;
        let captured = snapshot(crate::snapshot::EventBufferDisposition::Clear).unwrap();
        let retired_bytes = statistics().allocated_bytes;

        assert_eq!(
            (retained_bytes, captured.events.len(), retired_bytes < active_bytes),
            (active_bytes, 1, true)
        );
        configure(Configuration::default());
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

        for _ in 0..32 {
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
        assert!(recording_enabled_for(EventClass::General));
        let session = select_object(ObjectId::new(1)).unwrap();
        {
            let _suppression = SuppressionGuard::enter();
            assert!(!recording_enabled_for(EventClass::General));
            assert_eq!(select_object(ObjectId::new(1)), None);
            let event = Record::object(EventKind::MutexAccess, ObjectId::new(1));
            assert!(!record_in_session(session, || event));
        }
        configure(Configuration::default());
        assert_eq!(select_object(ObjectId::new(1)), None);
        let event = Record::object(EventKind::MutexAccess, ObjectId::new(1));
        assert!(!record_in_session(session, || event));
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
            ..Default::default()
        });
        assert_eq!(select_object(skipped), None);
        let selected = (1..100).map(ObjectId::new).find(|object| sampling.includes(*object)).unwrap();
        let session = select_object(selected).unwrap();
        let skipped_event = Record::object(EventKind::MutexAccess, skipped);
        assert!(!record_in_session(session, || skipped_event));
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
        configure(Configuration {
            general_events: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(128).unwrap(),
            ..Default::default()
        });
        assert!(!record_in_session(session, || { Record::object(EventKind::MutexAccess, selected) }));
        configure(Configuration::default());
    }

    #[test]
    fn backtrace_and_empty_recorder_paths_are_explicit() {
        let _test = TEST_LOCK.lock().unwrap();
        configure(Configuration {
            runtime_tasks: RecordingPolicy::all(true),
            ..Default::default()
        });
        assert_eq!(capture_backtrace(BacktraceCapture::Never), Vec::new());
        assert!(capture_backtrace(BacktraceCapture::Always).len() <= MAX_STACK_FRAMES);

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

        let recorder = local_recorder();
        // SAFETY: local_recorder returns this thread's process-lifetime recorder.
        unsafe { &*recorder }.writer_active.store(true, Ordering::Release);
        let address = recorder.addr();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            // SAFETY: recorder storage is retained for process lifetime.
            unsafe { &*(address as *const ThreadRecorder) }
                .writer_active
                .store(false, Ordering::Release);
        });
        wait_for_writers();
        thread.join().unwrap();
    }
}
