// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(all(feature = "rt", feature = "macros"))]

testing_aids::init_tracing!();

use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Builtins;
use tick::{ClockControl, FutureExt};

const TIMER_COUNT: usize = 8;
const CONTROLLED_TIMER_STEP: Duration = Duration::from_secs(1);

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
fn timer_callback_direct_submission_wakes_waiting_same_worker() {
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

    struct TimerWakeHarness {
        setup_delay: Pin<Box<tick::Delay>>,
        callback_delay: Pin<Box<tick::Delay>>,
        callback_waker: Waker,
        registration: Option<mpsc::Sender<thread::ThreadId>>,
        control: ClockControl,
        setup_wake_drained: bool,
    }

    impl Future for TimerWakeHarness {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            if let Some(registration) = this.registration.take() {
                assert_eq!(this.setup_delay.as_mut().poll(cx), Poll::Pending);
                assert_eq!(
                    this.callback_delay.as_mut().poll(&mut Context::from_waker(&this.callback_waker)),
                    Poll::Pending
                );
                registration.send(thread::current().id()).unwrap();
                let _configured = this.control.clone().auto_advance(CONTROLLED_TIMER_STEP);
                return Poll::Pending;
            }

            if !this.setup_wake_drained {
                assert_eq!(this.setup_delay.as_mut().poll(cx), Poll::Ready(()));
                this.setup_wake_drained = true;
            }
            Poll::Pending
        }
    }

    let control = ClockControl::new();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .clock(control.clone())
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (timer_registered, registration) = mpsc::channel();
    let (task_ran, completion) = mpsc::channel();
    let _timer_task = scheduler.spawn(move |cx| {
        // The first expiry wakes this setup task so the worker consumes every setup notification.
        // The second then depends exclusively on the direct submission notifying the worker.
        TimerWakeHarness {
            setup_delay: Box::pin(cx.clock().delay(CONTROLLED_TIMER_STEP)),
            callback_delay: Box::pin(cx.clock().delay(CONTROLLED_TIMER_STEP * 2)),
            callback_waker: Waker::from(Arc::new(SpawnOnWake {
                scheduler: cx.scheduler().clone(),
                task_ran,
            })),
            registration: Some(timer_registered),
            control,
            setup_wake_drained: false,
        }
    });
    let worker_thread = registration.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();

    let observed_threads = completion.recv_timeout(testing_aids::TEST_TIMEOUT);
    runtime.stop().unwrap();

    assert_eq!(
        observed_threads.expect("the timer callback's direct same-worker submission must wake the waiting worker"),
        (worker_thread, worker_thread)
    );
}
