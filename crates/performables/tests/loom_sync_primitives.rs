// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loom models for the synchronization protocols used by `performables::sync`.
//!
//! These model the production algorithms rather than the production types,
//! because Loom cannot instrument their `std` synchronization primitives.

#![cfg(loom)]
#![allow(clippy::unwrap_used, reason = "poisoning requires a prior model failure")]

use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;

// Same atomic header and queue-serialized assignment as Notify. Queue storage
// is preinitialized here; OnceLock publication is supplied by the standard library.
struct RetainedNotify {
    state: AtomicUsize,
    queue: Mutex<std::collections::VecDeque<Arc<AtomicBool>>>,
}

impl RetainedNotify {
    fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            queue: Mutex::new(std::collections::VecDeque::new()),
        }
    }

    fn take(&self) -> bool {
        self.state.compare_exchange(1, 0, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    fn register(&self, waiter: &Arc<AtomicBool>) {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if self.take() {
                waiter.store(true, Ordering::Release);
                return;
            }
            match self.state.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) | Err(2) => {
                    queue.push_back(Arc::clone(waiter));
                    return;
                }
                Err(_) => {}
            }
        }
    }

    fn notify(&self) {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state == 2 {
                loom::sync::atomic::fence(Ordering::Acquire);
                let mut queue = self.queue.lock().unwrap();
                if let Some(waiter) = queue.pop_front() {
                    if queue.is_empty() {
                        self.state.store(0, Ordering::Release);
                    }
                    waiter.store(true, Ordering::Release);
                    return;
                }
                drop(queue);
                state = self.state.load(Ordering::Relaxed);
                continue;
            }
            match self.state.compare_exchange_weak(state, 1, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => return,
                Err(current) => state = current,
            }
        }
    }

    fn cancel(&self, waiter: &Arc<AtomicBool>) {
        let mut queue = self.queue.lock().unwrap();
        if let Some(index) = queue.iter().position(|queued| Arc::ptr_eq(queued, waiter)) {
            queue.remove(index);
            if queue.is_empty() {
                self.state.store(0, Ordering::Release);
            }
        }
        let received = waiter.load(Ordering::Acquire);
        drop(queue);
        if received {
            self.notify();
        }
    }
}

#[test]
fn retained_notify_registration_cannot_lose_a_notification() {
    loom::model(|| {
        let notify = Arc::new(RetainedNotify::new());
        let received = Arc::new(AtomicBool::new(false));
        let worker = {
            let notify = Arc::clone(&notify);
            let received = Arc::clone(&received);
            thread::spawn(move || notify.register(&received))
        };
        notify.notify();
        worker.join().unwrap();
        assert!(received.load(Ordering::Acquire));
        assert!(!notify.take());
    });
}

#[test]
fn retained_notify_cancel_racing_delivery_passes_the_permit_on() {
    loom::model(|| {
        let notify = Arc::new(RetainedNotify::new());
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        notify.register(&first);
        notify.register(&second);
        let worker = {
            let notify = Arc::clone(&notify);
            thread::spawn(move || notify.notify())
        };
        notify.cancel(&first);
        worker.join().unwrap();
        assert!(second.load(Ordering::Acquire));
        assert!(!notify.take());
    });
}

#[test]
fn retained_notify_two_registered_waiters_receive_distinct_notifications() {
    loom::model(|| {
        let notify = Arc::new(RetainedNotify::new());
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        notify.register(&first);
        notify.register(&second);
        let worker = {
            let notify = Arc::clone(&notify);
            thread::spawn(move || notify.notify())
        };
        notify.notify();
        worker.join().unwrap();
        assert!(first.load(Ordering::Acquire));
        assert!(second.load(Ordering::Acquire));
        assert!(!notify.take());
    });
}

#[test]
fn retained_notify_waiting_header_publishes_queue_initialization() {
    loom::model(|| {
        let header = Arc::new(AtomicUsize::new(0));
        let initialized = Arc::new(AtomicBool::new(false));
        let worker = {
            let header = Arc::clone(&header);
            let initialized = Arc::clone(&initialized);
            thread::spawn(move || {
                initialized.store(true, Ordering::Relaxed);
                header.store(2, Ordering::Release);
            })
        };
        while header.load(Ordering::Relaxed) != 2 {
            thread::yield_now();
        }
        loom::sync::atomic::fence(Ordering::Acquire);
        assert!(initialized.load(Ordering::Relaxed));
        worker.join().unwrap();
    });
}

