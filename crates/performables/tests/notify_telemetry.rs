// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Notification events use the outer object identity and live recording policy.

#![cfg(all(feature = "seismograph", not(miri)))]

use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

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
    reentrant_delivery_records_signal_before_access();
}

struct ReentrantWait(Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>);

impl Wake for ReentrantWait {
    #[expect(clippy::unwrap_used, reason = "a poisoned test-future lock is a test harness failure")]
    fn wake(self: Arc<Self>) {
        let mut wait = self.0.lock().unwrap();
        assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper follows the surrounding test's assertion and snapshot checks"
)]
fn reentrant_delivery_records_signal_before_access() {
    let notify = Arc::new(Notify::<Async>::new());
    let id = ObjectId::from_ptr(Arc::as_ptr(&notify).cast::<()>());
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        ..Default::default()
    });
    let wait = Arc::new(ReentrantWait(Mutex::new(Box::pin({
        let notify = Arc::clone(&notify);
        async move { notify.wait_async().await }
    }))));
    let waker = Waker::from(Arc::clone(&wait));
    assert!(wait.0.lock().unwrap().as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

    notify.notify_one();

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
        [EventKind::NotifyContention, EventKind::NotifySignal, EventKind::NotifyAccess]
    );
}
