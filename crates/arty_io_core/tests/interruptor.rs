// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One-shot broadcasts, shared reset, and registration races.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Wake, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::Interruptor;
use static_assertions::assert_impl_all;

assert_impl_all!(Interruptor: Clone, Send, Sync, std::fmt::Debug);

#[derive(Default)]
struct Counter(AtomicUsize);
impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn request_is_one_shot_and_late_registration_is_not_lost() {
    let interruptor = Interruptor::new();
    let first = Arc::new(Counter::default());
    let also_registered = Arc::new(Counter::default());
    let second = Arc::new(Counter::default());
    interruptor.register(Waker::from(Arc::clone(&first)));
    interruptor.register(Waker::from(Arc::clone(&also_registered)));
    assert!(!interruptor.is_requested());
    interruptor.request();
    interruptor.request();
    interruptor.register(Waker::from(Arc::clone(&second)));
    assert_eq!(first.0.load(Ordering::Relaxed), 1);
    assert_eq!(also_registered.0.load(Ordering::Relaxed), 1);
    assert_eq!(second.0.load(Ordering::Relaxed), 1);
    assert!(format!("{interruptor:?}").contains("requested: true"));
}

#[test]
fn retained_handles_notify_the_current_round_after_shared_reset() {
    let current = Interruptor::new();
    let observer = current.clone();
    let first = Arc::new(Counter::default());
    current.register(Waker::from(Arc::clone(&first)));
    observer.request();
    assert!(current.is_requested());
    assert_eq!(first.0.load(Ordering::Relaxed), 1);
    current.request();
    assert_eq!(first.0.load(Ordering::Relaxed), 1);

    current.reset();
    assert!(!current.is_requested());
    assert!(!observer.is_requested());
    let second = Arc::new(Counter::default());
    observer.register(Waker::from(Arc::clone(&second)));
    observer.request();
    assert!(current.is_requested());
    assert_eq!(first.0.load(Ordering::Relaxed), 1);
    assert_eq!(second.0.load(Ordering::Relaxed), 1);
}

#[test]
fn old_broadcast_finishing_after_reset_preserves_new_registrations() {
    struct HeldBroadcast {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Wake for HeldBroadcast {
        fn wake(self: Arc<Self>) {
            // Hold dispatch outside the lock to force the reset/registration overlap.
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }
    let current = Interruptor::new();
    let observer = current.clone();
    let previous = Arc::new(Counter::default());
    for _ in 0..8 {
        current.register(Waker::from(Arc::clone(&previous)));
    }
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    current.register(Waker::from(Arc::new(HeldBroadcast {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    })));
    let broadcaster = observer.clone();
    let thread = thread::spawn(move || broadcaster.request());
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    current.reset();
    let next = Arc::new(Counter::default());
    current.register(Waker::from(Arc::clone(&next)));
    release_tx.send(()).unwrap();
    thread.join().unwrap();

    assert_eq!(previous.0.load(Ordering::Relaxed), 8);
    assert!(!current.is_requested());
    assert_eq!(next.0.load(Ordering::Relaxed), 0);
    observer.request();
    assert_eq!(next.0.load(Ordering::Relaxed), 1);
}