// Same per-shard publication handshake as Broadcast. Slot storage and waker
// callbacks are tested through public native/Miri tests; this isolates ordering.
struct WatchBroadcast {
    version: AtomicUsize,
    pending: AtomicBool,
    waiters: Mutex<Vec<usize>>,
    received: AtomicUsize,
}

impl WatchBroadcast {
    fn new() -> Self {
        Self {
            version: AtomicUsize::new(0),
            pending: AtomicBool::new(false),
            waiters: Mutex::new(Vec::new()),
            received: AtomicUsize::new(0),
        }
    }

    fn register(&self, id: usize) {
        let mut waiters = self.waiters.lock().unwrap();
        waiters.push(id);
        self.pending.store(true, Ordering::Relaxed);
        loom::sync::atomic::fence(Ordering::SeqCst);
        if self.version.load(Ordering::Acquire) != 0 {
            waiters.retain(|waiter| *waiter != id);
            self.received.fetch_or(1 << id, Ordering::Release);
            if waiters.is_empty() {
                self.pending.store(false, Ordering::Release);
            }
        }
    }

    fn cancel(&self, id: usize) {
        if !self.pending.load(Ordering::Acquire) {
            return;
        }
        let mut waiters = self.waiters.lock().unwrap();
        waiters.retain(|waiter| *waiter != id);
        if waiters.is_empty() {
            self.pending.store(false, Ordering::Release);
        }
    }

    fn publish(&self) {
        self.version.fetch_add(1, Ordering::Release);
        loom::sync::atomic::fence(Ordering::SeqCst);
        if self.pending.load(Ordering::Acquire) {
            let mut waiters = self.waiters.lock().unwrap();
            for id in waiters.drain(..) {
                self.received.fetch_or(1 << id, Ordering::Release);
            }
            self.pending.store(false, Ordering::Release);
        }
    }
}

#[test]
fn watch_broadcast_cannot_miss_a_registration_or_published_version() {
    loom::model(|| {
        let watch = Arc::new(WatchBroadcast::new());
        let worker = {
            let watch = Arc::clone(&watch);
            thread::spawn(move || watch.register(0))
        };
        watch.publish();
        worker.join().unwrap();
        assert_eq!(watch.received.load(Ordering::Acquire), 1);
        assert!(watch.waiters.lock().unwrap().is_empty());
    });
}

#[test]
fn watch_cancel_cannot_clear_another_registration_during_broadcast() {
    loom::model(|| {
        let watch = Arc::new(WatchBroadcast::new());
        watch.register(0);
        let cancellation = {
            let watch = Arc::clone(&watch);
            thread::spawn(move || watch.cancel(0))
        };
        let registration = {
            let watch = Arc::clone(&watch);
            thread::spawn(move || watch.register(1))
        };
        watch.publish();
        cancellation.join().unwrap();
        registration.join().unwrap();
        assert_eq!(watch.received.load(Ordering::Acquire) & 2, 2);
        assert!(watch.waiters.lock().unwrap().is_empty());
    });
}

#[test]
fn watch_last_sender_closure_includes_the_final_published_value() {
    loom::model(|| {
        let status = Arc::new(AtomicUsize::new(0));
        let value = Arc::new(AtomicUsize::new(0));
        let senders = Arc::new(AtomicUsize::new(2));
        let publisher = {
            let status = Arc::clone(&status);
            let value = Arc::clone(&value);
            let senders = Arc::clone(&senders);
            thread::spawn(move || {
                value.store(7, Ordering::Relaxed);
                status.fetch_add(2, Ordering::Release);
                if senders.fetch_sub(1, Ordering::AcqRel) == 1 {
                    status.fetch_or(1, Ordering::Release);
                }
            })
        };
        if senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            status.fetch_or(1, Ordering::Release);
        }
        let observed = status.load(Ordering::Acquire);
        if observed & 1 != 0 {
            assert_eq!(observed, 3);
            assert_eq!(value.load(Ordering::Relaxed), 7);
        }
        publisher.join().unwrap();
        assert_eq!(status.load(Ordering::Acquire), 3);
    });
}

