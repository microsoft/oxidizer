// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Self-wake notifications queued before a panic cannot repoll a completed future.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};
use panic_support::{isolated, runtime};
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct SelfWakePanic {
    polls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    owner: std::thread::ThreadId,
    _local: Rc<()>,
}

impl Future for SelfWakePanic {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        for _ in 0..8 {
            cx.waker().wake_by_ref();
            cx.waker().clone().wake();
        }
        panic!("self-waking task poll");
    }
}

impl Drop for SelfWakePanic {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.owner);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn every_worker_retires_self_waking_panics_before_reusing_task_storage() {
    isolated("every_worker_retires_self_waking_panics_before_reusing_task_storage", || {
        let repetitions = if cfg!(miri) { 2 } else { 32 };
        let runtime = Runtime::builder()
            .processor_count(ProcessorCount::exactly(2))
            .blocking_pool_policy(BlockingPoolPolicy::shared(1))
            .build()
            .unwrap();
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let captured_polls = Arc::clone(&polls);
        let captured_drops = Arc::clone(&drops);
        runtime
            .scheduler()
            .block_on(async move |cx| {
                for _ in 0..repetitions {
                    let tasks = cx.scheduler().spawn_everywhere(
                        (cx.clone(), Unaware((Arc::clone(&captured_polls), Arc::clone(&captured_drops)))),
                        |(child, Unaware((polls, drops)))| SelfWakePanic {
                            polls,
                            drops,
                            owner: child.thread().id(),
                            _local: Rc::new(()),
                        },
                    );
                    for task in tasks {
                        assert!(task.await.unwrap_err().is_panic());
                    }
                    for task in cx.scheduler().spawn_everywhere((), |()| async { 42 }) {
                        assert_eq!(task.await.unwrap(), 42);
                    }
                }
            })
            .unwrap();
        runtime.stop().unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 2 * repetitions);
        assert_eq!(drops.load(Ordering::SeqCst), 2 * repetitions);
    });
}

#[test]
fn local_self_waking_panics_do_not_repoll_or_corrupt_later_tasks() {
    isolated("local_self_waking_panics_do_not_repoll_or_corrupt_later_tasks", || {
        let repetitions = if cfg!(miri) { 2 } else { 32 };
        let runtime = runtime();
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let captured_polls = Arc::clone(&polls);
        let captured_drops = Arc::clone(&drops);
        runtime
            .scheduler()
            .block_on(async move |cx| {
                for _ in 0..repetitions {
                    let polls = Arc::clone(&captured_polls);
                    let drops = Arc::clone(&captured_drops);
                    let task = cx.local_scheduler().unwrap().spawn(move || SelfWakePanic {
                        polls,
                        drops,
                        owner: std::thread::current().id(),
                        _local: Rc::new(()),
                    });
                    assert!(task.await.unwrap_err().is_panic());
                    assert_eq!(*cx.local_scheduler().unwrap().spawn(async || Rc::new(42)).await.unwrap(), 42);
                }
            })
            .unwrap();
        runtime.stop().unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), repetitions);
        assert_eq!(drops.load(Ordering::SeqCst), repetitions);
    });
}
