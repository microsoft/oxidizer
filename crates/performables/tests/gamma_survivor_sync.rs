// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic cancellation, completion and notification ownership contracts.

use std::cmp::Ordering as CmpOrdering;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use performables::sync::barrier::Barrier;
use performables::sync::channel::{bounded, oneshot, watch};
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::notify::Notify;
use performables::sync::{Async, Sync};

#[derive(Default)]
struct CountWake(AtomicUsize);

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn gamma_survivor_blocking_async_condvar_predicate_wait_releases_and_reacquires_owned_mutex() {
    let mutex = Arc::new(Mutex::<u32, Async>::new(0));
    let condition = Arc::new(Condvar::<Async>::new());
    let (ready, started) = std::sync::mpsc::channel();
    let worker_mutex = Arc::clone(&mutex);
    let worker_condition = Arc::clone(&condition);
    let worker = std::thread::spawn(move || {
        let guard = worker_mutex.lock();
        ready.send(()).unwrap();
        let guard = worker_condition.wait_while(guard, |value| *value == 0);
        assert_eq!(*guard, 7);
        assert!(worker_mutex.try_lock().is_none());
    });
    started.recv().unwrap();
    let mut guard = mutex.lock();
    assert_eq!(*guard, 0);
    *guard = 7;
    condition.notify_one();
    drop(guard);
    worker.join().unwrap();
    assert_eq!(*mutex.try_lock().unwrap(), 7);
}

