// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::panic, reason = "poisoning semantics require controlled panics caught by these tests")]

//! Integration tests for executor-independent synchronization.

#[path = "support/serializer.rs"]
mod serializer_support;
#[path = "support/waker.rs"]
mod waker_support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use performables::arc::Arc;
#[cfg(feature = "seismograph")]
use performables::arc::PerCore;
use performables::sync::barrier::Barrier;
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::once::{LazyLock, OnceLock};
#[cfg(feature = "seismograph")]
use seismograph::recorder::Configuration;
#[cfg(feature = "seismograph")]
use seismograph::recorder::event::{self as recorder, EventKind};
use serde::de::value::{Error as ValueError, U64Deserializer};
use serde::{Deserialize, Serialize};
use serializer_support::ValueSerializer;
#[cfg(feature = "seismograph")]
use thread_aware::Relocator;
use waker_support::clone_hook_waker;

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: StdArc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: StdArc<Self>) {
        self.0.unpark();
    }
}

fn waker(counter: &StdArc<WakeCounter>) -> Waker {
    Waker::from(StdArc::clone(counter))
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(StdArc::new(ThreadWaker(std::thread::current())));
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
fn mutex_uncontended_future_is_immediately_ready() {
    let mutex = Mutex::new(3);
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(mutex.lock());

    let Poll::Ready(mut guard) = future.as_mut().poll(&mut context) else {
        panic!("an uncontended mutex must be immediately ready");
    };
    *guard += 4;
    drop(guard);

    assert_eq!(*mutex.try_lock().unwrap(), 7);
}

#[test]
fn mutex_release_wakes_a_contender() {
    let mutex = Mutex::new(());
    let held = mutex.try_lock().unwrap();
    assert!(mutex.try_lock().is_none());
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(mutex.lock());

    assert!(future.as_mut().poll(&mut context).is_pending());
    drop(held);

    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(future.as_mut().poll(&mut context).is_ready());
}

#[test]
fn mutex_sync_lock_waits_for_release() {
    let mutex = Arc::new(Mutex::new(0));
    let held = mutex.try_lock().unwrap();
    let contender = Arc::clone(&mutex);
    let thread = std::thread::spawn(move || {
        *contender.lock_sync() = 7;
    });

    std::thread::yield_now();
    drop(held);
    thread.join().unwrap();

    assert_eq!(*mutex.lock_sync(), 7);
}

#[test]
fn mutex_supports_owned_access_defaults_and_formatting() {
    let mut mutex = Mutex::const_new(String::from("value"));
    mutex.get_mut().push('!');
    assert_eq!(format!("{mutex:?}"), "Mutex { value: \"value!\", poisoned: false }");

    let guard = mutex.lock_sync();
    assert_eq!(
        (format!("{guard:?}"), format!("{guard}"), format!("{mutex:?}")),
        (
            "\"value!\"".to_owned(),
            "value!".to_owned(),
            "Mutex { value: \"<locked>\", poisoned: false }".to_owned()
        )
    );
    drop(guard);

    assert_eq!(mutex.into_inner(), "value!");
    assert_eq!(Mutex::<usize>::default().into_inner(), 0);
}

#[test]
fn arc_and_mutex_serde_delegate_to_their_values() {
    let arc = Arc::new(42_u64);
    let mutex = Mutex::new(String::from("value"));
    let decoded = Mutex::<u64>::deserialize(U64Deserializer::<ValueError>::new(17)).unwrap();

    assert_eq!(
        (
            arc.serialize(ValueSerializer).unwrap(),
            mutex.serialize(ValueSerializer).unwrap(),
            decoded.into_inner(),
        ),
        ("42".to_owned(), "value".to_owned(), 17),
    );
}

#[test]
fn mutex_debug_reports_poisoned_values() {
    let mutex = Mutex::new(7);
    poison_mutex(&mutex);

    assert_eq!(format!("{mutex:?}"), "Mutex { value: 8, poisoned: true }");
}

#[test]
fn mutex_sync_result_waits_for_release() {
    let mutex = Arc::new(Mutex::new(0));
    let held = mutex.try_lock().unwrap();
    let contender = Arc::clone(&mutex);
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        started_sender.send(()).unwrap();
        *contender.lock_sync_result().unwrap() = 9;
    });
    started_receiver.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    assert!(!thread.is_finished());

    drop(held);
    thread.join().unwrap();

    assert_eq!(*mutex.lock_sync(), 9);
}

