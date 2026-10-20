// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Registration serialization, role consistency, and nested registration.

use super::*;

#[test]
fn each_worker_initializes_its_driver_on_first_retrieval() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);

    let first = workers[0]
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| builtins.io().context::<TestContext<CacheContext>>(&scenario)
        })
        .join()
        .unwrap();
    assert_eq!(
        first.thread.as_ref().unwrap().id(),
        workers[0].spawn(async |cx| cx.thread().id()).join().unwrap()
    );
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.option_relocations.load(Ordering::SeqCst), 0);

    flush_workers(&workers);
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.relocations.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);

    workers[0]
        .spawn(async |builtins| {
            let _ = builtins
                .io()
                .context::<TestContext<CacheContext>>(&Scenario::options(RolePlan::PreferPrimary));
        })
        .join()
        .unwrap();
    workers[1]
        .spawn(async |builtins| {
            let _ = builtins
                .io()
                .context::<TestContext<CacheContext>>(&Scenario::options(RolePlan::PreferPrimary));
        })
        .join()
        .unwrap();

    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), WORKERS);
    assert_eq!(scenario.driver_workers().len(), WORKERS);
    assert!(scenario.roles().iter().all(|role| *role == DriverRole::Primary));
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), WORKERS);
}
#[test]
fn runtime_registries_are_isolated() {
    let first_runtime = runtime(1);
    let second_runtime = runtime(1);
    let first = Scenario::options(RolePlan::PreferPrimary);
    let second = Scenario::options(RolePlan::PreferPrimary);

    first_runtime
        .scheduler()
        .block_on({
            let first = first.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<IsolatedContext>>(&first);
            }
        })
        .unwrap();
    second_runtime
        .scheduler()
        .block_on({
            let second = second.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<IsolatedContext>>(&second);
            }
        })
        .unwrap();

    assert_eq!(first.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(second.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(first.roles(), vec![DriverRole::Primary]);
    assert_eq!(second.roles(), vec![DriverRole::Primary]);
    first_runtime.stop().unwrap();
    second_runtime.stop().unwrap();
}
#[test]
fn waiting_registration_observes_another_workers_failure() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(1, Ordering::SeqCst);
    scenario.fail_driver_at.store(1, Ordering::SeqCst);
    let (first_outcome_tx, first_outcome_rx) = mpsc::channel();
    let (second_outcome_tx, second_outcome_rx) = mpsc::channel();

    let first = workers[0].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let outcome = builtins
                .io()
                .try_context::<TestContext<ConcurrentFirstFailure>>(&scenario)
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
                .try_context::<TestContext<ConcurrentFirstFailure>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
            second_outcome_tx.send(outcome).unwrap();
        }
    });
    let second_outcome = second_outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT);
    scenario.release_driver();

    assert_eq!(
        first_outcome_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap(),
        "driver construction failed"
    );
    assert_eq!(second_outcome.unwrap(), "ok");
    let _ = first.join();
    let _ = second.join();
    assert_eq!(runtime.stop().unwrap_err().to_string(), "driver construction failed");
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), WORKERS);
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}
#[test]
fn concurrent_same_context_registration_constructs_options_once() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(1, Ordering::SeqCst);
    let (second_done_tx, second_done_rx) = mpsc::channel();

    let first = workers[0].spawn({
        let scenario = scenario.clone();
        async move |builtins| {
            let _ = builtins.io().context::<TestContext<ConcurrentSameContext>>(&scenario);
        }
    });
    scenario.wait_until_driver_blocked();

    let second = workers[1].spawn(async move |builtins| {
        let _ = builtins
            .io()
            .context::<TestContext<ConcurrentSameContext>>(&Scenario::options(RolePlan::PreferPrimary));
        second_done_tx.send(()).unwrap();
    });
    let second_done = second_done_rx.recv_timeout(testing_aids::TEST_TIMEOUT);
    scenario.release_driver();
    second_done.unwrap();

    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), WORKERS);
    assert_eq!(scenario.driver_workers().len(), WORKERS);
    assert!(scenario.roles().iter().all(|role| *role == DriverRole::Primary));
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), WORKERS);
}

