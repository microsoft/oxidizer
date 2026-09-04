// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies the feature-gated public facade.

#[test]
fn core_types_are_reexported() {
    use arty::core::{NumaNode, Owner, Thread, ThreadAware};

    fn assert_thread_aware<T: ThreadAware>() {}

    let _: Option<(Thread, Owner, NumaNode)> = None;
    assert_thread_aware::<String>();
}

#[cfg(feature = "time")]
#[test]
fn time_types_are_reexported() {
    use arty::time::{
        Clock, Delay, Error, FutureExt, PeriodicTimer, Result, SimpleClock, Stopwatch,
        SystemTimeExt, Timeout,
    };

    fn assert_future_ext<T: FutureExt>() {}
    fn assert_system_time_ext<T: SystemTimeExt>() {}

    let _ = size_of::<Clock>();
    let _ = size_of::<Delay>();
    let _ = size_of::<Error>();
    let _ = size_of::<PeriodicTimer>();
    let _ = size_of::<Result<()>>();
    let _ = size_of::<SimpleClock>();
    let _ = size_of::<Stopwatch>();
    let _ = size_of::<Timeout<(), ()>>();
    assert_future_ext::<std::future::Ready<()>>();
    assert_system_time_ext::<std::time::SystemTime>();
}

#[cfg(all(feature = "time", feature = "test-util"))]
#[test]
fn clock_control_is_reexported_with_both_features() {
    let _ = size_of::<arty::time::ClockControl>();
}
