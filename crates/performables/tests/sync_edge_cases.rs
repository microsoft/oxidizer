// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Payload ownership, task migration and cancellation around synchronization.

use std::future::pending;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll};
use std::time::Duration;

use performables::sync::barrier::{Barrier, BarrierWaitOutcome};
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::{Async, Mode, Sync};
use support::{DEADLINE, Task, join};
use testing_aids::is_mutation_testing;

#[derive(Debug)]
struct Payload(Arc<AtomicUsize>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn mutex_and_rwlock_support_unsized_payload_borrows_in_both_modes() {
    fn check<M: Mode>() {
        let mutex = Mutex::<[u64; 3], M>::new([2, 4, 6]);
        let slice: &Mutex<[u64], M> = &mutex;
        slice.lock()[1] = 8;
        assert_eq!(&*slice.lock(), &[2, 8, 6]);

        let lock = RwLock::<[u64; 3], M>::new([3, 6, 9]);
        let slice: &RwLock<[u64], M> = &lock;
        slice.write()[2] = 12;
        assert_eq!(&*slice.read(), &[3, 6, 12]);
    }
    check::<Sync>();
    check::<Async>();
}

#[test]
fn mutex_and_rwlock_preserve_overaligned_payload_storage() {
    #[repr(align(128))]
    struct Aligned(u64);
    fn check<M: Mode>() {
        let mutex = Mutex::<Aligned, M>::new(Aligned(7));
        let guard = mutex.lock();
        assert_eq!(std::ptr::from_ref(&*guard).addr() % 128, 0);
        assert_eq!(guard.0, 7);
        drop(guard);

        let lock = RwLock::<Aligned, M>::new(Aligned(11));
        let guard = lock.write();
        assert_eq!(std::ptr::from_ref(&*guard).addr() % 128, 0);
        assert_eq!(guard.0, 11);
    }
    check::<Sync>();
    check::<Async>();
}

#[test]
fn consuming_a_lock_transfers_non_copy_payload_ownership_without_early_drop() {
    fn check<M: Mode>() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mutex = Mutex::<Payload, M>::new(Payload(Arc::clone(&drops)));
        let value = mutex.into_inner();
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(value);
        assert_eq!(drops.load(Ordering::Relaxed), 1);

        let lock = RwLock::<Payload, M>::new(Payload(Arc::clone(&drops)));
        let value = lock.into_inner();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        drop(value);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }
    check::<Sync>();
    check::<Async>();
}

#[test]
fn cancelled_tasks_release_held_async_guards_without_poisoning() {
    let mutex = Mutex::<usize, Async>::new(0);
    let task = Task::new();
    let mut future = Box::pin(async {
        let mut guard = mutex.lock_async().await;
        *guard = 7;
        pending::<()>().await;
    });
    assert!(task.poll(future.as_mut()).is_pending());
    drop(future);
    assert_eq!(*mutex.try_lock().unwrap(), 7);
    assert!(!mutex.is_poisoned());

    let lock = RwLock::<usize, Async>::new(0);
    let mut future = Box::pin(async {
        let mut guard = lock.write_async().await;
        *guard = 11;
        pending::<()>().await;
    });
    assert!(task.poll(future.as_mut()).is_pending());
    drop(future);
    assert_eq!(*lock.try_read().unwrap(), 11);
    assert!(!lock.is_poisoned());
}

#[test]
fn async_guards_can_be_released_on_a_different_os_thread() {
    let mutex = Mutex::<usize, Async>::new(7);
    let guard = mutex.lock();
    let task = Task::new();
    let mut wait = Box::pin(mutex.lock_async());
    assert!(task.poll(wait.as_mut()).is_pending());
    std::thread::scope(|scope| scope.spawn(move || drop(guard)).join().unwrap());
    assert_eq!(task.wakes(), 1);
    let Poll::Ready(guard) = task.poll(wait.as_mut()) else {
        panic!("the remotely released mutex must be available");
    };
    assert_eq!(*guard, 7);
    drop(guard);
    drop(wait);
    assert_eq!(task.references(), 2);

    let lock = RwLock::<usize, Async>::new(11);
    let mut guard = lock.write();
    *guard = 13;
    std::thread::scope(|scope| scope.spawn(move || drop(guard)).join().unwrap());
    let guard = lock.read();
    std::thread::scope(|scope| scope.spawn(move || drop(guard)).join().unwrap());
    assert_eq!(*lock.try_write().unwrap(), 13);
}