#[test]
fn gamma_survivor_cancelled_selected_mutex_waiter_does_not_wake_successor_while_barger_holds_lock() {
    let mutex = Mutex::<u32, Async>::new(7);
    let held = mutex.try_lock().unwrap();
    let first_count = Arc::new(CountWake::default());
    let second_count = Arc::new(CountWake::default());
    let first_waker = Waker::from(Arc::clone(&first_count));
    let second_waker = Waker::from(Arc::clone(&second_count));
    let mut first = Box::pin(mutex.lock_async());
    let mut second = Box::pin(mutex.lock_async());
    assert!(first.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    assert!(second.as_mut().poll(&mut Context::from_waker(&second_waker)).is_pending());
    drop(held);
    assert_eq!(first_count.0.load(Ordering::Relaxed), 1);
    let barger = mutex.try_lock().unwrap();
    drop(first);
    assert_eq!(second_count.0.load(Ordering::Relaxed), 0);
    drop(barger);
    assert_eq!(second_count.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(guard) = second.as_mut().poll(&mut Context::from_waker(&second_waker)) else {
        panic!("the successor must acquire after the barger releases");
    };
    assert_eq!(*guard, 7);
}

#[test]
fn gamma_survivor_cancelled_queue_receivers_reuse_slots_and_do_not_steal_wakes() {
    let (sender, receiver) = bounded::<u32>(2);
    let counts = std::array::from_fn::<_, 6, _>(|_| Arc::new(CountWake::default()));
    let wakers = counts.iter().map(|count| Waker::from(Arc::clone(count))).collect::<Vec<_>>();
    let mut waits = (0..6).map(|_| Some(Box::pin(receiver.recv_async()))).collect::<Vec<_>>();
    for (wait, waker) in waits.iter_mut().zip(&wakers) {
        assert!(wait.as_mut().unwrap().as_mut().poll(&mut Context::from_waker(waker)).is_pending());
    }
    for index in [1, 3, 0] {
        drop(waits[index].take());
        assert_eq!(Arc::strong_count(&counts[index]), 2);
    }
    let mut replacement = Box::pin(receiver.recv_async());
    assert!(replacement.as_mut().poll(&mut Context::from_waker(&wakers[0])).is_pending());
    sender.try_send(11).unwrap();
    assert_eq!(counts[2].0.load(Ordering::Relaxed), 1);
    drop(waits[2].take());
    assert_eq!(counts[4].0.load(Ordering::Relaxed), 1);
    assert_eq!(
        waits[4].as_mut().unwrap().as_mut().poll(&mut Context::from_waker(&wakers[4])),
        Poll::Ready(Ok(11))
    );
    drop(waits[4].take());
    sender.try_send(13).unwrap();
    assert_eq!(
        waits[5].as_mut().unwrap().as_mut().poll(&mut Context::from_waker(&wakers[5])),
        Poll::Ready(Ok(13))
    );
    drop(waits[5].take());
    sender.try_send(17).unwrap();
    assert_eq!(replacement.as_mut().poll(&mut Context::from_waker(&wakers[0])), Poll::Ready(Ok(17)));
    drop(replacement);
    drop(waits);
    for count in counts {
        assert_eq!(Arc::strong_count(&count), 2);
    }
}

#[test]
fn gamma_survivor_receiver_close_wakes_pending_sends_and_keeps_unsent_payloads() {
    let (sender, receiver) = bounded(1);
    let other = receiver.clone();
    sender.try_send(3).unwrap();
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut future = Box::pin(sender.send_async(7));
    assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    drop(receiver);
    assert!(!sender.is_closed());
    assert_eq!(count.0.load(Ordering::Relaxed), 0);
    drop(other);
    assert!(sender.is_closed());
    assert!(sender.is_empty());
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    let Poll::Ready(Err(error)) = future.as_mut().poll(&mut Context::from_waker(&waker)) else {
        panic!("last receiver cancellation must complete a blocked sender");
    };
    assert!(error.is_closed());
    assert_eq!(error.into_value(), Some(7));
    drop(future);
    assert_eq!(Arc::strong_count(&count), 2);
}

#[test]
fn gamma_survivor_cancelled_selected_sender_forwards_space_without_losing_following_payload() {
    let (sender, receiver) = bounded(1);
    sender.try_send(3).unwrap();
    let first_count = Arc::new(CountWake::default());
    let second_count = Arc::new(CountWake::default());
    let first_waker = Waker::from(Arc::clone(&first_count));
    let second_waker = Waker::from(Arc::clone(&second_count));
    let mut first = Box::pin(sender.send_async(5));
    let mut second = Box::pin(sender.send_async(7));
    assert!(first.as_mut().poll(&mut Context::from_waker(&first_waker)).is_pending());
    assert!(second.as_mut().poll(&mut Context::from_waker(&second_waker)).is_pending());
    assert_eq!(receiver.try_recv(), Ok(3));
    assert_eq!(first_count.0.load(Ordering::Relaxed), 1);
    drop(first);
    assert_eq!(second_count.0.load(Ordering::Relaxed), 1);
    assert_eq!(second.as_mut().poll(&mut Context::from_waker(&second_waker)), Poll::Ready(Ok(())));
    drop(second);
    assert_eq!(receiver.try_recv(), Ok(7));
    assert!(receiver.try_recv().unwrap_err().is_empty());
    assert_eq!(Arc::strong_count(&first_count), 2);
    assert_eq!(Arc::strong_count(&second_count), 2);
}

#[test]
fn gamma_survivor_condvar_cancelled_selection_forwards_and_reacquisition_waits_for_mutex() {
    let mutex = Mutex::<u32, Async>::new(7);
    let condition = Condvar::<Async>::new();
    let counts = [Arc::new(CountWake::default()), Arc::new(CountWake::default())];
    let wakers = counts.iter().map(|count| Waker::from(Arc::clone(count))).collect::<Vec<_>>();
    let mut first = Box::pin(condition.wait_async(mutex.lock()));
    assert!(first.as_mut().poll(&mut Context::from_waker(&wakers[0])).is_pending());
    let mut second = Box::pin(condition.wait_async(mutex.lock()));
    assert!(second.as_mut().poll(&mut Context::from_waker(&wakers[1])).is_pending());
    let mut held = mutex.lock();
    *held = 11;
    condition.notify_one();
    assert_eq!(counts[0].0.load(Ordering::Relaxed), 1);
    drop(first);
    assert_eq!(counts[1].0.load(Ordering::Relaxed), 1);
    assert!(second.as_mut().poll(&mut Context::from_waker(&wakers[1])).is_pending());
    drop(held);
    let Poll::Ready(guard) = second.as_mut().poll(&mut Context::from_waker(&wakers[1])) else {
        panic!("the selected condition waiter must reacquire the released mutex");
    };
    assert_eq!(*guard, 11);
    assert!(mutex.try_lock().is_none());
    drop((guard, second));
    assert_eq!(*mutex.try_lock().unwrap(), 11);
    for count in counts {
        assert_eq!(Arc::strong_count(&count), 2);
    }
}

#[test]
fn gamma_survivor_condvar_predicate_wait_rechecks_value_after_reacquiring_guard() {
    let mutex = Mutex::<u32, Async>::new(0);
    let condition = Condvar::<Async>::new();
    let mut wait = Box::pin(condition.wait_while_async(mutex.lock(), |value| *value == 0));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    *mutex.try_lock().unwrap() = 11;
    condition.notify_all();
    let Poll::Ready(guard) = wait.as_mut().poll(&mut cx) else {
        panic!("a notified predicate waiter must reacquire and observe the new value");
    };
    assert_eq!(*guard, 11);
    assert!(mutex.try_lock().is_none());
    drop((guard, wait));
    assert_eq!(*mutex.try_lock().unwrap(), 11);
}

#[test]
fn gamma_survivor_watch_predicate_observes_rejected_versions_and_returns_closed_error() {
    let (sender, receiver) = watch(0_u32);
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = Box::pin(receiver.wait_for_async(|value| *value == 7));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        sender.send(3).unwrap();
        assert!(future.as_mut().poll(&mut cx).is_pending());
        sender.send(7).unwrap();
        let Poll::Ready(Ok(value)) = future.as_mut().poll(&mut cx) else {
            panic!("the accepted version must complete the predicate wait");
        };
        assert_eq!(*value, 7);
        drop(value);
    }
    {
        let mut changed = Box::pin(receiver.changed_async());
        assert!(changed.as_mut().poll(&mut cx).is_pending());
    }
    let mut rejected = Box::pin(receiver.wait_for_async(|_| false));
    assert!(rejected.as_mut().poll(&mut cx).is_pending());
    drop(sender);
    let Poll::Ready(Err(error)) = rejected.as_mut().poll(&mut cx) else {
        panic!("closed watch with no accepted version must return an error");
    };
    assert!(error.is_closed());
    drop(rejected);
    assert_eq!(Arc::strong_count(&count), 2);
}

#[test]
fn gamma_survivor_watch_send_modify_wakes_pending_receiver_and_retains_modified_value() {
    let (sender, receiver) = watch(7);
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(receiver.changed_async());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    sender.send_modify(|value| *value = 11);
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    drop(future);
    assert_eq!(*receiver.borrow_and_update(), 11);
    let mut next = Box::pin(receiver.changed_async());
    assert!(next.as_mut().poll(&mut cx).is_pending());
    drop(next);
    assert_eq!(Arc::strong_count(&count), 2);
}

#[test]
fn gamma_survivor_oneshot_completion_removes_registration_before_receiver_drop() {
    for sent in [false, true] {
        let (sender, mut receiver) = oneshot();
        let count = Arc::new(CountWake::default());
        let waker = Waker::from(Arc::clone(&count));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut receiver).poll(&mut cx).is_pending());
        assert_eq!(Arc::strong_count(&count), 3);
        if sent {
            sender.send(7).unwrap();
            assert_eq!(receiver.try_recv(), Ok(7));
        } else {
            drop(sender);
            assert!(receiver.try_recv().unwrap_err().is_closed());
        }
        assert_eq!(Arc::strong_count(&count), 2);
        drop(receiver);
        assert_eq!(count.0.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn gamma_survivor_cancelled_barrier_arrival_does_not_count_in_next_generation() {
    let barrier = Barrier::<Async>::new(3);
    let count = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut cx = Context::from_waker(&waker);
    for _ in 0..4 {
        let mut cancelled = Box::pin(barrier.wait_async());
        assert!(cancelled.as_mut().poll(&mut cx).is_pending());
        drop(cancelled);
        assert_eq!(Arc::strong_count(&count), 2);
        let mut first = Box::pin(barrier.wait_async());
        let mut second = Box::pin(barrier.wait_async());
        let mut last = Box::pin(barrier.wait_async());
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert!(second.as_mut().poll(&mut cx).is_pending());
        let Poll::Ready(leader) = last.as_mut().poll(&mut cx) else {
            panic!("the third uncancelled arrival must release the generation");
        };
        assert!(leader.is_leader());
        for waiter in [&mut first, &mut second] {
            let Poll::Ready(outcome) = waiter.as_mut().poll(&mut cx) else {
                panic!("both followers must observe the released generation");
            };
            assert!(!outcome.is_leader());
        }
        drop((first, second, last));
        assert_eq!(Arc::strong_count(&count), 2);
    }
}

#[test]
fn gamma_survivor_notify_completed_wait_does_not_reserve_or_return_another_permit() {
    let notify = Notify::<Async>::new();
    let mut wait = Box::pin(notify.wait_async());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    notify.notify_one();
    assert!(wait.as_mut().poll(&mut cx).is_ready());
    assert!(wait.as_mut().enable());
    notify.notify_one();
    drop(wait);
    assert!(notify.try_wait());
    assert!(!notify.try_wait());
    let unpolled = notify.wait_async();
    drop(unpolled);
    assert!(!notify.try_wait());
}

#[test]
fn gamma_survivor_ready_lock_futures_and_debug_names_preserve_guard_state() {
    let mut cx = Context::from_waker(Waker::noop());
    let mutex = Mutex::<u32, Async>::new(7);
    let mut lock = Box::pin(mutex.lock_async());
    assert_eq!(format!("{lock:?}"), "MutexLock { result: MutexLockFuture { .. } }");
    let Poll::Ready(mut guard) = lock.as_mut().poll(&mut cx) else {
        panic!("an uncontended mutex must acquire on the first poll");
    };
    *guard = 11;
    assert!(mutex.try_lock().is_none());
    drop((guard, lock));
    assert_eq!(*mutex.try_lock().unwrap(), 11);
    let rwlock = RwLock::<u32, Async>::new(13);
    let mut read = Box::pin(rwlock.read_async());
    assert_eq!(format!("{read:?}"), "RwLockRead { result: RwLockReadFuture { .. } }");
    let Poll::Ready(guard) = read.as_mut().poll(&mut cx) else {
        panic!("an uncontended read must acquire on the first poll");
    };
    assert_eq!(*guard, 13);
    assert!(rwlock.try_write().is_none());
    drop((guard, read));
    let mut write = Box::pin(rwlock.write_async());
    assert_eq!(format!("{write:?}"), "RwLockWrite { result: RwLockWriteFuture { .. } }");
    let Poll::Ready(mut guard) = write.as_mut().poll(&mut cx) else {
        panic!("the released read guard must allow a writer");
    };
    *guard = 17;
    drop((guard, write));
    assert_eq!(*rwlock.try_read().unwrap(), 17);
}

#[test]
fn gamma_survivor_native_locks_release_each_guard_and_preserve_reader_exclusion() {
    let mutex = Mutex::<u32, Sync>::new(7);
    let mut guard = mutex.try_lock().unwrap();
    *guard = 11;
    assert!(mutex.try_lock().is_none());
    drop(guard);
    assert_eq!(*mutex.try_lock().unwrap(), 11);
    let rwlock = RwLock::<u32, Sync>::new(13);
    let first = rwlock.try_read().unwrap();
    let second = rwlock.try_read().unwrap();
    assert_eq!((*first, *second), (13, 13));
    assert!(rwlock.try_write().is_none());
    drop(first);
    assert!(rwlock.try_write().is_none());
    drop(second);
    let mut writer = rwlock.try_write().unwrap();
    assert!(rwlock.try_read().is_none());
    assert!(rwlock.try_write().is_none());
    *writer = 17;
    drop(writer);
    assert_eq!(*rwlock.try_read().unwrap(), 17);
    let reader = rwlock.read();
    assert!(rwlock.try_write().is_none());
    drop(reader);
    assert_eq!(*rwlock.try_write().unwrap(), 17);
    assert!(Barrier::<Sync>::new(1).wait().is_leader());
}

#[test]
fn gamma_survivor_writer_acquired_during_unwind_does_not_poison_its_lock() {
    struct AcquireOnDrop<'a>(&'a RwLock<u32, Async>);
    impl Drop for AcquireOnDrop<'_> {
        fn drop(&mut self) {
            let mut guard = self.0.write();
            *guard = 11;
        }
    }
    let lock = RwLock::<u32, Async>::new(7);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _during_unwind = AcquireOnDrop(&lock);
            panic!("controlled failure before guard acquisition");
        }))
        .is_err()
    );
    assert!(!lock.is_poisoned());
    assert_eq!(*lock.try_read().unwrap(), 11);
}

