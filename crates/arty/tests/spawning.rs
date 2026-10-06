// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained async, local, and blocking task submission behavior.

#![cfg(feature = "rt")]

use std::rc::Rc;
use std::thread;

use arty::core::{Thread, ThreadAware};
#[cfg(not(miri))]
use arty::runtime::ProcessorCount;
use arty::runtime::Runtime;
use arty::task::Scheduler;
#[cfg(not(miri))]
use many_cpus::SystemHardware;
use testing_aids::{YieldFuture, execute_or_terminate_process};
use thread_aware::Unaware;

testing_aids::init_tracing!();

#[test]
fn spawn_some_tasks() {
    execute_or_terminate_process(|| {
        let builder = Runtime::builder();
        // Keep cross-worker submission under Miri without repeating it over every fake processor.
        #[cfg(miri)]
        let builder = builder.processor_count(arty::runtime::ProcessorCount::exactly(2));
        let runtime = builder.build().unwrap();
        let async_task = runtime.scheduler().spawn_anywhere((), |cx, ()| async move {
            YieldFuture::default().await;
            let child1 = cx.scheduler().spawn(async |_| 1111);
            let child2 = cx.scheduler().spawn_anywhere((), |()| async { 2222 });
            let child5 = cx.scheduler().spawn_blocking(|| 5555);
            let child6 = cx.local_scheduler().unwrap().spawn(async || 6666);
            let results = futures::join!(child1, child2, child5, child6);
            assert_eq!(
                (results.0.unwrap(), results.1.unwrap(), results.2.unwrap(), results.3.unwrap()),
                (1111, 2222, 5555, 6666)
            );
        });

        let single_threaded_actions = runtime.scheduler().spawn_anywhere((), |cx, ()| async move {
            let canary = Rc::new("I am a little bird who only lives on one thread".to_owned());
            let length = cx
                .local_scheduler()
                .unwrap()
                .spawn({
                    let canary = Rc::clone(&canary);
                    async move || {
                        YieldFuture::default().await;
                        canary.len()
                    }
                })
                .await
                .unwrap();
            assert_eq!(length, canary.len());
        });

        runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move {
                YieldFuture::default().await;
                cx.local_scheduler()
                    .unwrap()
                    .spawn(async || {
                        YieldFuture::default().await;
                    })
                    .await
                    .unwrap();
            })
            .wait()
            .unwrap();

        runtime
            .scheduler()
            .spawn_anywhere(
                Unaware((async_task, single_threaded_actions)),
                |_, Unaware((async_task, single_threaded_actions))| async move {
                    async_task.await.unwrap();
                    single_threaded_actions.await.unwrap();
                },
            )
            .wait()
            .unwrap();
    });
}

#[test]
#[cfg(not(miri))]
fn test_worker_affinity() {
    if SystemHardware::current().processors().len() < 6 {
        eprintln!("requires six processors; two-worker affinity is covered by runtime_contracts");
        return;
    }
    let runtime = Runtime::builder().processor_count(ProcessorCount::exactly(6)).build().unwrap();
    let (thread1, scheduler1) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (thread::current().id(), cx.scheduler().clone()) })
        .wait()
        .unwrap();
    let (thread2, scheduler2) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (thread::current().id(), cx.scheduler().clone()) })
        .wait()
        .unwrap();
    let thread3 = scheduler1.spawn(async |_| thread::current().id()).wait().unwrap();
    let thread4 = scheduler2.spawn(async |_| thread::current().id()).wait().unwrap();

    assert_ne!(thread1, thread2, "round-robin submissions select different workers");
    assert_eq!(thread1, thread3, "bound submission preserves the first worker");
    assert_eq!(thread2, thread4, "bound submission preserves the second worker");
}

#[test]
fn remote_factories_create_non_send_futures_on_the_worker() {
    let runtime = Runtime::new().unwrap();
    let (created, polled, associated) = runtime
        .scheduler()
        .spawn_anywhere((), |cx: arty::task::Builtins, ()| {
            let value = Rc::new(thread::current().id());
            let associated = cx.thread().id();
            async move {
                YieldFuture::default().await;
                (*value, thread::current().id(), associated)
            }
        })
        .wait()
        .unwrap();
    assert_eq!((created, polled), (associated, associated));
}

