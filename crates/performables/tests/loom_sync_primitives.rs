// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loom models for the synchronization protocols used by `performables::sync`.
//!
//! These model the production algorithms rather than the production types,
//! because Loom cannot instrument their `std` synchronization primitives.

#![cfg(loom)]
#![allow(clippy::unwrap_used, reason = "poisoning requires a prior model failure")]

use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

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

#[test]
fn mutex_registration_cannot_lose_unlock_publication() {
    loom::model(|| {
        let locked = Arc::new(AtomicBool::new(true));
        let payload = Arc::new(AtomicUsize::new(0));
        let registration = Arc::new(Registration::new());

        let waiter = {
            let locked = Arc::clone(&locked);
            let payload = Arc::clone(&payload);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                let parked = if locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    false
                } else {
                    registration.park_if_not_ready(|| locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok())
                };
                if parked {
                    while !registration.was_woken() {
                        thread::yield_now();
                    }
                    acquire_flag(&locked);
                }
                let observed = payload.load(Ordering::Relaxed);
                locked.store(false, Ordering::Release);
                observed
            })
        };
        let unlocker = {
            let locked = Arc::clone(&locked);
            let payload = Arc::clone(&payload);
            let registration = Arc::clone(&registration);
            thread::spawn(move || {
                payload.store(7, Ordering::Relaxed);
                locked.store(false, Ordering::Release);
                registration.wake();
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
