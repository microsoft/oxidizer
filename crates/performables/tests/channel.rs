// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for executor-independent channels.

#[path = "support/waker.rs"]
mod waker_support;

use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use performables::sync::channel::{bounded, oneshot, unbounded, watch};
#[cfg(feature = "seismograph")]
use seismograph::recorder::Configuration;
#[cfg(feature = "seismograph")]
use seismograph::recorder::event::EventKind;
use waker_support::clone_hook_waker;

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn test_waker(counter: &Arc<WakeCounter>) -> Waker {
    Waker::from(Arc::clone(counter))
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[test]
fn bounded_channel_applies_async_backpressure() {
    let (sender, receiver) = bounded(1);
    sender.try_send(1).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut send = pin!(sender.send(2));

    assert!(send.as_mut().poll(&mut context).is_pending());
    assert_eq!(receiver.try_recv(), Ok(1));
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert_eq!(send.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    assert_eq!(receiver.try_recv(), Ok(2));
}

#[test]
#[should_panic(expected = "bounded channel capacity must be nonzero")]
fn bounded_channel_rejects_zero_capacity() {
    let _ = bounded::<usize>(0);
}

#[test]
fn bounded_channel_applies_blocking_backpressure() {
    let (sender, receiver) = bounded(1);
    sender.send_sync(1).unwrap();
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        started_sender.send(()).unwrap();
        sender.send_sync(2)
    });
    started_receiver.recv().unwrap();

    std::thread::sleep(Duration::from_millis(10));
    assert!(!thread.is_finished());
    assert_eq!(receiver.recv_sync(), Ok(1));
    assert_eq!(thread.join().unwrap(), Ok(()));
    assert_eq!(receiver.recv_sync(), Ok(2));
}

#[test]
fn queue_endpoints_report_state_and_format_errors() {
    let (sender, receiver) = bounded(2);
    assert_eq!(
        (
            sender.is_closed(),
            sender.len(),
            sender.is_empty(),
            sender.capacity(),
            receiver.is_closed(),
            receiver.len(),
            receiver.is_empty(),
            receiver.capacity(),
        ),
        (false, 0, true, Some(2), false, 0, true, Some(2)),
    );

    sender.try_send(1).unwrap();
    sender.try_send(2).unwrap();
    let full = sender.try_send(3).unwrap_err();
    assert_eq!(
        (
            full.to_string(),
            format!("{full:?}"),
            format!("{sender:?}"),
            format!("{receiver:?}"),
            sender.len(),
            sender.is_empty(),
            receiver.len(),
            receiver.is_empty(),
        ),
        (
            "channel has no available capacity".to_owned(),
            "Error { .. }".to_owned(),
            "Sender { len: 2, capacity: Some(2), closed: false, .. }".to_owned(),
            "Receiver { len: 2, capacity: Some(2), closed: false, .. }".to_owned(),
            2,
            false,
            2,
            false,
        ),
    );

    assert_eq!(receiver.try_recv(), Ok(1));
    assert_eq!(receiver.try_recv(), Ok(2));
    let empty = receiver.try_recv().unwrap_err();
    let timeout = receiver.recv_timeout_sync(Duration::from_millis(1)).unwrap_err();
    drop(sender);
    let closed = receiver.try_recv().unwrap_err();

    assert_eq!(
        (empty.to_string(), timeout.to_string(), closed.to_string()),
        (
            "channel has no value available".to_owned(),
            "channel operation timed out".to_owned(),
            "channel is closed".to_owned(),
        ),
    );
}

#[test]
fn queue_receive_timeout_returns_buffered_values() {
    let (sender, receiver) = unbounded();
    sender.try_send(17).unwrap();

    assert_eq!(receiver.recv_timeout_sync(Duration::MAX), Ok(17));
}

