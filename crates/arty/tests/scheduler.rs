// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Schedulers remain usable when stored in application and thread-local state.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::cell::RefCell;

use arty::runtime::Runtime;
use arty::task::TaskScheduler;
use testing_aids::execute_or_terminate_process;

#[test]
fn stash_scheduler() {
    // We store a scheduler in some object that can schedule tasks without having knowledge of the
    // exact type of the task or task context.

    struct Thingy {
        scheduler: TaskScheduler,
    }

    impl Thingy {
        async fn calculate_pi(&self) -> f64 {
            self.scheduler.spawn(async move |_| 3.0).await.unwrap()
        }
    }

    let runtime = Runtime::new().expect("Failed to create runtime");

    #[expect(
        clippy::float_cmp,
        reason = "fake logic for tests, no computation involved - direct comparison is fine"
    )]
    execute_or_terminate_process(move || {
        runtime
            .task_scheduler()
            .spawn(async move |cx| {
                // We store the scheduler in a thingy and try to use it from the thingy
                // without having direct access to the task context.
                let thingy = Thingy {
                    scheduler: cx.scheduler().clone(),
                };

                let pi = thingy.calculate_pi().await;

                assert_eq!(pi, 3.0);

                // It works even from a different task if we detach the scheduler.
                let thingy = Thingy {
                    scheduler: cx.scheduler().clone(),
                };

                cx.local_scheduler()
                    .expect("On the same thread")
                    .spawn(async move || {
                        let pi = thingy.calculate_pi().await;

                        assert_eq!(pi, 3.0);
                    })
                    .await
                    .unwrap();
            })
            .wait()
            .unwrap();

        runtime
            .task_scheduler()
            .spawn(async move |cx| {
                // We store the scheduler in a thread-local variable.
                THREAD_LOCAL_STASH.with_borrow_mut(|stash| {
                    *stash = Some(cx.scheduler().clone());
                });

                // And we try to use it from another task on the same thread.
                let result = cx
                    .local_scheduler()
                    .expect("On the same thread as cx")
                    .spawn(async move || {
                        let scheduler = THREAD_LOCAL_STASH.with_borrow(|stash| stash.clone().unwrap());

                        // It works, right? Right.
                        scheduler.spawn(async move |_| 49).await.unwrap()
                    })
                    .await
                    .unwrap();

                assert_eq!(result, 49);
            })
            .wait()
            .unwrap();
    });
}

thread_local! {
    static THREAD_LOCAL_STASH: RefCell<Option<TaskScheduler>> = const { RefCell::new(None) };
}
