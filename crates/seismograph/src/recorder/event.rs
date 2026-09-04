// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! General event types.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::alloc::{Allocation, AllocationId};
use super::io::IoEvent;
use super::runtime::RuntimeEvent;
use super::thread::{ThreadId, ThreadLog};

/// Independently configurable class of recorded event.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum EventClass {
    /// Allocation and deallocation lifecycle events.
    Allocation,
    /// Primitive events other than Arc dereferences.
    General,
    /// High-frequency Arc dereference events.
    ArcDereference,
    /// Runtime, worker, task, transfer, and I/O scheduling events.
    RuntimeTask,
    /// I/O primitive read and write operations.
    Io,
    /// Cache lookup, mutation, refresh, and eviction events.
    Cache,
}

macro_rules! identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Creates an identifier from its stable numeric representation.
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// Returns the stable numeric representation.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }
    };
}

identifier!(EventSequence, "Monotonic sequence number within one thread recorder.");
identifier!(ObjectId, "Identity assigned to an instrumented object.");
impl ObjectId {
    /// Uses a pointer's numeric address as an object identity.
    #[must_use]
    pub fn from_ptr<T>(address: *const T) -> Self {
        Self(address.addr() as u64)
    }
}

/// Historical process address retained for offline analysis.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Address(u64);

impl Address {
    /// Creates a historical address from its fixed-width representation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Captures the numeric address represented by a pointer.
    #[must_use]
    pub fn from_ptr<T>(address: *const T) -> Self {
        Self(address.addr() as u64)
    }

    /// Returns the fixed-width address representation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Timestamp from the process-wide seismograph monotonic clock.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct EventTimestamp(u64);

impl EventTimestamp {
    /// Reconstructs a timestamp from its fixed-width tick representation.
    #[must_use]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self(ticks)
    }

    /// Reads the process-wide monotonic event clock.
    #[must_use]
    #[inline]
    pub fn now() -> Self {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();

        let elapsed = ORIGIN.get_or_init(Instant::now).elapsed();
        Self(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
    }

    /// Returns the timestamp's clock ticks.
    #[must_use]
    pub const fn ticks(self) -> u64 {
        self.0
    }

    /// Returns the duration elapsed since `earlier`.
    #[must_use]
    pub fn duration_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

/// Clock used by timestamps in an [`Events`] collection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum EventClock {
    /// Historical events did not carry timestamps.
    #[default]
    Unspecified,
    /// Nanoseconds elapsed from a process-local monotonic origin.
    ProcessMonotonic,
}

impl EventClock {
    /// Clock metadata emitted for newly recorded events.
    pub const CURRENT: Self = Self::ProcessMonotonic;

