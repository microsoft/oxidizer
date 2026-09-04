// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[test]
#[ignore = "stub"]
fn core_types_are_reexported() {
    // Verify the thread-aware core vocabulary is nameable through `arty::core`.
}

#[cfg(feature = "time")]
#[test]
#[ignore = "stub"]
fn time_types_are_reexported() {
    // Verify Tick's primary public types are nameable through `arty::time`.
}

#[cfg(all(feature = "time", feature = "test-util"))]
#[test]
#[ignore = "stub"]
fn clock_control_is_reexported_with_both_features() {
    // Verify `ClockControl` is nameable through `arty::time`.
}