#[test]
fn unbounded_channel_supports_multiple_producers_and_consumers() {
    let (sender, receiver) = unbounded();
    let producer_a = sender.clone();
    let producer_b = sender.clone();
    let consumer_a = receiver.clone();
    let consumer_b = receiver.clone();
    drop(receiver);

    let first_consumer = std::thread::spawn(move || {
        let mut values = Vec::new();
        while let Ok(value) = consumer_a.recv_sync() {
            values.push(value);
        }
        values
    });
    let second_consumer = std::thread::spawn(move || {
        let mut values = Vec::new();
        while let Ok(value) = consumer_b.recv_sync() {
            values.push(value);
        }
        values
    });
    let first_producer = std::thread::spawn(move || {
        for value in 0..200 {
            producer_a.send_sync(value).unwrap();
        }
    });
    let second_producer = std::thread::spawn(move || {
        for value in 200..400 {
            producer_b.send_sync(value).unwrap();
        }
    });
    drop(sender);
    first_producer.join().unwrap();
    second_producer.join().unwrap();

    let values = first_consumer
        .join()
        .unwrap()
        .into_iter()
        .chain(second_consumer.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!((values.len(), values.into_iter().sum::<usize>()), (400, 79_800));
}

#[test]
fn cancelled_queue_operations_do_not_consume_values_or_capacity() {
    let (sender, receiver) = bounded(1);
    sender.try_send(1).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    {
        let mut cancelled_send = pin!(sender.send(2));
        assert!(cancelled_send.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!(receiver.try_recv(), Ok(1));
    assert!(receiver.try_recv().unwrap_err().is_empty());
    sender.try_send(3).unwrap();
    assert_eq!(receiver.try_recv(), Ok(3));

    {
        let mut cancelled_receive = pin!(receiver.recv());
        assert!(cancelled_receive.as_mut().poll(&mut context).is_pending());
    }
    sender.try_send(4).unwrap();
    assert_eq!(receiver.try_recv(), Ok(4));
}

#[test]
fn cancelling_a_selected_queue_waiter_hands_readiness_to_the_next_waiter() {
    let (sender, receiver) = bounded(1);
    let first_counter = Arc::new(WakeCounter::default());
    let first_waker = test_waker(&first_counter);
    let mut first_context = Context::from_waker(&first_waker);
    let second_counter = Arc::new(WakeCounter::default());
    let second_waker = test_waker(&second_counter);
    let mut second_context = Context::from_waker(&second_waker);
    let mut first = Box::pin(receiver.recv());
    let mut second = Box::pin(receiver.recv());
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    assert!(second.as_mut().poll(&mut second_context).is_pending());

    sender.try_send(1).unwrap();
    assert_eq!(first_counter.0.load(Ordering::Relaxed), 1);
    drop(first);
    assert_eq!(second_counter.0.load(Ordering::Relaxed), 1);
    assert_eq!(second.as_mut().poll(&mut second_context), Poll::Ready(Ok(1)));

    sender.try_send(2).unwrap();
    let mut first = Box::pin(sender.send(3));
    let mut second = Box::pin(sender.send(4));
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    assert!(second.as_mut().poll(&mut second_context).is_pending());
    assert_eq!(receiver.try_recv(), Ok(2));
    drop(first);
    assert_eq!(second.as_mut().poll(&mut second_context), Poll::Ready(Ok(())));
    assert_eq!(receiver.try_recv(), Ok(4));
}

#[test]
fn cancelling_a_queue_wait_releases_its_registered_waker() {
    let (_sender, receiver) = bounded::<usize>(1);
    let counter = Arc::new(WakeCounter::default());
    let weak = Arc::downgrade(&counter);
    {
        let waker = test_waker(&counter);
        let mut context = Context::from_waker(&waker);
        let mut receive = pin!(receiver.recv());
        assert!(receive.as_mut().poll(&mut context).is_pending());
    }
    drop(counter);

    assert!(weak.upgrade().is_none());
}

#[test]
fn dropping_final_endpoints_wakes_blocked_queue_operations() {
    let (sender, receiver) = bounded::<usize>(1);
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut receive = pin!(receiver.recv());
    assert!(receive.as_mut().poll(&mut context).is_pending());
    drop(sender);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(Err(error)) = receive.as_mut().poll(&mut context) else {
        panic!("dropping the final sender must close a pending receive");
    };
    assert!(error.is_closed());

    let (sender, receiver) = bounded(1);
    sender.try_send(1).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut send = pin!(sender.send(2));
    assert!(send.as_mut().poll(&mut context).is_pending());
    drop(receiver);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(Err(error)) = send.as_mut().poll(&mut context) else {
        panic!("dropping the final receiver must close a pending send");
    };
    assert!(error.is_closed());
    assert_eq!(error.into_value(), Some(2));
}

#[test]
fn closing_queue_preserves_buffered_values_and_rejects_sends() {
    let (sender, receiver) = bounded(2);
    sender.try_send(1).unwrap();

    assert!(receiver.close());
    assert!(!receiver.close());
    assert_eq!(sender.try_send(2).unwrap_err().into_value(), Some(2));
    assert_eq!(receiver.recv_sync(), Ok(1));
    assert!(receiver.recv_sync().unwrap_err().is_closed());
}

#[test]
fn sender_close_wakes_receivers_and_preserves_buffered_values() {
    let (sender, receiver) = bounded(2);
    sender.try_send(1).unwrap();
    assert!(sender.close());
    assert!(!sender.close());
    assert_eq!(receiver.recv_sync(), Ok(1));
    assert!(receiver.recv_sync().unwrap_err().is_closed());

    let (sender, receiver) = unbounded::<usize>();
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut receive = pin!(receiver.recv());
    assert!(receive.as_mut().poll(&mut context).is_pending());
    assert!(sender.close());
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(Err(error)) = receive.as_mut().poll(&mut context) else {
        panic!("closing the sender must close a pending receive");
    };
    assert!(error.is_closed());
}

#[test]
fn queue_receive_timeout_distinguishes_timeout_and_close() {
    let (sender, receiver) = unbounded::<usize>();
    assert!(receiver.recv_timeout_sync(Duration::from_millis(1)).unwrap_err().is_timeout());
    drop(sender);
    assert!(receiver.recv_timeout_sync(Duration::from_secs(1)).unwrap_err().is_closed());
}

#[test]
fn oneshot_sends_once_and_reports_sender_drop() {
    let (sender, receiver) = oneshot();
    sender.send(17).unwrap();
    assert_eq!(block_on(receiver), Ok(17));

    let (sender, receiver) = oneshot::<usize>();
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut receiver = pin!(receiver);
    assert!(receiver.as_mut().poll(&mut context).is_pending());
    drop(sender);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(Err(error)) = receiver.as_mut().poll(&mut context) else {
        panic!("dropping the oneshot sender must close the receiver");
    };
    assert!(error.is_closed());

    let (sender, receiver) = oneshot();
    drop(receiver);
    assert_eq!(sender.send(23).unwrap_err().into_value(), Some(23));
}

#[test]
fn oneshot_supports_blocking_timeout_and_nonblocking_receives() {
    let (sender, receiver) = oneshot();
    sender.send(11).unwrap();
    assert_eq!(receiver.recv_sync(), Ok(11));

    let (sender, receiver) = oneshot();
    sender.send(12).unwrap();
    assert_eq!(receiver.recv_timeout_sync(Duration::MAX), Ok(12));

    let (_sender, mut receiver) = oneshot::<usize>();
    assert!(receiver.try_recv().unwrap_err().is_empty());
    assert!(receiver.recv_timeout_sync(Duration::from_millis(1)).unwrap_err().is_timeout());

    let (sender, mut receiver) = oneshot::<usize>();
    drop(sender);
    assert!(receiver.try_recv().unwrap_err().is_closed());

    let (sender, receiver) = oneshot::<usize>();
    drop(sender);
    assert!(receiver.recv_sync().unwrap_err().is_closed());

    let (sender, receiver) = oneshot::<usize>();
    drop(sender);
    assert!(receiver.recv_timeout_sync(Duration::MAX).unwrap_err().is_closed());
}

#[test]
fn oneshot_reports_endpoint_state_and_cancels_registered_waiters() {
    let (sender, mut receiver) = oneshot();
    assert_eq!(
        (
            sender.is_closed(),
            receiver.is_closed(),
            format!("{sender:?}"),
            format!("{receiver:?}"),
        ),
        (
            false,
            false,
            "OneshotSender { closed: false, .. }".to_owned(),
            "OneshotReceiver { closed: false, .. }".to_owned(),
        ),
    );

    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    assert!(Pin::new(&mut receiver).poll(&mut context).is_pending());
    sender.send(13).unwrap();
    assert_eq!(receiver.try_recv(), Ok(13));

    let (sender, mut receiver) = oneshot::<usize>();
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    assert!(Pin::new(&mut receiver).poll(&mut context).is_pending());
    drop(receiver);
    assert!(sender.is_closed());
}

#[test]
fn oneshot_observes_values_sent_during_waiter_registration() {
    let (sender, mut receiver) = oneshot();
    let waker = clone_hook_waker(move || sender.send(17).unwrap());
    let mut context = Context::from_waker(&waker);

    assert_eq!(Pin::new(&mut receiver).poll(&mut context), Poll::Ready(Ok(17)));
}

#[test]
fn watch_tracks_versions_per_receiver() {
    let (sender, first) = watch("initial");
    let second = first.clone();
    assert_eq!(*sender.borrow(), "initial");
    assert_eq!(first.has_changed(), Ok(false));
    assert_eq!(sender.send_replace("replaced"), "initial");
    sender.send_modify(|value| *value = "updated");
    assert_eq!(first.has_changed(), Ok(true));
    assert_eq!(second.has_changed(), Ok(true));

    assert_eq!(block_on(first.changed()), Ok(()));
    assert_eq!(*first.borrow(), "updated");
    assert_eq!(first.has_changed(), Ok(false));
    assert_eq!(*second.borrow_and_update(), "updated");
    assert_eq!(second.has_changed(), Ok(false));

    let subscribed = sender.subscribe();
    assert_eq!(subscribed.has_changed(), Ok(false));
    drop(sender);
    assert!(subscribed.changed_sync().unwrap_err().is_closed());
}

#[test]
fn watch_reports_closed_endpoints_and_final_unmatched_values() {
    let (sender, receiver) = watch(1);
    let sender_clone = sender.clone();
    drop(sender);
    assert!(!receiver.is_closed());
    drop(sender_clone);

    assert!(receiver.is_closed());
    assert!(receiver.has_changed().unwrap_err().is_closed());
    assert!(block_on(receiver.wait_for(|value| *value == 2)).unwrap_err().is_closed());
    assert_eq!(format!("{receiver:?}"), "WatchReceiver { closed: true, changed: false, .. }");

    let (sender, receiver) = watch(1);
    drop(receiver);
    assert!(sender.is_closed());
    let error = sender.send(2).unwrap_err();
    assert_eq!(
        (error.into_value(), format!("{sender:?}")),
        (Some(2), "WatchSender { closed: true, .. }".to_owned()),
    );
}

#[test]
fn watch_reference_formats_the_borrowed_value() {
    let (sender, receiver) = watch(17);
    let sender_ref = sender.borrow();
    assert_eq!(
        (format!("{sender_ref:?}"), format!("{sender_ref}")),
        ("17".to_owned(), "17".to_owned())
    );
    drop(sender_ref);

    let receiver_ref = receiver.borrow();
    assert_eq!(
        (format!("{receiver_ref:?}"), format!("{receiver_ref}")),
        ("17".to_owned(), "17".to_owned())
    );
}

#[test]
fn watch_wait_for_observes_values_until_the_predicate_matches() {
    let (sender, receiver) = watch(1);
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(receiver.wait_for(|value| *value >= 3));
    assert!(wait.as_mut().poll(&mut context).is_pending());

    sender.send_replace(2);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(wait.as_mut().poll(&mut context).is_pending());
    sender.send_modify(|value| *value += 1);
    let Poll::Ready(Ok(value)) = wait.as_mut().poll(&mut context) else {
        panic!("the matching watch value must complete wait_for");
    };
    assert_eq!(*value, 3);
}

#[test]
fn dropping_final_watch_sender_wakes_changed() {
    let (sender, receiver) = watch(1);
    let counter = Arc::new(WakeCounter::default());
    let waker = test_waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut changed = pin!(receiver.changed());
    assert!(changed.as_mut().poll(&mut context).is_pending());

    drop(sender);
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(Err(error)) = changed.as_mut().poll(&mut context) else {
        panic!("dropping the final watch sender must close changed");
    };
    assert!(error.is_closed());
}

#[test]
fn watch_observes_updates_sent_during_waiter_registration() {
    let (sender, receiver) = watch(1);
    let waker = clone_hook_waker(move || sender.send(2).unwrap());
    let mut context = Context::from_waker(&waker);
    let mut changed = pin!(receiver.changed());

    assert_eq!(changed.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    assert_eq!(*receiver.borrow(), 2);
}

#[test]
fn watch_blocking_change_wakes_for_updates() {
    let (sender, receiver) = watch(1);
    let waiter = std::thread::spawn(move || {
        receiver.changed_sync().unwrap();
        *receiver.borrow()
    });

    sender.send(2).unwrap();
    assert_eq!(waiter.join().unwrap(), 2);
}

#[test]
fn channel_futures_are_send_for_send_values() {
    fn assert_send<T: Send>(_: T) {}

    let (sender, receiver) = bounded(1);
    assert_send(sender.send(1));
    assert_send(receiver.recv());

    let (sender, receiver) = oneshot::<usize>();
    assert_send(sender);
    assert_send(receiver);

    let (sender, receiver) = watch(1);
    assert_send(sender);
    assert_send(receiver.changed());
    assert_send(receiver.wait_for(|value| *value == 2));
}

#[test]
#[cfg(feature = "seismograph")]
fn channel_operations_share_runtime_telemetry_identity() {
    seismograph::recorder(Configuration {
        general_events: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let _ = seismograph::snapshot(seismograph::snapshot::SnapshotOptions {
        event_buffers: seismograph::snapshot::EventBufferDisposition::Release,
    });

    let (bounded_sender, bounded_receiver) = bounded(1);
    bounded_sender.try_send(1).unwrap();
    assert!(bounded_sender.try_send(2).is_err());
    assert_eq!(bounded_receiver.try_recv(), Ok(1));

    let (sender, receiver) = unbounded();
    sender.try_send(1).unwrap();
    sender.try_send(2).unwrap();
    sender.try_send(3).unwrap();
    assert_eq!(receiver.try_recv(), Ok(1));
    sender.try_send(4).unwrap();
    drop((bounded_sender, bounded_receiver, sender, receiver));

    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap().events;
    let channel_ids = snapshot
        .events
        .iter()
        .filter(|event| event.kind == EventKind::ChannelSend)
        .filter_map(seismograph::recorder::event::Event::object_id)
        .collect::<std::collections::HashSet<_>>();
    assert!(channel_ids.iter().any(|object_id| {
        snapshot
            .events
            .iter()
            .filter(|event| event.object_id() == Some(*object_id))
            .any(|event| event.kind == EventKind::ChannelReceive)
            && snapshot
                .events
                .iter()
                .filter(|event| event.object_id() == Some(*object_id))
                .any(|event| event.kind == EventKind::ChannelSendContention)
            && snapshot
                .events
                .iter()
                .filter(|event| event.object_id() == Some(*object_id))
                .any(|event| event.kind == EventKind::ChannelClose)
    }));
    let watermark_object = snapshot
        .events
        .iter()
        .find(|event| event.kind == EventKind::ChannelHighWatermark && event.measurement() == Some(3))
        .unwrap()
        .object_id()
        .unwrap();
    assert_eq!(
        snapshot
            .events
            .iter()
            .filter(|event| event.object_id() == Some(watermark_object) && event.kind == EventKind::ChannelHighWatermark)
            .filter_map(seismograph::recorder::event::Event::measurement)
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
    );

    seismograph::recorder(Configuration::default());
}