#[test]
fn cancelled_mutex_waiter_does_not_consume_the_lock() {
    let mutex = Mutex::new(());
    let held = mutex.try_lock().unwrap();
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);

    {
        let mut future = pin!(mutex.lock());
        assert!(future.as_mut().poll(&mut context).is_pending());
    }
    drop(held);

    assert!(mutex.try_lock().is_some());
}

fn poison_mutex(mutex: &Mutex<usize>) {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut guard = mutex.lock_sync();
        *guard += 1;
        panic!("poison mutex");
    }));
    assert!(panic.is_err());
    assert!(mutex.is_poisoned());
}

#[test]
fn mutex_default_acquisitions_panic_on_poison() {
    let mutex = Mutex::new(0);
    poison_mutex(&mutex);

    catch_unwind(AssertUnwindSafe(|| mutex.lock_sync())).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| block_on(mutex.lock()))).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| mutex.try_lock())).unwrap_err();
}

#[test]
fn mutex_result_acquisitions_retain_usable_guards() {
    let mutex = Mutex::new(0);
    poison_mutex(&mutex);

    let mut sync_error = mutex.lock_sync_result().unwrap_err();
    **sync_error.get_mut() += 2;
    drop(sync_error.into_inner());

    let mut async_guard = block_on(mutex.lock_result()).unwrap_err().into_inner();
    *async_guard += 4;
    drop(async_guard);

    let mut try_guard = mutex.try_lock_result().unwrap_err().into_inner();
    *try_guard += 8;
    drop(try_guard);

    assert_eq!(**mutex.lock_sync_result().unwrap_err().get_ref(), 15);
    assert!(mutex.is_poisoned());
}

#[test]
fn clearing_mutex_poison_restores_default_acquisition() {
    let mutex = Mutex::new(0);
    poison_mutex(&mutex);
    drop(mutex.lock_sync_result().unwrap_err().into_inner());

    mutex.clear_poison();

    assert!(!mutex.is_poisoned());
    assert_eq!(*mutex.lock_sync(), 1);
}

#[test]
fn guard_acquired_during_an_existing_unwind_does_not_poison_mutex() {
    struct AcquireOnDrop<'a>(&'a Mutex<()>);

    impl Drop for AcquireOnDrop<'_> {
        fn drop(&mut self) {
            drop(self.0.lock_sync_result().unwrap());
        }
    }

    let mutex = Mutex::new(());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _acquire_on_drop = AcquireOnDrop(&mutex);
        panic!("start unwind");
    }));

    assert!(panic.is_err());
    assert!(!mutex.is_poisoned());
}

#[test]
fn cancelling_a_selected_mutex_waiter_wakes_the_next_waiter() {
    let mutex = Mutex::new(());
    let held = mutex.try_lock().unwrap();
    let first_counter = StdArc::new(WakeCounter::default());
    let first_waker = waker(&first_counter);
    let mut first_context = Context::from_waker(&first_waker);
    let second_counter = StdArc::new(WakeCounter::default());
    let second_waker = waker(&second_counter);
    let mut second_context = Context::from_waker(&second_waker);
    let mut first = Box::pin(mutex.lock());
    let mut second = Box::pin(mutex.lock());
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    assert!(second.as_mut().poll(&mut second_context).is_pending());

    drop(held);
    assert_eq!(first_counter.0.load(Ordering::Relaxed), 1);
    drop(first);
    assert_eq!(second_counter.0.load(Ordering::Relaxed), 1);
    assert!(second.as_mut().poll(&mut second_context).is_ready());
}

