// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(all(feature = "rt", feature = "macros"))]

use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::task::{Context as PollContext, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Context;
use tick::{ClockControl, FutureExt};

const TIMER_COUNT: usize = 8;
const CONTROLLED_TIMER_STEP: Duration = Duration::from_secs(1);

#[arty::test]
async fn many_timers_with_relocation_ensure_advanced(context: Context) {
    let scheduler = context.scheduler();
    // Make sure that each relocated task keeps access to the runtime clock.
    let handles: Vec<_> = (0..TIMER_COUNT)
        .map(|_| {
            scheduler.spawn_anywhere(context.clone(), move |context| async move {
                println!("delay(pending) - clock: {:?}, thread: {:?}", context.clock(), context.thread());

                let watch = context.clock().stopwatch();
                context.clock().delay(Duration::from_millis(50)).await;

                println!("delay(done) - clock: {:?}, thread: {:?}", context.clock(), context.thread());

                assert!(watch.elapsed().as_millis() >= 50);
            })
        })
        .collect();

    for handle in handles {
        handle.timeout(context.clock(), Duration::from_secs(10)).await.unwrap().unwrap();
    }
}

#[arty::test]
async fn many_timers_ensure_advanced(context: Context) {
    let handles: Vec<_> = (0..TIMER_COUNT)
        .map(|_| {
            context
                .scheduler()
                .spawn_anywhere(context.clone(), |context| context.clock().delay(Duration::from_millis(1)))
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}

#[arty::test]
async fn timer_with_relocated_context(context: Context) {
    context
        .scheduler()
        .spawn_anywhere(context.clone(), |context| context.clock().delay(Duration::from_millis(10)))
        .await
        .unwrap();
}

#[test]
fn controlled_timer_callback_submission_wakes_bound_worker() {
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
        callback_delay: Pin<Box<tick::Delay>>,
        callback_waker: Waker,
        registration: Option<mpsc::Sender<thread::ThreadId>>,
    }

    impl Future for TimerWakeHarness {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut PollContext<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            if let Some(registration) = this.registration.take() {
                assert_eq!(
                    this.callback_delay
                        .as_mut()
                        .poll(&mut PollContext::from_waker(&this.callback_waker)),
                    Poll::Pending
                );
                registration.send(thread::current().id()).unwrap();
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
    let _timer_task = scheduler.spawn(move |cx| TimerWakeHarness {
        callback_delay: Box::pin(cx.clock().delay(CONTROLLED_TIMER_STEP)),
        callback_waker: Waker::from(Arc::new(SpawnOnWake {
            scheduler: cx.scheduler().clone(),
            task_ran,
        })),
        registration: Some(timer_registered),
    });
    let worker_thread = registration.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let control_thread = thread::current().id();

    control.advance(CONTROLLED_TIMER_STEP);

    let observed_threads = completion.recv_timeout(testing_aids::TEST_TIMEOUT);
    runtime.stop().unwrap();

    assert_eq!(
        observed_threads.expect("the controlled timer callback submission must wake its bound worker"),
        (control_thread, worker_thread)
    );
}