    /// Returns the number of clock ticks per second, when known.
    #[must_use]
    pub const fn ticks_per_second(self) -> Option<u64> {
        match self {
            Self::Unspecified => None,
            Self::ProcessMonotonic => Some(1_000_000_000),
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn wire_value(self) -> u16 {
        match self {
            Self::Unspecified => 0,
            Self::ProcessMonotonic => 1,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn from_wire_value(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Unspecified),
            1 => Some(Self::ProcessMonotonic),
            _ => None,
        }
    }
}

/// Backtrace behavior requested by an event producer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BacktraceCapture {
    /// Follows the process-wide recorder configuration.
    #[default]
    Configured,
    /// Omits a backtrace even when global capture is enabled.
    Never,
    /// Captures a backtrace even when global capture is disabled.
    Always,
}

/// Kind of operation represented by a runtime event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EventKind {
    /// A shared-pointer allocation was created.
    ArcCreate,
    /// The final strong pointer released and destroyed its allocation.
    ArcDrop,
    /// A shared pointer was dereferenced.
    ArcDeref,
    /// A shared pointer was cloned.
    ArcClone,
    /// A thread-aware shared pointer was relocated.
    ArcRelocate,
    /// A mutex was acquired.
    MutexAccess,
    /// A mutex acquisition observed contention.
    MutexContention,
    /// A mutex guard released its lock.
    MutexRelease,
    /// A reader-writer lock was acquired for reading.
    RwLockReadAccess,
    /// A read acquisition observed contention.
    RwLockReadContention,
    /// A read guard released its lock.
    RwLockReadRelease,
    /// A reader-writer lock was acquired for writing.
    RwLockWriteAccess,
    /// A write acquisition observed contention.
    RwLockWriteContention,
    /// A write guard released its lock.
    RwLockWriteRelease,
    /// A barrier participant completed its wait.
    BarrierAccess,
    /// A barrier participant waited for the current generation.
    BarrierContention,
    /// The final participant released a barrier generation.
    BarrierRelease,
    /// A condition-variable waiter reacquired its mutex.
    CondvarAccess,
    /// A condition-variable waiter blocked for notification.
    CondvarContention,
    /// A condition variable emitted a notification.
    CondvarNotify,
    /// A one-time cell returned its initialized value.
    OnceAccess,
    /// A one-time cell waited for concurrent initialization.
    OnceContention,
    /// A one-time cell initialized its value.
    OnceInitialize,
    /// A channel successfully sent or published a value.
    ChannelSend,
    /// A channel send observed unavailable capacity.
    ChannelSendContention,
    /// A channel successfully received or observed a value.
    ChannelReceive,
    /// A channel receive observed no available value or version.
    ChannelReceiveContention,
    /// A channel endpoint closed a direction or completed its lifecycle.
    ChannelClose,
    /// A queue channel reached a new maximum number of buffered values.
    ChannelHighWatermark,
    /// A logical runtime was created.
    RuntimeCreated,
    /// A logical runtime began stopping.
    RuntimeStopping,
    /// A logical runtime stopped.
    RuntimeStopped,
    /// A runtime worker started.
    WorkerStarted,
    /// A runtime worker stopped.
    WorkerStopped,
    /// A runtime worker parked.
    WorkerParked,
    /// A runtime worker unparked.
    WorkerUnparked,
    /// A task was assigned its process identity.
    TaskSpawned,
    /// A task was enqueued for execution.
    TaskEnqueued,
    /// A task instance was materialized.
    TaskMaterialized,
    /// A task poll began.
    TaskPollStarted,
    /// A task poll finished.
    TaskPollFinished,
    /// A task completed successfully.
    TaskCompleted,
    /// A task was canceled.
    TaskCanceled,
    /// A task panicked.
    TaskPanicked,
    /// A task instance transfer began.
    TransferStarted,
    /// A task instance changed worker ownership.
    InstanceRelocated,
    /// A task instance transfer finished.
    TransferFinished,
    /// An exclusive lock guard poisoned its lock during panic unwinding.
    LockPoisoned,
    /// A successful lock acquisition observed that the lock was poisoned.
    LockPoisonObserved,
    /// A lock's poison state was explicitly cleared.
    LockPoisonCleared,
    /// An allocator created an allocation.
    Allocation,
    /// An allocator released an allocation.
    Deallocation,
    /// A logical I/O read operation began.
    IoReadStarted,
    /// A logical I/O read operation finished.
    IoReadFinished,
    /// A logical I/O write operation began.
    IoWriteStarted,
    /// A logical I/O write operation finished.
    IoWriteFinished,
    /// A cache lookup returned a valid entry.
    CacheHit,
    /// A cache lookup did not find an entry.
    CacheMiss,
    /// A cache lookup found an expired entry.
    CacheExpired,
    /// A cache lookup failed.
    CacheGetError,
    /// A cache tier accepted an entry.
    CacheInserted,
    /// A cache tier rejected an entry.
    CacheInsertRejected,
    /// A cache insertion failed.
    CacheInsertError,
    /// A cache entry was invalidated.
    CacheInvalidated,
    /// A cache invalidation failed.
    CacheInvalidateError,
    /// A cache tier was cleared.
    CacheCleared,
    /// Clearing a cache tier failed.
    CacheClearError,
    /// A background refresh found an entry.
    CacheRefreshHit,
    /// A background refresh did not find an entry.
    CacheRefreshMiss,
    /// A background refresh lookup failed.
    CacheRefreshError,
    /// A cache entry was evicted because of capacity pressure.
    CacheEvicted,
    /// A cache-aside computation produced a value.
    CacheComputeSucceeded,
    /// A cache-aside computation failed.
    CacheComputeFailed,
    /// An optional cache-aside computation produced no value.
    CacheComputeReturnedNone,
    /// A fallback entry was promoted into a higher-priority tier.
    CachePromotionAccepted,
    /// A higher-priority tier rejected a fallback promotion.
    CachePromotionRejected,
    /// A fallback promotion failed.
    CachePromotionFailed,
    /// A background refresh was already in progress.
    CacheRefreshSuppressed,
}

impl EventKind {
    #[doc(hidden)]
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        match self {
            Self::ArcDeref => 1,
            Self::ArcClone => 2,
            Self::MutexAccess => 3,
            Self::MutexContention => 4,
            Self::RwLockReadAccess => 5,
            Self::RwLockReadContention => 6,
            Self::RwLockWriteAccess => 7,
            Self::RwLockWriteContention => 8,
            Self::ArcCreate => 9,
            Self::MutexRelease => 10,
            Self::RwLockReadRelease => 11,
            Self::RwLockWriteRelease => 12,
            Self::ArcDrop => 13,
            Self::Allocation => 14,
            Self::Deallocation => 15,
            Self::ArcRelocate => 16,
            Self::BarrierAccess => 17,
            Self::BarrierContention => 18,
            Self::BarrierRelease => 19,
            Self::CondvarAccess => 20,
            Self::CondvarContention => 21,
            Self::CondvarNotify => 22,
            Self::OnceAccess => 23,
            Self::OnceContention => 24,
            Self::OnceInitialize => 25,
            Self::ChannelSend => 26,
            Self::ChannelSendContention => 27,
            Self::ChannelReceive => 28,
            Self::ChannelReceiveContention => 29,
            Self::ChannelClose => 30,
            Self::ChannelHighWatermark => 31,
            Self::RuntimeCreated => 32,
            Self::RuntimeStopping => 33,
            Self::RuntimeStopped => 34,
            Self::WorkerStarted => 35,
            Self::WorkerStopped => 36,
            Self::WorkerParked => 37,
            Self::WorkerUnparked => 38,
            Self::TaskSpawned => 39,
            Self::TaskEnqueued => 40,
            Self::TaskMaterialized => 41,
            Self::TaskPollStarted => 42,
            Self::TaskPollFinished => 43,
            Self::TaskCompleted => 44,
            Self::TaskCanceled => 45,
            Self::TaskPanicked => 46,
            Self::TransferStarted => 47,
            Self::InstanceRelocated => 48,
            Self::TransferFinished => 49,
            Self::LockPoisoned => 54,
            Self::LockPoisonObserved => 55,
            Self::LockPoisonCleared => 56,
            Self::IoReadStarted => 57,
            Self::IoReadFinished => 58,
            Self::IoWriteStarted => 59,
            Self::IoWriteFinished => 60,
            Self::CacheHit => 61,
            Self::CacheMiss => 62,
            Self::CacheExpired => 63,
            Self::CacheGetError => 64,
            Self::CacheInserted => 65,
            Self::CacheInsertRejected => 66,
            Self::CacheInsertError => 67,
            Self::CacheInvalidated => 68,
            Self::CacheInvalidateError => 69,
            Self::CacheCleared => 70,
            Self::CacheClearError => 71,
            Self::CacheRefreshHit => 72,
            Self::CacheRefreshMiss => 73,
            Self::CacheRefreshError => 74,
            Self::CacheEvicted => 75,
            Self::CacheComputeSucceeded => 76,
            Self::CacheComputeFailed => 77,
            Self::CacheComputeReturnedNone => 78,
            Self::CachePromotionAccepted => 79,
            Self::CachePromotionRejected => 80,
            Self::CachePromotionFailed => 81,
            Self::CacheRefreshSuppressed => 82,
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::ArcDeref),
            2 => Some(Self::ArcClone),
            3 => Some(Self::MutexAccess),
            4 => Some(Self::MutexContention),
            5 => Some(Self::RwLockReadAccess),
            6 => Some(Self::RwLockReadContention),
            7 => Some(Self::RwLockWriteAccess),
            8 => Some(Self::RwLockWriteContention),
            9 => Some(Self::ArcCreate),
            10 => Some(Self::MutexRelease),
            11 => Some(Self::RwLockReadRelease),
            12 => Some(Self::RwLockWriteRelease),
            13 => Some(Self::ArcDrop),
            14 => Some(Self::Allocation),
            15 => Some(Self::Deallocation),
            16 => Some(Self::ArcRelocate),
            17 => Some(Self::BarrierAccess),
            18 => Some(Self::BarrierContention),
            19 => Some(Self::BarrierRelease),
            20 => Some(Self::CondvarAccess),
            21 => Some(Self::CondvarContention),
            22 => Some(Self::CondvarNotify),
            23 => Some(Self::OnceAccess),
            24 => Some(Self::OnceContention),
            25 => Some(Self::OnceInitialize),
            26 => Some(Self::ChannelSend),
            27 => Some(Self::ChannelSendContention),
            28 => Some(Self::ChannelReceive),
            29 => Some(Self::ChannelReceiveContention),
            30 => Some(Self::ChannelClose),
            31 => Some(Self::ChannelHighWatermark),
            32 => Some(Self::RuntimeCreated),
            33 => Some(Self::RuntimeStopping),
            34 => Some(Self::RuntimeStopped),
            35 => Some(Self::WorkerStarted),
            36 => Some(Self::WorkerStopped),
            37 => Some(Self::WorkerParked),
            38 => Some(Self::WorkerUnparked),
            39 => Some(Self::TaskSpawned),
            40 => Some(Self::TaskEnqueued),
            41 => Some(Self::TaskMaterialized),
            42 => Some(Self::TaskPollStarted),
            43 => Some(Self::TaskPollFinished),
            44 => Some(Self::TaskCompleted),
            45 => Some(Self::TaskCanceled),
            46 => Some(Self::TaskPanicked),
            47 => Some(Self::TransferStarted),
            48 => Some(Self::InstanceRelocated),
            49 => Some(Self::TransferFinished),
            54 => Some(Self::LockPoisoned),
            55 => Some(Self::LockPoisonObserved),
            56 => Some(Self::LockPoisonCleared),
            57 => Some(Self::IoReadStarted),
            58 => Some(Self::IoReadFinished),
            59 => Some(Self::IoWriteStarted),
            60 => Some(Self::IoWriteFinished),
            61 => Some(Self::CacheHit),
            62 => Some(Self::CacheMiss),
            63 => Some(Self::CacheExpired),
            64 => Some(Self::CacheGetError),
            65 => Some(Self::CacheInserted),
            66 => Some(Self::CacheInsertRejected),
            67 => Some(Self::CacheInsertError),
            68 => Some(Self::CacheInvalidated),
            69 => Some(Self::CacheInvalidateError),
            70 => Some(Self::CacheCleared),
            71 => Some(Self::CacheClearError),
            72 => Some(Self::CacheRefreshHit),
            73 => Some(Self::CacheRefreshMiss),
            74 => Some(Self::CacheRefreshError),
            75 => Some(Self::CacheEvicted),
            76 => Some(Self::CacheComputeSucceeded),
            77 => Some(Self::CacheComputeFailed),
            78 => Some(Self::CacheComputeReturnedNone),
            79 => Some(Self::CachePromotionAccepted),
            80 => Some(Self::CachePromotionRejected),
            81 => Some(Self::CachePromotionFailed),
            82 => Some(Self::CacheRefreshSuppressed),
            _ => None,
        }
    }

    /// Returns whether this kind uses the fixed runtime context payload.
    #[must_use]
    pub const fn is_runtime(self) -> bool {
        matches!(
            self,
            Self::RuntimeCreated
                | Self::RuntimeStopping
                | Self::RuntimeStopped
                | Self::WorkerStarted
                | Self::WorkerStopped
                | Self::WorkerParked
                | Self::WorkerUnparked
                | Self::TaskSpawned
                | Self::TaskEnqueued
                | Self::TaskMaterialized
                | Self::TaskPollStarted
                | Self::TaskPollFinished
                | Self::TaskCompleted
                | Self::TaskCanceled
                | Self::TaskPanicked
                | Self::TransferStarted
                | Self::InstanceRelocated
                | Self::TransferFinished
        )
    }

    /// Returns whether this kind uses the fixed I/O context payload.
    #[must_use]
    pub const fn is_io(self) -> bool {
        matches!(
            self,
            Self::IoReadStarted | Self::IoReadFinished | Self::IoWriteStarted | Self::IoWriteFinished
        )
    }
}

