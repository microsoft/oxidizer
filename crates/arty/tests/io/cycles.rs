// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Driver cycles, ordering, wake-up, and shutdown behavior.

use std::future::Future as _;
use std::task::Poll;

use crate::*;

const DRIVER_PROGRESS_TIMEOUT: Duration = Duration::from_millis(500);

fn cycle_failure<M: Marker>(role_plan: RolePlan)
where
    TestContext<M>: IoContext<ContextOptions = ScenarioOptions>,
{
    let _serial = serialize_invariant_test();
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(role_plan);
    let primary = Scenario::options(RolePlan::PreferPrimary);

    worker
        .spawn({
            let scenario = scenario.clone();
            async move |cx| {
                if matches!(role_plan, RolePlan::Secondary) {
                    let _ = cx.io().context::<TestContext<OrderedPrimary>>(&primary);
                }
                let _ = cx.io().context::<TestContext<M>>(&scenario);
            }
        })
        .join()
        .unwrap();
    let cancelled = worker.spawn(async |_| std::future::pending::<()>().await);
    scenario.fail_normal_cycle.store(true, Ordering::SeqCst);
    scenario.wake_owners();
    let cancellation = cancelled.join().unwrap_err();

    assert_eq!(cancellation.source().unwrap().to_string(), "normal driver cycle failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "normal driver cycle failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn primary_cycle_failure_stops_runtime() {
    cycle_failure::<PrimaryCycleFailure>(RolePlan::PreferPrimary);
}

#[test]
fn secondary_cycle_failure_stops_runtime() {
    cycle_failure::<SecondaryCycleFailure>(RolePlan::Secondary);
}

#[test]
fn secondary_only_worker_is_woken_for_another_cycle() {
    let _serial = serialize_invariant_test();
    let runtime = runtime(2);
    let workers = worker_schedulers(&runtime, 2);
    let primary = Scenario::options(RolePlan::PreferPrimary);
    let secondary = Scenario::options(RolePlan::Secondary);

    workers[0]
        .spawn(async move |cx| {
            let _ = cx.io().context::<TestContext<OrderedPrimary>>(&primary);
        })
        .join()
        .unwrap();
    workers[1]
        .spawn({
            let secondary = secondary.clone();
            async move |cx| {
                let _ = cx.io().context::<TestContext<SecondaryCycleFailure>>(&secondary);
            }
        })
        .join()
        .unwrap();

    let cancelled = workers[0].spawn(async |_| std::future::pending::<()>().await);
    secondary.fail_normal_cycle.store(true, Ordering::SeqCst);
    secondary.wake_owners();
    let cancellation = cancelled.join().unwrap_err();
    assert_eq!(cancellation.source().unwrap().to_string(), "normal driver cycle failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "normal driver cycle failed");
    assert_eq!(secondary.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn cycle_failure_remains_primary_when_driver_shutdown_also_fails() {
    let _serial = serialize_invariant_test();
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_shutdown.store(true, Ordering::SeqCst);

    worker
        .spawn({
            let scenario = scenario.clone();
            async move |cx| {
                let _ = cx.io().context::<TestContext<CycleAndShutdownFailure>>(&scenario);
            }
        })
        .join()
        .unwrap();
    let cancelled = worker.spawn(async |_| std::future::pending::<()>().await);
    scenario.fail_normal_cycle.store(true, Ordering::SeqCst);
    scenario.wake_owners();
    let cancellation = cancelled.join().unwrap_err();

    assert_eq!(cancellation.source().unwrap().to_string(), "normal driver cycle failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "normal driver cycle failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn secondaries_execute_before_the_primary() {
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::new(AtomicBool::new(false));
    let primary = Scenario::with_log_and_recording(RolePlan::PreferPrimary, Arc::clone(&log), Arc::clone(&recording));
    let secondary = Scenario::with_log_and_recording(RolePlan::Secondary, Arc::clone(&log), Arc::clone(&recording));

    worker
        .spawn({
            async move |cx| {
                let _ = cx.io().context::<TestContext<OrderedPrimary>>(&primary);
                let _ = cx.io().context::<TestContext<OrderedSecondary>>(&secondary);
            }
        })
        .join()
        .unwrap();

    log.lock().unwrap().clear();
    worker
        .spawn({
            let recording = Arc::clone(&recording);
            async move |_| recording.store(true, Ordering::SeqCst)
        })
        .join()
        .unwrap();
    wait_until(|| log.lock().unwrap().len() >= 2);
    assert_eq!(&log.lock().unwrap()[..2], &[OrderedSecondary::NAME, OrderedPrimary::NAME]);
    runtime.stop().unwrap();
}

#[test]
fn primary_wait_is_interrupted_by_new_runtime_work() {
    let runtime = runtime(1);
    let control = support::runtime_control(&runtime);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_primary.store(true, Ordering::SeqCst);
    scenario.block_primary_until_woken.store(true, Ordering::SeqCst);

    worker
        .spawn({
            let scenario = scenario.clone();
            async move |cx| {
                let _ = cx.io().context::<TestContext<BlockingPrimary>>(&scenario);
            }
        })
        .join()
        .unwrap();
    wait_until(|| scenario.primary_wait_entries.load(Ordering::SeqCst) > 0);

    let (completed, completion) = mpsc::channel();
    worker.spawn(async move |_| completed.send(()).unwrap());
    let result = completion.recv_timeout(DRIVER_PROGRESS_TIMEOUT);
    if let Err(error) = result {
        control.request_stop();
        scenario.wake_driver_interrupts();
        runtime.stop().unwrap();
        panic!("new runtime work did not interrupt the primary wait: {error}");
    }
    assert!(scenario.driver_wakes.load(Ordering::SeqCst) > 0);
    assert_eq!(scenario.primary_wait_timeouts.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}

#[test]
fn notification_during_first_driver_construction_prevents_primary_wait() {
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(1, Ordering::SeqCst);
    scenario.block_primary.store(true, Ordering::SeqCst);
    scenario.block_primary_until_woken.store(true, Ordering::SeqCst);
    let initialization = worker.spawn({
        let scenario = scenario.clone();
        async move |cx| {
            let _ = cx.io().context::<TestContext<BlockingPrimary>>(&scenario);
            std::future::pending::<()>().await;
        }
    });
    scenario.wait_until_driver_blocked();
    let (completed, completion) = mpsc::channel();
    let queued = worker.spawn(async move |_| completed.send(()).unwrap());

    scenario.release_driver();
    let result = completion.recv_timeout(DRIVER_PROGRESS_TIMEOUT);
    if result.is_err() {
        scenario.wake_driver_interrupts();
    }

    result.unwrap();
    queued.join().unwrap();
    runtime.stop().unwrap();
    assert!(initialization.join().unwrap_err().is_shutdown());
}

#[test]
fn controlled_timer_fires_while_a_primary_driver_is_initialized() {
    let control = tick::ClockControl::new();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .clock(control.clone())
        .build()
        .unwrap();
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_primary.store(true, Ordering::SeqCst);
    let (registered, registration) = mpsc::channel();
    let (completed, completion) = mpsc::channel();
    let task = worker.spawn(async move |cx| {
        let _ = cx.io().context::<TestContext<BlockingPrimary>>(&scenario);
        let mut delay = Box::pin(cx.clock().delay(Duration::from_secs(1)));
        let mut registered = Some(registered);
        std::future::poll_fn(move |cx| match delay.as_mut().poll(cx) {
            Poll::Pending => {
                if let Some(registered) = registered.take() {
                    registered.send(()).unwrap();
                }
                Poll::Pending
            }
            Poll::Ready(()) => Poll::Ready(()),
        })
        .await;
        completed.send(()).unwrap();
    });

    registration.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    control.advance(Duration::from_secs(1));
    completion.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    task.join().unwrap();
    runtime.stop().unwrap();
}

#[test]
fn shutdown_failures_do_not_skip_other_drivers() {
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let primary = Scenario::options(RolePlan::PreferPrimary);
    let secondary = Scenario::options(RolePlan::Secondary);
    primary.fail_shutdown.store(true, Ordering::SeqCst);
    secondary.fail_shutdown.store(true, Ordering::SeqCst);

    worker
        .spawn({
            let primary = primary.clone();
            let secondary = secondary.clone();
            async move |cx| {
                let _ = cx.io().context::<TestContext<ShutdownPrimary>>(&primary);
                let _ = cx.io().context::<TestContext<ShutdownSecondary>>(&secondary);
            }
        })
        .join()
        .unwrap();

    runtime.stop().unwrap();
    assert_eq!(primary.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(secondary.shutdowns.load(Ordering::SeqCst), 1);
    assert!(primary.driver_interrupts.lock().unwrap().is_empty());
    assert!(secondary.driver_interrupts.lock().unwrap().is_empty());
}

#[test]
fn task_cancellation_runs_before_driver_shutdown() {
    struct ObserveDriverOnDrop {
        scenario: ScenarioOptions,
        dropped_tx: mpsc::Sender<bool>,
    }

    impl Drop for ObserveDriverOnDrop {
        fn drop(&mut self) {
            self.dropped_tx.send(self.scenario.shutdowns.load(Ordering::SeqCst) == 0).unwrap();
        }
    }

    let _serial = serialize_invariant_test();
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    let (started_tx, started_rx) = mpsc::channel();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let task = worker.spawn({
        let scenario = scenario.clone();
        async move |cx| {
            let _ = cx.io().context::<TestContext<ShutdownTaskCleanup>>(&scenario);
            let _observe = ObserveDriverOnDrop { scenario, dropped_tx };
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        }
    });
    started_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    drop(task);

    runtime.stop().unwrap();
    assert!(dropped_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap());
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}