#[test]
fn mutex_waiter_uses_only_its_latest_task_waker() {
    let mutex = Mutex::<usize, Async>::new(7);
    let held = mutex.lock();
    let first = Task::new();
    let second = Task::new();
    let mut wait = Box::pin(mutex.lock_async());
    assert!(first.poll(wait.as_mut()).is_pending());
    assert!(second.poll(wait.as_mut()).is_pending());
    assert_eq!((first.references(), second.references()), (2, 3));
    drop(held);
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    let Poll::Ready(guard) = second.poll(wait.as_mut()) else {
        panic!("releasing the owner must allow the contender to acquire");
    };
    assert_eq!(*guard, 7);
    drop(guard);
    drop(wait);
    assert_eq!(second.references(), 2);
}

#[test]
fn rwlock_read_and_write_waiters_replace_their_task_wakers() {
    let lock = RwLock::<usize, Async>::new(11);
    let held = lock.write();
    let first = Task::new();
    let second = Task::new();
    let mut read = Box::pin(lock.read_async());
    assert!(first.poll(read.as_mut()).is_pending());
    assert!(second.poll(read.as_mut()).is_pending());
    assert_eq!(first.references(), 2);
    drop(held);
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    let Poll::Ready(guard) = second.poll(read.as_mut()) else {
        panic!("the released writer must allow a reader");
    };
    drop(read);
    let first = Task::new();
    let second = Task::new();
    let mut write = Box::pin(lock.write_async());
    assert!(first.poll(write.as_mut()).is_pending());
    assert!(second.poll(write.as_mut()).is_pending());
    drop(guard);
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    let Poll::Ready(guard) = second.poll(write.as_mut()) else {
        panic!("the last released reader must allow a writer");
    };
    drop(guard);
    drop(write);
    assert_eq!(second.references(), 2);
}

#[test]
fn all_queued_async_readers_can_hold_the_lock_together_after_writer_release() {
    let lock = RwLock::<usize, Async>::new(17);
    let held = lock.write();
    let tasks = [Task::new(), Task::new(), Task::new()];
    let mut waits = tasks
        .iter()
        .map(|task| {
            let mut wait = Box::pin(lock.read_async());
            assert!(task.poll(wait.as_mut()).is_pending());
            wait
        })
        .collect::<Vec<_>>();
    drop(held);
    let mut guards = Vec::new();
    for (task, wait) in tasks.iter().zip(&mut waits) {
        assert_eq!(task.wakes(), 1);
        let Poll::Ready(guard) = task.poll(wait.as_mut()) else {
            panic!("each queued reader must acquire after writer release");
        };
        guards.push(guard);
    }
    assert_eq!(guards.iter().map(|guard| **guard).collect::<Vec<_>>(), vec![17; 3]);
    assert!(lock.try_write().is_none());
    drop(guards);
    assert!(lock.try_write().is_some());
}