/// Numeric measurement associated with an instrumented object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NumericEvent {
    /// Identity of the measured object.
    pub object_id: ObjectId,
    /// Numeric value observed for the object.
    pub value: u64,
}

/// Fixed-shape payload carried by one event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EventPayload {
    /// Identity of a general instrumented object.
    Object(ObjectId),
    /// Numeric measurement associated with an instrumented object.
    Numeric(NumericEvent),
    /// Allocator lifecycle fields.
    Allocation(Allocation),
    /// Runtime, worker, task, transfer, or I/O context.
    Runtime(RuntimeEvent),
    /// I/O resource, operation, and buffer context.
    Io(IoEvent),
}

/// One retained runtime event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    /// Identifier assigned to the thread-local event recorder.
    pub thread_id: ThreadId,
    /// Monotonic sequence number within that thread's event log.
    pub sequence: EventSequence,
    /// Process-monotonic timestamp captured when the event was recorded.
    pub timestamp: EventTimestamp,
    /// Operation that was observed.
    pub kind: EventKind,
    /// Fixed-shape data selected by the event kind.
    pub payload: EventPayload,
    /// Captured instruction-pointer frames, when requested.
    pub call_stack: Vec<Address>,
}

impl Event {
    /// Returns an object identity for object and numeric events.
    #[must_use]
    pub const fn object_id(&self) -> Option<ObjectId> {
        match self.payload {
            EventPayload::Object(object_id) => Some(object_id),
            EventPayload::Numeric(payload) => Some(payload.object_id),
            EventPayload::Allocation(allocation) => Some(ObjectId::new(allocation.allocation_id.get())),
            EventPayload::Runtime(_) => None,
            EventPayload::Io(io) => Some(ObjectId::new(io.resource_id.get())),
        }
    }

