// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::future::join_all;
use futures::join;
use many_cpus::ProcessorSet;
use oxidizer_rt::meta_builders::{LocalTaskMetaBuilder, SystemTaskMetaBuilder};
use oxidizer_rt::{
    BasicThreadState, Instantiation, Placement, ResourceQuota, Runtime, RuntimeBuilder,
    RuntimeOperations, SpawnInstance, TaskMeta,
};
use oxidizer_testing::execute_or_abandon;

#[test]
#[expect(clippy::too_many_lines, reason = "test code")]
fn spawn_some_tasks() {
    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    let background_task = runtime.spawn_with_meta(
        TaskMeta::with_placement(Placement::Background),
        async move |cx| {
            RuntimeOperations::yield_now().await;

            let child1 = cx.scheduler().spawn(async move |_| 1111);
            let child2 = cx.scheduler().spawn_with_meta(
                TaskMeta::with_placement(Placement::Background),
                async move |_| 2222,
            );
            let children3 = cx.scheduler().spawn_multiple_with_meta(
                Instantiation::All,
                TaskMeta::with_placement(Placement::Background),
                async move |_, _| 3333,
            );
            let children4 = cx
                .scheduler()
                .spawn_multiple(Instantiation::All, async move |_, _| 4444);
            let child5 = cx.system_scheduler().spawn(|| 5555);

            let results = futures::join!(
                child1,
                child2,
                join_all(children3),
                join_all(children4),
                child5,
            );

            assert_eq!(results.0, 1111);
            assert_eq!(results.1, 2222);
            assert!(!results.2.is_empty());
            for x in results.2 {
                assert_eq!(x, 3333);
            }
            assert!(!results.3.is_empty());
            for x in results.3 {
                assert_eq!(x, 4444);
            }
            assert_eq!(results.4, 5555);
        },
    );

    let async_task = runtime.spawn(async move |cx| {
        RuntimeOperations::yield_now().await;

        let child1 = cx.scheduler().spawn(async move |_| 1111);
        let child2 = cx.scheduler().spawn_with_meta(
            TaskMeta::with_placement(Placement::Background),
            async move |_| 2222,
        );
        let children3 = cx.scheduler().spawn_multiple_with_meta(
            Instantiation::All,
            TaskMeta::with_placement(Placement::Background),
            async move |_, _| 3333,
        );
        let children4 = cx
            .scheduler()
            .spawn_multiple(Instantiation::All, async move |_, _| 4444);
        let child5 = cx.system_scheduler().spawn(|| 5555);
        let child6 = cx.local_scheduler().spawn(async move || 6666);

        let results = join!(
            child1,
            child2,
            join_all(children3),
            join_all(children4),
            child5,
            child6,
        );

        assert_eq!(results.0, 1111);
        assert_eq!(results.1, 2222);
        assert!(!results.2.is_empty());
        for x in results.2 {
            assert_eq!(x, 3333);
        }
        assert!(!results.3.is_empty());
        for x in results.3 {
            assert_eq!(x, 4444);
        }
        assert_eq!(results.4, 5555);
        assert_eq!(results.5, 6666);
    });

    let multiple_background = runtime.spawn_multiple_with_meta(
        Instantiation::All,
        TaskMeta::with_placement(Placement::Background),
        async move |_, _| 2,
    );

    let single_threaded_actions = runtime.spawn(async move |cx| {
        let canary = Rc::new("I am a little bird who only lives on one thread".to_string());

        let length = cx
            .local_scheduler()
            .spawn({
                let canary = Rc::clone(&canary);
                async move || {
                    RuntimeOperations::yield_now().await;

                    canary.len()
                }
            })
            .await;

        assert_eq!(length, canary.len());
    });

    let mut synchronously_waited_task = runtime.spawn(async move |cx| {
        RuntimeOperations::yield_now().await;

        cx.local_scheduler()
            .spawn(async move || {
                RuntimeOperations::yield_now().await;
            })
            .await;
    });

    execute_or_abandon(move || synchronously_waited_task.wait()).unwrap();

    let multiple_counters =
        runtime.spawn_multiple(Instantiation::All, async move |_, instance| instance);

    let background_with_meta = runtime.spawn_with_meta(
        TaskMeta::builder()
            .placement(Placement::CurrentRegion)
            .build(),
        async move |_| {
            RuntimeOperations::yield_now().await;
        },
    );

    let background_multiple_with_meta = runtime.spawn_multiple_with_meta(
        Instantiation::All,
        TaskMeta::with_placement(Placement::CurrentRegion),
        async move |_, _| {
            RuntimeOperations::yield_now().await;
        },
    );

    let with_meta = runtime.spawn_with_meta(
        TaskMeta::builder()
            .placement(Placement::CurrentRegion)
            .build(),
        async move |_| {
            RuntimeOperations::yield_now().await;
        },
    );

    let multiple_with_meta = runtime.spawn_multiple_with_meta(
        Instantiation::All,
        TaskMeta::builder().build(),
        async move |_, _| {
            RuntimeOperations::yield_now().await;
        },
    );

    _ = runtime.spawn(async move |cx| {
        RuntimeOperations::yield_now().await;

        background_task.await;
        async_task.await;
        single_threaded_actions.await;

        let multiple_background_results = join_all(multiple_background).await;

        for x in multiple_background_results {
            assert_eq!(x, 2);
        }

        // We add up all the instance numbers here and ensure the total instances seen matches the total count seen.
        let multiple_results = join_all(multiple_counters).await;
        let expected_instance_count = multiple_results.first().unwrap().count();

        assert_eq!(multiple_results.len(), expected_instance_count);

        // We expect to have seen indexes 0..expected_instance_count - 1.
        let expected_index_sum = (0..expected_instance_count).sum::<usize>();
        let index_sum = multiple_results
            .iter()
            .map(SpawnInstance::index)
            .sum::<usize>();

        assert_eq!(index_sum, expected_index_sum);

        background_with_meta.await;
        with_meta.await;

        for task in background_multiple_with_meta {
            task.await;
        }

        for task in multiple_with_meta {
            task.await;
        }

        cx.runtime_ops().stop();
    });

    execute_or_abandon(move || runtime.wait()).unwrap();
}

