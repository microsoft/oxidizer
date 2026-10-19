// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Notification events use the outer object identity and live recording policy.

#![cfg(all(feature = "seismograph", not(miri)))]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use performables::sync::Async;
use performables::sync::notify::Notify;
use seismograph::recorder::event::{EventKind, ObjectId};
use seismograph::recorder::{Configuration, RecordingPolicy};
use seismograph::snapshot::{SnapshotOptions, decode};

#[test]
fn notifications_have_distinct_stable_events_and_live_recording() {
    let notify = Box::new(Notify::<Async>::new());
    let id = ObjectId::from_ptr(std::ptr::from_ref(&*notify).cast::<()>());
    seismograph::recorder(Configuration::default());
    let mut wait = pin!(notify.wait_async());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        ..Default::default()
    });
    notify.notify_one();
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    let mut pending = notify.wait_async();
    assert!(!pending.enable());
    drop(pending);
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::recorder(Configuration::default());
    let kinds = decode(encoded.as_bytes())
        .unwrap()
        .events
        .events
        .into_iter()
        .filter(|event| event.object_id() == Some(id))
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [EventKind::NotifySignal, EventKind::NotifyAccess, EventKind::NotifyContention]
    );
}
