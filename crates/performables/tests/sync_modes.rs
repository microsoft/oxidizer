// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contracts shared by compact blocking and async-capable synchronization.

#[cfg(feature = "serde")]
#[path = "support/serializer.rs"]
mod serializer_support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use performables::sync::barrier::Barrier;
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mode;
use performables::sync::mutex::Mutex;

#[cfg(miri)]
const DEADLINE: Duration = Duration::from_secs(120);
#[cfg(not(miri))]
const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn synchronous_objects_have_native_sized_storage() {
    assert_eq!(
        (size_of::<Mutex<u64>>(), size_of::<RwLock<u64>>(), size_of::<Condvar>(),),
        (
            size_of::<std::sync::Mutex<u64>>(),
            size_of::<std::sync::RwLock<u64>>(),
            size_of::<std::sync::Condvar>(),
        ),
    );
    assert!(size_of::<Barrier>() <= size_of::<std::sync::Barrier>());
}

#[test]
fn synchronous_locks_support_const_construction_and_owned_access() {
    let mut mutex = const { Mutex::<u64>::const_new(3) };
    let mut lock = const { RwLock::<u64>::new(5) };
    let _condition = const { Condvar::<mode::Sync>::new() };
    *mutex.get_mut() += 7;
    *lock.get_mut() += 11;

    assert_eq!((mutex.into_inner(), lock.into_inner()), (10, 16));
}

#[test]
fn synchronous_mutex_blocks_until_release() {
    let mutex = Arc::new(Mutex::<u64>::new(0));
    let held = mutex.lock();
    let contender = Arc::clone(&mutex);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        *contender.lock() = 7;
        finished_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    finished_rx.try_recv().unwrap_err();
    drop(held);
    finished_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();

    assert_eq!(*mutex.lock(), 7);
}

#[test]
fn synchronous_mutex_try_lock_never_waits() {
    let mutex = Mutex::<u64>::new(3);
    let guard = mutex.lock();
    assert!(mutex.try_lock_result().unwrap().is_none());
    drop(guard);

    assert_eq!(*mutex.try_lock().unwrap(), 3);
}

#[test]
fn synchronous_mutex_preserves_poison_recovery() {
    let mutex = Mutex::<u64>::new(3);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        *mutex.lock() = 5;
        let _guard = mutex.lock();
        panic!("poison the protected value");
    }));
    assert!(panic.is_err());
    assert!(mutex.is_poisoned());
    catch_unwind(AssertUnwindSafe(|| mutex.lock())).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| mutex.try_lock())).unwrap_err();

    let mut guard = mutex.lock_result().unwrap_err().into_inner();
    *guard += 7;
    drop(guard);
    let guard = mutex.try_lock_result().unwrap_err().into_inner();
    assert_eq!(*guard, 12);
    drop(guard);
    mutex.clear_poison();

    assert_eq!(*mutex.lock(), 12);
}

#[test]
fn synchronous_owned_access_does_not_discard_poisoned_values() {
    let mut mutex = Mutex::<u64>::new(3);
    catch_unwind(AssertUnwindSafe(|| {
        let _guard = mutex.lock();
        panic!("poison mutex");
    }))
    .unwrap_err();
    *mutex.get_mut() += 5;

    assert_eq!(mutex.into_inner(), 8);
}

#[test]
fn synchronous_guard_acquired_during_unwind_does_not_poison() {
    struct AcquireOnDrop<'a>(&'a Mutex<u64>);

    impl Drop for AcquireOnDrop<'_> {
        fn drop(&mut self) {
            drop(self.0.lock());
        }
    }

    let mutex = Mutex::<u64>::new(7);
    catch_unwind(AssertUnwindSafe(|| {
        let _acquire = AcquireOnDrop(&mutex);
        panic!("an unwind began before acquisition");
    }))
    .unwrap_err();

    assert!(!mutex.is_poisoned());
    assert_eq!(*mutex.lock(), 7);
}

#[test]
fn synchronous_rwlock_allows_readers_and_excludes_writers() {
    let lock = RwLock::<u64>::new(7);
    let first = lock.read();
    let second = lock.try_read().unwrap();
    assert!(lock.try_write_result().unwrap().is_none());
    drop((first, second));
    let mut writer = lock.write();
    *writer += 3;
    assert!(lock.try_read_result().unwrap().is_none());
    drop(writer);

    assert_eq!(*lock.read(), 10);
}

#[test]
fn synchronous_rwlock_writer_waits_for_readers() {
    let lock = Arc::new(RwLock::<u64>::new(0));
    let reader = lock.read();
    let contender = Arc::clone(&lock);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        *contender.write_result().unwrap() = 11;
        finished_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    finished_rx.try_recv().unwrap_err();
    drop(reader);
    finished_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();

    assert_eq!(*lock.read(), 11);
}