#[test]
fn panicking_waker_clones_leave_mutex_and_rwlock_contenders_recoverable() {
    let mutex = Mutex::<usize, Async>::new(19);
    let held = mutex.lock();
    let mut wait = Box::pin(mutex.lock_async());
    let waker = waker_support::clone_hook_waker(|| panic!("test a lock waker clone"));
    catch_unwind(AssertUnwindSafe(|| wait.as_mut().poll(&mut Context::from_waker(&waker)))).unwrap_err();
    let task = Task::new();
    assert!(task.poll(wait.as_mut()).is_pending());
    drop(held);
    let Poll::Ready(guard) = task.poll(wait.as_mut()) else {
        panic!("a later valid waker must recover the contender");
    };
    drop(guard);
    drop(wait);
    assert!(!mutex.is_poisoned());

    let lock = RwLock::<usize, Async>::new(23);
    let held = lock.read();
    let mut wait = Box::pin(lock.write_async());
    let waker = waker_support::clone_hook_waker(|| panic!("test an exclusive waker clone"));
    catch_unwind(AssertUnwindSafe(|| wait.as_mut().poll(&mut Context::from_waker(&waker)))).unwrap_err();
    assert!(task.poll(wait.as_mut()).is_pending());
    drop(held);
    let Poll::Ready(guard) = task.poll(wait.as_mut()) else {
        panic!("a later valid waker must recover the exclusive contender");
    };
    drop(guard);
    drop(wait);
    assert!(!lock.is_poisoned());
}

#[test]
fn a_panicking_mutex_waker_replacement_keeps_the_old_task_registered() {
    let mutex = Mutex::<usize, Async>::new(27);
    let held = mutex.lock();
    let task = Task::new();
    let mut wait = Box::pin(mutex.lock_async());
    assert!(task.poll(wait.as_mut()).is_pending());
    let waker = waker_support::clone_hook_waker(|| panic!("test mutex replacement clone"));
    catch_unwind(AssertUnwindSafe(|| wait.as_mut().poll(&mut Context::from_waker(&waker)))).unwrap_err();
    assert_eq!(task.references(), 3);
    drop(held);
    assert_eq!(task.wakes(), 1);
    let Poll::Ready(guard) = task.poll(wait.as_mut()) else {
        panic!("the original task must remain able to acquire the lock");
    };
    assert_eq!(*guard, 27);
}

#[test]
fn an_already_satisfied_condition_predicate_keeps_the_mutex_guard() {
    let mutex = Mutex::<usize>::new(29);
    let condition = Condvar::<Sync>::new();
    let guard = condition.wait_while(mutex.lock(), |value| *value == 0);
    assert_eq!(*guard, 29);
    assert!(mutex.try_lock().is_none());
    drop(guard);

    let mutex = Mutex::<usize, Async>::new(31);
    let condition = Condvar::<Async>::new();
    let task = Task::new();
    let mut wait = Box::pin(condition.wait_while_async(mutex.lock(), |value| *value == 0));
    let Poll::Ready(guard) = task.poll(wait.as_mut()) else {
        panic!("a false wait predicate must not suspend");
    };
    assert_eq!(*guard, 31);
    assert!(mutex.try_lock().is_none());
}

#[test]
fn cancelling_a_notified_condvar_wait_during_mutex_reacquisition_does_not_strand_the_mutex() {
    let mutex = Mutex::<usize, Async>::new(37);
    let condition = Condvar::<Async>::new();
    let first = Task::new();
    let second = Task::new();
    let mut wait = Box::pin(condition.wait_async(mutex.lock()));
    assert!(first.poll(wait.as_mut()).is_pending());
    let held = mutex.lock();
    condition.notify_one();
    assert!(first.poll(wait.as_mut()).is_pending());
    drop(wait);
    assert_eq!(first.references(), 2);
    let mut lock = Box::pin(mutex.lock_async());
    assert!(second.poll(lock.as_mut()).is_pending());
    drop(held);
    assert_eq!(second.wakes(), 1);
    let Poll::Ready(guard) = second.poll(lock.as_mut()) else {
        panic!("cancelled reacquisition must not keep the mutex owned");
    };
    assert_eq!(*guard, 37);
}