#[test]
fn concurrent_watch_observers_claim_one_receiver_version_once() {
    loom::model(|| {
        let observed = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let claim = |observed: &AtomicUsize, completed: &AtomicUsize| {
            let mut previous = observed.load(Ordering::Acquire);
            loop {
                if previous == 2 {
                    return;
                }
                match observed.compare_exchange_weak(previous, 2, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => {
                        completed.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Err(current) => previous = current,
                }
            }
        };
        let worker = {
            let observed = Arc::clone(&observed);
            let completed = Arc::clone(&completed);
            thread::spawn(move || claim(&observed, &completed))
        };
        claim(&observed, &completed);
        worker.join().unwrap();
        assert_eq!(completed.load(Ordering::Relaxed), 1);
    });
}

#[derive(Debug)]
struct Registration {
    queued: Mutex<bool>,
    woken: AtomicBool,
}

impl Registration {
    fn new() -> Self {
        Self {
            queued: Mutex::new(false),
            woken: AtomicBool::new(false),
        }
    }

    fn park_if_not_ready(&self, ready: impl FnOnce() -> bool) -> bool {
        let mut queued = self.queued.lock().unwrap();
        if ready() {
            false
        } else {
            *queued = true;
            true
        }
    }

    fn park_if_not_ready_marked(&self, mark: impl FnOnce(), ready: impl FnOnce() -> bool, clear_waiting: impl FnOnce()) -> bool {
        let mut queued = self.queued.lock().unwrap();
        mark();
        if ready() {
            clear_waiting();
            false
        } else {
            *queued = true;
            true
        }
    }

    fn wake(&self) {
        self.wake_marked(|| {});
    }

    fn wake_marked(&self, clear_waiting: impl FnOnce()) {
        let mut queued = self.queued.lock().unwrap();
        clear_waiting();
        if *queued {
            *queued = false;
            self.woken.store(true, Ordering::Release);
        }
    }

    fn was_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }
}

fn acquire_flag(lock: &AtomicBool) {
    while lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        thread::yield_now();
    }
}

// OnceLock's release/acquire publication is represented by `published`; Loom
// cannot instrument std::sync::OnceLock. The initialization gate is the same
// gate used by the production lazy queue's empty-notification path.
struct LazyRegistration {
    published: AtomicBool,
    initialization: Mutex<()>,
    registration: Registration,
}

impl LazyRegistration {
    fn new() -> Self {
        Self {
            published: AtomicBool::new(false),
            initialization: Mutex::new(()),
            registration: Registration::new(),
        }
    }

    fn publish(&self) -> &Registration {
        if !self.published.load(Ordering::Acquire) {
            let _initialization = self.initialization.lock().unwrap();
            self.published.store(true, Ordering::Release);
        }
        &self.registration
    }

    fn wake_marked(&self, clear_waiting: impl FnOnce()) {
        if self.published.load(Ordering::Acquire) {
            self.registration.wake_marked(clear_waiting);
            return;
        }
        let initialization = self.initialization.lock().unwrap();
        if self.published.load(Ordering::Acquire) {
            drop(initialization);
            self.registration.wake_marked(clear_waiting);
        } else {
            clear_waiting();
        }
    }
}

#[test]
fn lazy_queue_publication_cannot_lose_a_notification() {
    loom::model(|| {
        let queue = Arc::new(LazyRegistration::new());
        let ready = Arc::new(AtomicBool::new(false));
        let waiter = {
            let queue = Arc::clone(&queue);
            let ready = Arc::clone(&ready);
            thread::spawn(move || queue.publish().park_if_not_ready(|| ready.load(Ordering::Acquire)))
        };
        let notifier = {
            let queue = Arc::clone(&queue);
            let ready = Arc::clone(&ready);
            thread::spawn(move || {
                ready.store(true, Ordering::Release);
                queue.wake_marked(|| {});
            })
        };
        let parked = waiter.join().unwrap();
        notifier.join().unwrap();

        assert!(!parked || queue.registration.was_woken());
    });
}

#[test]
fn lazy_queue_empty_clear_cannot_erase_a_new_waiter_marker() {
    loom::model(|| {
        let queue = Arc::new(LazyRegistration::new());
        let marked = Arc::new(AtomicBool::new(false));
        let waiter = {
            let queue = Arc::clone(&queue);
            let marked = Arc::clone(&marked);
            thread::spawn(move || {
                queue.publish().park_if_not_ready_marked(
                    || marked.store(true, Ordering::Release),
                    || false,
                    || marked.store(false, Ordering::Release),
                )
            })
        };
        let notifier = {
            let queue = Arc::clone(&queue);
            let marked = Arc::clone(&marked);
            thread::spawn(move || queue.wake_marked(|| marked.store(false, Ordering::Release)))
        };
        assert!(waiter.join().unwrap());
        notifier.join().unwrap();

        assert_ne!(queue.registration.was_woken(), marked.load(Ordering::Acquire));
    });
}