#[test]
fn test_abort_local_task() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_cloned = Arc::clone(&counter);

    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    runtime.spawn(async move |context| {
        let handle = context.local_scheduler().spawn(async move || {
            RuntimeOperations::yield_now().await;
            counter.fetch_add(1, Ordering::SeqCst);
        });

        RuntimeOperations::yield_now().await;
        handle.request_abort();
        RuntimeOperations::yield_now().await;

        let counter = counter_cloned.load(Ordering::SeqCst);
        assert!(counter > 0);
        RuntimeOperations::yield_now().await;

        assert_eq!(counter, counter_cloned.load(Ordering::SeqCst));
    });
}

#[test]
fn test_meta_shorthands() {
    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    // single
    runtime.spawn_with_meta(Placement::Any, async move |context| {
        context
            .local_scheduler()
            .spawn_with_meta(LocalTaskMetaBuilder::new(), async move || {
                RuntimeOperations::yield_now().await;
            })
            .await;

        context
            .system_scheduler()
            .spawn_with_meta(SystemTaskMetaBuilder::new(), move || {})
            .await;

        context
            .scheduler()
            .spawn_with_meta(Placement::Any, async move |_| {
                RuntimeOperations::yield_now().await;
            })
            .await;
    });

    // multiple
    runtime.spawn_multiple_with_meta(
        Instantiation::All,
        Placement::Any,
        async move |context, _| {
            let _ = context.scheduler().spawn_multiple_with_meta(
                Instantiation::All,
                Placement::Any,
                async move |_, _| {
                    RuntimeOperations::yield_now().await;
                },
            );
        },
    );
}

#[test]
fn test_worker_placement() {
    if ProcessorSet::default().len() < 6 {
        eprintln!("skipping test - requires at least 6 processors");
        return;
    }

    let runtime = RuntimeBuilder::new::<BasicThreadState>()
        .with_resource_quota(ResourceQuota::new().with_num_processors(6))
        .build()
        .expect("Failed to create runtime");

    let mut handle_1 = runtime.spawn(async move |_cx| std::thread::current().id());
    let mut handle_2 = runtime.spawn(async move |cx| {
        (
            std::thread::current().id(),
            cx.runtime_ops().placement().unwrap(),
        )
    });

    let thread_id_1 = handle_1.wait();
    let (thread_id_2, placement_2) = handle_2.wait();

    let mut handle_3 = runtime.spawn_with_meta(
        Placement::SameThreadAs(handle_1.placement().unwrap()),
        async move |_cx| std::thread::current().id(),
    );

    let mut handle_4 = runtime
        .spawn_with_meta(Placement::SameThreadAs(placement_2), async move |_cx| {
            std::thread::current().id()
        });

    let thread_id_3 = handle_3.wait();
    let thread_id_4 = handle_4.wait();

    assert_ne!(
        thread_id_1, thread_id_2,
        "Thread 1 and Thread 2 should be different due to round-robin scheduling"
    );

    assert_eq!(
        thread_id_1, thread_id_3,
        "Thread 1 and Thread 3 should be the same due to same thread placement"
    );

    assert_eq!(
        thread_id_2, thread_id_4,
        "Thread 2 and Thread 4 should be the same due to some thread placement"
    );
}