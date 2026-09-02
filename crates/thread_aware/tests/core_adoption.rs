// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "integration test")]

#[test]
#[ignore = "implementation stub"]
fn reexports_core_vocabulary() {
    // Assert that ThreadAware, Thread, Owner, and NumaNode are available from thread_aware.
}

#[test]
#[ignore = "implementation stub"]
fn storage_starts_with_default_capacity() {
    // Construct Storage and assert that it reserves capacity for at least 32 entries.
}

#[test]
#[ignore = "implementation stub"]
fn relocate_across_owners_keeps_carried_value() {
    // Relocate an Arc between different owners and assert that its carried allocation is unchanged.
}

#[test]
#[ignore = "implementation stub"]
fn relocate_within_owner_materializes_destination() {
    // Relocate an Arc within one owner and assert that the destination-keyed value is materialized.
}
