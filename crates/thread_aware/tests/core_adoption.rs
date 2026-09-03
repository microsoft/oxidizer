// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "integration test")]

use std::sync::Arc as StdArc;
use std::thread;

use thread_aware::storage::Storage;
use thread_aware::{Arc, FromStorageError, NumaNode, Owner, PerThread, Thread, ThreadAware, ThreadBuilder};

#[test]
fn reexports_core_vocabulary() {
    fn accepts_core_types<T: ThreadAware>(_: Option<&Thread>, _: &Owner, _: &NumaNode) {}

    let thread = ThreadBuilder::default().build(thread::current().id());
    accepts_core_types::<u32>(Some(&thread), thread.owner(), thread.numa_node());
}

#[test]
fn relocate_across_owners_keeps_carried_value() {
    let source = ThreadBuilder::default().build(thread::current().id());
    let destination = ThreadBuilder::default().numa_node(1).build(thread::current().id());
    let mut value = Arc::<_, PerThread>::from_unaware(42_u32);
    let carried = value.clone().into_arc();

    value.relocate(Some(&source), &destination);

    assert!(StdArc::ptr_eq(&carried, &value.into_arc()));
}

#[test]
fn relocate_within_foreign_owner_keeps_original_carried_value() {
    let source = ThreadBuilder::default().build(thread::current().id());
    let foreign_builder = ThreadBuilder::default();
    let foreign_source = foreign_builder.build(thread::current().id());
    let foreign_destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let foreign_destination = foreign_builder.build(foreign_destination_id);
    let mut value = Arc::<_, PerThread>::from_unaware(42_u32);
    let carried = value.clone().into_arc();

    value.relocate(Some(&source), &foreign_source);
    value.relocate(Some(&foreign_source), &foreign_destination);

    assert!(StdArc::ptr_eq(&carried, &value.into_arc()));
}

#[test]
fn prebound_foreign_storage_cannot_publish_carried_value() {
    let source = ThreadBuilder::default().build(thread::current().id());
    let foreign_builder = ThreadBuilder::default();
    let foreign_source = foreign_builder.build(thread::current().id());
    let foreign_destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let foreign_destination = foreign_builder.build(foreign_destination_id);
    let mut value = Arc::<_, PerThread>::from_unaware(42_u32);
    let mut foreign_value = value.clone();
    let carried = value.clone().into_arc();

    foreign_value.relocate(Some(&foreign_source), &foreign_source);
    value.relocate(Some(&source), &foreign_source);
    value.relocate(Some(&foreign_source), &foreign_destination);

    assert!(StdArc::ptr_eq(&carried, &value.into_arc()));
}

#[test]
fn rejected_relocation_does_not_claim_unowned_value() {
    let owner_builder = ThreadBuilder::default();
    let owner_source = owner_builder.build(thread::current().id());
    let owner_destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let owner_destination = owner_builder.build(owner_destination_id);
    let foreign = ThreadBuilder::default().build(thread::current().id());
    let mut owner_value = Arc::<_, PerThread>::from_unaware(42_u32);
    let mut unowned_value = owner_value.clone();
    let carried = unowned_value.clone().into_arc();

    owner_value.relocate(Some(&owner_source), &owner_source);
    unowned_value.relocate(Some(&foreign), &foreign);
    unowned_value.relocate(Some(&owner_source), &owner_destination);

    assert!(!StdArc::ptr_eq(&carried, &unowned_value.into_arc()));
}

#[test]
fn relocate_within_owner_materializes_destination() {
    let builder = ThreadBuilder::default();
    let source = builder.build(thread::current().id());
    let destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let destination = builder.numa_node(1).build(destination_id);
    let mut value = Arc::<_, PerThread>::from_unaware(42_u32);
    let carried = value.clone().into_arc();

    value.relocate(Some(&source), &destination);

    assert!(!StdArc::ptr_eq(&carried, &value.into_arc()));
}

#[test]
fn cross_owner_no_op_does_not_claim_shared_storage() {
    let builder = ThreadBuilder::default();
    let source = builder.build(thread::current().id());
    let destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let destination = builder.build(destination_id);
    let foreign = ThreadBuilder::default().build(thread::current().id());
    let mut rejected = Arc::<_, PerThread>::from_unaware(42_u32);
    let mut sibling = rejected.clone();
    let carried = sibling.clone().into_arc();

    rejected.relocate(Some(&source), &foreign);
    sibling.relocate(Some(&source), &destination);

    assert!(!StdArc::ptr_eq(&carried, &sibling.into_arc()));
}

#[test]
fn try_from_storage_reports_owner_and_partition_errors() {
    let builder = ThreadBuilder::default();
    let populated = builder.build(thread::current().id());
    let missing_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let missing = builder.build(missing_id);
    let foreign = ThreadBuilder::default().build(thread::current().id());
    let storage = StdArc::new(Storage::<u32, PerThread>::new());
    storage.insert(&populated, StdArc::new(42)).unwrap();

    assert_eq!(
        Arc::try_from_storage(StdArc::clone(&storage), &missing).unwrap_err(),
        FromStorageError::EmptyPartition
    );
    assert_eq!(
        Arc::try_from_storage(storage, &foreign).unwrap_err(),
        FromStorageError::ForeignOwner
    );
}

#[test]
fn from_storage_errors_have_descriptive_messages() {
    assert_eq!(
        FromStorageError::ForeignOwner.to_string(),
        "storage belongs to another runtime owner"
    );
    assert_eq!(
        FromStorageError::EmptyPartition.to_string(),
        "the selected strategy partition is empty"
    );
}
