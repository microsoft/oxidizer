// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained asynchronous, local, and blocking task submission behavior.

#![cfg(feature = "rt")]

#[cfg(not(miri))]
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::thread;

#[cfg(not(miri))]
use arty::runtime::ProcessorCount;
use arty::runtime::Runtime;
#[cfg(not(miri))]
use many_cpus::SystemHardware;
use testing_aids::{YieldFuture, execute_or_terminate_process};

testing_aids::init_tracing!();

#[test]
fn spawn_some_tasks() {
    execute_or_terminate_process(|| {
        let runtime = Runtime::new().unwrap();
        let async_task = runtime.task_scheduler().spawn(async |cx| {
            YieldFuture::default().await;
            let child1 = cx.scheduler().spawn(async |_| 1111);
            let child2 = cx.scheduler().spawn_anywhere((), |()| async { 2222 });
            let child5 = cx.scheduler().spawn_blocking(|| 5555);
            let child6 = cx.local_scheduler().unwrap().spawn(async || 6666);
            let results = futures::join!(child1, child2, child5, child6);
            assert_eq!(results, (1111, 2222, 5555, 6666));
        });

        let single_threaded_actions = runtime.task_scheduler().spawn(async |cx| {
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
                .await;
            assert_eq!(length, canary.len());
        });

        runtime
            .task_scheduler()
            .spawn(async |cx| {
                YieldFuture::default().await;
                cx.local_scheduler()
                    .unwrap()
                    .spawn(async || {
                        YieldFuture::default().await;
                    })
                    .await;
            })
            .wait();

        runtime
            .task_scheduler()
            .spawn(async move |_| {
                async_task.await;
                single_threaded_actions.await;
            })
            .wait();
    });
}

#[test]
#[cfg(not(miri))]
fn test_worker_affinity() {
    if SystemHardware::current().processors().len() < 6 {
        eprintln!("requires six processors; two-worker affinity is covered by runtime_contracts");
        return;
    }
    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::new(6).unwrap()))
        .build()
        .unwrap();
    let (thread1, scheduler1) = runtime
        .task_scheduler()
        .spawn(async |cx| (thread::current().id(), cx.scheduler().clone()))
        .wait();
    let (thread2, scheduler2) = runtime
        .task_scheduler()
        .spawn(async |cx| (thread::current().id(), cx.scheduler().clone()))
        .wait();
    let thread3 = scheduler1.spawn(async |_| thread::current().id()).wait();
    let thread4 = scheduler2.spawn(async |_| thread::current().id()).wait();

    assert_ne!(thread1, thread2, "round-robin submissions select different workers");
    assert_eq!(thread1, thread3, "bound submission preserves the first worker");
    assert_eq!(thread2, thread4, "bound submission preserves the second worker");
}

#[test]
fn remote_factories_create_non_send_futures_on_the_worker() {
    let runtime = Runtime::new().unwrap();
    let (created, polled, associated) = runtime
        .task_scheduler()
        .spawn(|cx: arty::runtime::Builtins| {
            let value = Rc::new(thread::current().id());
            let associated = cx.thread().id();
            async move {
                YieldFuture::default().await;
                (*value, thread::current().id(), associated)
            }
        })
        .wait();
    assert_eq!((created, polled), (associated, associated));
}

#[test]
fn remote_results_do_not_require_thread_awareness() {
    struct ResultValue(u32);

    static_assertions::assert_not_impl_any!(ResultValue: thread_aware::ThreadAware);
    let runtime = Runtime::new().unwrap();
    assert_eq!(runtime.task_scheduler().spawn(async |_| ResultValue(42)).wait().0, 42);
    assert_eq!(runtime.task_scheduler().spawn(async |_| ResultValue(43)).wait().0, 43);
}
