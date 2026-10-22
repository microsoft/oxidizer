// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Initialization failures and shutdown races.

use crate::*;

#[test]
fn external_initialization_failure_is_returned_and_stops_runtime() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_driver_at.store(1, Ordering::SeqCst);

    let error = {
        let scenario = scenario.clone();
        thread::spawn(move || {
            builtins
                .io()
                .try_context::<TestContext<ExternalConstructionFailure>>(&scenario)
                .unwrap_err()
                .to_string()
        })
        .join()
        .unwrap()
    };

    assert_eq!(error, "driver construction failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "driver construction failed");
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 0);
}

#[test]
fn try_context_reports_driver_construction_failure_and_stops_runtime() {
    let (runtime, processor) = runtime_with_telemetry(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_driver_at.store(1, Ordering::SeqCst);

    let error = worker
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                builtins
                    .io()
                    .try_context::<TestContext<ConstructionFailure>>(&scenario)
                    .unwrap_err()
                    .to_string()
            }
        })
        .join()
        .unwrap();
    assert_eq!(error, "driver construction failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "driver construction failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 0);
    assert_eq!(event_count(&processor, "arty.rt.io.driver.initialization_failed"), 1);
}

#[test]
fn context_panics_after_requesting_shutdown_on_initialization_failure() {
    let runtime = runtime(1);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_driver_at.store(1, Ordering::SeqCst);

    let panicked = worker
        .spawn({
            async move |builtins| {
                catch_unwind(AssertUnwindSafe(|| {
                    let _ = builtins.io().context::<TestContext<GetFailure>>(&scenario);
                }))
                .is_err()
            }
        })
        .join()
        .unwrap();
    assert!(panicked);
    assert_eq!(runtime.stop().unwrap_err().to_string(), "driver construction failed");
}

#[test]
fn worker_driver_construction_failure_stops_an_already_registered_context() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.fail_driver_at.store(2, Ordering::SeqCst);

    workers[0]
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<LateConstructionFailure>>(&scenario);
            }
        })
        .join()
        .unwrap();
    let error = workers[1]
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                builtins
                    .io()
                    .try_context::<TestContext<LateConstructionFailure>>(&scenario)
                    .unwrap_err()
                    .to_string()
            }
        })
        .join()
        .unwrap();

    assert_eq!(error, "driver construction failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "driver construction failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_during_first_registration_is_not_reported_as_driver_failure() {
    let runtime = runtime(1);
    let control = support::runtime_control(&runtime);
    let worker = worker_schedulers(&runtime, 1).pop().unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(1, Ordering::SeqCst);
    let (outcome_tx, outcome_rx) = mpsc::channel();

    let initialization = worker.spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let outcome = builtins
                .io()
                .try_context::<TestContext<ShutdownDuringFirstRegistration>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
            outcome_tx.send(outcome).unwrap();
        }
    });
    scenario.wait_until_driver_blocked();
    control.request_stop();
    scenario.release_driver();

    assert_eq!(
        outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap(),
        "the runtime is shutting down"
    );
    let _ = initialization.join();
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_during_late_initialization_is_observed_by_the_request() {
    let runtime = runtime(WORKERS);
    let control = support::runtime_control(&runtime);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(2, Ordering::SeqCst);

    workers[0]
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<ShutdownDuringLateInitialization>>(&scenario);
            }
        })
        .join()
        .unwrap();

    let (outcome_tx, outcome_rx) = mpsc::channel();
    let initialization = workers[1].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let outcome = builtins
                .io()
                .try_context::<TestContext<ShutdownDuringLateInitialization>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
            outcome_tx.send(outcome).unwrap();
        }
    });
    scenario.wait_until_driver_blocked();
    control.request_stop();
    scenario.release_driver();

    assert_eq!(
        outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap(),
        "the runtime is shutting down"
    );
    let _ = initialization.join();
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), WORKERS);
}

