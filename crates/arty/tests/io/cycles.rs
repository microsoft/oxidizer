// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Driver cycles, ordering, wake-up, and shutdown behavior.

use super::*;

fn cycle_failure<M: Marker>(role_plan: RolePlan, expected_role: &'static str)
where
    TestContext<M>: IoContext<ContextOptions = ScenarioOptions>,
{
    let _serial = serialize_invariant_test();
    let (runtime, processor) = runtime_with_telemetry(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(role_plan);
    let primary = Scenario::options(RolePlan::PreferPrimary);

    worker
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                if matches!(role_plan, RolePlan::Secondary) {
                    let _ = builtins.io().context::<TestContext<OrderedPrimary>>(&primary);
                }
                let _ = builtins.io().context::<TestContext<M>>(&scenario);
            }
        })
        .join()
        .unwrap();
    scenario.fail_normal_cycle.store(true, Ordering::SeqCst);
    scenario.wake_owners();
    wait_until(|| event_count(&processor, "arty.rt.io.driver.cycle_failed") == 1);

    assert_eq!(runtime.stop().unwrap_err().to_string(), "normal driver cycle failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(event_count(&processor, "arty.rt.io.driver.cycle_failed"), 1);
    let events = processor.events();
    let event = events
        .iter()
        .find(|event| event.name() == "arty.rt.io.driver.cycle_failed")
        .unwrap();
    assert_eq!(
        event.dimensions().into_iter().find(|(name, _)| name == "io.driver.role").unwrap().1,
        expected_role.into()
    );
}
#[test]
fn primary_cycle_failure_stops_runtime() {
    cycle_failure::<PrimaryCycleFailure>(RolePlan::PreferPrimary, "primary");
}
#[test]
fn secondary_cycle_failure_stops_runtime() {
    cycle_failure::<SecondaryCycleFailure>(RolePlan::Secondary, "secondary");
}

#[test]
fn secondary_only_worker_is_woken_for_another_cycle() {
    let _serial = serialize_invariant_test();
    let (runtime, processor) = runtime_with_telemetry(2);
    let workers = worker_schedulers(&runtime, 2);
    let primary = Scenario::options(RolePlan::PreferPrimary);
    let secondary = Scenario::options(RolePlan::Secondary);

    workers[0]
        .spawn({
            let primary = primary.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<OrderedPrimary>>(&primary);
            }
        })
        .join()
        .unwrap();
    workers[1]
        .spawn({
            let secondary = secondary.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<SecondaryCycleFailure>>(&secondary);
            }
        })
        .join()
        .unwrap();

    secondary.fail_normal_cycle.store(true, Ordering::SeqCst);
    secondary.wake_owners();
    wait_until(|| event_count(&processor, "arty.rt.io.driver.cycle_failed") == 1);

    assert_eq!(runtime.stop().unwrap_err().to_string(), "normal driver cycle failed");
    assert_eq!(secondary.shutdowns.load(Ordering::SeqCst), 1);
}
#[test]
fn cycle_failure_remains_primary_when_driver_shutdown_also_fails() {
    let _serial = serialize_invariant_test();
    let (runtime, processor) = runtime_with_telemetry(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_shutdown.store(true, Ordering::SeqCst);

    worker
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<CycleAndShutdownFailure>>(&scenario);
            }
        })
        .join()
        .unwrap();
    scenario.fail_normal_cycle.store(true, Ordering::SeqCst);
    scenario.wake_owners();
    wait_until(|| event_count(&processor, "arty.rt.io.driver.cycle_failed") == 1);

    assert_eq!(runtime.stop().unwrap_err().to_string(), "normal driver cycle failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(event_count(&processor, "arty.rt.io.driver.cycle_failed"), 1);
    assert_eq!(event_count(&processor, "arty.rt.io.driver.shutdown_failed"), 1);
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
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<OrderedPrimary>>(&primary);
                let _ = builtins.io().context::<TestContext<OrderedSecondary>>(&secondary);
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
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_primary.store(true, Ordering::SeqCst);

    worker
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<BlockingPrimary>>(&scenario);
            }
        })
        .join()
        .unwrap();
    wait_until(|| scenario.normal_cycles.load(Ordering::SeqCst) > 0);

    let (completed, completion) = mpsc::channel();
    worker.spawn(async move |_| completed.send(()).unwrap());
    completion.recv_timeout(Duration::from_millis(500)).unwrap();
    assert!(scenario.driver_wakes.load(Ordering::SeqCst) > 0);
    runtime.stop().unwrap();
}

#[test]
fn timers_fire_while_a_primary_driver_is_initialized() {
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_primary.store(true, Ordering::SeqCst);
    let (completed, completion) = mpsc::channel();

    worker.spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let _ = builtins.io().context::<TestContext<BlockingPrimary>>(&scenario);
            builtins.clock().delay(Duration::from_millis(20)).await;
            completed.send(()).unwrap();
        }
    });

    completion.recv_timeout(Duration::from_millis(500)).unwrap();
    runtime.stop().unwrap();
}
#[test]
fn shutdown_failures_are_diagnostic_and_do_not_skip_other_drivers() {
    let (runtime, processor) = runtime_with_telemetry(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let primary = Scenario::options(RolePlan::PreferPrimary);
    let secondary = Scenario::options(RolePlan::Secondary);
    primary.fail_shutdown.store(true, Ordering::SeqCst);
    secondary.fail_shutdown.store(true, Ordering::SeqCst);

    worker
        .spawn({
            let primary = primary.clone();
            let secondary = secondary.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<ShutdownPrimary>>(&primary);
                let _ = builtins.io().context::<TestContext<ShutdownSecondary>>(&secondary);
            }
        })
        .join()
        .unwrap();

    runtime.stop().unwrap();
    assert_eq!(primary.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(secondary.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(event_count(&processor, "arty.rt.io.driver.shutdown_failed"), 2);
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
        async move |builtins| {
            let _ = builtins.io().context::<TestContext<ShutdownTaskCleanup>>(&scenario);
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
