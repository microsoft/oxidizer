// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! I/O access from ordinary threads and non-Arty runtimes.

use crate::*;

#[test]
fn builtins_clone_initializes_and_accesses_io_from_a_non_runtime_thread() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let worker = builtins.thread().id();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    let cached_options = Scenario::options(RolePlan::Secondary);
    let external = {
        let scenario = scenario.clone();
        thread::spawn(move || {
            let external = thread::current().id();
            let first = builtins.io().try_context::<TestContext<CacheContext>>(&scenario).unwrap();
            let second = builtins.io().context::<TestContext<CacheContext>>(&cached_options);
            (external, first.thread.unwrap().id(), second.thread.unwrap().id())
        })
        .join()
        .unwrap()
    };

    assert_ne!(external.0, worker);
    assert_eq!(external.1, worker);
    assert_eq!(external.2, worker);
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    assert!(scenario.option_workers().is_empty());
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn stopped_runtime_is_reported_before_external_io_dispatch() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    runtime.stop().unwrap();

    let error = builtins
        .io()
        .try_context::<TestContext<ExternalConstructionFailure>>(&Scenario::options(RolePlan::PreferPrimary))
        .unwrap_err();

    assert_eq!(error.to_string(), "the runtime is shutting down");
    assert!(format!("{:?}", builtins.io()).contains("Io"));
}

#[test]
fn concurrent_external_access_registers_one_context_and_driver() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let worker = builtins.thread().id();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    let barrier = Arc::new(Barrier::new(2));

    #[expect(
        clippy::needless_collect,
        reason = "all threads must be spawned before any barrier participant is joined"
    )]
    let accesses: Vec<_> = (0..2)
        .map(|_| {
            let builtins = builtins.clone();
            let scenario = scenario.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                builtins
                    .io()
                    .try_context::<TestContext<ConcurrentExternal>>(&scenario)
                    .unwrap()
                    .thread
                    .unwrap()
                    .id()
            })
        })
        .collect();
    let context_workers: Vec<_> = accesses.into_iter().map(|access| access.join().unwrap()).collect();

    assert_eq!(context_workers, vec![worker; 2]);
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn external_access_from_a_running_futures_executor_uses_the_bound_worker() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let scenario = Scenario::options(RolePlan::PreferPrimary);

    let context = futures::executor::block_on(async { builtins.io().try_context::<TestContext<NestedExecutorAccess>>(&scenario) }).unwrap();
    assert_eq!(context.thread.as_ref().unwrap().id(), builtins.thread().id());
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);

    let _ = builtins
        .io()
        .context::<TestContext<NestedExecutorAccess>>(&Scenario::options(RolePlan::PreferPrimary));
    runtime.stop().unwrap();
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_during_external_initialization_unblocks_without_io_failure() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let operations = builtins.operations().clone();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    scenario.block_driver_at.store(1, Ordering::SeqCst);

    let external = {
        let scenario = scenario.clone();
        thread::spawn(move || {
            builtins
                .io()
                .try_context::<TestContext<ExternalShutdownDuringInitialization>>(&scenario)
                .map_or_else(|error| error.to_string(), |_| "ok".to_owned())
        })
    };
    scenario.wait_until_driver_blocked();
    operations.request_stop();
    scenario.release_driver();

    assert_eq!(external.join().unwrap(), "the runtime is shutting down");
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn external_access_racing_shutdown_returns_shutdown() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let operations = builtins.operations().clone();
    let worker = builtins.scheduler().clone();
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let (release_blocker_tx, release_blocker_rx) = mpsc::channel();
    let blocker = worker.spawn(move |_| {
        blocker_started_tx.send(()).unwrap();
        release_blocker_rx.recv().unwrap();
        async {}
    });
    blocker_started_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();

    let scenario = Scenario::options(RolePlan::PreferPrimary);
    let external = thread::spawn(move || {
        builtins
            .io()
            .try_context::<TestContext<QueuedExternalShutdown>>(&scenario)
            .unwrap_err()
            .to_string()
    });
    operations.request_stop();
    release_blocker_tx.send(()).unwrap();

    assert_eq!(external.join().unwrap(), "the runtime is shutting down");
    let _ = blocker.join();
    runtime.stop().unwrap();
}