#[test]
fn rw_lock_allows_readers_and_excludes_writers() {
    let lock = RwLock::new(5);
    let first = lock.try_read().unwrap();
    let second = lock.try_read().unwrap();
    assert!(lock.try_write().is_none());
    drop((first, second));

    let mut writer = lock.try_write().unwrap();
    *writer = 8;
    assert!(lock.try_read().is_none());
    drop(writer);

    assert_eq!(*lock.try_read().unwrap(), 8);
}

#[test]
fn rw_lock_release_wakes_waiting_writer() {
    let lock = RwLock::new(());
    let held = lock.try_read().unwrap();
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(lock.write());

    assert!(future.as_mut().poll(&mut context).is_pending());
    drop(held);

    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    assert!(future.as_mut().poll(&mut context).is_ready());
}

#[test]
fn rw_lock_sync_access_waits_for_conflicting_guards() {
    let lock = Arc::new(RwLock::new(0));
    let reader = lock.read_sync();
    let contender = Arc::clone(&lock);
    let thread = std::thread::spawn(move || {
        *contender.write_sync() = 11;
    });

    std::thread::yield_now();
    drop(reader);
    thread.join().unwrap();

    assert_eq!(*lock.read_sync(), 11);
}

#[test]
fn rw_lock_supports_owned_access_defaults_and_formatting() {
    let mut lock = RwLock::new(String::from("value"));
    lock.get_mut().push('!');
    assert_eq!(format!("{lock:?}"), "RwLock { value: \"value!\", poisoned: false }");

    let read = lock.read_sync();
    assert_eq!(
        (format!("{read:?}"), format!("{read}")),
        ("\"value!\"".to_owned(), "value!".to_owned())
    );
    drop(read);

    let write = lock.write_sync();
    assert_eq!(
        (format!("{write:?}"), format!("{write}"), format!("{lock:?}")),
        (
            "\"value!\"".to_owned(),
            "value!".to_owned(),
            "RwLock { value: \"<write-locked>\", poisoned: false }".to_owned(),
        ),
    );
    drop(write);

    assert_eq!(lock.into_inner(), "value!");
    assert_eq!(RwLock::<usize>::default().into_inner(), 0);
}

#[test]
fn rw_lock_debug_reports_poisoned_values() {
    let lock = RwLock::new(7);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut guard = lock.write_sync();
        *guard += 1;
        panic!("poison writer");
    }));
    assert!(panic.is_err());

    assert_eq!(format!("{lock:?}"), "RwLock { value: 8, poisoned: true }");
}

#[test]
fn rw_lock_sync_results_wait_for_conflicting_guards() {
    let lock = Arc::new(RwLock::new(0));
    let writer = lock.write_sync();
    let read_lock = Arc::clone(&lock);
    let (read_started_sender, read_started_receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        read_started_sender.send(()).unwrap();
        *read_lock.read_sync_result().unwrap()
    });
    read_started_receiver.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    assert!(!reader.is_finished());
    drop(writer);
    assert_eq!(reader.join().unwrap(), 0);

    let reader = lock.read_sync();
    let write_lock = Arc::clone(&lock);
    let (write_started_sender, write_started_receiver) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        write_started_sender.send(()).unwrap();
        *write_lock.write_sync_result().unwrap() = 12;
    });
    write_started_receiver.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    assert!(!writer.is_finished());
    drop(reader);
    writer.join().unwrap();

    assert_eq!(*lock.read_sync(), 12);
}

#[test]
fn cancelled_rw_lock_waiters_leave_the_lock_available() {
    let lock = RwLock::new(());
    let writer = lock.write_sync();
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    {
        let mut read = pin!(lock.read_result());
        assert!(read.as_mut().poll(&mut context).is_pending());
    }
    drop(writer);
    assert!(lock.try_read().is_some());

    let reader = lock.read_sync();
    {
        let mut write = pin!(lock.write_result());
        assert!(write.as_mut().poll(&mut context).is_pending());
    }
    drop(reader);
    assert!(lock.try_write().is_some());
}