#[test]
fn mutex_never_admits_two_owners() {
    loom::model(|| {
        let locked = Arc::new(AtomicBool::new(false));
        let owners = Arc::new(AtomicUsize::new(0));

        let workers = (0..2)
            .map(|_| {
                let locked = Arc::clone(&locked);
                let owners = Arc::clone(&owners);
                thread::spawn(move || {
                    acquire_flag(&locked);
                    assert_eq!(owners.fetch_add(1, Ordering::SeqCst), 0);
                    thread::yield_now();
                    assert_eq!(owners.fetch_sub(1, Ordering::SeqCst), 1);
                    locked.store(false, Ordering::Release);
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(owners.load(Ordering::Relaxed), 0);
        assert!(!locked.load(Ordering::Relaxed));
    });
}

const MUTEX_LOCKED: usize = 1;
const MUTEX_WAITERS: usize = 2;

// Reduced owner cardinality keeps overflow distinct from writer ownership
// while modeling the same registration bit and expected-value parking protocol.
const NATIVE_WAITERS: usize = 1 << 3;
const NATIVE_OWNERS: usize = NATIVE_WAITERS - 1;
const NATIVE_WRITER: usize = NATIVE_OWNERS;
const NATIVE_MAX_READERS: usize = NATIVE_WRITER - 1;

struct NativeRwLock {
    state: AtomicUsize,
    parking: Mutex<()>,
    changed: Condvar,
}

impl NativeRwLock {
    fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            parking: Mutex::new(()),
            changed: Condvar::new(),
        }
    }

    fn try_read(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        while state < NATIVE_MAX_READERS {
            match self
                .state
                .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return true,
                Err(current) => state = current,
            }
        }
        false
    }

    fn read(&self) {
        loop {
            if self.try_read() {
                return;
            }
            let state = self.state.load(Ordering::Relaxed);
            assert_ne!(state & NATIVE_OWNERS, NATIVE_MAX_READERS);
            if state >= NATIVE_MAX_READERS
                && self
                    .state
                    .compare_exchange(state, state | NATIVE_WAITERS, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                self.wait(state | NATIVE_WAITERS);
            }
        }
    }

    fn write(&self) {
        if self
            .state
            .compare_exchange(0, NATIVE_WRITER, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & NATIVE_OWNERS == 0 {
                match self
                    .state
                    .compare_exchange_weak(state, NATIVE_WRITER | NATIVE_WAITERS, Ordering::Acquire, Ordering::Relaxed)
                {
                    Ok(_) => return,
                    Err(current) => state = current,
                }
            } else {
                match self
                    .state
                    .compare_exchange(state, state | NATIVE_WAITERS, Ordering::Relaxed, Ordering::Relaxed)
                {
                    Ok(_) => {
                        self.wait(state | NATIVE_WAITERS);
                        state = self.state.load(Ordering::Relaxed);
                    }
                    Err(current) => state = current,
                }
            }
        }
    }

    fn read_unlock(&self) {
        if self.state.fetch_sub(1, Ordering::Release) == NATIVE_WAITERS + 1
            && self
                .state
                .compare_exchange(NATIVE_WAITERS, 0, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            self.wake_all();
        }
    }

    fn write_unlock(&self) {
        if self.state.swap(0, Ordering::Release) & NATIVE_WAITERS != 0 {
            self.wake_all();
        }
    }

    fn wait(&self, expected: usize) {
        // Model the OS's atomic value comparison and wait registration.
        let parking = self.parking.lock().unwrap();
        if self.state.load(Ordering::Relaxed) == expected {
            drop(self.changed.wait(parking).unwrap());
        }
    }

    fn wake_all(&self) {
        let _parking = self.parking.lock().unwrap();
        self.changed.notify_all();
    }
}

#[test]
fn native_rwlock_writer_excludes_readers_and_publishes_updates() {
    loom::model(|| {
        let lock = Arc::new(NativeRwLock::new());
        let fields = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
        let writer = {
            let lock = Arc::clone(&lock);
            let fields = Arc::clone(&fields);
            thread::spawn(move || {
                lock.write();
                fields.0.store(1, Ordering::Relaxed);
                thread::yield_now();
                fields.1.store(1, Ordering::Relaxed);
                lock.write_unlock();
            })
        };
        lock.read();
        let first = fields.0.load(Ordering::Relaxed);
        thread::yield_now();
        let second = fields.1.load(Ordering::Relaxed);
        assert_eq!(first, second);
        lock.read_unlock();
        writer.join().unwrap();
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn native_rwlock_last_reader_cannot_lose_a_waiting_writer() {
    loom::model(|| {
        let lock = Arc::new(NativeRwLock::new());
        lock.read();
        lock.read();
        let writer = {
            let lock = Arc::clone(&lock);
            thread::spawn(move || {
                lock.write();
                lock.write_unlock();
            })
        };
        lock.read_unlock();
        thread::yield_now();
        lock.read_unlock();
        writer.join().unwrap();
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn native_rwlock_two_writers_never_overlap() {
    loom::model(|| {
        let lock = Arc::new(NativeRwLock::new());
        let owners = Arc::new(AtomicUsize::new(0));
        let writer = {
            let lock = Arc::clone(&lock);
            let owners = Arc::clone(&owners);
            thread::spawn(move || {
                lock.write();
                assert_eq!(owners.fetch_add(1, Ordering::SeqCst), 0);
                thread::yield_now();
                assert_eq!(owners.fetch_sub(1, Ordering::SeqCst), 1);
                lock.write_unlock();
            })
        };
        lock.write();
        assert_eq!(owners.fetch_add(1, Ordering::SeqCst), 0);
        thread::yield_now();
        assert_eq!(owners.fetch_sub(1, Ordering::SeqCst), 1);
        lock.write_unlock();
        writer.join().unwrap();
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn native_rwlock_mixed_waiters_all_complete_after_writer_release() {
    // The three-thread case bounds preemptions; two-party exclusion,
    // publication and last-reader registration above remain exhaustive.
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(2);
    model.check(|| {
        let lock = Arc::new(NativeRwLock::new());
        lock.write();
        let reader = {
            let lock = Arc::clone(&lock);
            thread::spawn(move || {
                lock.read();
                lock.read_unlock();
            })
        };
        let writer = {
            let lock = Arc::clone(&lock);
            thread::spawn(move || {
                lock.write();
                lock.write_unlock();
            })
        };
        lock.write_unlock();
        reader.join().unwrap();
        writer.join().unwrap();
        assert_eq!(lock.state.load(Ordering::Relaxed), 0);
    });
}

struct NativeCondition {
    generation: AtomicUsize,
    waiters: AtomicUsize,
    parking: Mutex<()>,
    changed: Condvar,
}

impl NativeCondition {
    fn new() -> Self {
        Self {
            generation: AtomicUsize::new(0),
            waiters: AtomicUsize::new(0),
            parking: Mutex::new(()),
            changed: Condvar::new(),
        }
    }

    fn wait<'a>(&self, mutex: &'a Mutex<usize>, guard: loom::sync::MutexGuard<'a, usize>) -> loom::sync::MutexGuard<'a, usize> {
        let generation = self.generation.load(Ordering::SeqCst);
        self.waiters.fetch_add(1, Ordering::SeqCst);
        drop(guard);
        let parking = self.parking.lock().unwrap();
        // The parking lock models the OS's atomic expected-value check and park.
        if self.generation.load(Ordering::Relaxed) == generation {
            drop(self.changed.wait(parking).unwrap());
        } else {
            drop(parking);
        }
        assert!(self.waiters.fetch_sub(1, Ordering::SeqCst) > 0);
        mutex.lock().unwrap()
    }

    fn notify(&self, all: bool) {
        if self.waiters.load(Ordering::SeqCst) != 0 {
            self.generation.fetch_add(1, Ordering::SeqCst);
            let _parking = self.parking.lock().unwrap();
            if all {
                self.changed.notify_all();
            } else {
                self.changed.notify_one();
            }
        }
    }
}

#[test]
fn native_condvar_notification_cannot_miss_a_registering_waiter() {
    loom::model(|| {
        let condition = Arc::new(NativeCondition::new());
        let predicate = Arc::new(Mutex::new(0));
        condition.notify(false);
        assert_eq!(condition.generation.load(Ordering::SeqCst), 0);
        let waiter = {
            let condition = Arc::clone(&condition);
            let predicate = Arc::clone(&predicate);
            thread::spawn(move || {
                let mut guard = predicate.lock().unwrap();
                while *guard == 0 {
                    guard = condition.wait(&predicate, guard);
                }
                assert_eq!(*guard, 7);
            })
        };
        *predicate.lock().unwrap() = 7;
        condition.notify(false);
        waiter.join().unwrap();
        assert_eq!(condition.waiters.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn native_condvar_notify_all_releases_registered_and_arriving_waiters() {
    // Bound only this three-thread case; the two-party registration model above
    // explores the default exhaustive state space.
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(2);
    model.check(|| {
        let condition = Arc::new(NativeCondition::new());
        let predicate = Arc::new(Mutex::new(0));
        let waiters = (0..2)
            .map(|_| {
                let condition = Arc::clone(&condition);
                let predicate = Arc::clone(&predicate);
                thread::spawn(move || {
                    let mut guard = predicate.lock().unwrap();
                    while *guard == 0 {
                        guard = condition.wait(&predicate, guard);
                    }
                })
            })
            .collect::<Vec<_>>();
        *predicate.lock().unwrap() = 1;
        condition.notify(true);
        for waiter in waiters {
            waiter.join().unwrap();
        }
        assert_eq!(condition.waiters.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn address_mutex_never_admits_two_owners() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(0));
        let owners = Arc::new(AtomicUsize::new(0));
        let workers = (0..2)
            .map(|_| {
                let state = Arc::clone(&state);
                let owners = Arc::clone(&owners);
                thread::spawn(move || {
                    if state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
                        while state.swap(2, Ordering::Acquire) != 0 {
                            thread::yield_now();
                        }
                    }
                    assert_eq!(owners.fetch_add(1, Ordering::SeqCst), 0);
                    thread::yield_now();
                    assert_eq!(owners.fetch_sub(1, Ordering::SeqCst), 1);
                    state.swap(0, Ordering::Release);
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!((state.load(Ordering::Relaxed), owners.load(Ordering::Relaxed)), (0, 0));
    });
}

#[test]
fn address_mutex_registration_cannot_lose_release() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(1));
        let registration = Arc::new(Registration::new());
        let waiter = {
            let state = Arc::clone(&state);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                if state.swap(2, Ordering::Acquire) == 0 {
                    false
                } else {
                    registration.park_if_not_ready(|| state.load(Ordering::Relaxed) != 2)
                }
            })
        };
        let releaser = {
            let state = Arc::clone(&state);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                if state.swap(0, Ordering::Release) == 2 {
                    registration.wake();
                }
            })
        };
        let parked = waiter.join().unwrap();
        releaser.join().unwrap();
        assert!(!parked || registration.was_woken());
    });
}

fn try_acquire_mutex(state: &AtomicUsize) -> bool {
    let mut current = state.load(Ordering::Relaxed);
    while current & MUTEX_LOCKED == 0 {
        match state.compare_exchange_weak(current, current | MUTEX_LOCKED, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
    false
}

#[test]
fn mutex_registration_cannot_lose_unlock_publication() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(MUTEX_LOCKED));
        let payload = Arc::new(AtomicUsize::new(0));
        let registration = Arc::new(Registration::new());

        let waiter = {
            let state = Arc::clone(&state);
            let payload = Arc::clone(&payload);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                let parked = if try_acquire_mutex(&state) {
                    false
                } else {
                    registration.park_if_not_ready_marked(
                        || {
                            state.fetch_or(MUTEX_WAITERS, Ordering::Release);
                        },
                        || try_acquire_mutex(&state),
                        || {
                            state.fetch_and(!MUTEX_WAITERS, Ordering::Release);
                        },
                    )
                };
                if parked {
                    while !registration.was_woken() {
                        thread::yield_now();
                    }
                    while !try_acquire_mutex(&state) {
                        thread::yield_now();
                    }
                }
                let observed = payload.load(Ordering::Relaxed);
                state.fetch_and(!MUTEX_LOCKED, Ordering::Release);
                observed
            })
        };
        let unlocker = {
            let state = Arc::clone(&state);
            let payload = Arc::clone(&payload);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                payload.store(7, Ordering::Relaxed);
                let previous = state.fetch_and(!MUTEX_LOCKED, Ordering::Release);
                if previous & MUTEX_WAITERS != 0 {
                    registration.wake_marked(|| {
                        state.fetch_and(!MUTEX_WAITERS, Ordering::Release);
                    });
                }
            })
        };

        assert_eq!(waiter.join().unwrap(), 7);
        unlocker.join().unwrap();
    });
}

const RW_WRITER: usize = 1 << (usize::BITS - 1);
const RW_WAITERS: usize = 1 << (usize::BITS - 2);
const RW_READERS: usize = RW_WAITERS - 1;

fn acquire_read(state: &AtomicUsize) {
    loop {
        let previous = state.fetch_add(1, Ordering::Acquire);
        if previous & RW_WRITER == 0 && previous & RW_READERS != RW_READERS {
            return;
        }
        state.fetch_sub(1, Ordering::Release);
        thread::yield_now();
    }
}

fn acquire_write(state: &AtomicUsize) {
    while state
        .compare_exchange_weak(0, RW_WRITER, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        thread::yield_now();
    }
}

fn try_acquire_marked_write(state: &AtomicUsize) -> bool {
    state
        .compare_exchange(RW_WAITERS, RW_WAITERS | RW_WRITER, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
}

#[test]
fn rwlock_never_overlaps_readers_and_writer() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(0));
        let active_readers = Arc::new(AtomicUsize::new(0));
        let active_writer = Arc::new(AtomicBool::new(false));

        let reader = {
            let state = Arc::clone(&state);
            let active_readers = Arc::clone(&active_readers);
            let active_writer = Arc::clone(&active_writer);
            thread::spawn(move || {
                acquire_read(&state);
                active_readers.fetch_add(1, Ordering::SeqCst);
                assert!(!active_writer.load(Ordering::SeqCst));
                thread::yield_now();
                active_readers.fetch_sub(1, Ordering::SeqCst);
                state.fetch_sub(1, Ordering::Release);
            })
        };
        let writer = {
            let state = Arc::clone(&state);
            let active_readers = Arc::clone(&active_readers);
            let active_writer = Arc::clone(&active_writer);
            thread::spawn(move || {
                acquire_write(&state);
                assert!(!active_writer.swap(true, Ordering::SeqCst));
                assert_eq!(active_readers.load(Ordering::SeqCst), 0);
                thread::yield_now();
                active_writer.store(false, Ordering::SeqCst);
                state.fetch_and(!RW_WRITER, Ordering::Release);
            })
        };

        reader.join().unwrap();
        writer.join().unwrap();
        assert_eq!(state.load(Ordering::Acquire), 0);
    });
}