#[test]
fn relocation_to_a_foreign_runtime_preserves_the_original_worker_binding() {
    let original_runtime = runtime(1);
    let foreign_runtime = runtime(1);
    let (mut io, source, original_worker) = original_runtime
        .scheduler()
        .block_on(async |builtins| (builtins.io().clone(), builtins.thread().clone(), builtins.scheduler().clone()))
        .unwrap();
    let rejected_options = Scenario::options(RolePlan::PreferPrimary);

    let (io, error) = foreign_runtime
        .scheduler()
        .block_on({
            let rejected_options = rejected_options.clone();
            async move |builtins| {
                io.relocate(Some(&source), builtins.thread());
                let error = io
                    .try_context::<TestContext<ForeignRelocationContext>>(&rejected_options)
                    .unwrap_err()
                    .to_string();
                (io, error)
            }
        })
        .unwrap();
    assert!(error.contains("cross between Arty workers"));
    assert_eq!(rejected_options.context_constructions.load(Ordering::SeqCst), 0);

    let scenario = Scenario::options(RolePlan::PreferPrimary);
    original_worker
        .spawn({
            let scenario = scenario.clone();
            async move |_| {
                let _ = io.context::<TestContext<ForeignRelocationContext>>(&scenario);
            }
        })
        .join()
        .unwrap();

    foreign_runtime.stop().unwrap();
    original_runtime.stop().unwrap();
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn io_access_from_another_worker_is_rejected_without_registration() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let io = workers[0].spawn(async |builtins| builtins.io().clone()).join().unwrap();
    let rejected_options = Scenario::options(RolePlan::PreferPrimary);

    let error = workers[1]
        .spawn({
            let rejected_options = rejected_options.clone();
            async move |_| {
                io.try_context::<TestContext<SameRuntimeCrossWorker>>(&rejected_options)
                    .unwrap_err()
                    .to_string()
            }
        })
        .join()
        .unwrap();

    assert!(error.contains("cross between Arty workers"));
    assert_eq!(rejected_options.context_constructions.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}

#[test]
fn reciprocal_worker_io_access_is_rejected_without_deadlock() {
    let runtime = runtime(WORKERS);
    let workers = worker_schedulers(&runtime, WORKERS);
    let first_io = workers[0].spawn(async |builtins| builtins.io().clone()).join().unwrap();
    let second_io = workers[1].spawn(async |builtins| builtins.io().clone()).join().unwrap();
    let barrier = Arc::new(Barrier::new(WORKERS));
    let options = Scenario::options(RolePlan::PreferPrimary);

    let first = workers[0].spawn({
        let barrier = Arc::clone(&barrier);
        let options = options.clone();
        async move |_| {
            barrier.wait();
            second_io
                .try_context::<TestContext<SameRuntimeCrossWorker>>(&options)
                .unwrap_err()
                .to_string()
        }
    });
    let second = workers[1].spawn({
        let barrier = Arc::clone(&barrier);
        let options = options.clone();
        async move |_| {
            barrier.wait();
            first_io
                .try_context::<TestContext<SameRuntimeCrossWorker>>(&options)
                .unwrap_err()
                .to_string()
        }
    });

    assert!(first.join().unwrap().contains("cross between Arty workers"));
    assert!(second.join().unwrap().contains("cross between Arty workers"));
    assert_eq!(options.context_constructions.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}

#[test]
fn builtins_clone_accesses_io_from_a_tokio_runtime_thread() {
    let runtime = runtime(1);
    let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
    let bound_worker = builtins.thread().id();
    let scenario = Scenario::options(RolePlan::PreferPrimary);
    let tokio = tokio::runtime::Builder::new_current_thread().build().unwrap();

    let (tokio_thread, context_worker) = tokio.block_on({
        let scenario = scenario.clone();
        async move {
            let tokio_thread = thread::current().id();
            let context = builtins.io().try_context::<TestContext<TokioExternalAccess>>(&scenario).unwrap();
            (tokio_thread, context.thread.unwrap().id())
        }
    });

    assert_ne!(tokio_thread, bound_worker);
    assert_eq!(context_worker, bound_worker);
    assert_eq!(scenario.context_constructions.load(Ordering::SeqCst), 1);
    assert_eq!(scenario.driver_attempts.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
    assert_eq!(scenario.shutdowns.load(Ordering::SeqCst), 1);
}
