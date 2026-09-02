// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "integration test")]

use std::sync::Arc as StdArc;
use std::thread;

use thread_aware::thread::ThreadBuilder;
use thread_aware::{Arc, NumaNode, Owner, PerThread, Thread, ThreadAware};

#[test]
fn reexports_core_vocabulary() {
    fn accepts_core_types<T: ThreadAware>(_: Option<&Thread>, _: &Owner, _: &NumaNode) {}

    let thread = ThreadBuilder::default().build(thread::current().id());
    accepts_core_types::<u32>(Some(&thread), thread.owner(), thread.numa_node());
}

#[test]
fn relocate_across_owners_keeps_carried_value() {
    let source = ThreadBuilder::default().build(thread::current().id());
    let destination = ThreadBuilder::default().with_numa_node(1).build(thread::current().id());
    let mut value = Arc::<_, PerThread>::from_unaware(42_u32);
    let carried = value.clone().into_arc();

    value.relocate(Some(&source), &destination);

    assert!(StdArc::ptr_eq(&carried, &value.into_arc()));
}

#[test]
fn relocate_within_owner_materializes_destination() {
    let builder = ThreadBuilder::default();
    let source = builder.build(thread::current().id());
    let destination_id = thread::spawn(|| thread::current().id()).join().unwrap();
    let destination = builder.with_numa_node(1).build(destination_id);
    let mut value = Arc::<_, PerThread>::from_unaware(42_u32);
    let carried = value.clone().into_arc();

    value.relocate(Some(&source), &destination);

    assert!(!StdArc::ptr_eq(&carried, &value.into_arc()));
}
