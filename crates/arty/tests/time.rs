// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(all(feature = "rt", feature = "macros"))]

testing_aids::init_tracing!();

use std::sync::{Arc, mpsc};
use std::task::{Context, Wake, Waker};
use std::time::Duration;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Builtins;
use tick::FutureExt;

const TIMER_COUNT: usize = 8;

struct CompletionWake(mpsc::Sender<()>);

impl Wake for CompletionWake {
    fn wake(self: Arc<Self>) {
        _ = self.0.send(());
    }
}

#[test]
#[cfg(feature = "test-util")]
fn first_cross_thread_timer_interrupts_idle_worker_wait() {
    let (idle_wait_started, idle_wait) = mpsc::channel();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .notify_on_idle_wait(idle_wait_started)
        .build()
        .unwrap();
    idle_wait
        .recv_timeout(Duration::from_millis(900))
        .expect("the worker must establish its initial no-deadline notification wait");
    let clock = runtime.scheduler().block_on(async |builtins| builtins.clock().clone()).unwrap();
    idle_wait
        .recv_timeout(Duration::from_millis(900))
        .expect("the worker must consume the submission wake and reestablish its idle wait");
    let (completed, completion) = mpsc::channel();
    let waker = Waker::from(Arc::new(CompletionWake(completed)));
    let mut context = Context::from_waker(&waker);
    let mut delay = Box::pin(clock.delay(Duration::from_millis(20)));

    assert!(delay.as_mut().poll(&mut context).is_pending());
    completion
        .recv_timeout(Duration::from_millis(900))
        .expect("the first cross-thread timer must interrupt the idle worker before the former one-second fallback");

    runtime.stop().unwrap();
}

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