    /// Returns a numeric measurement for numeric events.
    #[must_use]
    pub const fn measurement(&self) -> Option<u64> {
        match self.payload {
            EventPayload::Numeric(payload) => Some(payload.value),
            _ => None,
        }
    }

    /// Returns allocator fields for allocation lifecycle events.
    #[must_use]
    pub const fn allocation(&self) -> Option<Allocation> {
        match self.payload {
            EventPayload::Allocation(allocation) => Some(allocation),
            _ => None,
        }
    }

    /// Returns runtime context for runtime events.
    #[must_use]
    pub const fn runtime(&self) -> Option<RuntimeEvent> {
        match self.payload {
            EventPayload::Runtime(runtime) => Some(runtime),
            _ => None,
        }
    }

    /// Returns I/O context for I/O events.
    #[must_use]
    pub const fn io(&self) -> Option<IoEvent> {
        match self.payload {
            EventPayload::Io(io) => Some(io),
            _ => None,
        }
    }
}

impl EventKind {
    /// Returns the independently configurable recording class for this event.
    #[must_use]
    pub const fn class(self) -> EventClass {
        match self {
            Self::Allocation | Self::Deallocation => EventClass::Allocation,
            Self::ArcDeref => EventClass::ArcDereference,
            Self::RuntimeCreated
            | Self::RuntimeStopping
            | Self::RuntimeStopped
            | Self::WorkerStarted
            | Self::WorkerStopped
            | Self::WorkerParked
            | Self::WorkerUnparked
            | Self::TaskSpawned
            | Self::TaskEnqueued
            | Self::TaskMaterialized
            | Self::TaskPollStarted
            | Self::TaskPollFinished
            | Self::TaskCompleted
            | Self::TaskCanceled
            | Self::TaskPanicked
            | Self::TransferStarted
            | Self::InstanceRelocated
            | Self::TransferFinished => EventClass::RuntimeTask,
            Self::IoReadStarted | Self::IoReadFinished | Self::IoWriteStarted | Self::IoWriteFinished => EventClass::Io,
            Self::CacheHit
            | Self::CacheMiss
            | Self::CacheExpired
            | Self::CacheGetError
            | Self::CacheInserted
            | Self::CacheInsertRejected
            | Self::CacheInsertError
            | Self::CacheInvalidated
            | Self::CacheInvalidateError
            | Self::CacheCleared
            | Self::CacheClearError
            | Self::CacheRefreshHit
            | Self::CacheRefreshMiss
            | Self::CacheRefreshError
            | Self::CacheEvicted
            | Self::CacheComputeSucceeded
            | Self::CacheComputeFailed
            | Self::CacheComputeReturnedNone
            | Self::CachePromotionAccepted
            | Self::CachePromotionRejected
            | Self::CachePromotionFailed
            | Self::CacheRefreshSuppressed => EventClass::Cache,
            _ => EventClass::General,
        }
    }
}

