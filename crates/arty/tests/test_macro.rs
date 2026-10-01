// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime test attributes preserve asynchronous bodies and return values.

#![cfg(feature = "rt")]
#![cfg(feature = "macros")]

testing_aids::init_tracing!();

use arty::task::Builtins;
use arty::test;

#[test]
async fn simple_main(cx: Builtins) {
    println!("Hello, world!");
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await
        .unwrap();
}

#[test]
async fn simple_main_returning(cx: Builtins) -> Result<(), Box<dyn std::error::Error + Send + 'static>> {
    println!("Hello, world!");
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello again!");
        })
        .await
        .unwrap();
    Ok(())
}

#[test(workers = 1)]
async fn root_and_children_keep_their_worker_affinity(cx: Builtins) {
    let worker = std::thread::current().id();
    assert_eq!(cx.thread().id(), worker);
    let child = cx.scheduler().spawn(async |child| child.thread().id()).await.unwrap();
    assert_eq!(child, worker);
}

#[test]
async fn default_test_runtime_has_exactly_one_worker(cx: Builtins) {
    let home = cx.thread().id();
    let tasks = cx.scheduler().spawn_everywhere((), |()| async { std::thread::current().id() });
    assert_eq!(tasks.len(), 1);
    for task in tasks {
        assert_eq!(task.await.unwrap(), home);
    }
}

#[test(workers = 4_294_967_295usize)]
async fn worker_limit_clamps_instead_of_failing_construction(cx: Builtins) {
    assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
}

#[test(workers = 0)]
#[should_panic(expected = "failed to create the runtime for the entry point")]
async fn zero_workers_fail_during_construction(_cx: Builtins) {
    panic!("the test body must not run");
}

fn custom_builder() -> arty::runtime::RuntimeBuilder {
    use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};

    Runtime::builder()
        .processor_count(ProcessorCount::at_most(1))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
}

#[test(builder = custom_builder())]
async fn custom_runtime_keeps_owned_builtins(cx: Builtins) {
    assert_eq!(
        cx.local_scheduler()
            .unwrap()
            .spawn(async || std::rc::Rc::new(42))
            .await
            .unwrap()
            .as_ref(),
        &42,
    );
}

#[cfg(feature = "test-util")]
mod controlled_time {
    use std::time::{Duration, UNIX_EPOCH};

    use arty as renamed_arty;
    use arty::task::Builtins;
    use arty::time::ClockControl;
    use futures::FutureExt;

    trait TestTypes {
        type Context;
        type Control;
    }

    struct Types;

    impl TestTypes for Types {
        type Context = Builtins;
        type Control = ClockControl;
    }

    #[arty::test(workers = 1)]
    async fn clock_starts_frozen(cx: Builtins, control: ClockControl) {
        assert_eq!(cx.clock().system_time(), UNIX_EPOCH);
        let before = cx.clock().instant();
        assert_eq!(cx.clock().instant(), before);
        control.advance(Duration::from_secs(42));
        assert_eq!(cx.clock().system_time(), UNIX_EPOCH + Duration::from_secs(42));
    }

    #[arty::test(workers = 1)]
    async fn manual_advance_wakes_only_due_timers(cx: Builtins, control: ClockControl) {
        assert_eq!(cx.clock().system_time(), UNIX_EPOCH);
        let mut delay = std::pin::pin!(cx.clock().delay(Duration::from_secs(10)));
        assert!(delay.as_mut().now_or_never().is_none());

        control.advance(Duration::from_secs(9));
        assert!(delay.as_mut().now_or_never().is_none());
        control.advance(Duration::from_secs(1));
        assert!(delay.as_mut().now_or_never().is_some());
    }

    #[renamed_arty::test(workers = 1, runtime_path = renamed_arty::runtime)]
    async fn aliases_and_eager_advancement(cx: <Types as TestTypes>::Context, control: <Types as TestTypes>::Control) {
        let control = control.auto_advance_timers(true);
        let watch = cx.clock().stopwatch();
        cx.clock().delay(Duration::from_secs(30)).await;
        assert_eq!(watch.elapsed(), Duration::from_secs(30));

        control.advance(Duration::from_secs(5));
        assert_eq!(watch.elapsed(), Duration::from_secs(35));
    }

    #[arty::test(workers = 1)]
    async fn child_uses_the_same_control(cx: Builtins, control: ClockControl) {
        let child_control = control.clone();
        let now = cx
            .scheduler()
            .spawn(async move |child| {
                child_control.advance(Duration::from_secs(7));
                child.clock().system_time()
            })
            .await
            .unwrap();
        assert_eq!(now, UNIX_EPOCH + Duration::from_secs(7));
        assert_eq!(cx.clock().system_time(), now);
        assert_eq!(control.to_clock().system_time(), now);
    }

    #[test]
    fn each_test_gets_a_fresh_clock() {
        clock_starts_frozen();
        manual_advance_wakes_only_due_timers();
    }

    #[arty::test(workers = 1)]
    #[should_panic(expected = "controlled test panic")]
    async fn controlled_test_preserves_panic_payload(cx: Builtins, control: ClockControl) {
        control.advance(Duration::from_secs(1));
        assert_eq!(cx.clock().system_time(), UNIX_EPOCH + Duration::from_secs(1));
        panic!("controlled test panic");
    }

    #[arty::test(
        builder = super::custom_builder()
            .clock(ClockControl::new_at(UNIX_EPOCH + Duration::from_secs(123)))
    )]
    async fn custom_builder_clock_remains_in_effect(cx: Builtins) {
        assert_eq!(cx.clock().system_time(), UNIX_EPOCH + Duration::from_secs(123));
    }
}
