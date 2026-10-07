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
    use arty::time::Clock;

    let _ = std::mem::size_of::<Clock>();
}

#[cfg(all(feature = "time", feature = "test-util"))]
#[test]
fn clock_control_is_reexported_with_both_features() {
    let _ = std::mem::size_of::<arty::time::ClockControl>();
}
