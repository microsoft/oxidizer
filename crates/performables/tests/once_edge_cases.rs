// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Initialization contention, unwinding and exact resource ownership.
//!
//! These std-backed once-cell cases verify public outcomes, not native parking
//! registration. The owned lock and notification backends test registration with
//! private deterministic seams.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use performables::sync::once::{LazyLock, OnceLock};
use testing_aids::is_mutation_testing;

const DEADLINE: Duration = Duration::from_secs(if cfg!(miri) { 120 } else { 10 });

fn receive<T>(receiver: &mpsc::Receiver<T>) -> Result<T, mpsc::RecvTimeoutError> {
    if is_mutation_testing() {
        receiver.recv().map_err(Into::into)
    } else {
        receiver.recv_timeout(DEADLINE)
    }
}

#[derive(Debug)]
struct Payload(Arc<AtomicUsize>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn get_and_clone_do_not_wait_for_an_initialization_in_progress() {
    let cell = Arc::new(OnceLock::new());
    let (entered, initialization) = mpsc::channel();
    let (resume, gate) = mpsc::channel();
    let worker = std::thread::spawn({
        let cell = Arc::clone(&cell);
        move || {
            *cell.get_or_init(|| {
                entered.send(()).unwrap();
                receive(&gate).unwrap();
                7
            })
        }
    });
    receive(&initialization).unwrap();
    assert_eq!(cell.get(), None);
    let cloned = OnceLock::clone(&cell);
    assert_eq!(cloned.get(), None);
    resume.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), 7);
    assert_eq!(cell.get(), Some(&7));
    assert_eq!(cloned.get(), None);
}

#[test]
fn set_waits_for_an_active_initializer_then_returns_its_unconsumed_value() {
    let cell = Arc::new(OnceLock::new());
    let (entered, initialization) = mpsc::channel();
    let (resume, gate) = mpsc::channel();
    let initializer = std::thread::spawn({
        let cell = Arc::clone(&cell);
        move || {
            *cell.get_or_init(|| {
                entered.send(()).unwrap();
                receive(&gate).unwrap();
                11
            })
        }
    });
    receive(&initialization).unwrap();
    let (finished, outcome) = mpsc::channel();
    let setter = std::thread::spawn({
        let cell = Arc::clone(&cell);
        move || finished.send(cell.set(13)).unwrap()
    });
    assert_eq!(outcome.try_recv(), Err(mpsc::TryRecvError::Empty));
    resume.send(()).unwrap();
    assert_eq!(receive(&outcome).unwrap(), Err(13));
    initializer.join().unwrap();
    setter.join().unwrap();
    assert_eq!(cell.get(), Some(&11));
}

#[test]
fn a_waiter_survives_an_initializer_panic_and_observes_a_later_success() {
    let cell = Arc::new(OnceLock::<usize>::new());
    let (entered, initialization) = mpsc::channel();
    let (resume, gate) = mpsc::channel();
    let initializer = std::thread::spawn({
        let cell = Arc::clone(&cell);
        move || {
            catch_unwind(AssertUnwindSafe(|| {
                cell.get_or_init(|| {
                    entered.send(()).unwrap();
                    receive(&gate).unwrap();
                    panic!("test the first initialization failing");
                });
            }))
            .unwrap_err();
        }
    });
    receive(&initialization).unwrap();
    let (observed, observations) = mpsc::channel();
    let waiter = std::thread::spawn({
        let cell = Arc::clone(&cell);
        move || observed.send(*cell.wait()).unwrap()
    });
    resume.send(()).unwrap();
    initializer.join().unwrap();
    assert_eq!(cell.get(), None);
    cell.set(17).unwrap();
    assert_eq!(receive(&observations).unwrap(), 17);
    waiter.join().unwrap();
}

