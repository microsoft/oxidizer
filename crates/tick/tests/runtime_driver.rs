// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public custom-runtime driver behavior.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::task::{Context, RawWaker, RawWakerVTable, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use testing_aids::TEST_TIMEOUT;
use thread_aware::{ThreadAware, ThreadBuilder};
use tick::Clock;
use tick::runtime::InactiveClock;

#[derive(Debug)]
struct CountingWake(AtomicUsize);

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct SharedCountingWake(Arc<AtomicUsize>);

impl Wake for SharedCountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct ClockOwningWake {
    clock: Mutex<Option<Clock>>,
    wake_count: AtomicUsize,
}

impl Wake for ClockOwningWake {
    fn wake(self: Arc<Self>) {
        self.wake_count.fetch_add(1, Ordering::Relaxed);
    }
}

struct CloneHook {
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

fn clone_hook_waker(hook: impl FnOnce() + Send + 'static) -> Waker {
    let state = Arc::new(CloneHook {
        hook: Mutex::new(Some(Box::new(hook))),
    });
    let raw = RawWaker::new(Arc::into_raw(state).cast(), &CLONE_HOOK_VTABLE);
    // SAFETY: CLONE_HOOK_VTABLE maintains one Arc strong reference for every RawWaker.
    unsafe { Waker::from_raw(raw) }
}

unsafe fn clone_hook_clone(data: *const ()) -> RawWaker {
    // SAFETY: data was created by Arc::into_raw in clone_hook_waker or this function.
    let state = unsafe { &*data.cast::<CloneHook>() };
    let hook = state.hook.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(hook) = hook {
        hook();
    }
    // SAFETY: the source RawWaker retains its strong reference and the clone owns the increment.
    unsafe { Arc::increment_strong_count(data.cast::<CloneHook>()) };
    RawWaker::new(data, &CLONE_HOOK_VTABLE)
}

unsafe fn clone_hook_wake(data: *const ()) {
    // SAFETY: consuming wake owns one Arc strong reference.
    drop(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
}

unsafe fn clone_hook_wake_by_ref(_data: *const ()) {}

unsafe fn clone_hook_drop(data: *const ()) {
    // SAFETY: dropping the RawWaker releases its owned Arc strong reference.
    drop(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
}

static CLONE_HOOK_VTABLE: RawWakerVTable = RawWakerVTable::new(clone_hook_clone, clone_hook_wake, clone_hook_wake_by_ref, clone_hook_drop);

impl Drop for ClockOwningWake {
    fn drop(&mut self) {
        drop(self.clock.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).take());
    }
}

#[test]
fn dropping_the_final_clock_owner_wakes_the_driver() {
    let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
    let driver_waker = Waker::from(Arc::clone(&wake_count));
    let (clock, _driver) = InactiveClock::new_isolated().activate(driver_waker);

    drop(clock);

    assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn concurrent_final_owner_drops_wake_the_driver_once() {
    const OWNER_COUNT: usize = 8;
    let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
    let driver_waker = Waker::from(Arc::clone(&wake_count));
    let (clock, _driver) = InactiveClock::new_isolated().activate(driver_waker);
    let release = Arc::new(Barrier::new(OWNER_COUNT + 1));
    let owners: [_; OWNER_COUNT] = std::array::from_fn(|_| clock.clone());
    drop(clock);
    let threads: Vec<_> = owners
        .into_iter()
        .map(|clock| {
            let release = Arc::clone(&release);
            thread::spawn(move || {
                release.wait();
                drop(clock);
            })
        })
        .collect();

    release.wait();
    for thread in threads {
        thread.join().unwrap();
    }

    assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn driver_reports_clock_gone_only_after_the_final_clone_is_dropped() {
    let (clock, mut driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let final_clone = clock.clone();
    drop(clock);

    assert_eq!(driver.advance_timers(Instant::now()).unwrap(), None);

    drop(final_clone);
    driver.advance_timers(Instant::now()).unwrap_err();
}

#[test]
fn relocating_the_only_clock_owner_finishes_its_previous_driver() {
    let (mut clock, mut previous_driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let builder = ThreadBuilder::default();
    let source = builder.build(thread::current().id());
    let destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let destination = builder.build(destination_id);

    clock.relocate(Some(&source), &destination);

    previous_driver.advance_timers(Instant::now()).unwrap_err();
    drop(clock);
}

#[test]
fn dropping_a_newer_driver_restores_notifications_to_the_surviving_driver() {
    let first_wakes = Arc::new(AtomicUsize::new(0));
    let second_wakes = Arc::new(AtomicUsize::new(0));
    let root = InactiveClock::new_isolated();
    let (first_clock, first_driver) = root
        .clone()
        .activate(Waker::from(Arc::new(SharedCountingWake(Arc::clone(&first_wakes)))));
    let (second_clock, second_driver) = root.activate(Waker::from(Arc::new(SharedCountingWake(Arc::clone(&second_wakes)))));
    drop(second_clock);
    drop(second_driver);

    let mut context = Context::from_waker(Waker::noop());
    let mut delay = std::pin::pin!(first_clock.delay(Duration::from_secs(1)));
    assert!(delay.as_mut().poll(&mut context).is_pending());

    assert_eq!(first_wakes.load(Ordering::Relaxed), 1);
    assert_eq!(second_wakes.load(Ordering::Relaxed), 0);
    drop(first_driver);
}

#[test]
fn all_live_drivers_receive_first_timer_and_final_owner_notifications() {
    let first_wakes = Arc::new(AtomicUsize::new(0));
    let second_wakes = Arc::new(AtomicUsize::new(0));
    let root = InactiveClock::new_isolated();
    let (first_clock, first_driver) = root
        .clone()
        .activate(Waker::from(Arc::new(SharedCountingWake(Arc::clone(&first_wakes)))));
    let (second_clock, second_driver) = root.activate(Waker::from(Arc::new(SharedCountingWake(Arc::clone(&second_wakes)))));
    {
        let mut context = Context::from_waker(Waker::noop());
        let mut delay = std::pin::pin!(first_clock.delay(Duration::from_secs(1)));
        assert!(delay.as_mut().poll(&mut context).is_pending());
    }
    assert_eq!((first_wakes.load(Ordering::Relaxed), second_wakes.load(Ordering::Relaxed)), (1, 1));

    drop(first_clock);
    drop(second_clock);
    assert_eq!((first_wakes.load(Ordering::Relaxed), second_wakes.load(Ordering::Relaxed)), (2, 2));

    drop(first_driver);
    drop(second_driver);
}

#[test]
fn unregistering_a_driver_drops_its_clock_owning_waker_outside_the_registry_lock() {
    let root = InactiveClock::new_isolated();
    let (owned_clock, first_driver) = root.clone().activate(Waker::noop().clone());
    let clock_owning_waker = Waker::from(Arc::new(ClockOwningWake {
        clock: Mutex::new(Some(owned_clock)),
        wake_count: AtomicUsize::new(0),
    }));
    let (second_clock, second_driver) = root.activate(clock_owning_waker);
    drop(second_clock);
    let (completed, completion) = mpsc::channel();

    let dropping = thread::spawn(move || {
        drop(second_driver);
        completed.send(()).unwrap();
    });

    completion.recv_timeout(TEST_TIMEOUT).unwrap();
    dropping.join().unwrap();
    drop(first_driver);
}

#[test]
fn notifying_drivers_does_not_clone_custom_wakers_under_the_registry_lock() {
    let root = InactiveClock::new_isolated();
    let (first_clock, first_driver) = root.clone().activate(Waker::noop().clone());
    let first_driver = Arc::new(Mutex::new(Some(first_driver)));
    let first_driver_for_hook = Arc::clone(&first_driver);
    let hook_waker = clone_hook_waker(move || {
        drop(
            first_driver_for_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
    });
    let (second_clock, second_driver) = root.activate(hook_waker);
    let (completed, completion) = mpsc::channel();

    let registering = thread::spawn(move || {
        let mut context = Context::from_waker(Waker::noop());
        let mut delay = std::pin::pin!(second_clock.delay(Duration::from_secs(1)));
        assert!(delay.as_mut().poll(&mut context).is_pending());
        completed.send(()).unwrap();
        drop(second_driver);
        drop(first_clock);
    });

    completion.recv_timeout(TEST_TIMEOUT).unwrap();
    registering.join().unwrap();
    assert!(first_driver.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some());
}
