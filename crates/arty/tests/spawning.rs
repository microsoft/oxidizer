// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained async, local, and blocking task submission behavior.

#![cfg(feature = "rt")]

mod support;

use std::rc::Rc;
use std::thread;

use arty::core::{Thread, ThreadAware};
use arty::runtime::Runtime;
#[cfg(not(miri))]
use arty::runtime::WorkersPolicy;
#[cfg(feature = "macros")]
use arty::task::Builtins;
use arty::task::Scheduler;
#[cfg(not(miri))]
use many_cpus::SystemHardware;
use support::JoinHandleExt as _;
use testing_aids::YieldFuture;
#[cfg(feature = "macros")]
use thread_aware::Unaware;

testing_aids::init_tracing!();

#[test]
fn assert_send_sync() {
    static_assertions::assert_impl_all!(Scheduler: Send, Sync);
}

#[cfg(feature = "macros")]
#[arty::test(workers = 2)]
async fn spawn_some_tasks(cx: Builtins) {
    let async_task = cx.scheduler().spawn_anywhere(cx.clone(), |cx: Builtins| async move {
        YieldFuture::default().await;
        let child1 = cx.scheduler().spawn(async |_| 1111);
        let child2 = cx.scheduler().spawn_anywhere((), |()| async { 2222 });
        let child5 = cx.scheduler().spawn_blocking(|| 5555);
        let results = futures::join!(child1, child2, child5);
        assert_eq!((results.0.unwrap(), results.1.unwrap(), results.2.unwrap()), (1111, 2222, 5555));
    });

    let single_threaded_actions = cx.scheduler().spawn_anywhere(cx.clone(), |_: Builtins| async move {
        let canary = Rc::new("I am a little bird who only lives on one thread".to_owned());
        YieldFuture::default().await;
        let length = canary.len();
        assert_eq!(length, canary.len());
    });

    cx.scheduler()
        .spawn_anywhere(cx.clone(), |cx: Builtins| async move {
            YieldFuture::default().await;
            cx.scheduler()
                .spawn(async |_| {
                    YieldFuture::default().await;
                })
                .await
                .unwrap();
        })
        .await
        .unwrap();

    cx.scheduler()
        .spawn_anywhere(
            Unaware((async_task, single_threaded_actions)),
            |Unaware((async_task, single_threaded_actions))| async move {
                async_task.await.unwrap();
                single_threaded_actions.await.unwrap();
            },
        )
        .await
        .unwrap();
}

#[test]
#[cfg(not(miri))]
fn test_worker_affinity() {
    if SystemHardware::current().processors().len() < 6 {
        eprintln!("requires six processors; two-worker affinity is covered by runtime_contracts");
        return;
    }
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(6)).build().unwrap();
    let (thread1, scheduler1) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (thread::current().id(), cx.scheduler().clone()) })
        .join()
        .unwrap();
    let (thread2, scheduler2) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (thread::current().id(), cx.scheduler().clone()) })
        .join()
        .unwrap();
    let thread3 = scheduler1.spawn(async |_| thread::current().id()).join().unwrap();
    let thread4 = scheduler2.spawn(async |_| thread::current().id()).join().unwrap();

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
        .join()
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
        .join()
        .unwrap();
    assert!(source.is_none());
    assert_eq!(destination, Some(worker.clone()));
    assert_eq!(worker.id(), executed_on);
}

#[cfg(feature = "macros")]
#[arty::test]
async fn worker_bound_spawn_results_do_not_require_thread_awareness(cx: Builtins) {
    struct ResultValue(u32);

    static_assertions::assert_not_impl_any!(ResultValue: thread_aware::ThreadAware);
    assert_eq!(cx.scheduler().spawn(async |_| ResultValue(42)).await.unwrap().0, 42);
    assert_eq!(cx.scheduler().spawn(async |_| ResultValue(43)).await.unwrap().0, 43);
}