#[test]
fn many_contenders_execute_one_once_lock_initializer() {
    let workers = if cfg!(miri) { 3 } else { 8 };
    let cell = Arc::new(OnceLock::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(workers));
    let (finished, outcomes) = mpsc::channel();
    let handles = (0..workers)
        .map(|_| {
            let cell = Arc::clone(&cell);
            let calls = Arc::clone(&calls);
            let start = Arc::clone(&start);
            let finished = finished.clone();
            std::thread::spawn(move || {
                start.wait();
                let value = cell.get_or_init(|| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    (0..32).collect::<Vec<_>>()
                });
                finished.send(value.clone()).unwrap();
            })
        })
        .collect::<Vec<_>>();
    drop(finished);
    for _ in 0..workers {
        assert_eq!(receive(&outcomes).unwrap(), (0..32).collect::<Vec<_>>());
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn rejected_set_and_taken_values_keep_ownership_until_the_caller_drops_them() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut cell = OnceLock::new();
    cell.set(Payload(Arc::clone(&drops))).unwrap();
    let rejected = cell.set(Payload(Arc::clone(&drops))).unwrap_err();
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(rejected);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    let taken = cell.take().unwrap();
    assert!(cell.get().is_none());
    cell.set(Payload(Arc::clone(&drops))).unwrap();
    drop(cell.into_inner());
    assert_eq!(drops.load(Ordering::Relaxed), 2);
    drop(taken);
    assert_eq!(drops.load(Ordering::Relaxed), 3);
}

#[test]
fn initialized_once_lock_drops_its_value_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let cell = OnceLock::from(Payload(Arc::clone(&drops)));
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(cell);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn a_panicking_value_clone_does_not_damage_the_original_once_lock() {
    #[derive(Debug)]
    struct CannotClone;
    impl Clone for CannotClone {
        fn clone(&self) -> Self {
            panic!("test a stored value's clone failing");
        }
    }
    let cell = OnceLock::from(CannotClone);
    catch_unwind(|| drop(cell.clone())).unwrap_err();
    assert!(cell.get().is_some());
    assert!(cell.into_inner().is_some());
}

#[test]
fn a_partial_initializer_value_is_dropped_before_retrying() {
    let drops = Arc::new(AtomicUsize::new(0));
    let cell = OnceLock::new();
    catch_unwind(AssertUnwindSafe(|| {
        cell.get_or_init(|| {
            let _partial = Payload(Arc::clone(&drops));
            panic!("test construction failing after acquiring a resource");
        });
    }))
    .unwrap_err();
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    assert!(cell.get().is_none());
    cell.get_or_init(|| Payload(Arc::clone(&drops)));
    drop(cell);
    assert_eq!(drops.load(Ordering::Relaxed), 2);
}

#[test]
fn dropping_an_unforced_lazy_lock_releases_initializer_captures_without_running_it() {
    let drops = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = Payload(Arc::clone(&drops));
    let lazy = LazyLock::new({
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::Relaxed);
            drop(captured);
            19
        }
    });
    drop(lazy);
    assert_eq!((drops.load(Ordering::Relaxed), calls.load(Ordering::Relaxed)), (1, 0));
}

#[test]
fn dropping_a_forced_lazy_lock_destroys_the_value_not_a_second_initializer() {
    let drops = Arc::new(AtomicUsize::new(0));
    let captured = Payload(Arc::clone(&drops));
    let lazy = LazyLock::new(move || {
        drop(captured);
        Payload(Arc::clone(&drops))
    });
    let value = LazyLock::force(&lazy);
    let counter = Arc::clone(&value.0);
    assert_eq!(counter.load(Ordering::Relaxed), 1);
    drop(lazy);
    assert_eq!(counter.load(Ordering::Relaxed), 2);
}

#[test]
fn lazy_initializer_panics_preserve_the_original_payload_and_do_not_retry() {
    let calls = AtomicUsize::new(0);
    let lazy = LazyLock::new(|| -> usize {
        calls.fetch_add(1, Ordering::Relaxed);
        std::panic::panic_any(String::from("edge-case initializer failure"));
    });
    let panic = catch_unwind(AssertUnwindSafe(|| LazyLock::force(&lazy))).unwrap_err();
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some("edge-case initializer failure")
    );
    catch_unwind(AssertUnwindSafe(|| LazyLock::force(&lazy))).unwrap_err();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn lazy_initializers_need_not_be_sync_when_their_results_are_sync() {
    fn send_sync<T: Send + Sync>(_: &T) {}
    let captured = std::cell::Cell::new(23);
    let lazy = LazyLock::new(move || captured.get());
    send_sync(&lazy);
    assert_eq!(*lazy, 23);
}

#[test]
fn many_threads_force_one_lazy_initializer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lazy = Arc::new(LazyLock::new({
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::Relaxed);
            String::from("initialized")
        }
    }));
    let workers = if cfg!(miri) { 3 } else { 8 };
    let start = Arc::new(Barrier::new(workers));
    let handles = (0..workers)
        .map(|_| {
            let lazy = Arc::clone(&lazy);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                assert_eq!(LazyLock::force(&lazy), "initialized");
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