#[test]
fn runtime_spawn_anywhere_relocates_payload_with_unknown_source() {
    struct Probe {
        source: Option<Thread>,
        destination: Option<Thread>,
    }

    impl ThreadAware for Probe {
        fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
            self.source = source.cloned();
            self.destination = Some(destination.clone());
        }
    }

    let runtime = Runtime::new().unwrap();
    let (source, destination, worker, executed_on) = runtime
        .scheduler()
        .spawn_anywhere(
            Probe {
                source: None,
                destination: None,
            },
            |cx, probe| async move { (probe.source, probe.destination, cx.thread().clone(), thread::current().id()) },
        )
        .wait()
        .unwrap();
    assert!(source.is_none());
    assert_eq!(destination, Some(worker.clone()));
    assert_eq!(worker.id(), executed_on);
}

#[test]
fn worker_bound_spawn_results_do_not_require_thread_awareness() {
    struct ResultValue(u32);

    static_assertions::assert_not_impl_any!(ResultValue: thread_aware::ThreadAware);
    let runtime = Runtime::new().unwrap();
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .wait()
        .unwrap();
    assert_eq!(scheduler.spawn(async |_| ResultValue(42)).wait().unwrap().0, 42);
    assert_eq!(scheduler.spawn(async |_| ResultValue(43)).wait().unwrap().0, 43);
}

#[test]
fn runtime_spawn_anywhere_accepts_send_only_non_sync_results() {
    struct ResultValue(std::cell::Cell<u32>);

    static_assertions::assert_impl_all!(ResultValue: Send);
    static_assertions::assert_not_impl_any!(ResultValue: Sync, ThreadAware);
    let runtime = Runtime::builder()
        .processor_count(arty::runtime::ProcessorCount::at_most(1))
        .build()
        .unwrap();
    let result = runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async { ResultValue(std::cell::Cell::new(42)) })
        .wait()
        .unwrap();
    assert_eq!(result.0.get(), 42);
    runtime.stop().unwrap();
}

struct EverywhereProbe {
    scheduler: Scheduler,
    source: Option<Thread>,
    destination: Option<Thread>,
}

impl Clone for EverywhereProbe {
    fn clone(&self) -> Self {
        // Interleave round-robin submissions with fan-out to expose incorrect routing.
        drop(self.scheduler.spawn_anywhere((), |()| async {}));
        Self {
            scheduler: self.scheduler.clone(),
            source: self.source.clone(),
            destination: self.destination.clone(),
        }
    }
}

impl ThreadAware for EverywhereProbe {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.source = source.cloned();
        self.destination = Some(destination.clone());
    }
}

#[test]
fn spawn_everywhere_relocates_one_clone_to_each_worker_despite_interleaving() {
    let expected_workers = if cfg!(miri) {
        2
    } else {
        many_cpus::SystemHardware::current().processors().len().min(2)
    };
    let runtime = Runtime::builder()
        .processor_count(arty::runtime::ProcessorCount::at_most(2))
        .build()
        .unwrap();
    let (home, results) = runtime
        .scheduler()
        .block_on(async |cx| {
            let home = cx.thread().id();
            let probe = EverywhereProbe {
                scheduler: cx.scheduler().clone(),
                source: None,
                destination: None,
            };
            let tasks = cx.scheduler().spawn_everywhere(probe, |probe| {
                let probe = Rc::new(probe);
                async move {
                    (
                        probe.source.as_ref().unwrap().id(),
                        probe.destination.as_ref().unwrap().id(),
                        thread::current().id(),
                    )
                }
            });
            let results = futures::future::join_all(tasks)
                .await
                .into_iter()
                .map(Result::unwrap)
                .collect::<Vec<_>>();
            (home, results)
        })
        .unwrap();
    assert_eq!(results.len(), expected_workers);
    assert_eq!(results[0].1, home);
    assert!(
        results
            .iter()
            .all(|(source, destination, executed)| *source == home && destination == executed)
    );
    assert_eq!(
        results
            .iter()
            .map(|(_, destination, _)| *destination)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        expected_workers
    );
}

#[test]
fn spawn_everywhere_returns_shutdown_joins_without_invoking_factories() {
    let runtime = Runtime::builder()
        .processor_count(arty::runtime::ProcessorCount::at_most(2))
        .build()
        .unwrap();
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .wait()
        .unwrap();
    arty::runtime::RuntimeOperations::from(&runtime).request_stop();
    let tasks = scheduler.spawn_everywhere::<(), _, ()>((), |()| async { panic!("rejected factories must not run") });
    assert!(!tasks.is_empty());
    for task in tasks {
        assert!(task.wait().unwrap_err().is_shutdown());
    }
    runtime.stop().unwrap();
}
