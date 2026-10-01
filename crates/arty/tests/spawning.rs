// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained asynchronous, local, and blocking task submission behavior.

#![cfg(feature = "rt")]

use std::rc::Rc;
use std::thread;

use arty::core::{Thread, ThreadAware};
#[cfg(not(miri))]
use arty::runtime::ProcessorCount;
use arty::runtime::Runtime;
use arty::task::TaskScheduler;
#[cfg(not(miri))]
use many_cpus::SystemHardware;
use testing_aids::{YieldFuture, execute_or_terminate_process};

testing_aids::init_tracing!();

#[test]
fn spawn_some_tasks() {
    execute_or_terminate_process(|| {
        let runtime = Runtime::new().unwrap();
        let async_task = runtime.scheduler().spawn_anywhere(async |cx| {
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

        let single_threaded_actions = runtime.scheduler().spawn_anywhere(async |cx| {
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
            .spawn_anywhere(async |cx| {
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
            .spawn_anywhere(async move |_| {
                async_task.await.unwrap();
                single_threaded_actions.await.unwrap();
            })
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
        .spawn_anywhere(async |cx| (thread::current().id(), cx.scheduler().clone()))
        .wait()
        .unwrap();
    let (thread2, scheduler2) = runtime
        .scheduler()
        .spawn_anywhere(async |cx| (thread::current().id(), cx.scheduler().clone()))
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
        .spawn_anywhere(|cx: arty::task::Builtins| {
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
fn remote_results_do_not_require_thread_awareness() {
    struct ResultValue(u32);

    static_assertions::assert_not_impl_any!(ResultValue: thread_aware::ThreadAware);
    let runtime = Runtime::new().unwrap();
    assert_eq!(runtime.scheduler().spawn_anywhere(async |_| ResultValue(42)).wait().unwrap().0, 42);
    assert_eq!(runtime.scheduler().spawn_anywhere(async |_| ResultValue(43)).wait().unwrap().0, 43);
}

struct EverywhereProbe {
    scheduler: TaskScheduler,
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
        .spawn_anywhere(async |cx| cx.scheduler().clone())
        .wait()
        .unwrap();
    arty::runtime::RuntimeOperations::from(&runtime).request_stop();
    let tasks = scheduler.spawn_everywhere((), |()| async { panic!("rejected factories must not run") });
    assert!(!tasks.is_empty());
    for task in tasks {
        assert!(task.wait().unwrap_err().is_shutdown());
    }
    runtime.stop().unwrap();
}