#[test]
fn rwlock_last_reader_wakes_a_marked_writer() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(1));
        let payload = Arc::new(AtomicUsize::new(0));
        let registration = Arc::new(Registration::new());

        let writer = {
            let state = Arc::clone(&state);
            let payload = Arc::clone(&payload);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                let parked = registration.park_if_not_ready_marked(
                    || {
                        state.fetch_or(RW_WAITERS, Ordering::Release);
                    },
                    || try_acquire_marked_write(&state),
                    || {
                        state.fetch_and(!RW_WAITERS, Ordering::Release);
                    },
                );
                if parked {
                    while !registration.was_woken() {
                        thread::yield_now();
                    }
                    acquire_write(&state);
                }
                let observed = payload.load(Ordering::Relaxed);
                state.fetch_and(!RW_WRITER, Ordering::Release);
                observed
            })
        };
        let reader = {
            let state = Arc::clone(&state);
            let payload = Arc::clone(&payload);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                payload.store(11, Ordering::Relaxed);
                let previous = state.fetch_sub(1, Ordering::Release);
                if previous & RW_READERS == 1 && previous & RW_WAITERS != 0 {
                    registration.wake_marked(|| {
                        state.fetch_and(!RW_WAITERS, Ordering::Release);
                    });
                }
            })
        };

        assert_eq!(writer.join().unwrap(), 11);
        reader.join().unwrap();
        assert_eq!(state.load(Ordering::Acquire), 0);
    });
}