/// Retained runtime events from all initialized recording threads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Events {
    /// Clock shared by all timestamps in this collection.
    pub clock: EventClock,
    /// Total events emitted across all thread logs.
    pub total_events: u64,
    /// Events overwritten across all thread logs.
    pub lost_events: u64,
    /// Per-class policies used while recording these events.
    pub recording: super::RecordingPolicies,
    /// Per-thread bounded-log summaries.
    pub threads: Vec<ThreadLog>,
    /// Events retained at snapshot time.
    pub events: Vec<Event>,
}

impl Default for Events {
    fn default() -> Self {
        Self {
            clock: EventClock::Unspecified,
            total_events: 0,
            lost_events: 0,
            recording: super::RecordingPolicies::default(),
            threads: Vec::new(),
            events: Vec::new(),
        }
    }
}

/// Event data constructed only when runtime telemetry is enabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub(super) timestamp: EventTimestamp,
    pub(super) kind: EventKind,
    pub(super) payload: EventPayload,
    pub(super) backtrace: BacktraceCapture,
    pub(super) sample_object: bool,
}

impl Record {
    /// Creates a general-purpose object event.
    #[must_use]
    pub fn object(kind: EventKind, object_id: ObjectId) -> Self {
        Self {
            timestamp: EventTimestamp::now(),
            kind,
            payload: EventPayload::Object(object_id),
            backtrace: BacktraceCapture::Configured,
            sample_object: true,
        }
    }

