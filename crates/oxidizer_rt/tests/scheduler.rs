// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.

use std::cell::RefCell;

use oxidizer_rt::{BasicThreadState, Runtime, TaskScheduler};
use oxidizer_testing::execute_or_terminate_process;

#[test]
fn stash_scheduler() {
    // We store a scheduler in some object that can schedule tasks without having knowledge of the
    // exact type of the task or task context.

    struct Thingy<'a> {
        scheduler: TaskScheduler<'a, BasicThreadState>,
    }

    impl Thingy<'_> {
        async fn calculate_pi(&self) -> f64 {
            self.scheduler.spawn(async move |_| 3.0).await
        }
    }

    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    #[expect(
        clippy::float_cmp,
        reason = "fake logic for tests, no computation involved - direct comparison is fine"
    )]
    execute_or_terminate_process(move || {
        runtime
            .spawn(async move |cx| {
                // We store the scheduler in a thingy and try to use it from the thingy
                // without having direct access to the task context.
                let thingy = Thingy {
                    scheduler: cx.scheduler(),
                };

                let pi = thingy.calculate_pi().await;

                assert_eq!(pi, 3.0);

                // It works even from a different task if we detach the scheduler.
                let thingy = Thingy {
                    scheduler: cx.scheduler().detach(),
                };

                cx.local_scheduler()
                    .spawn(async move || {
                        let pi = thingy.calculate_pi().await;

                        assert_eq!(pi, 3.0);
                    })
                    .await;
            })
            .wait();

        runtime.spawn(async move |cx| {
            // We store the scheduler in a thread-local variable.
            THREAD_LOCAL_STASH.with_borrow_mut(|stash| {
                *stash = Some(cx.scheduler().detach());
            });

            // And we try to use it from another task on the same thread.
            let result = cx
                .local_scheduler()
                .spawn(async move || {
                    let scheduler =
                        THREAD_LOCAL_STASH.with_borrow(|stash| stash.as_ref().unwrap().detach());

                    // It works, right? Right.
                    scheduler.spawn(async move |_| 49).await
                })
                .await;

            assert_eq!(result, 49);
        });
    });
}

thread_local! {
    static THREAD_LOCAL_STASH: RefCell<Option<TaskScheduler<'static, BasicThreadState>>> = const { RefCell::new(None) };
}