#[test]
fn condvar_notification_cannot_race_past_registration() {
    loom::model(|| {
        let generation = Arc::new(AtomicUsize::new(0));
        let waiting = Arc::new(AtomicBool::new(false));
        let predicate = Arc::new(Mutex::new(false));
        let registration = Arc::new(Registration::new());

        let waiter = {
            let generation = Arc::clone(&generation);
            let waiting = Arc::clone(&waiting);
            let predicate = Arc::clone(&predicate);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                assert!(!*predicate.lock().unwrap());
                let observed = generation.load(Ordering::Acquire);
                waiting.store(true, Ordering::Release);
                let parked = registration.park_if_not_ready(|| generation.load(Ordering::Acquire) != observed);
                if parked {
                    while !registration.was_woken() {
                        thread::yield_now();
                    }
                }
                assert!(*predicate.lock().unwrap());
            })
        };
        let notifier = {
            let generation = Arc::clone(&generation);
            let waiting = Arc::clone(&waiting);
            let predicate = Arc::clone(&predicate);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                while !waiting.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                *predicate.lock().unwrap() = true;
                generation.fetch_add(1, Ordering::Release);
                registration.wake();
            })
        };

        waiter.join().unwrap();
        notifier.join().unwrap();
    });
}

const BARRIER_GENERATION_SHIFT: u32 = usize::BITS / 2;
const BARRIER_COUNT_MASK: usize = (1 << BARRIER_GENERATION_SHIFT) - 1;

