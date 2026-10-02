// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task destruction may submit local cleanup work while its worker is still running.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll};

use arty::task::LocalTaskScheduler;
use panic_support::{isolated, runtime};
use testing_aids::TEST_TIMEOUT;

testing_aids::init_tracing!();

struct Cleanup {
    scheduler: LocalTaskScheduler,
    drops: Arc<AtomicUsize>,
    factory: mpsc::Sender<()>,
    finished: mpsc::Sender<()>,
    panic_poll: bool,
}

impl Future for Cleanup {
    type Output = u32;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<u32> {
        if self.panic_poll {
            panic!("task poll before local cleanup submission");
        }
        Poll::Ready(42)
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        let factory = self.factory.clone();
        let finished = self.finished.clone();
        let child = self.scheduler.spawn(move || {
            factory.send(()).unwrap();
            let value = std::rc::Rc::new(7);
            async move {
                assert_eq!(*value, 7);
                finished.send(()).unwrap();
            }
        });
        drop(child);
    }
}

#[test]
fn remote_future_drop_can_spawn_local_cleanup_after_success_or_panic() {
    isolated("remote_future_drop_can_spawn_local_cleanup_after_success_or_panic", || {
        for panic_poll in [false, true] {
            let runtime = runtime();
            let drops = Arc::new(AtomicUsize::new(0));
            let captured = Arc::clone(&drops);
            let (factory, invoked) = mpsc::channel();
            let (finished, completed) = mpsc::channel();
            runtime
                .scheduler()
                .block_on(async move |cx| {
                    let join = cx.scheduler().spawn(move |child| Cleanup {
                        scheduler: child.local_scheduler().unwrap(),
                        drops: captured,
                        factory,
                        finished,
                        panic_poll,
                    });
                    if panic_poll {
                        assert!(join.await.unwrap_err().is_panic());
                    } else {
                        assert_eq!(join.await.unwrap(), 42);
                    }
                })
                .unwrap();
            invoked.recv_timeout(TEST_TIMEOUT).unwrap();
            completed.recv_timeout(TEST_TIMEOUT).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            runtime.stop().unwrap();
        }
    });
}

#[test]
fn local_future_drop_can_spawn_local_cleanup_after_success_or_panic() {
    isolated("local_future_drop_can_spawn_local_cleanup_after_success_or_panic", || {
        for panic_poll in [false, true] {
            let runtime = runtime();
            let drops = Arc::new(AtomicUsize::new(0));
            let captured = Arc::clone(&drops);
            let (factory, invoked) = mpsc::channel();
            let (finished, completed) = mpsc::channel();
            runtime
                .scheduler()
                .block_on(async move |cx| {
                    let scheduler = cx.local_scheduler().unwrap();
                    let captured_scheduler = scheduler.clone();
                    let join = scheduler.spawn(move || Cleanup {
                        scheduler: captured_scheduler,
                        drops: captured,
                        factory,
                        finished,
                        panic_poll,
                    });
                    if panic_poll {
                        assert!(join.await.unwrap_err().is_panic());
                    } else {
                        assert_eq!(join.await.unwrap(), 42);
                    }
                })
                .unwrap();
            invoked.recv_timeout(TEST_TIMEOUT).unwrap();
            completed.recv_timeout(TEST_TIMEOUT).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            runtime.stop().unwrap();
        }
    });
}
