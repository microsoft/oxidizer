// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(all(feature = "rt", feature = "macros"))]

testing_aids::init_tracing!();

use std::future::pending;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Builtins;
use tick::FutureExt;

const TIMER_COUNT: usize = 8;

#[arty::test]
async fn many_timers_with_relocation_ensure_advanced(builtins: Builtins) {
    let scheduler = builtins.scheduler();
    // Ensure each relocated task keeps access to the runtime clock.
    let handles: Vec<_> = (0..TIMER_COUNT)
        .map(|_| {
            scheduler.spawn_anywhere(builtins.clone(), move |builtins| async move {
                println!("delay(pending) - clock: {:?}, thread: {:?}", builtins.clock(), builtins.thread());

                let watch = builtins.clock().stopwatch();
                builtins.clock().delay(Duration::from_millis(50)).await;

                println!("delay(done) - clock: {:?}, thread: {:?}", builtins.clock(), builtins.thread());

                assert!(watch.elapsed().as_millis() >= 50);
            })
        })
        .collect();

    for handle in handles {
        handle.timeout(builtins.clock(), Duration::from_secs(10)).await.unwrap().unwrap();
    }
}

#[arty::test]
async fn many_timers_ensure_advanced(builtins: Builtins) {
    let handles: Vec<_> = (0..TIMER_COUNT)
        .map(|_| {
            builtins
                .scheduler()
                .spawn_anywhere(builtins.clone(), |builtins| builtins.clock().delay(Duration::from_millis(1)))
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}

#[arty::test]
async fn timer_with_relocated_builtins(builtins: Builtins) {
    builtins
        .scheduler()
        .spawn_anywhere(builtins.clone(), |builtins| builtins.clock().delay(Duration::from_millis(10)))
        .await
        .unwrap();
}

#[test]
fn timer_callback_direct_submission_keeps_same_worker_live() {
    struct SpawnOnWake {
        scheduler: arty::task::Scheduler,
        task_ran: mpsc::Sender<(thread::ThreadId, thread::ThreadId)>,
    }

    impl Wake for SpawnOnWake {
        fn wake(self: Arc<Self>) {
            let callback_thread = thread::current().id();
            let task_ran = self.task_ran.clone();
            drop(self.scheduler.spawn(move |_| async move {
                task_ran.send((callback_thread, thread::current().id())).unwrap();
            }));
        }
    }

    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (timer_registered, registration) = mpsc::channel();
    let (task_ran, completion) = mpsc::channel();
    let _timer_task = scheduler.spawn(move |cx| async move {
        let mut delay = Box::pin(cx.clock().delay(Duration::from_millis(20)));
        let timer_waker = Waker::from(Arc::new(SpawnOnWake {
            scheduler: cx.scheduler().clone(),
            task_ran,
        }));
        assert_eq!(delay.as_mut().poll(&mut Context::from_waker(&timer_waker)), Poll::Pending);
        timer_registered.send(()).unwrap();

        pending::<()>().await;
        drop(delay);
    });
    registration.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();

    let (callback_thread, task_thread) = completion
        .recv_timeout(testing_aids::TEST_TIMEOUT)
        .expect("the timer callback's direct same-worker submission must run");
    assert_eq!(task_thread, callback_thread);

    runtime.stop().unwrap();
}