#[test]
fn rw_lock_writer_panic_poisons_but_reader_panic_does_not() {
    let reader_lock = RwLock::new(0);
    let reader_panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = reader_lock.read_sync();
        panic!("reader panic");
    }));
    assert!(reader_panic.is_err());
    assert!(!reader_lock.is_poisoned());

    let writer_lock = RwLock::new(0);
    let writer_panic = catch_unwind(AssertUnwindSafe(|| {
        let mut guard = writer_lock.write_sync();
        *guard = 7;
        panic!("writer panic");
    }));
    assert!(writer_panic.is_err());
    assert!(writer_lock.is_poisoned());
    assert_eq!(**writer_lock.read_sync_result().unwrap_err().get_ref(), 7);
}

#[test]
fn rw_lock_result_acquisitions_recover_without_clearing_poison() {
    let lock = RwLock::new(0);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let mut guard = lock.write_sync();
        *guard = 1;
        panic!("poison writer");
    }));
    assert!(panic.is_err());

    let read = block_on(lock.read_result()).unwrap_err().into_inner();
    assert_eq!(*read, 1);
    drop(read);

    let mut sync_write = lock.write_sync_result().unwrap_err().into_inner();
    *sync_write = 2;
    drop(sync_write);

    let mut write = block_on(lock.write_result()).unwrap_err().into_inner();
    *write = 3;
    drop(write);

    drop(lock.try_read_result().unwrap_err().into_inner());
    drop(lock.try_write_result().unwrap_err().into_inner());
    assert!(lock.is_poisoned());

    lock.clear_poison();
    assert_eq!(*lock.read_sync(), 3);
}

#[test]
fn rw_lock_default_acquisitions_panic_on_poison() {
    let lock = RwLock::new(());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = lock.write_sync();
        panic!("poison writer");
    }));
    assert!(panic.is_err());

    catch_unwind(AssertUnwindSafe(|| lock.read_sync())).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| lock.write_sync())).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| block_on(lock.read()))).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| block_on(lock.write()))).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| lock.try_read())).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| lock.try_write())).unwrap_err();
}

#[test]
fn mutex_coordinates_executor_threads_under_contention() {
    let lock = Arc::new(Mutex::new(0_usize));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let lock = Arc::clone(&lock);
            scope.spawn(move || {
                block_on(async move {
                    for _ in 0..1_000 {
                        *lock.lock().await += 1;
                    }
                });
            });
        }
    });

    assert_eq!(*lock.try_lock().unwrap(), 4_000);
}

#[test]
fn rw_lock_coordinates_executor_threads_under_contention() {
    let lock = Arc::new(RwLock::new(0_usize));
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let lock = Arc::clone(&lock);
            scope.spawn(move || {
                block_on(async move {
                    for _ in 0..1_000 {
                        *lock.write().await += 1;
                    }
                });
            });
        }
        for _ in 0..2 {
            let lock = Arc::clone(&lock);
            scope.spawn(move || {
                block_on(async move {
                    for _ in 0..1_000 {
                        std::hint::black_box(*lock.read().await);
                    }
                });
            });
        }
    });

    assert_eq!(*lock.try_read().unwrap(), 2_000);
}

#[test]
fn barrier_supports_async_and_blocking_waiters() {
    let barrier = Arc::new(Barrier::new(3));
    let async_barrier = Arc::clone(&barrier);
    let async_thread = std::thread::spawn(move || block_on(async_barrier.wait()));
    let blocking_barrier = Arc::clone(&barrier);
    let blocking_thread = std::thread::spawn(move || blocking_barrier.wait_sync());

    let result = block_on(barrier.wait());
    let results = [result, async_thread.join().unwrap(), blocking_thread.join().unwrap()];

    assert_eq!(results.iter().filter(|result| result.is_leader()).count(), 1);
}

#[test]
fn dropping_a_released_barrier_waiter_observes_the_new_generation() {
    let barrier = Barrier::new(2);
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut waiter = Box::pin(barrier.wait());
    assert!(waiter.as_mut().poll(&mut context).is_pending());

    assert!(block_on(barrier.wait()).is_leader());
    drop(waiter);

    let mut next = pin!(barrier.wait());
    assert!(next.as_mut().poll(&mut context).is_pending());
}

