// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Registration ordering and adversarial callback contracts for retained waits.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Instant;

use performables::sync::Async;
use performables::sync::notify::Notify;
use support::{DEADLINE, Task, join};
use testing_aids::is_mutation_testing;

#[test]
fn notification_order_follows_registration_not_future_construction() {
    let notify = Notify::<Async>::new();
    let mut first_created = Box::pin(notify.wait_async());
    let mut first_registered = Box::pin(notify.wait_async());
    let earlier = Task::new();
    let later = Task::new();
    assert!(earlier.poll(first_registered.as_mut()).is_pending());
    assert!(later.poll(first_created.as_mut()).is_pending());
    notify.notify_one();
    assert_eq!((earlier.wakes(), later.wakes()), (1, 0));
    assert!(earlier.poll(first_registered.as_mut()).is_ready());
    assert!(later.poll(first_created.as_mut()).is_pending());
    notify.notify_one();
    assert!(later.poll(first_created.as_mut()).is_ready());
}

#[test]
fn enabled_wait_reserves_a_permit_until_completion_or_cancellation() {
    let notify = Notify::<Async>::new();
    notify.notify_one();
    let mut reserved = notify.wait_async();
    assert!(reserved.enable());
    assert!(!notify.try_wait());
    let mut next = Box::pin(notify.wait_async());
    let task = Task::new();
    assert!(task.poll(next.as_mut()).is_pending());
    drop(reserved);
    assert_eq!(task.wakes(), 1);
    assert!(task.poll(next.as_mut()).is_ready());
    assert!(!notify.try_wait());
}

#[test]
fn consecutive_selected_cancellations_forward_one_notification_without_duplication() {
    let notify = Notify::<Async>::new();
    let tasks = [Task::new(), Task::new(), Task::new(), Task::new()];
    let mut waits = tasks
        .iter()
        .map(|task| {
            let mut wait = Box::pin(notify.wait_async());
            assert!(task.poll(wait.as_mut()).is_pending());
            wait
        })
        .collect::<Vec<_>>();
    notify.notify_one();
    for task in &tasks[..3] {
        assert_eq!(task.wakes(), 1);
        drop(waits.remove(0));
        assert!(!notify.try_wait());
    }
    assert_eq!(tasks[3].wakes(), 1);
    assert!(tasks[3].poll(waits[0].as_mut()).is_ready());
    drop(waits);
    assert!(!notify.try_wait());
}

struct NotifyOnDrop(Arc<Notify<Async>>);

impl Wake for NotifyOnDrop {
    fn wake(self: Arc<Self>) {}
}

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[test]
fn cancelling_a_wait_can_notify_again_from_its_waker_destructor() {
    let notify = Arc::new(Notify::<Async>::new());
    let mut wait = Box::pin(notify.wait_async());
    let waker = Waker::from(Arc::new(NotifyOnDrop(Arc::clone(&notify))));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    drop(wait);
    assert!(notify.try_wait());
    assert!(!notify.try_wait());
}

#[test]
fn replacing_a_waker_can_notify_the_same_wait_during_destruction() {
    let notify = Arc::new(Notify::<Async>::new());
    let mut wait = Box::pin(notify.wait_async());
    let waker = Waker::from(Arc::new(NotifyOnDrop(Arc::clone(&notify))));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    let task = Task::new();
    assert!(task.poll(wait.as_mut()).is_ready());
    drop(wait);
    assert_eq!(task.references(), 2);
    assert!(!notify.try_wait());
}

#[test]
fn a_panicking_waker_clone_does_not_prevent_reregistration() {
    let notify = Notify::<Async>::new();
    let mut wait = Box::pin(notify.wait_async());
    let waker = waker_support::clone_hook_waker(|| panic!("test notify waker clone unwinding"));
    catch_unwind(AssertUnwindSafe(|| wait.as_mut().poll(&mut Context::from_waker(&waker)))).unwrap_err();
    let task = Task::new();
    assert!(task.poll(wait.as_mut()).is_pending());
    notify.notify_one();
    assert_eq!(task.wakes(), 1);
    assert!(task.poll(wait.as_mut()).is_ready());
    drop(wait);
    assert_eq!(task.references(), 2);
    assert!(!notify.try_wait());
}

#[test]
fn a_panicking_waker_replacement_does_not_erase_the_existing_notify_task() {
    let notify = Notify::<Async>::new();
    let task = Task::new();
    let mut wait = Box::pin(notify.wait_async());
    assert!(task.poll(wait.as_mut()).is_pending());
    let waker = waker_support::clone_hook_waker(|| panic!("test notify replacement clone"));
    catch_unwind(AssertUnwindSafe(|| wait.as_mut().poll(&mut Context::from_waker(&waker)))).unwrap_err();
    assert_eq!(task.references(), 3);
    notify.notify_one();
    assert_eq!(task.wakes(), 1);
    assert!(task.poll(wait.as_mut()).is_ready());
    drop(wait);
    assert_eq!(task.references(), 2);
}

#[test]
fn a_panicking_wake_keeps_the_selected_notification_assigned() {
    struct Panicking;
    impl Wake for Panicking {
        fn wake(self: Arc<Self>) {
            panic!("test notify wake unwinding");
        }
    }

    let notify = Notify::<Async>::new();
    let mut wait = Box::pin(notify.wait_async());
    let waker = Waker::from(Arc::new(Panicking));
    assert!(wait.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    catch_unwind(AssertUnwindSafe(|| notify.notify_one())).unwrap_err();
    assert!(!notify.try_wait());
    let task = Task::new();
    assert!(task.poll(wait.as_mut()).is_ready());
    drop(wait);
    notify.notify_one();
    assert!(notify.try_wait());
    assert!(!notify.try_wait());
}

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

#[test]
fn separately_registered_executor_threads_each_receive_one_notification() {
    let notify = Arc::new(Notify::<Async>::new());
    let workers = if cfg!(miri) { 2 } else { 8 };
    let (registered, registrations) = mpsc::channel();
    let handles = (0..workers)
        .map(|worker| {
            let notify = Arc::clone(&notify);
            let registered = registered.clone();
            thread::spawn(move || {
                let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
                let mut context = Context::from_waker(&waker);
                let mut wait = Box::pin(notify.wait_async());
                assert!(wait.as_mut().poll(&mut context).is_pending());
                registered.send(()).unwrap();
                let deadline = (!is_mutation_testing()).then(|| Instant::now() + DEADLINE);
                loop {
                    match wait.as_mut().poll(&mut context) {
                        Poll::Ready(()) => return worker,
                        Poll::Pending => match deadline {
                            Some(deadline) => thread::park_timeout(deadline.checked_duration_since(Instant::now()).unwrap()),
                            None => thread::park(),
                        },
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    drop(registered);
    for _ in 0..workers {
        if is_mutation_testing() {
            registrations.recv().unwrap();
        } else {
            registrations.recv_timeout(DEADLINE).unwrap();
        }
    }
    for _ in 0..workers {
        notify.notify_one();
    }
    let mut completed = Vec::with_capacity(workers);
    for handle in handles {
        completed.push(join(handle));
    }
    completed.sort_unstable();
    assert_eq!(completed, (0..workers).collect::<Vec<_>>());
    assert!(!notify.try_wait());
}
#[path = "support/edge_cases.rs"]
mod support;
#[path = "support/waker.rs"]
mod waker_support;
