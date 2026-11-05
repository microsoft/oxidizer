// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated disabled-recorder contract: unit tests enable the process-wide recorder.
//! Compile the private telemetry implementation here without exposing a production hook.

use std::cell::Cell;
use std::ptr;

use seismograph::recorder::event::EventClass;

#[path = "../src/telemetry.rs"]
mod telemetry;

use telemetry::{EventKind, record, record_arc_drop, record_channel_high_watermark};

#[test]
fn disabled_drop_recording_does_not_resolve_objects() {
    assert_recording_disabled();

    for kind in [
        EventKind::ArcCreate,
        EventKind::ArcDeref,
        EventKind::ArcClone,
        EventKind::ArcRelocate,
        EventKind::MutexAccess,
        EventKind::MutexContention,
        EventKind::MutexRelease,
        EventKind::RwLockReadAccess,
        EventKind::RwLockReadContention,
        EventKind::RwLockReadRelease,
        EventKind::RwLockWriteAccess,
        EventKind::RwLockWriteContention,
        EventKind::RwLockWriteRelease,
        EventKind::BarrierAccess,
        EventKind::BarrierContention,
        EventKind::BarrierRelease,
        EventKind::CondvarAccess,
        EventKind::CondvarContention,
        EventKind::CondvarNotify,
        EventKind::NotifyAccess,
        EventKind::NotifyContention,
        EventKind::NotifySignal,
        EventKind::OnceAccess,
        EventKind::OnceContention,
        EventKind::OnceInitialize,
        EventKind::ChannelSend,
        EventKind::ChannelSendContention,
        EventKind::ChannelReceive,
        EventKind::ChannelReceiveContention,
        EventKind::ChannelClose,
        EventKind::LockPoisoned,
        EventKind::LockPoisonObserved,
        EventKind::LockPoisonCleared,
    ] {
        record(kind, ptr::null());
    }
    record_channel_high_watermark(ptr::null(), usize::MAX);

    let resolved = Cell::new(false);
    record_arc_drop(|| {
        resolved.set(true);
        Some(ptr::null())
    });

    assert!(!resolved.get());
    assert_recording_disabled();
}

fn assert_recording_disabled() {
    for class in [
        EventClass::Allocation,
        EventClass::General,
        EventClass::ArcDereference,
        EventClass::RuntimeTask,
        EventClass::Io,
        EventClass::Cache,
    ] {
        assert!(!seismograph::recorder::is_enabled(class), "{class:?}");
    }
}