#[test]
fn barrier_handles_heavy_concurrent_arrival_and_cancellation() {
    let barrier = StdArc::new(Barrier::new(16));
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let barrier = StdArc::clone(&barrier);
            scope.spawn(move || {
                for _ in 0..100 {
                    block_on(barrier.wait());
                }
            });
        }
    });

    let barrier = Barrier::new(64);
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let waiters = (0..32)
        .map(|_| {
            let mut waiter = Box::pin(barrier.wait());
            assert!(waiter.as_mut().poll(&mut context).is_pending());
            waiter
        })
        .collect::<Vec<_>>();
    std::thread::scope(|scope| {
        for waiter in waiters {
            scope.spawn(move || drop(waiter));
        }
    });
}

#[test]
fn cancelled_barrier_waiter_withdraws_its_arrival() {
    let barrier = Barrier::new(2);
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);

    {
        let mut cancelled = pin!(barrier.wait());
        assert!(cancelled.as_mut().poll(&mut context).is_pending());
    }

    let mut replacement = pin!(barrier.wait());
    assert!(replacement.as_mut().poll(&mut context).is_pending());
    assert!(block_on(barrier.wait()).is_leader());
    assert!(replacement.as_mut().poll(&mut context).is_ready());
}

#[test]
fn condvar_supports_async_and_blocking_waiters() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let waiter_pair = Arc::clone(&pair);
    let waiter = std::thread::spawn(move || {
        block_on(async {
            let (mutex, condition) = &*waiter_pair;
            let guard = mutex.lock().await;
            drop(condition.wait_while(guard, |ready| !*ready).await);
        });
    });

    let (mutex, condition) = &*pair;
    *mutex.lock_sync() = true;
    condition.notify_one();
    waiter.join().unwrap();
}

#[test]
fn condvar_wait_while_async_rechecks_the_predicate() {
    let mutex = Mutex::new(false);
    let condition = Condvar::new();
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(condition.wait_while(mutex.lock_sync(), |ready| !*ready));
    assert!(wait.as_mut().poll(&mut context).is_pending());

    *mutex.lock_sync() = true;
    condition.notify_one();

    assert!(wait.as_mut().poll(&mut context).is_ready());
}

#[test]
fn condvar_observes_notifications_during_waiter_registration() {
    let mutex = StdArc::new(Mutex::new(()));
    let condition = StdArc::new(Condvar::new());
    let notify_condition = StdArc::clone(&condition);
    let waker = clone_hook_waker(move || notify_condition.notify_one());
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(condition.wait(mutex.lock_sync()));

    assert!(wait.as_mut().poll(&mut context).is_ready());
}

#[test]
fn condvar_direct_waits_support_blocking_and_async_notification() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let waiter_pair = Arc::clone(&pair);
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let (mutex, condition) = &*waiter_pair;
        let guard = mutex.lock_sync();
        started_sender.send(()).unwrap();
        drop(condition.wait_sync(guard));
    });
    started_receiver.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    pair.1.notify_one();
    waiter.join().unwrap();

    let mutex = Mutex::new(());
    let condition = Condvar::new();
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(condition.wait(mutex.lock_sync()));
    assert!(wait.as_mut().poll(&mut context).is_pending());
    condition.notify_one();
    assert!(wait.as_mut().poll(&mut context).is_ready());
}

#[test]
fn condvar_wait_while_sync_rechecks_the_predicate() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let waiter_pair = Arc::clone(&pair);
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let (mutex, condition) = &*waiter_pair;
        let guard = mutex.lock_sync();
        started_sender.send(()).unwrap();
        drop(condition.wait_while_sync(guard, |ready| !*ready));
    });
    started_receiver.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    *pair.0.lock_sync() = true;
    pair.1.notify_one();

    waiter.join().unwrap();
}

