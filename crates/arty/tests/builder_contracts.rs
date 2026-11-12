// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder configuration through running workers.

#![cfg(feature = "rt")]

mod support;

use std::time::Duration;

use arty::runtime::{PanicPolicy, Runtime, WorkersPolicy};
use support::JoinHandleExt as _;
use tick::ClockControl;

#[test]
fn validation_checks_the_base_name() {
    Runtime::builder().name("requests").build().unwrap().stop().unwrap();
    assert_eq!(
        Runtime::builder().name("invalid\0name").build().unwrap_err().to_string(),
        "runtime name must not contain a null character"
    );
}

#[test]
fn custom_clock_ok() {
    let clock_control = ClockControl::new();
    let runtime = Runtime::builder().clock(clock_control.clone()).build().unwrap();
    let clock = runtime.scheduler().block_on(async |cx| cx.clock().clone()).unwrap();
    let now = clock.system_time();

    clock_control.advance(Duration::from_secs(1));

    assert_eq!(clock.system_time().duration_since(now).unwrap(), Duration::from_secs(1));
    runtime.stop().unwrap();
}

#[test]
fn maximum_worker_count_replaces_exact_count() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .workers(WorkersPolicy::at_most(1))
        .build()
        .unwrap();

    assert_eq!(
        runtime
            .scheduler()
            .block_on(async |cx| cx.control().workers().into_iter().count())
            .unwrap(),
        1
    );
    runtime.stop().unwrap();
}

#[test]
fn zero_worker_policies_can_be_replaced_before_building() {
    for policy in [WorkersPolicy::exactly(0), WorkersPolicy::at_most(0)] {
        let runtime = Runtime::builder()
            .workers(policy)
            .workers(WorkersPolicy::at_most(1))
            .build()
            .unwrap();

        assert_eq!(
            runtime
                .scheduler()
                .block_on(async |cx| cx.control().workers().into_iter().count())
                .unwrap(),
            1
        );
        runtime.stop().unwrap();
    }
}

#[test]
fn automatic_worker_selection_replaces_exact_count() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .workers(WorkersPolicy::default())
        .build()
        .unwrap();

    assert_eq!(
        runtime
            .scheduler()
            .block_on(async |cx| cx.control().workers().into_iter().count())
            .unwrap(),
        1
    );
    runtime.stop().unwrap();
}

#[test]
fn default_policy_starts_shutdown() {
    let runtime = Runtime::builder().panic(PanicPolicy::default()).build().unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();

    assert!(scheduler.spawn(async |_| panic!("default policy")).join().unwrap_err().is_panic());
    assert!(scheduler.spawn(async |_| ()).join().unwrap_err().is_shutdown());
    assert!(runtime.stop().is_err());
}

#[test]
fn ignore_policy_keeps_the_runtime_running() {
    let runtime = Runtime::builder().panic(PanicPolicy::ignore()).build().unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();

    assert!(scheduler.spawn(async |_| panic!("ignore policy")).join().unwrap_err().is_panic());
    assert_eq!(scheduler.spawn(async |_| 42).join().unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn panic_replaces_the_default() {
    let runtime = Runtime::builder()
        .panic(PanicPolicy::shutdown())
        .panic(PanicPolicy::ignore())
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();

    assert!(
        scheduler
            .spawn(async |_| panic!("replacement policy"))
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(scheduler.spawn(async |_| 42).join().unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn build_and_stop_inside_futures_executor() {
    futures::executor::block_on(async {
        let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();

        let (value, scheduler) = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { (42, cx.scheduler().clone()) })
            .await
            .unwrap();
        assert_eq!(value, 42);

        runtime.stop().unwrap();
        assert!(scheduler.spawn(async |_| ()).await.unwrap_err().is_shutdown());
    });
}
