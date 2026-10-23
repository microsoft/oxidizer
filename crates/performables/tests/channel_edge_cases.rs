// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ownership and callback edge cases for queue, oneshot and watch channels.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::spawn;

use performables::sync::channel::{bounded, oneshot, unbounded, watch};
use support::{DEADLINE, Task, join};
use testing_aids::is_mutation_testing;

#[derive(Debug)]
struct Payload(Arc<AtomicUsize>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct PanicWake;

impl Wake for PanicWake {
    #[expect(clippy::panic, reason = "this adversarial test waker deliberately unwinds")]
    fn wake(self: Arc<Self>) {
        panic!("test a consumer waker that panics");
    }
}

#[test]
fn last_queue_receiver_drops_buffered_payloads_while_senders_remain() {
    for capacity in [None, Some(4)] {
        let (sender, receiver) = capacity.map_or_else(unbounded, bounded);
        let second = receiver.clone();
        let drops = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            sender.try_send(Payload(Arc::clone(&drops))).unwrap();
        }
        drop(receiver);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(second);
        assert_eq!(drops.load(Ordering::Relaxed), 3);
        assert!(sender.is_closed());
        assert!(sender.is_empty());
    }
}

#[test]
fn failed_queue_sends_preserve_their_payload_until_recovered_or_dropped() {
    let (sender, receiver) = bounded(1);
    let buffered = Arc::new(AtomicUsize::new(0));
    let rejected = Arc::new(AtomicUsize::new(0));
    sender.try_send(Payload(Arc::clone(&buffered))).unwrap();
    let error = sender.try_send(Payload(Arc::clone(&rejected))).unwrap_err();
    assert!(error.is_full());
    assert_eq!(rejected.load(Ordering::Relaxed), 0);
    let value = error.into_value().unwrap();
    assert_eq!(rejected.load(Ordering::Relaxed), 0);
    drop(value);
    assert_eq!(rejected.load(Ordering::Relaxed), 1);
    drop(receiver);
    let error = sender.send(Payload(Arc::clone(&rejected))).unwrap_err();
    assert!(error.is_closed());
    drop(error);
    assert_eq!((buffered.load(Ordering::Relaxed), rejected.load(Ordering::Relaxed)), (1, 2));
}

#[test]
fn cancelling_unpolled_and_pending_sends_drops_only_the_unsent_values() {
    let (sender, receiver) = bounded(1);
    let buffered = Arc::new(AtomicUsize::new(0));
    let unsent = Arc::new(AtomicUsize::new(0));
    sender.try_send(Payload(Arc::clone(&buffered))).unwrap();
    drop(sender.send_async(Payload(Arc::clone(&unsent))));
    let task = Task::new();
    let mut pending = Box::pin(sender.send_async(Payload(Arc::clone(&unsent))));
    assert!(task.poll(pending.as_mut()).is_pending());
    assert_eq!(task.references(), 3);
    drop(pending);
    assert_eq!((unsent.load(Ordering::Relaxed), buffered.load(Ordering::Relaxed)), (2, 0));
    assert_eq!(task.references(), 2);
    drop(receiver.recv().unwrap());
    assert_eq!(buffered.load(Ordering::Relaxed), 1);
}

#[test]
fn buffered_payload_cleanup_continues_when_one_destructor_panics() {
    #[derive(Debug)]
    struct PanickingPayload {
        drops: Arc<AtomicUsize>,
        panic: bool,
    }
    impl Drop for PanickingPayload {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
            assert!(!self.panic, "test the first queued payload unwinding");
        }
    }

    let (sender, receiver) = unbounded();
    let drops = Arc::new(AtomicUsize::new(0));
    for index in 0..4 {
        sender
            .try_send(PanickingPayload {
                drops: Arc::clone(&drops),
                panic: index == 0,
            })
            .unwrap();
    }
    catch_unwind(AssertUnwindSafe(|| drop(receiver))).unwrap_err();
    assert_eq!(drops.load(Ordering::Relaxed), 4);
    assert!(sender.is_closed());
    assert!(sender.is_empty());
}