    /// Creates a general-purpose object event with a numeric measurement.
    #[must_use]
    pub fn object_measurement(kind: EventKind, object_id: ObjectId, measurement: u64) -> Self {
        Self {
            timestamp: EventTimestamp::now(),
            kind,
            payload: EventPayload::Numeric(NumericEvent {
                object_id,
                value: measurement,
            }),
            backtrace: BacktraceCapture::Configured,
            sample_object: true,
        }
    }

    /// Creates an allocation event.
    #[must_use]
    pub fn allocation(allocation: Allocation) -> Self {
        Self::allocation_lifecycle(EventKind::Allocation, allocation)
    }

    /// Creates a deallocation event.
    #[must_use]
    pub fn deallocation(allocation: Allocation) -> Self {
        Self::allocation_lifecycle(EventKind::Deallocation, allocation)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn runtime(timestamp: EventTimestamp, kind: EventKind, runtime: RuntimeEvent, backtrace: BacktraceCapture) -> Self {
        Self {
            timestamp,
            kind,
            payload: EventPayload::Runtime(runtime),
            backtrace,
            sample_object: false,
        }
    }

    /// Creates an I/O event.
    #[must_use]
    pub fn io(kind: EventKind, io: IoEvent) -> Self {
        debug_assert!(kind.is_io());
        Self {
            timestamp: EventTimestamp::now(),
            kind,
            payload: EventPayload::Io(io),
            backtrace: BacktraceCapture::Configured,
            sample_object: true,
        }
    }

    fn allocation_lifecycle(kind: EventKind, allocation: Allocation) -> Self {
        Self {
            timestamp: EventTimestamp::now(),
            kind,
            payload: EventPayload::Allocation(allocation),
            backtrace: BacktraceCapture::Configured,
            sample_object: true,
        }
    }

    pub(super) const fn class(self) -> EventClass {
        self.kind.class()
    }

    pub(super) const fn sampling_object_id(self) -> Option<ObjectId> {
        if !self.sample_object {
            return None;
        }
        match self.payload {
            EventPayload::Object(object_id) => Some(object_id),
            EventPayload::Numeric(payload) => Some(payload.object_id),
            EventPayload::Allocation(allocation) => Some(ObjectId::new(AllocationId::get(allocation.allocation_id))),
            EventPayload::Runtime(_) => None,
            EventPayload::Io(io) => Some(ObjectId::new(io.resource_id.get())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::alloc::{EventThreadId, HeapId, HeapKind};
    use crate::recorder::runtime::RuntimeId;

    fn allocation() -> Allocation {
        Allocation {
            allocation_id: AllocationId::new(11),
            event_thread_id: EventThreadId::new(12),
            heap_id: HeapId::new(13),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(14),
            size: 15,
            alignment: 16,
        }
    }

    #[test]
    fn pointer_identities_preserve_numeric_addresses() {
        let value = 42_u64;
        let pointer = &raw const value;
        assert_eq!(ObjectId::from_ptr(pointer).get(), pointer.addr() as u64);
        assert_eq!(Address::from_ptr(pointer).get(), pointer.addr() as u64);
    }

    #[test]
    fn clocks_report_metadata_and_reject_unknown_values() {
        assert_eq!(EventClock::Unspecified.ticks_per_second(), None);
        assert_eq!(EventClock::Unspecified.wire_value(), 0);
        assert_eq!(EventClock::from_wire_value(2), None);
    }

    #[test]
    fn every_event_kind_round_trips_its_stable_wire_value() {
        for value in (1..=49).chain(54..=82) {
            let kind = EventKind::from_wire_value(value).unwrap();
            assert_eq!(kind.wire_value(), value);
        }
        for value in [0, 50, 51, 52, 53] {
            assert_eq!(EventKind::from_wire_value(value), None);
        }
    }

    #[test]
    fn payload_accessors_distinguish_all_payload_shapes() {
        let allocation = allocation();
        let runtime = RuntimeEvent {
            runtime_id: RuntimeId::from_raw(1).unwrap(),
            worker_id: None,
            subject_id: 2,
            related_id: 3,
            value_0: 4,
            value_1: 5,
        };
        let events = [
            Event {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(1),
                timestamp: EventTimestamp::from_ticks(1),
                kind: EventKind::MutexAccess,
                payload: EventPayload::Object(ObjectId::new(7)),
                call_stack: Vec::new(),
            },
            Event {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(2),
                timestamp: EventTimestamp::from_ticks(2),
                kind: EventKind::ChannelHighWatermark,
                payload: EventPayload::Numeric(NumericEvent {
                    object_id: ObjectId::new(8),
                    value: 9,
                }),
                call_stack: Vec::new(),
            },
            Event {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(3),
                timestamp: EventTimestamp::from_ticks(3),
                kind: EventKind::Allocation,
                payload: EventPayload::Allocation(allocation),
                call_stack: Vec::new(),
            },
            Event {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(4),
                timestamp: EventTimestamp::from_ticks(4),
                kind: EventKind::RuntimeCreated,
                payload: EventPayload::Runtime(runtime),
                call_stack: Vec::new(),
            },
        ];

        assert_eq!(
            events
                .iter()
                .map(|event| (event.object_id(), event.measurement(), event.allocation(), event.runtime(),))
                .collect::<Vec<_>>(),
            vec![
                (Some(ObjectId::new(7)), None, None, None),
                (Some(ObjectId::new(8)), Some(9), None, None),
                (Some(ObjectId::new(11)), None, Some(allocation), None),
                (None, None, None, Some(runtime)),
            ]
        );
    }

    #[test]
    fn record_constructors_select_payload_and_sampling_identity() {
        let allocation = allocation();
        let object = Record::object_measurement(EventKind::ChannelHighWatermark, ObjectId::new(1), 2);
        let allocated = Record::allocation(allocation);
        let deallocated = Record::deallocation(allocation);
        let runtime = Record::runtime(
            EventTimestamp::from_ticks(3),
            EventKind::RuntimeCreated,
            RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: None,
                subject_id: 0,
                related_id: 0,
                value_0: 0,
                value_1: 0,
            },
            BacktraceCapture::Never,
        );
        let sampled_runtime = Record {
            sample_object: true,
            ..runtime
        };

        assert_eq!(
            (
                object.sampling_object_id(),
                allocated.kind,
                allocated.sampling_object_id(),
                deallocated.kind,
                deallocated.sampling_object_id(),
                runtime.sampling_object_id(),
                sampled_runtime.sampling_object_id(),
            ),
            (
                Some(ObjectId::new(1)),
                EventKind::Allocation,
                Some(ObjectId::new(11)),
                EventKind::Deallocation,
                Some(ObjectId::new(11)),
                None,
                None,
            )
        );
    }
}
