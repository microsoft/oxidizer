// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained async, local, and blocking task submission behavior.

#![cfg(feature = "rt")]

mod support;

use std::marker::PhantomPinned;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
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
fn queued_factory_outlives_its_submitting_thread_and_accepts_non_unpin_captures() {
    struct Capture {
        dropped: mpsc::Sender<thread::ThreadId>,
        _pin: PhantomPinned,
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            self.dropped.send(thread::current().id()).unwrap();
        }
    }

    static_assertions::assert_impl_all!(Capture: Send);
    static_assertions::assert_not_impl_any!(Capture: Unpin);
    let runtime = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::at_most(1))
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (started, worker_started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let release = scopeguard::guard(release, |release| {
        _ = release.send(());
    });
    let blocker = scheduler.spawn(move |_| {
        started.send(()).unwrap();
        released.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
        std::future::ready(())
    });
    worker_started.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    let (dropped, drop_thread) = mpsc::channel();
    let capture = Capture {
        dropped,
        _pin: PhantomPinned,
    };
    let queued = thread::spawn(move || {
        scheduler.spawn(move |cx| {
            let value = Rc::new(42);
            async move {
                YieldFuture::default().await;
                drop(capture);
                (*value, cx.thread().id())
            }
        })
    })
    .join()
    .unwrap();
    // The source thread and its factory pool are gone before the worker can register the task.
    drop(release);
    blocker.join().unwrap();
    let (value, owner) = queued.join().unwrap();
    assert_eq!(value, 42);
    assert_eq!(drop_thread.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap(), owner);
    runtime.stop().unwrap();
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
    let expected_workers = 2;
    let home = cx.thread().id();
    let destinations: Vec<_> = cx.control().workers().into_iter().map(Thread::id).collect();
    let tasks: Vec<_> = cx
        .control()
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
    let (scheduler, control) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (cx.scheduler().clone(), cx.control().clone()) })
        .join()
        .unwrap();
    control.request_stop();
    let tasks: Vec<_> = control
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
    struct RejectionProbe {
        relocations: Arc<AtomicUsize>,
        invocations: Arc<AtomicUsize>,
    }

    impl ThreadAware for RejectionProbe {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
            self.relocations.fetch_add(1, Ordering::SeqCst);
        }
    }

    let runtime = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::exactly(1))
        .build()
        .unwrap();
    let (scheduler, control) = runtime
        .scheduler()
        .block_on(async |cx| (cx.scheduler().clone(), cx.control().clone()))
        .unwrap();
    let other = Runtime::builder()
        .workers(arty::runtime::WorkersPolicy::exactly(1))
        .build()
        .unwrap();
    let destination = other
        .scheduler()
        .block_on(async |cx| cx.control().workers().into_iter().next().unwrap().clone())
        .unwrap();
    let relocations = Arc::new(AtomicUsize::new(0));
    let invocations = Arc::new(AtomicUsize::new(0));
    let task = scheduler.spawn_at(
        &destination,
        RejectionProbe {
            relocations: Arc::clone(&relocations),
            invocations: Arc::clone(&invocations),
        },
        |probe| {
            probe.invocations.fetch_add(1, Ordering::SeqCst);
            async {}
        },
    );

    let error = task.join().unwrap_err();

    assert!(error.is_invalid_destination());
    assert!(!error.is_panic());
    assert!(!error.is_shutdown());
    assert_eq!(error.to_string(), "target thread is not a worker of this runtime");
    assert!(error.into_panic().is_none());
    assert_eq!((relocations.load(Ordering::SeqCst), invocations.load(Ordering::SeqCst)), (0, 0));
    assert_eq!(control.workers().into_iter().len(), 1);
    runtime.stop().unwrap();
    other.stop().unwrap();
}