#[test]
fn concurrent_worker_initialization_failure_is_reported_consistently() {
    const CONCURRENT_WORKERS: usize = 3;

    let runtime = runtime(CONCURRENT_WORKERS);
    let workers = worker_schedulers(&runtime, CONCURRENT_WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(2, Ordering::SeqCst);
    scenario.fail_driver_at.store(3, Ordering::SeqCst);

    workers[0]
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<ConcurrentLateFailure>>(&scenario);
            }
        })
        .join()
        .unwrap();

    let (outcome_tx, outcome_rx) = mpsc::channel();
    let blocked_initialization = workers[1].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            builtins
                .io()
                .try_context::<TestContext<ConcurrentLateFailure>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned())
        }
    });
    scenario.wait_until_driver_blocked();

    let failed_initialization = workers[2].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let outcome = builtins
                .io()
                .try_context::<TestContext<ConcurrentLateFailure>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
            outcome_tx.send(outcome).unwrap();
        }
    });

    assert_eq!(
        outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap(),
        "driver construction failed"
    );
    let _ = failed_initialization.join();
    scenario.release_driver();
    assert_eq!(blocked_initialization.join().unwrap(), "driver construction failed");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "driver construction failed");
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 2);
}

#[test]
fn concurrent_failures_preserve_the_first_shutdown_cause() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let first = Scenario::options(RolePlan::PreferPrimary);
    let second = Scenario::options(RolePlan::Secondary);
    first.fail_driver_at.store(1, Ordering::SeqCst);
    second.fail_driver_at.store(1, Ordering::SeqCst);
    first.block_driver_at.store(1, Ordering::SeqCst);
    second.block_driver_at.store(1, Ordering::SeqCst);
    first.set_driver_failure_message("first environmental failure");
    second.set_driver_failure_message("second environmental failure");

    let first_failure = workers[0].spawn({
        let first = first.clone();
        async move |builtins| {
            builtins
                .io()
                .try_context::<TestContext<ConcurrentFailureA>>(&first)
                .unwrap_err()
                .to_string()
        }
    });
    first.wait_until_driver_blocked();
    let second_failure = workers[1].spawn({
        let second = second.clone();
        async move |builtins| {
            builtins
                .io()
                .try_context::<TestContext<ConcurrentFailureB>>(&second)
                .unwrap_err()
                .to_string()
        }
    });
    second.wait_until_driver_blocked();

    first.release_driver();
    assert_eq!(first_failure.join().unwrap(), "first environmental failure");
    second.release_driver();
    assert_eq!(second_failure.join().unwrap(), "second environmental failure");
    assert_eq!(runtime.stop().unwrap_err().to_string(), "first environmental failure");
    assert_eq!(first.driver_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(second.driver_attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_does_not_couple_independent_worker_initialization() {
    let runtime = runtime(WORKERS);
    let control = support::runtime_control(&runtime);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(1, Ordering::SeqCst);
    let (first_outcome_tx, first_outcome_rx) = mpsc::channel();
    let (second_outcome_tx, second_outcome_rx) = mpsc::channel();

    let first = workers[0].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let outcome = builtins
                .io()
                .try_context::<TestContext<ShutdownWhileWaiting>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
            first_outcome_tx.send(outcome).unwrap();
        }
    });
    scenario.wait_until_driver_blocked();

    let second = workers[1].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let outcome = builtins
                .io()
                .try_context::<TestContext<ShutdownWhileWaiting>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
            second_outcome_tx.send(outcome).unwrap();
        }
    });
    let second_outcome = second_outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT);
    control.request_stop();
    scenario.release_driver();

    assert_eq!(
        first_outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap(),
        "the runtime is shutting down"
    );
    assert_eq!(second_outcome.unwrap(), "ok");
    let _ = first.join();
    let _ = second.join();
    runtime.stop().unwrap();
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), WORKERS);
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), WORKERS);
}
