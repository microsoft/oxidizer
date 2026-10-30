// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies the public facade.

#[test]
fn core_types_are_reexported() {
    use arty::core::{NumaNode, Owner, Thread, ThreadAware};

    fn assert_thread_aware<T: ThreadAware>() {}

    let _: Option<(Thread, Owner, NumaNode)> = None;
    assert_thread_aware::<String>();
}

#[test]
fn time_types_are_reexported() {
    use arty::time::Clock;

    let _ = std::mem::size_of::<Clock>();
}

#[cfg(feature = "test-util")]
#[test]
fn clock_control_is_reexported_with_test_util() {
    let _ = std::mem::size_of::<arty::time::ClockControl>();
}