fn barrier_wait(state: &AtomicUsize, parties: usize) -> bool {
    let previous = loop {
        let current = state.load(Ordering::Acquire);
        let generation = current >> BARRIER_GENERATION_SHIFT;
        let count = current & BARRIER_COUNT_MASK;
        let next = if count + 1 == parties {
            generation.wrapping_add(1) << BARRIER_GENERATION_SHIFT
        } else {
            current + 1
        };
        if state
            .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break current;
        }
    };

    let generation = previous >> BARRIER_GENERATION_SHIFT;
    if (previous & BARRIER_COUNT_MASK) + 1 == parties {
        true
    } else {
        while state.load(Ordering::Acquire) >> BARRIER_GENERATION_SHIFT == generation {
            thread::yield_now();
        }
        false
    }
}

#[test]
fn barrier_releases_each_generation_once() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(0));
        let leaders = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let published = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)]);

        let workers = (0..2)
            .map(|worker_index| {
                let state = Arc::clone(&state);
                let leaders = Arc::clone(&leaders);
                let published = Arc::clone(&published);
                thread::spawn(move || {
                    for (generation, leader_count) in leaders.iter().enumerate() {
                        let own_slot = generation * 2 + worker_index;
                        let peer_slot = generation * 2 + (1 - worker_index);
                        published[own_slot].store(generation + 1, Ordering::Relaxed);
                        if barrier_wait(&state, 2) {
                            leader_count.fetch_add(1, Ordering::Relaxed);
                        }
                        assert_eq!(published[peer_slot].load(Ordering::Relaxed), generation + 1);
                    }
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(state.load(Ordering::Relaxed), 2 << BARRIER_GENERATION_SHIFT);
        assert_eq!(leaders[0].load(Ordering::Relaxed), 1);
        assert_eq!(leaders[1].load(Ordering::Relaxed), 1);
    });
}

#[derive(Debug, Default)]
struct ChannelState {
    value: Option<usize>,
    closed: bool,
}

#[test]
fn channel_send_cannot_race_past_receiver_registration() {
    loom::model(|| {
        let state = Arc::new(Mutex::new(ChannelState::default()));
        let registration = Arc::new(Registration::new());

        let receiver = {
            let state = Arc::clone(&state);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                {
                    let mut state = state.lock().unwrap();
                    if let Some(value) = state.value.take() {
                        return Some(value);
                    }
                    if state.closed {
                        return None;
                    }
                }

                let parked = registration.park_if_not_ready(|| {
                    let state = state.lock().unwrap();
                    state.value.is_some() || state.closed
                });
                if parked {
                    while !registration.was_woken() {
                        thread::yield_now();
                    }
                }
                state.lock().unwrap().value.take()
            })
        };
        let sender = {
            let state = Arc::clone(&state);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                state.lock().unwrap().value = Some(7);
                registration.wake();
            })
        };

        assert_eq!(receiver.join().unwrap(), Some(7));
        sender.join().unwrap();
    });
}