#[test]
fn queue_receive_replaces_and_releases_the_previous_task_waker() {
    let (sender, receiver) = bounded(1);
    let first = Task::new();
    let second = Task::new();
    let mut wait = Box::pin(receiver.recv_async());
    assert!(first.poll(wait.as_mut()).is_pending());
    assert_eq!(first.references(), 3);
    assert!(second.poll(wait.as_mut()).is_pending());
    assert_eq!((first.references(), second.references()), (2, 3));
    sender.send(11).unwrap();
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    assert_eq!(second.poll(wait.as_mut()), Poll::Ready(Ok(11)));
    drop(wait);
    assert_eq!(second.references(), 2);
}

#[test]
fn queue_send_replaces_and_releases_the_previous_task_waker() {
    let (sender, receiver) = bounded(1);
    sender.send(7).unwrap();
    let first = Task::new();
    let second = Task::new();
    let mut wait = Box::pin(sender.send_async(11));
    assert!(first.poll(wait.as_mut()).is_pending());
    assert!(second.poll(wait.as_mut()).is_pending());
    assert_eq!((first.references(), second.references()), (2, 3));
    assert_eq!(receiver.recv(), Ok(7));
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    assert_eq!(second.poll(wait.as_mut()), Poll::Ready(Ok(())));
    drop(wait);
    assert_eq!(second.references(), 2);
    assert_eq!(receiver.recv(), Ok(11));
}

#[test]
fn panicking_receiver_wake_does_not_lose_a_committed_queue_value() {
    let (sender, receiver) = bounded(1);
    let mut wait = Box::pin(receiver.recv_async());
    let waker = Waker::from(Arc::new(PanicWake));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    catch_unwind(AssertUnwindSafe(|| sender.send(17))).unwrap_err();
    assert_eq!((sender.len(), receiver.try_recv()), (1, Ok(17)));
    drop(wait);
    sender.send(23).unwrap();
    assert_eq!(receiver.recv(), Ok(23));
}

#[test]
fn a_panicking_waker_clone_leaves_queue_registration_recoverable() {
    let (sender, receiver) = bounded(1);
    let mut wait = Box::pin(receiver.recv_async());
    let waker = waker_support::clone_hook_waker(|| panic!("test a waker clone failure"));
    catch_unwind(AssertUnwindSafe(|| wait.as_mut().poll(&mut Context::from_waker(&waker)))).unwrap_err();
    // A panicked async-fn future cannot be resumed; its registration must still
    // be removable so a fresh operation on the same channel can proceed.
    drop(wait);
    let mut wait = Box::pin(receiver.recv_async());
    let task = Task::new();
    assert!(task.poll(wait.as_mut()).is_pending());
    sender.send(29).unwrap();
    assert_eq!(task.wakes(), 1);
    assert_eq!(task.poll(wait.as_mut()), Poll::Ready(Ok(29)));
}