#[test]
fn synchronous_rwlock_reader_waits_for_writer() {
    let lock = Arc::new(RwLock::<u64>::new(7));
    let writer = lock.write();
    let contender = Arc::clone(&lock);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        finished_tx.send(*contender.read_result().unwrap()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    finished_rx.try_recv().unwrap_err();
    drop(writer);
    let value = finished_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();

    assert_eq!(value, 7);
}

#[test]
fn synchronous_rwlock_only_exclusive_guards_poison() {
    let lock = RwLock::<u64>::new(7);
    catch_unwind(AssertUnwindSafe(|| {
        let _reader = lock.read();
        panic!("a reader does not poison");
    }))
    .unwrap_err();
    assert!(!lock.is_poisoned());

    catch_unwind(AssertUnwindSafe(|| {
        let mut writer = lock.write();
        *writer += 3;
        panic!("a writer poisons");
    }))
    .unwrap_err();
    assert!(lock.is_poisoned());
    catch_unwind(AssertUnwindSafe(|| lock.read())).unwrap_err();
    catch_unwind(AssertUnwindSafe(|| lock.write())).unwrap_err();
    assert_eq!(*lock.read_result().unwrap_err().into_inner(), 10);
    let mut writer = lock.write_result().unwrap_err().into_inner();
    *writer += 5;
    drop(writer);
    lock.clear_poison();

    assert_eq!(*lock.read(), 15);
}

#[test]
fn synchronous_condition_wait_releases_and_reacquires_the_mutex() {
    let pair = Arc::new((Mutex::<u64>::new(0), Condvar::<mode::Sync>::new()));
    let other = Arc::clone(&pair);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let guard = other.0.lock();
        started_tx.send(()).unwrap();
        let guard = other.1.wait_while(guard, |value| *value == 0);
        finished_tx.send(*guard).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    *pair.0.lock() = 7;
    pair.1.notify_one();
    let value = finished_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();

    assert_eq!(value, 7);
}

#[test]
fn synchronous_condition_timeout_returns_the_guard() {
    let mutex = Mutex::<u64>::new(3);
    let condition = Condvar::<mode::Sync>::new();
    let (mut guard, result) = condition.wait_timeout(mutex.lock(), Duration::ZERO);
    assert!(result.timed_out());
    *guard += 5;
    drop(guard);

    assert_eq!(*mutex.lock(), 8);
}

#[test]
fn synchronous_condition_timeout_reports_notification() {
    let pair = Arc::new((Mutex::<u64>::new(0), Condvar::<mode::Sync>::new()));
    let other = Arc::clone(&pair);
    let (started_tx, started_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let guard = other.0.lock();
        started_tx.send(()).unwrap();
        other.1.wait_timeout(guard, DEADLINE).1.timed_out()
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    let guard = pair.0.lock();
    pair.1.notify_one();
    drop(guard);

    assert!(!worker.join().unwrap());
}

#[test]
fn synchronous_condition_wait_preserves_poisoning_on_reacquisition() {
    let pair = Arc::new((Mutex::<u64>::new(0), Condvar::<mode::Sync>::new()));
    let other = Arc::clone(&pair);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let guard = other.0.lock();
            started_tx.send(()).unwrap();
            drop(other.1.wait_while(guard, |value| *value == 0));
        }));
        finished_tx.send(result.is_err()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    catch_unwind(AssertUnwindSafe(|| {
        let mut guard = pair.0.lock();
        *guard = 1;
        panic!("poison while the waiter has released ownership");
    }))
    .unwrap_err();
    pair.1.notify_all();
    let poisoned = finished_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();

    assert!(poisoned);
}

#[test]
fn synchronous_condition_timeout_preserves_poisoning_after_notification() {
    let pair = Arc::new((Mutex::<u64>::new(0), Condvar::<mode::Sync>::new()));
    let other = Arc::clone(&pair);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let guard = other.0.lock();
            started_tx.send(()).unwrap();
            drop(other.1.wait_timeout(guard, DEADLINE));
        }));
        finished_tx.send(result.is_err()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    catch_unwind(AssertUnwindSafe(|| {
        let mut guard = pair.0.lock();
        *guard = 1;
        panic!("poison while the timed waiter has released ownership");
    }))
    .unwrap_err();
    pair.1.notify_one();
    let poisoned = finished_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();

    assert!(poisoned);
}

#[test]
fn synchronous_barrier_reuses_generations_with_one_leader() {
    let barrier = Arc::new(Barrier::<mode::Sync>::new(3));
    let (finished_tx, finished_rx) = mpsc::channel();
    let workers = (0..3)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let finished_tx = finished_tx.clone();
            std::thread::spawn(move || {
                let leaders = [barrier.wait().is_leader(), barrier.wait().is_leader()];
                finished_tx.send(leaders).unwrap();
            })
        })
        .collect::<Vec<_>>();
    let results = (0..3).map(|_| finished_rx.recv_timeout(DEADLINE).unwrap()).collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }

    assert_eq!(
        (
            results.iter().filter(|leaders| leaders[0]).count(),
            results.iter().filter(|leaders| leaders[1]).count(),
        ),
        (1, 1),
    );
}

#[test]
#[should_panic(expected = "nonzero")]
fn synchronous_barrier_rejects_zero_participants() {
    let _barrier = Barrier::<mode::Sync>::new(0);
}

#[cfg(feature = "serde")]
#[test]
fn serde_supports_both_mutex_modes() {
    use serde::de::value::{Error, U64Deserializer};
    use serde::{Deserialize, Serialize};
    use serializer_support::ValueSerializer;

    let synchronous = Mutex::<u64>::deserialize(U64Deserializer::<Error>::new(7)).unwrap();
    let asynchronous = Mutex::<u64, mode::Async>::deserialize(U64Deserializer::<Error>::new(11)).unwrap();

    assert_eq!(
        (
            synchronous.serialize(ValueSerializer).unwrap(),
            asynchronous.serialize(ValueSerializer).unwrap(),
        ),
        ("7".to_owned(), "11".to_owned()),
    );
}

#[test]
fn async_guards_retain_cross_thread_mobility() {
    fn require_send<T: Send>(_: T) {}

    let mutex = Mutex::<u64, mode::Async>::new(7);
    let lock = RwLock::<u64, mode::Async>::new(11);
    require_send(mutex.lock());
    require_send(lock.read());
    require_send(lock.write());
}