#[test]
fn channel_close_cannot_race_past_receiver_registration() {
    loom::model(|| {
        let state = Arc::new(Mutex::new(ChannelState::default()));
        let registration = Arc::new(Registration::new());

        let receiver = {
            let state = Arc::clone(&state);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                if state.lock().unwrap().closed {
                    return;
                }
                let parked = registration.park_if_not_ready(|| state.lock().unwrap().closed);
                if parked {
                    while !registration.was_woken() {
                        thread::yield_now();
                    }
                }
                assert!(state.lock().unwrap().closed);
            })
        };
        let closer = {
            let state = Arc::clone(&state);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                state.lock().unwrap().closed = true;
                registration.wake();
            })
        };

        receiver.join().unwrap();
        closer.join().unwrap();
    });
}

fn initialize_once(state: &AtomicUsize, value: &AtomicUsize, initializations: &AtomicUsize) -> usize {
    if state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
        initializations.fetch_add(1, Ordering::Relaxed);
        value.store(42, Ordering::Relaxed);
        state.store(2, Ordering::Release);
    } else {
        while state.load(Ordering::Acquire) != 2 {
            thread::yield_now();
        }
    }
    value.load(Ordering::Relaxed)
}

#[test]
fn once_initialization_publishes_exactly_one_value() {
    loom::model(|| {
        let state = Arc::new(AtomicUsize::new(0));
        let value = Arc::new(AtomicUsize::new(0));
        let initializations = Arc::new(AtomicUsize::new(0));

        let workers = (0..2)
            .map(|_| {
                let state = Arc::clone(&state);
                let value = Arc::clone(&value);
                let initializations = Arc::clone(&initializations);
                thread::spawn(move || initialize_once(&state, &value, &initializations))
            })
            .collect::<Vec<_>>();

        for worker in workers {
            assert_eq!(worker.join().unwrap(), 42);
        }
        assert_eq!(initializations.load(Ordering::Relaxed), 1);
    });
}
