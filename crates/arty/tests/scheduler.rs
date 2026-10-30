// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Schedulers remain usable when stored in application and thread-local state.

#![cfg(all(feature = "rt", feature = "macros"))]

use std::cell::RefCell;

use arty::task::{Context, Scheduler};

#[arty::test]
#[expect(clippy::float_cmp, reason = "the test uses a fixed value, so direct comparison is correct")]
async fn stash_scheduler(cx: Context) {
    // The object stores a scheduler and can submit tasks without knowing the task or context type.

    struct Thingy {
        scheduler: Scheduler,
    }

    impl Thingy {
        async fn calculate_pi(&self) -> f64 {
            self.scheduler.spawn(async move |_| 3.0).await.unwrap()
        }
    }

    cx.scheduler()
        .spawn(async move |cx| {
            // The object uses the stored scheduler without direct access to the task context.
            let thingy = Thingy {
                scheduler: cx.scheduler().clone(),
            };

            let pi = thingy.calculate_pi().await;

            assert_eq!(pi, 3.0);

            // The stored worker-bound scheduler also works from a different task.
            let thingy = Thingy {
                scheduler: cx.scheduler().clone(),
            };

            cx.scheduler()
                .spawn(async move |_| {
                    let pi = thingy.calculate_pi().await;
                    assert_eq!(pi, 3.0);
                })
                .await
                .unwrap();
        })
        .await
        .unwrap();

    cx.scheduler()
        .spawn(async move |cx| {
            // The task stores the scheduler in thread-local state.
            THREAD_LOCAL_STASH.with_borrow_mut(|stash| {
                *stash = Some(cx.scheduler().clone());
            });

            // Another task on the same thread uses the stored scheduler.
            let result = cx
                .scheduler()
                .spawn(async move |_| {
                    let scheduler = THREAD_LOCAL_STASH.with_borrow(|stash| stash.clone().unwrap());

                    // The stored scheduler remains usable.
                    scheduler.spawn(async move |_| 49).await.unwrap()
                })
                .await
                .unwrap();

            assert_eq!(result, 49);
        })
        .await
        .unwrap();
}

thread_local! {
    static THREAD_LOCAL_STASH: RefCell<Option<Scheduler>> = const { RefCell::new(None) };
}
