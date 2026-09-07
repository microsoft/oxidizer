// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[test]
#[ignore = "contract stub"]
fn public_traits_have_expected_object_safety() {
    // Assert that Parker and BlockingTaskSpawner can be used as trait objects.
}

#[test]
#[ignore = "contract stub"]
fn driver_can_remain_thread_local() {
    // Implement Driver for a type containing Rc and assert it is neither Send nor Sync.
}

#[test]
#[ignore = "contract stub"]
fn driver_init_exposes_runtime_facilities() {
    // Verify the worker, waiting-point availability, and blocking task spawner.
}

#[test]
#[ignore = "contract stub"]
fn provider_creation_is_typed_and_fallible() {
    // Create a driver through a consumed provider returning a typed Result.
}

#[test]
#[ignore = "contract stub"]
fn shutdown_future_begins_and_polls_shutdown() {
    // Poll Driver::shutdown and verify begin_shutdown and poll_shutdown are invoked.
}

#[test]
#[ignore = "contract stub"]
fn different_driver_types_have_distinct_identity() {
    // Compare TypeIds for two version-shaped driver types.
}

#[test]
#[ignore = "contract stub"]
fn parker_contract_supports_latched_wakeup() {
    // Wake before parking and verify the next park returns immediately.
}