#[test]
fn condvar_waiter_replaces_its_task_waker_before_reacquiring_the_mutex() {
    let mutex = Mutex::<usize, Async>::new(41);
    let condition = Condvar::<Async>::new();
    let first = Task::new();
    let second = Task::new();
    let mut wait = Box::pin(condition.wait_async(mutex.lock()));
    assert!(first.poll(wait.as_mut()).is_pending());
    assert!(second.poll(wait.as_mut()).is_pending());
    assert_eq!((first.references(), second.references()), (2, 3));
    condition.notify_one();
    assert_eq!((first.wakes(), second.wakes()), (0, 1));
    let Poll::Ready(guard) = second.poll(wait.as_mut()) else {
        panic!("a notified condition must reacquire its available mutex");
    };
    assert_eq!(*guard, 41);
    drop(wait);
    assert_eq!(second.references(), 2);
}

#[test]
fn native_condvar_large_duration_conversion_does_not_report_an_early_timeout() {
    let mutex = Arc::new(Mutex::<bool>::new(false));
    let condition = Arc::new(Condvar::<Sync>::new());
    let (waiting, started) = mpsc::channel();
    let worker = std::thread::spawn({
        let mutex = Arc::clone(&mutex);
        let condition = Arc::clone(&condition);
        move || {
            let mut guard = mutex.lock();
            waiting.send(()).unwrap();
            while !*guard {
                let timeout = Duration::from_secs(u64::MAX.div_ceil(1_000_000_000));
                let (next, outcome) = condition.wait_timeout(guard, timeout);
                assert!(!outcome.timed_out());
                guard = next;
            }
        }
    });
    if is_mutation_testing() {
        started.recv().unwrap();
    } else {
        started.recv_timeout(DEADLINE).unwrap();
    }
    *mutex.lock() = true;
    condition.notify_one();
    join(worker);
}

#[test]
fn unpolled_barrier_futures_do_not_count_as_arrivals() {
    let barrier = Barrier::<Async>::new(2);
    drop(barrier.wait_async());
    let task = Task::new();
    let mut first = Box::pin(barrier.wait_async());
    assert!(task.poll(first.as_mut()).is_pending());
    let mut second = Box::pin(barrier.wait_async());
    assert_eq!(task.poll(second.as_mut()).map(BarrierWaitOutcome::is_leader), Poll::Ready(true));
    assert_eq!(task.poll(first.as_mut()).map(BarrierWaitOutcome::is_leader), Poll::Ready(false));
}

#[test]
fn barrier_waiters_refresh_wakers_without_leaking_the_previous_generation() {
    let barrier = Barrier::<Async>::new(2);
    let first = Task::new();
    let second = Task::new();
    for _ in 0..3 {
        let mut wait = Box::pin(barrier.wait_async());
        assert!(first.poll(wait.as_mut()).is_pending());
        assert!(second.poll(wait.as_mut()).is_pending());
        assert_eq!(first.references(), 2);
        assert!(barrier.wait().is_leader());
        assert_eq!(second.poll(wait.as_mut()).map(BarrierWaitOutcome::is_leader), Poll::Ready(false));
        drop(wait);
        assert_eq!(second.references(), 2);
    }
    assert_eq!((first.wakes(), second.wakes()), (0, 3));
}

#[test]
fn repolling_many_barrier_waiters_does_not_duplicate_arrivals_between_generations() {
    let participants = if cfg!(miri) { 4 } else { 40 };
    let barrier = Barrier::<Async>::new(participants);
    let task = Task::new();
    for generation in 1..=3 {
        let mut waits = (0..participants - 1)
            .map(|_| {
                let mut wait = Box::pin(barrier.wait_async());
                assert!(task.poll(wait.as_mut()).is_pending());
                wait
            })
            .collect::<Vec<_>>();
        for wait in &mut waits {
            assert!(task.poll(wait.as_mut()).is_pending());
        }
        assert!(barrier.wait().is_leader());
        for wait in &mut waits {
            assert_eq!(task.poll(wait.as_mut()).map(BarrierWaitOutcome::is_leader), Poll::Ready(false));
        }
        drop(waits);
        assert_eq!(task.references(), 2);
        assert_eq!(task.wakes(), generation * (participants - 1));
    }
}
#[path = "support/edge_cases.rs"]
mod support;
#[path = "support/waker.rs"]
mod waker_support;

testing_aids::init_tracing!();
