// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "seismograph")]
use seismograph::recorder::event::{EventClass, EventKind as SeismographEventKind, ObjectId, Record};

#[derive(Clone, Copy)]
pub(crate) enum EventKind {
    ArcCreate,
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
    NotifyAccess,
    NotifyContention,
    NotifySignal,
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
    seismograph::record(kind.class(), move || {
        let kind = match kind {
            EventKind::ArcCreate => SeismographEventKind::ArcCreate,
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
            EventKind::NotifyAccess => SeismographEventKind::NotifyAccess,
            EventKind::NotifyContention => SeismographEventKind::NotifyContention,
            EventKind::NotifySignal => SeismographEventKind::NotifySignal,
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
        Some(Record::object(kind, ObjectId::from_ptr(object)))
    });

    #[cfg(not(feature = "seismograph"))]
    let _ = (kind, object);
}

#[cfg(feature = "seismograph")]
impl EventKind {
    const fn class(self) -> EventClass {
        match self {
            Self::ArcDeref => EventClass::ArcDereference,
            _ => EventClass::General,
        }
    }
}

pub(crate) fn record_arc_drop(object: impl FnOnce() -> Option<*const ()>) {
    // A fixed event kind keeps conditional drop captures small enough to pass
    // by value, without preparing an aggregate on the disabled path.
    #[cfg(feature = "seismograph")]
    seismograph::record(EventClass::General, move || {
        Some(Record::object(SeismographEventKind::ArcDrop, ObjectId::from_ptr(object()?)))
    });

    #[cfg(not(feature = "seismograph"))]
    let _ = object;
}

#[inline]
pub(crate) fn record_channel_high_watermark(object: *const (), high_watermark: usize) {
    #[cfg(feature = "seismograph")]
    seismograph::record(EventClass::General, || {
        Some(Record::object_measurement(
            SeismographEventKind::ChannelHighWatermark,
            ObjectId::from_ptr(object),
            u64::try_from(high_watermark).unwrap_or(u64::MAX),
        ))
    });

    #[cfg(not(feature = "seismograph"))]
    let _ = (object, high_watermark);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::ptr;

    use super::*;

    #[test]
    fn disabled_drop_recording_does_not_resolve_objects() {
        let resolved = Cell::new(false);
        record_arc_drop(|| {
            resolved.set(true);
            Some(ptr::null())
        });

        assert!(!resolved.get());
    }
}