#[test]
fn nested_context_retrieval_panics_and_stops_the_runtime() {
    let runtime = runtime(1);
    let scenario = Scenario::options(RolePlan::PreferPrimary);

    let error = runtime
        .scheduler()
        .block_on({
            let scenario = scenario.clone();
            async move |builtins| {
                scenario.register_self_from_context(builtins.io().clone());
                let _ = builtins.io().context::<TestContext<RecursiveContext>>(&scenario);
            }
        })
        .unwrap_err();

    assert!(
        error
            .source()
            .and_then(|source| source.downcast_ref::<arty::task::JoinError>())
            .is_some_and(arty::task::JoinError::is_panic)
    );
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.stop().unwrap_err().to_string(),
        "I/O context retrieval cannot be nested during initialization"
    );
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn inconsistent_primary_role_on_another_worker_is_rejected() {
    let (runtime, processor) = runtime_with_telemetry(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let scenario = Scenario::options(RolePlan::Inconsistent);
    scenario.fail_shutdown.store(true, Ordering::SeqCst);

    workers[0]
        .spawn({
            let scenario = scenario.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<InconsistentPrimary>>(&scenario);
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
                    .try_context::<TestContext<InconsistentPrimary>>(&scenario)
                    .unwrap_err()
                    .to_string()
            }
        })
        .join()
        .unwrap();

    assert!(error.contains("driver does not support assigned role Primary"));
    assert!(
        runtime
            .stop()
            .unwrap_err()
            .to_string()
            .contains("driver does not support assigned role Primary")
    );
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(event_count(&processor, "arty.rt.io.driver.shutdown_failed"), 1);
}
#[test]
fn racing_registrations_select_one_consistent_primary() {
    for _ in 0..16 {
        race_primary_registration_once();
    }
}
#[test]
fn additional_drivers_are_secondary_on_every_worker() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let primary = Scenario::options(RolePlan::PreferPrimary);
    let first_additional = Scenario::options(RolePlan::PreferPrimary);
    let second_additional = Scenario::options(RolePlan::PreferPrimary);

    workers[0]
        .spawn({
            let primary = primary.clone();
            let first_additional = first_additional.clone();
            let second_additional = second_additional.clone();
            async move |builtins| {
                let _ = builtins.io().context::<TestContext<CacheContext>>(&primary);
                let _ = builtins.io().context::<TestContext<AdditionalSecondaryA>>(&first_additional);
                let _ = builtins.io().context::<TestContext<AdditionalSecondaryB>>(&second_additional);
            }
        })
        .join()
        .unwrap();
    flush_workers(&workers);

    for worker in &workers {
        worker
            .spawn(async |builtins| {
                let _ = builtins
                    .io()
                    .context::<TestContext<CacheContext>>(&Scenario::options(RolePlan::PreferPrimary));
                let _ = builtins
                    .io()
                    .context::<TestContext<AdditionalSecondaryA>>(&Scenario::options(RolePlan::PreferPrimary));
                let _ = builtins
                    .io()
                    .context::<TestContext<AdditionalSecondaryB>>(&Scenario::options(RolePlan::PreferPrimary));
            })
            .join()
            .unwrap();
    }

    assert!(primary.roles().iter().all(|role| *role == DriverRole::Primary));
    for additional in [&first_additional, &second_additional] {
        assert_eq!(additional.driver_workers().len(), WORKERS);
        assert_eq!(additional.driver_attempts.load(Ordering::SeqCst), WORKERS);
        assert!(additional.roles().iter().all(|role| *role == DriverRole::Secondary));
    }
    runtime.stop().unwrap();
}

#[test]
fn driver_can_reject_the_assigned_primary_role() {
    let runtime = runtime(1);
    let scenario = Scenario::options(RolePlan::Secondary);

    let error = runtime
        .scheduler()
        .block_on({
            let scenario = scenario.clone();
            async move |builtins| {
                builtins
                    .io()
                    .try_context::<TestContext<ReservedSecondary>>(&scenario)
                    .unwrap_err()
                    .to_string()
            }
        })
        .unwrap();

    assert!(error.contains("driver does not support assigned role Primary"));
    assert!(
        runtime
            .stop()
            .unwrap_err()
            .to_string()
            .contains("driver does not support assigned role Primary")
    );
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 0);
}

fn race_primary_registration_once() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let first = Scenario::options(RolePlan::PreferPrimary);
    let second = Scenario::options(RolePlan::PreferPrimary);
    let barrier = Arc::new(Barrier::new(WORKERS));

    let first_registration = workers[0].spawn({
        let first = first.clone();
        let barrier = Arc::clone(&barrier);
        async move |builtins| {
            barrier.wait();
            let _ = builtins.io().context::<TestContext<RacePrimaryA>>(&first);
        }
    });
    let second_registration = workers[1].spawn({
        let second = second.clone();
        let barrier = Arc::clone(&barrier);
        async move |builtins| {
            barrier.wait();
            let _ = builtins.io().context::<TestContext<RacePrimaryB>>(&second);
        }
    });
    first_registration.join().unwrap();
    second_registration.join().unwrap();
    flush_workers(&workers);

    for worker in &workers {
        worker
            .spawn(async |builtins| {
                let _ = builtins
                    .io()
                    .context::<TestContext<RacePrimaryA>>(&Scenario::options(RolePlan::PreferPrimary));
                let _ = builtins
                    .io()
                    .context::<TestContext<RacePrimaryB>>(&Scenario::options(RolePlan::PreferPrimary));
            })
            .join()
            .unwrap();
    }

    let first_roles = first.roles();
    let second_roles = second.roles();
    assert_eq!(first_roles.len(), WORKERS);
    assert_eq!(second_roles.len(), WORKERS);
    assert_eq!(first.driver_workers().len(), WORKERS);
    assert_eq!(second.driver_workers().len(), WORKERS);
    assert!(first_roles.iter().all(|role| *role == first_roles[0]));
    assert!(second_roles.iter().all(|role| *role == second_roles[0]));
    assert_ne!(first_roles[0], second_roles[0]);
    assert_eq!(
        [first_roles[0], second_roles[0]]
            .into_iter()
            .filter(|role| *role == DriverRole::Primary)
            .count(),
        1
    );
    runtime.stop().unwrap();
}