#[test]
fn gamma_survivor_arc_ordering_delegates_all_three_outcomes() {
    use performables::arc::Arc as PerformableArc;

    let small = PerformableArc::new(3);
    let large = PerformableArc::new(7);
    assert_eq!(small.cmp(&large), CmpOrdering::Less);
    assert_eq!(large.cmp(&small), CmpOrdering::Greater);
    assert_eq!(small.cmp(&PerformableArc::new(3)), CmpOrdering::Equal);
}

#[test]
fn gamma_survivor_raw_arc_count_operations_preserve_one_owned_handle_and_drop_once() {
    use performables::arc::Arc as PerformableArc;

    struct Dropped(Arc<AtomicUsize>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let pointer = PerformableArc::into_raw(PerformableArc::new(Dropped(Arc::clone(&drops))));
    // SAFETY: into_raw retained one live strong reference at this pointer.
    unsafe { PerformableArc::increment_strong_count(pointer) };
    // SAFETY: recover the original reference; the incremented reference remains raw.
    let owner = unsafe { PerformableArc::from_raw(pointer) };
    assert_eq!(PerformableArc::strong_count(&owner), 2);
    // SAFETY: release only the incremented raw reference while owner keeps the value live.
    unsafe { PerformableArc::decrement_strong_count(pointer) };
    assert_eq!(PerformableArc::strong_count(&owner), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(owner);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn gamma_survivor_affinity_materialization_forwards_source_and_reuses_origin_value() {
    use performables::arc::{Arc as PerformableArc, PerThread};
    use thread_aware::{Relocator, Thread, ThreadAware};

    #[derive(Clone)]
    struct Input(Option<Thread>);
    impl ThreadAware for Input {
        fn relocate(&mut self, source: Option<&Thread>, _: &Thread) {
            self.0 = source.cloned();
        }
    }
    let (source, destination) = Relocator::between_threads().relocate(&mut ());
    let source = source.unwrap();
    let mut value = PerformableArc::<Option<Thread>, PerThread>::new_with_data(Input(None), |input| input.0);
    let original = PerformableArc::as_ptr(&value);
    ThreadAware::relocate(&mut value, Some(&source), &destination);
    assert_eq!(*value, Some(source.clone()));
    let materialized = PerformableArc::as_ptr(&value);
    ThreadAware::relocate(&mut value, Some(&destination), &destination);
    assert_eq!(PerformableArc::as_ptr(&value), materialized);
    ThreadAware::relocate(&mut value, Some(&destination), &source);
    assert_eq!(PerformableArc::as_ptr(&value), original);
    assert_eq!(*value, None);
    let another_destination = std::thread::spawn(|| Relocator::between_threads().relocate(&mut ()).1)
        .join()
        .unwrap();
    assert_ne!(destination.id(), another_destination.id());
    ThreadAware::relocate(&mut value, Some(&source), &destination);
    ThreadAware::relocate(&mut value, Some(&destination), &another_destination);
    assert_eq!(*value, Some(source.clone()));
    let mut cloned = PerformableArc::<Input, PerThread>::with_clone_fn(Input(None), |input| Box::new(input.clone()));
    ThreadAware::relocate(&mut cloned, Some(&source), &destination);
    assert_eq!(cloned.0, Some(source));
}
