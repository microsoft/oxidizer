// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[derive(Clone, Copy)]
pub(crate) enum EventKind {
    ArcCreate,
    ArcDrop,
    ArcDeref,
    ArcClone,
    ArcRelocate,
    MutexAccess,
    MutexContention,
    MutexRelease,
    RwLockReadAccess,
    RwLockReadContention,
    RwLockReadRelease,
    RwLockWriteAccess,
    RwLockWriteContention,
    RwLockWriteRelease,
    BarrierAccess,
    BarrierContention,
    BarrierRelease,
    CondvarAccess,
    CondvarContention,
    CondvarNotify,
    OnceAccess,
    OnceContention,
    OnceInitialize,
    ChannelSend,
    ChannelSendContention,
    ChannelReceive,
    ChannelReceiveContention,
    ChannelClose,
    LockPoisoned,
    LockPoisonObserved,
    LockPoisonCleared,
}

#[inline]
pub(crate) fn record(kind: EventKind, object: *const ()) {
    #[cfg(feature = "seismograph")]
    seismograph::record(kind.class(), || {
        use seismograph::recorder::event::{EventKind as SeismographEventKind, ObjectId, Record};

        let kind = match kind {
            EventKind::ArcCreate => SeismographEventKind::ArcCreate,
            EventKind::ArcDrop => SeismographEventKind::ArcDrop,
            EventKind::ArcDeref => SeismographEventKind::ArcDeref,
            EventKind::ArcClone => SeismographEventKind::ArcClone,
            EventKind::ArcRelocate => SeismographEventKind::ArcRelocate,
            EventKind::MutexAccess => SeismographEventKind::MutexAccess,
            EventKind::MutexContention => SeismographEventKind::MutexContention,
            EventKind::MutexRelease => SeismographEventKind::MutexRelease,
            EventKind::RwLockReadAccess => SeismographEventKind::RwLockReadAccess,
            EventKind::RwLockReadContention => SeismographEventKind::RwLockReadContention,
            EventKind::RwLockReadRelease => SeismographEventKind::RwLockReadRelease,
            EventKind::RwLockWriteAccess => SeismographEventKind::RwLockWriteAccess,
            EventKind::RwLockWriteContention => SeismographEventKind::RwLockWriteContention,
            EventKind::RwLockWriteRelease => SeismographEventKind::RwLockWriteRelease,
            EventKind::BarrierAccess => SeismographEventKind::BarrierAccess,
            EventKind::BarrierContention => SeismographEventKind::BarrierContention,
            EventKind::BarrierRelease => SeismographEventKind::BarrierRelease,
            EventKind::CondvarAccess => SeismographEventKind::CondvarAccess,
            EventKind::CondvarContention => SeismographEventKind::CondvarContention,
            EventKind::CondvarNotify => SeismographEventKind::CondvarNotify,
            EventKind::OnceAccess => SeismographEventKind::OnceAccess,
            EventKind::OnceContention => SeismographEventKind::OnceContention,
            EventKind::OnceInitialize => SeismographEventKind::OnceInitialize,
            EventKind::ChannelSend => SeismographEventKind::ChannelSend,
            EventKind::ChannelSendContention => SeismographEventKind::ChannelSendContention,
            EventKind::ChannelReceive => SeismographEventKind::ChannelReceive,
            EventKind::ChannelReceiveContention => SeismographEventKind::ChannelReceiveContention,
            EventKind::ChannelClose => SeismographEventKind::ChannelClose,
            EventKind::LockPoisoned => SeismographEventKind::LockPoisoned,
            EventKind::LockPoisonObserved => SeismographEventKind::LockPoisonObserved,
            EventKind::LockPoisonCleared => SeismographEventKind::LockPoisonCleared,
        };
        Record::object(kind, ObjectId::from_ptr(object))
    });

    #[cfg(not(feature = "seismograph"))]
    let _ = (kind, object);
}

#[cfg(feature = "seismograph")]
impl EventKind {
    const fn class(self) -> seismograph::recorder::event::EventClass {
        match self {
            Self::ArcDeref => seismograph::recorder::event::EventClass::ArcDereference,
            _ => seismograph::recorder::event::EventClass::General,
        }
    }
}

#[inline]
pub(crate) fn record_channel_high_watermark(object: *const (), high_watermark: usize) {
    #[cfg(feature = "seismograph")]
    seismograph::record(seismograph::recorder::event::EventClass::General, || {
        use seismograph::recorder::event::{EventKind, ObjectId, Record};

        Record::object_measurement(
            EventKind::ChannelHighWatermark,
            ObjectId::from_ptr(object),
            u64::try_from(high_watermark).unwrap_or(u64::MAX),
        )
    });

    #[cfg(not(feature = "seismograph"))]
    let _ = (object, high_watermark);
}