#[test]
fn condvar_notify_all_wakes_every_waiter() {
    let mutex = Mutex::new(());
    let condition = Condvar::default();
    let first_counter = StdArc::new(WakeCounter::default());
    let first_waker = waker(&first_counter);
    let mut first_context = Context::from_waker(&first_waker);
    let second_counter = StdArc::new(WakeCounter::default());
    let second_waker = waker(&second_counter);
    let mut second_context = Context::from_waker(&second_waker);
    let mut first = Box::pin(condition.wait(mutex.lock_sync()));
    assert!(first.as_mut().poll(&mut first_context).is_pending());
    let mut second = Box::pin(condition.wait(mutex.lock_sync()));
    assert!(second.as_mut().poll(&mut second_context).is_pending());

    condition.notify_all();

    let Poll::Ready(first_guard) = first.as_mut().poll(&mut first_context) else {
        panic!("the first notified waiter must reacquire the mutex");
    };
    drop(first_guard);
    let second_ready = second.as_mut().poll(&mut second_context).is_ready();
    assert_eq!(
        (
            first_counter.0.load(Ordering::Relaxed),
            second_counter.0.load(Ordering::Relaxed),
            second_ready,
        ),
        (1, 1, true),
    );
}

#[test]
fn condvar_wait_reacquires_after_lock_contention() {
    let mutex = Mutex::new(());
    let condition = Condvar::new();
    let counter = StdArc::new(WakeCounter::default());
    let waker = waker(&counter);
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(condition.wait(mutex.lock_sync()));
    assert!(wait.as_mut().poll(&mut context).is_pending());

    let held = mutex.lock_sync();
    condition.notify_one();
    assert!(wait.as_mut().poll(&mut context).is_pending());
    drop(held);

    assert!(wait.as_mut().poll(&mut context).is_ready());
}

#[test]
fn condvar_timeout_can_observe_notification() {
    let pair = Arc::new((Mutex::new(()), Condvar::new()));
    let notifier = Arc::clone(&pair);
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        notifier.1.notify_one();
    });

    let (_guard, result) = pair.1.wait_timeout_sync(pair.0.lock_sync(), std::time::Duration::from_secs(1));
    thread.join().unwrap();

    assert!(!result.timed_out());
}

#[test]
fn condvar_timeout_reacquires_the_mutex() {
    let mutex = Mutex::new(7);
    let condition = Condvar::new();

    let (mut guard, result) = condition.wait_timeout_sync(mutex.lock_sync(), std::time::Duration::from_millis(1));
    *guard = 9;

    assert!(result.timed_out());
    drop(guard);
    assert_eq!(*mutex.lock_sync(), 9);
}

#[test]
fn once_lock_and_lazy_lock_initialize_once() {
    static LAZY: LazyLock<String> = LazyLock::new(|| "ready".to_owned());

    let once = OnceLock::new();
    assert_eq!(once.get_or_init(|| 7), &7);
    assert_eq!(once.get_or_init(|| 9), &7);
    let cloned = once.clone();
    assert_eq!(cloned, OnceLock::from(7));
    assert_eq!(once.get(), Some(&7));

    assert_eq!(&*LAZY, "ready");
}

#[test]
fn once_lock_supports_mutation_set_take_wait_and_owned_access() {
    let mut once = OnceLock::default();
    assert_eq!(once.get_mut(), None);
    assert_eq!(once.set(7), Ok(()));
    assert_eq!(once.set(9), Err(9));
    *once.get_mut().unwrap() = 8;
    assert_eq!(once.wait(), &8);
    assert_eq!(format!("{once:?}"), "OnceLock(OnceLock(8))");
    assert_eq!(once.take(), Some(8));
    assert_eq!(once.into_inner(), None);

    assert_eq!(OnceLock::from(11).into_inner(), Some(11));
    let empty = OnceLock::<usize>::new();
    let empty_clone = empty.clone();
    assert_eq!((empty.get(), empty_clone.get()), (None, None));
}

#[test]
fn once_lock_waits_for_concurrent_initialization() {
    let once = Arc::new(OnceLock::new());
    let waiter_once = Arc::clone(&once);
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        started_sender.send(()).unwrap();
        *waiter_once.wait()
    });
    started_receiver.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));

    once.set(17).unwrap();

    assert_eq!(waiter.join().unwrap(), 17);
}