#[test]
fn runtime_spawn_anywhere_accepts_send_only_non_sync_results() {
    struct ResultValue(std::cell::Cell<u32>);

    static_assertions::assert_impl_all!(ResultValue: Send);
    static_assertions::assert_not_impl_any!(ResultValue: Sync, ThreadAware);
    let runtime = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::at_most(1))
        .build()
        .unwrap();
    let result = runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async { ResultValue(std::cell::Cell::new(42)) })
        .join()
        .unwrap();
    assert_eq!(result.0.get(), 42);
    runtime.stop().unwrap();
}

#[cfg(feature = "macros")]
struct DestinationProbe {
    source: Option<Thread>,
    destination: Option<Thread>,
}

#[cfg(feature = "macros")]
impl ThreadAware for DestinationProbe {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.source = source.cloned();
        self.destination = Some(destination.clone());
    }
}

#[cfg(feature = "macros")]
#[arty::test(workers = 2)]
async fn spawn_at_relocates_payload_to_each_selected_worker_despite_interleaving(cx: Builtins) {
    let expected_workers = if cfg!(miri) {
        2
    } else {
        many_cpus::SystemHardware::current().processors().len().min(2)
    };
    let home = cx.thread().id();
    let destinations: Vec<_> = cx.operations().workers().into_iter().map(Thread::id).collect();
    let tasks: Vec<_> = cx
        .operations()
        .workers()
        .into_iter()
        .map(|worker| {
            drop(cx.scheduler().spawn_anywhere((), |()| async {}));
            cx.scheduler().spawn_at(
                worker,
                DestinationProbe {
                    source: None,
                    destination: None,
                },
                |probe| {
                    let probe = Rc::new(probe);
                    async move {
                        (
                            probe.source.as_ref().unwrap().id(),
                            probe.destination.as_ref().unwrap().id(),
                            thread::current().id(),
                        )
                    }
                },
            )
        })
        .collect();
    let results = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), expected_workers);
    assert!(
        results
            .iter()
            .all(|(source, destination, executed)| *source == home && destination == executed)
    );
    assert_eq!(
        results.iter().map(|(_, destination, _)| *destination).collect::<Vec<_>>(),
        destinations
    );
}

#[test]
fn spawn_at_returns_shutdown_joins_without_invoking_factories() {
    let runtime = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::at_most(2))
        .build()
        .unwrap();
    let (scheduler, operations) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (cx.scheduler().clone(), cx.operations().clone()) })
        .join()
        .unwrap();
    operations.request_stop();
    let tasks: Vec<_> = operations
        .workers()
        .into_iter()
        .map(|worker| scheduler.spawn_at::<(), _, ()>(worker, (), |()| async { panic!("rejected factories must not run") }))
        .collect();
    assert!(!tasks.is_empty());
    for task in tasks {
        assert!(task.join().unwrap_err().is_shutdown());
    }
    runtime.stop().unwrap();
}

#[test]
fn spawn_at_rejects_a_worker_from_another_runtime() {
    let runtime = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::exactly(1))
        .build()
        .unwrap();
    let (scheduler, operations) = runtime
        .scheduler()
        .block_on(async |cx| (cx.scheduler().clone(), cx.operations().clone()))
        .unwrap();
    let other = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::exactly(1))
        .build()
        .unwrap();
    let destination = other
        .scheduler()
        .block_on(async |cx| cx.operations().workers().into_iter().next().unwrap().clone())
        .unwrap();
    let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let task = scheduler.spawn_at(
        &destination,
        thread_aware::Unaware(std::sync::Arc::clone(&invoked)),
        |thread_aware::Unaware(invoked)| async move {
            invoked.store(true, std::sync::atomic::Ordering::SeqCst);
        },
    );

    let error = task.join().unwrap_err();

    assert!(error.is_invalid_destination());
    assert!(!error.is_panic());
    assert!(!error.is_shutdown());
    assert_eq!(error.to_string(), "target thread is not a worker of this runtime");
    assert!(!invoked.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(operations.workers().into_iter().len(), 1);
    runtime.stop().unwrap();
    other.stop().unwrap();
}
