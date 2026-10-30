// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct same-pool blocking joins must be rejected instead of starving their pool.

#![cfg(feature = "rt")]
#![cfg(not(miri))]

mod panic_support;

use std::cell::RefCell;
use std::future::Future as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::task::{Context, Waker};

use panic_support::runtime;
use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

thread_local! {
    static JOIN_ON_DROP: RefCell<Option<JoinOnDrop>> = const { RefCell::new(None) };
}

struct JoinOnDrop {
    task: Option<arty::task::JoinHandle<u32>>,
    completed: mpsc::Sender<u32>,
}

impl Drop for JoinOnDrop {
    fn drop(&mut self) {
        let task = self.task.take().expect("the cleanup owner retains one blocking task");
        let result = futures::executor::block_on(task).expect("the runtime remains active during external thread cleanup");
        self.completed
            .send(result)
            .expect("the test retains the cleanup completion receiver");
    }
}

fn completes_without_deadlock(body: impl FnOnce() + Send + 'static) {
    let (completed, completion) = mpsc::channel();
    std::thread::spawn(move || {
        body();
        _ = completed.send(());
    });
    let deadlocked = if is_mutation_testing() {
        completion.recv().is_err()
    } else {
        completion.recv_timeout(TEST_TIMEOUT).is_err()
    };
    if deadlocked {
        eprintln!("the supported operation deadlocked");
        #[expect(clippy::exit, reason = "a deadlocked runtime thread prevents normal child-process teardown")]
        std::process::exit(112);
    }
}

#[test]
fn blocking_join_future_rejects_its_current_pool() {
    completes_without_deadlock(|| {
        let runtime = runtime();
        let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
        let nested = scheduler.clone();
        let task = scheduler.spawn_blocking(move || {
            let inner = nested.spawn_blocking(|| 42);
            catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(inner))).is_err()
        });

        assert!(futures::executor::block_on(task).unwrap());
        runtime.stop().unwrap();
    });
}

#[test]
fn blocking_join_survives_local_worker_slot_teardown() {
    completes_without_deadlock(|| {
        let runtime = runtime();
        let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
        let (completed, completion) = mpsc::channel();

        std::thread::spawn(move || {
            JOIN_ON_DROP.with(|owner| {
                assert!(
                    owner
                        .replace(Some(JoinOnDrop {
                            task: Some(scheduler.spawn_blocking(|| 42)),
                            completed,
                        }))
                        .is_none()
                );
            });
            let mut task = std::pin::pin!(scheduler.spawn_blocking(|| ()));
            _ = task.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        })
        .join()
        .unwrap();

        assert_eq!(completion.recv_timeout(TEST_TIMEOUT).unwrap(), 42);
        runtime.stop().unwrap();
    });
}