#[test]
fn once_lock_records_concurrent_get_or_init_contention() {
    let once = Arc::new(OnceLock::new());
    let initializer_once = Arc::clone(&once);
    let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let initializer = std::thread::spawn(move || {
        *initializer_once.get_or_init(|| {
            entered_sender.send(()).unwrap();
            release_receiver.recv().unwrap();
            19
        })
    });
    entered_receiver.recv().unwrap();

    let contender_once = Arc::clone(&once);
    let contender = std::thread::spawn(move || *contender_once.get_or_init(|| 23));
    std::thread::sleep(std::time::Duration::from_millis(10));
    release_sender.send(()).unwrap();

    assert_eq!((initializer.join().unwrap(), contender.join().unwrap()), (19, 19));
}

#[test]
fn lazy_lock_supports_debug_default_and_poisoning() {
    let lazy = LazyLock::new(|| String::from("ready"));
    assert_eq!(format!("{lazy:?}"), "LazyLock(<uninitialized>)");
    assert_eq!(LazyLock::force(&lazy), "ready");
    assert_eq!(format!("{lazy:?}"), "LazyLock(\"ready\")");

    let default = LazyLock::<String>::default();
    assert_eq!(&*default, "");

    let poisoned = LazyLock::new(|| -> String { panic!("initializer panic") });
    catch_unwind(AssertUnwindSafe(|| LazyLock::force(&poisoned))).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| LazyLock::force(&poisoned))).unwrap_err();
}
#[test]
#[cfg(feature = "seismograph")]
#[expect(clippy::too_many_lines, reason = "one scenario verifies all telemetry-enabled primitives")]
fn ownership_and_lock_operations_emit_runtime_telemetry() {
    seismograph::recorder(Configuration {
        general_events: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            ..Default::default()
        },
        arc_dereferences: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let value = Arc::new(7_u64);
    let arc_id = Arc::telemetry_object_id(&value);
    let other_thread = Arc::clone(&value);
    std::thread::spawn(move || std::hint::black_box(*other_thread)).join().unwrap();
    std::hint::black_box(*value);
    let dropped = Arc::new(8_u64);
    let dropped_id = Arc::telemetry_object_id(&dropped);
    drop(dropped);

    let mut relocated = Arc::<u64, PerCore>::new_with(|| 13);
    let relocated_id = Arc::telemetry_object_id(&relocated);
    _ = Relocator::between_threads().relocate(&mut relocated);

    let mutex = Mutex::new(());
    let mutex_id = recorder::ObjectId::from_ptr(std::ptr::from_ref(&mutex).cast::<()>());
    let mutex_guard = mutex.try_lock().unwrap();
    assert!(mutex.try_lock().is_none());
    drop(mutex_guard);
    let poison_panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = mutex.lock_sync();
        panic!("poison telemetry mutex");
    }));
    assert!(poison_panic.is_err());
    drop(mutex.lock_sync_result().unwrap_err().into_inner());
    mutex.clear_poison();
    mutex.clear_poison();

    let rw_lock = RwLock::new(());
    let rw_lock_id = recorder::ObjectId::from_ptr(std::ptr::from_ref(&rw_lock).cast::<()>());
    let read_guard = rw_lock.try_read().unwrap();
    assert!(rw_lock.try_write().is_none());
    drop(read_guard);
    let write_guard = rw_lock.try_write().unwrap();
    assert!(rw_lock.try_read().is_none());
    drop(write_guard);

    let barrier = Arc::new(Barrier::new(2));
    let barrier_id = recorder::ObjectId::from_ptr(std::ptr::from_ref(&*barrier).cast::<()>());
    let other_barrier = Arc::clone(&barrier);
    let barrier_thread = std::thread::spawn(move || other_barrier.wait_sync());
    let barrier_result = barrier.wait_sync();
    let other_barrier_result = barrier_thread.join().unwrap();
    assert_ne!(barrier_result.is_leader(), other_barrier_result.is_leader());

    let condition = Condvar::new();
    let condition_id = recorder::ObjectId::from_ptr(std::ptr::from_ref(&condition).cast::<()>());
    let condition_mutex = Mutex::new(());
    let counter = StdArc::new(WakeCounter::default());
    let condition_waker = waker(&counter);
    let mut condition_context = Context::from_waker(&condition_waker);
    let mut condition_wait = pin!(condition.wait(condition_mutex.lock_sync()));
    assert!(condition_wait.as_mut().poll(&mut condition_context).is_pending());
    condition.notify_one();
    assert!(condition_wait.as_mut().poll(&mut condition_context).is_ready());

    let once = OnceLock::new();
    let once_id = recorder::ObjectId::from_ptr(std::ptr::from_ref(&once).cast::<()>());
    assert_eq!(once.get_or_init(|| 17), &17);
    let waiting_once = Arc::new(OnceLock::new());
    let waiting_once_id = recorder::ObjectId::from_ptr(std::ptr::from_ref(&*waiting_once).cast::<()>());
    let setter_once = Arc::clone(&waiting_once);
    let once_setter = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        setter_once.set(19).unwrap();
    });
    assert_eq!(waiting_once.wait(), &19);
    once_setter.join().unwrap();

    let (channel_sender, channel_receiver) = performables::sync::channel::unbounded::<usize>();
    assert!(channel_receiver.try_recv().unwrap_err().is_empty());
    drop(channel_sender);

    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap().events;
    let arc_events = snapshot
        .events
        .iter()
        .filter(|event| event.object_id() == Some(arc_id))
        .collect::<Vec<_>>();
    let arc_threads = arc_events
        .iter()
        .map(|event| event.thread_id)
        .collect::<std::collections::HashSet<_>>();
    assert!(
        arc_events
            .iter()
            .any(|event| event.kind == EventKind::ArcCreate && !event.call_stack.is_empty())
    );
    assert!(arc_events.iter().any(|event| event.kind == EventKind::ArcClone));
    assert!(
        arc_events
            .iter()
            .any(|event| event.kind == EventKind::ArcDeref && !event.call_stack.is_empty())
    );
    assert!(arc_threads.len() >= 2);
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(dropped_id) && event.kind == EventKind::ArcDrop)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(relocated_id) && event.kind == EventKind::ArcRelocate)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(mutex_id) && event.kind == EventKind::MutexContention)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(mutex_id) && event.kind == EventKind::MutexRelease)
    );
    let poison_events = snapshot
        .events
        .iter()
        .filter(|event| event.object_id() == Some(mutex_id))
        .filter_map(|event| {
            matches!(
                event.kind,
                EventKind::LockPoisoned | EventKind::LockPoisonObserved | EventKind::LockPoisonCleared
            )
            .then_some(event.kind)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        poison_events,
        vec![EventKind::LockPoisoned, EventKind::LockPoisonObserved, EventKind::LockPoisonCleared,]
    );
    assert!(
        snapshot
            .events
            .iter()
            .filter(|event| event.object_id() == Some(mutex_id))
            .filter(|event| {
                matches!(
                    event.kind,
                    EventKind::LockPoisoned | EventKind::LockPoisonObserved | EventKind::LockPoisonCleared
                )
            })
            .all(|event| !event.call_stack.is_empty())
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(rw_lock_id) && event.kind == EventKind::RwLockReadAccess)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(rw_lock_id) && event.kind == EventKind::RwLockWriteContention)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(rw_lock_id) && event.kind == EventKind::RwLockReadContention)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(rw_lock_id) && event.kind == EventKind::RwLockReadRelease)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(rw_lock_id) && event.kind == EventKind::RwLockWriteRelease)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(barrier_id) && event.kind == EventKind::BarrierContention)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(barrier_id) && event.kind == EventKind::BarrierRelease)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(condition_id) && event.kind == EventKind::CondvarNotify)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(condition_id) && event.kind == EventKind::CondvarContention)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(condition_id) && event.kind == EventKind::CondvarAccess)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(once_id) && event.kind == EventKind::OnceInitialize)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.object_id() == Some(waiting_once_id) && event.kind == EventKind::OnceContention)
    );
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.kind == EventKind::ChannelReceiveContention)
    );

    seismograph::recorder(Configuration::default());
}