#[test]
fn queue_payload_destruction_can_reenter_its_channel_after_receiver_drop() {
    #[derive(Debug)]
    struct Reenter(performables::sync::channel::Sender<Self>, Arc<AtomicUsize>);
    impl Drop for Reenter {
        fn drop(&mut self) {
            assert!(self.0.is_closed());
            assert!(self.0.is_empty());
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
    let (sender, receiver) = bounded(1);
    let drops = Arc::new(AtomicUsize::new(0));
    sender.try_send(Reenter(sender.clone(), Arc::clone(&drops))).unwrap();
    drop(receiver);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

struct CloseQueueOnDrop(performables::sync::channel::Receiver<usize>);

impl Wake for CloseQueueOnDrop {
    fn wake(self: Arc<Self>) {
        unreachable!("the test replaces this waker before its drop closes the channel");
    }
}

impl Drop for CloseQueueOnDrop {
    fn drop(&mut self) {
        let _closed = self.0.close();
    }
}

#[test]
fn replacing_a_sender_waker_can_close_its_own_channel() {
    let (sender, receiver) = bounded(1);
    sender.send(7).unwrap();
    let waker = Waker::from(Arc::new(CloseQueueOnDrop(receiver.clone())));
    let mut wait = Box::pin(sender.send_async(11));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    let task = Task::new();
    let Poll::Ready(Err(error)) = task.poll(wait.as_mut()) else {
        panic!("the old waker destructor closes the pending sender's channel");
    };
    assert!(error.is_closed());
    assert_eq!(error.into_value(), Some(11));
    assert_eq!(receiver.recv(), Ok(7));
    assert!(receiver.recv().unwrap_err().is_closed());
}

#[test]
fn oneshot_receive_moves_waker_ownership_between_tasks() {
    let (sender, mut receiver) = oneshot();
    let first = Task::new();
    let second = Task::new();
    assert!(first.poll(Pin::new(&mut receiver)).is_pending());
    assert!(second.poll(Pin::new(&mut receiver)).is_pending());
    assert_eq!((first.references(), second.references()), (2, 3));
    sender.send(31).unwrap();
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    assert_eq!(second.poll(Pin::new(&mut receiver)), Poll::Ready(Ok(31)));
    assert_eq!(second.references(), 2);
    assert!(receiver.try_recv().unwrap_err().is_closed());
}

#[test]
fn a_panicking_oneshot_waker_replacement_keeps_the_previous_registration() {
    let (sender, mut receiver) = oneshot();
    let task = Task::new();
    assert!(task.poll(Pin::new(&mut receiver)).is_pending());
    let waker = waker_support::clone_hook_waker(|| panic!("test replacement clone failing"));
    catch_unwind(AssertUnwindSafe(|| Pin::new(&mut receiver).poll(&mut Context::from_waker(&waker)))).unwrap_err();
    assert_eq!(task.references(), 3);
    sender.send(33).unwrap();
    assert_eq!(task.wakes(), 1);
    assert_eq!(task.poll(Pin::new(&mut receiver)), Poll::Ready(Ok(33)));
    assert_eq!(task.references(), 2);
}

#[test]
fn dropping_a_pending_oneshot_releases_its_waker_and_returns_the_send_value() {
    let (sender, mut receiver) = oneshot();
    let task = Task::new();
    assert!(task.poll(Pin::new(&mut receiver)).is_pending());
    assert_eq!(task.references(), 3);
    drop(receiver);
    assert_eq!(task.references(), 2);
    let drops = Arc::new(AtomicUsize::new(0));
    let error = sender.send(Payload(Arc::clone(&drops))).unwrap_err();
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(error.into_value().unwrap());
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn dropping_an_unread_oneshot_destroys_its_sent_payload_once() {
    let (sender, receiver) = oneshot();
    let drops = Arc::new(AtomicUsize::new(0));
    sender.send(Payload(Arc::clone(&drops))).unwrap();
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(receiver);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn panicking_oneshot_wake_keeps_the_sent_value_available() {
    let (sender, mut receiver) = oneshot();
    let waker = Waker::from(Arc::new(PanicWake));
    assert!(Pin::new(&mut receiver).poll(&mut Context::from_waker(&waker)).is_pending());
    catch_unwind(AssertUnwindSafe(|| sender.send(37))).unwrap_err();
    assert_eq!(receiver.try_recv(), Ok(37));
    assert!(receiver.try_recv().unwrap_err().is_closed());
}

#[test]
fn non_send_payloads_work_without_leaving_the_current_thread() {
    let value = Rc::new(String::from("local"));
    let (sender, receiver) = bounded(1);
    sender.send(Rc::clone(&value)).unwrap();
    assert!(Rc::ptr_eq(&receiver.recv().unwrap(), &value));
    let (sender, receiver) = unbounded();
    sender.send(Rc::clone(&value)).unwrap();
    assert!(Rc::ptr_eq(&receiver.recv().unwrap(), &value));
    let (sender, receiver) = oneshot();
    sender.send(Rc::clone(&value)).unwrap();
    assert!(Rc::ptr_eq(&receiver.recv().unwrap(), &value));
    let (_sender, receiver) = watch(Rc::clone(&value));
    assert!(Rc::ptr_eq(&receiver.borrow(), &value));
}

#[test]
fn watch_subscription_reopens_a_channel_after_the_last_receiver_leaves() {
    let (sender, receiver) = watch(41);
    drop(receiver);
    assert!(sender.is_closed());
    assert_eq!(sender.send(43).unwrap_err().into_value(), Some(43));
    let receiver = sender.subscribe();
    assert!(!sender.is_closed());
    assert_eq!((*receiver.borrow(), receiver.has_changed()), (41, Ok(false)));
    sender.send(47).unwrap();
    assert_eq!((receiver.changed(), *receiver.borrow()), (Ok(()), 47));
}

#[test]
fn watch_replace_and_modify_keep_values_when_no_receivers_exist() {
    let (sender, receiver) = watch(1);
    drop(receiver);
    assert_eq!(sender.send_replace(2), 1);
    sender.send_modify(|value| *value += 3);
    let receiver = sender.subscribe();
    assert_eq!((*receiver.borrow(), receiver.has_changed()), (5, Ok(false)));
}

#[test]
fn watch_publishes_equal_values_and_receiver_clones_keep_independent_versions() {
    let (sender, first) = watch(7);
    sender.send(7).unwrap();
    let second = first.clone();
    assert_eq!(first.changed(), Ok(()));
    assert_eq!((first.has_changed(), second.has_changed()), (Ok(false), Ok(true)));
    assert_eq!(*second.borrow_and_update(), 7);
    assert_eq!(second.has_changed(), Ok(false));
}

#[test]
fn watch_wait_for_returns_the_exact_value_accepted_by_its_predicate() {
    let (sender, receiver) = watch(1);
    sender.send(5).unwrap();
    let mut accepted = None;
    let mut wait = Box::pin(receiver.wait_for_async(|value| {
        accepted = Some(*value);
        true
    }));
    let task = Task::new();
    let Poll::Ready(Ok(value)) = task.poll(wait.as_mut()) else {
        panic!("an accepting predicate must return the borrowed value");
    };
    assert_eq!(*value, 5);
    drop(value);
    drop(wait);
    assert_eq!(accepted, Some(5));
}

#[test]
fn concurrent_sender_drops_preserve_the_final_watch_publication() {
    let (sender, receiver) = watch(0);
    let workers = if cfg!(miri) { 2 } else { 4 };
    let start = Arc::new(Barrier::new(workers + 1));
    let handles = (0..workers)
        .map(|worker| {
            let sender = sender.clone();
            let start = Arc::clone(&start);
            spawn(move || {
                start.wait();
                if worker == 0 {
                    sender.send(53).unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    let task = Task::new();
    let mut changed = Box::pin(receiver.changed_async());
    assert!(task.poll(changed.as_mut()).is_pending());
    drop(sender);
    start.wait();
    for handle in handles {
        join(handle);
    }
    assert!(task.wakes() >= 1);
    assert_eq!(task.poll(changed.as_mut()), Poll::Ready(Ok(())));
    drop(changed);
    assert_eq!(*receiver.borrow(), 53);
    assert!(receiver.is_closed());
    assert!(receiver.changed().unwrap_err().is_closed());
}

#[test]
fn concurrent_mpmc_delivery_has_no_duplicate_or_missing_messages() {
    let producers = if cfg!(miri) { 2 } else { 4 };
    let items = if cfg!(miri) { 8 } else { 256 };
    let (sender, receiver) = bounded(3);
    let consumers = (0..producers)
        .map(|_| {
            let receiver = receiver.clone();
            spawn(move || {
                let mut messages = Vec::new();
                loop {
                    let result = if is_mutation_testing() {
                        receiver.recv()
                    } else {
                        receiver.recv_timeout(DEADLINE)
                    };
                    match result {
                        Ok(value) => messages.push(value),
                        Err(error) => {
                            assert!(error.is_closed(), "a consumer must not lose progress");
                            return messages;
                        }
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    drop(receiver);
    let workers = (0..producers)
        .map(|producer| {
            let sender = sender.clone();
            spawn(move || {
                for item in 0..items {
                    sender.send(producer * items + item).unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    drop(sender);
    for worker in workers {
        join(worker);
    }
    let mut messages = Vec::new();
    for consumer in consumers {
        messages.extend(join(consumer));
    }
    messages.sort_unstable();
    assert_eq!(messages, (0..producers * items).collect::<Vec<_>>());
}

#[test]
fn oneshot_send_racing_receiver_drop_destroys_each_value_exactly_once() {
    for _ in 0..if cfg!(miri) { 2 } else { 32 } {
        let (sender, receiver) = oneshot();
        let drops = Arc::new(AtomicUsize::new(0));
        let worker = spawn({
            let drops = Arc::clone(&drops);
            move || drop(sender.send(Payload(drops)))
        });
        drop(receiver);
        join(worker);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
#[path = "support/edge_cases.rs"]
mod support;
#[path = "support/waker.rs"]
mod waker_support;
