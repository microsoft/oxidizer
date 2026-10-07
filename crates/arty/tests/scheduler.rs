// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Schedulers remain usable when stored in application and thread-local state.

#![cfg(all(feature = "rt", feature = "macros"))]

testing_aids::init_tracing!();

use std::cell::RefCell;

use arty::task::{Builtins, Scheduler};

#[arty::test]
#[expect(
    clippy::float_cmp,
    reason = "fake logic for tests, no computation involved - direct comparison is fine"
)]
async fn stash_scheduler(cx: Builtins) {
    // We store a scheduler in some object that can schedule tasks without having knowledge of the
    // exact type of the task or task context.

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
            // We store the scheduler in a thingy and try to use it from the thingy
            // without having direct access to the task context.
            let thingy = Thingy {
                scheduler: cx.scheduler().clone(),
            };

            let pi = thingy.calculate_pi().await;

            assert_eq!(pi, 3.0);

            // The stored worker-bound scheduler also works from a different task.
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
        .await
        .unwrap();

    cx.scheduler()
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
        .await
        .unwrap();
}

thread_local! {
    static THREAD_LOCAL_STASH: RefCell<Option<Scheduler>> = const { RefCell::new(None) };
}
