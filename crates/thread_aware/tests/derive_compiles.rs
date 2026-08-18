// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]
#![allow(dead_code, reason = "This is a test module")]

use core::marker::PhantomData;
use std::sync::Arc;

use thread_aware::ThreadAware as _;
use thread_aware::affinity::{Affinity, pinned_affinities};
use thread_aware_macros::ThreadAware;

#[derive(ThreadAware)]
struct Inner(u32);

#[derive(ThreadAware)]
struct Container<T: thread_aware::ThreadAware> {
    val: T,
    #[thread_aware(skip)]
    raw: usize,
}

#[test]
fn derive_thread_aware_compiles_and_calls() {
    let mut addrs = pinned_affinities(&[2]);
    let a = Some(addrs.remove(0));
    let b = addrs.remove(0);
    let mut c = Container { val: Inner(5), raw: 10 };
    thread_aware::ThreadAware::relocate(&mut c, a, b);
}

/// Counts how many times it was relocated, so tests can assert which fields the
/// generated body actually reaches.
#[derive(Default)]
struct Tracker {
    relocations: usize,
}

impl thread_aware::ThreadAware for Tracker {
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
        self.relocations += 1;
    }
}

fn affinity_pair() -> (Option<Affinity>, Affinity) {
    let affinities = pinned_affinities(&[2]);
    (Some(affinities[0]), affinities[1])
}

/// A generic named only inside `PhantomData` must still compile.
///
/// Regression test: the derive used to drop the parameter entirely, so the
/// generated impl could not satisfy the `ThreadAware: Send` supertrait.
#[derive(ThreadAware)]
struct DirectPhantom<T, U>(T, PhantomData<U>);

#[test]
fn phantom_only_generic_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    // `Arc<i32>` is `Send` but deliberately not `ThreadAware`, so this only
    // compiles if the phantom parameter is bound by `Send` rather than `ThreadAware`.
    let mut value = DirectPhantom::<Tracker, Arc<i32>>(Tracker::default(), PhantomData);
    value.relocate(source, destination);

    assert_eq!(value.0.relocations, 1, "the non-phantom field must be relocated exactly once");
}

/// `PhantomData` nested inside another type is relocated like any other field,
/// which requires a real `ThreadAware` impl for `PhantomData`.
#[derive(ThreadAware)]
struct NestedPhantom<T>((PhantomData<T>,));

#[test]
fn nested_phantom_data_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    let mut value = NestedPhantom::<Arc<i32>>((PhantomData,));
    value.relocate(source, destination);
}

/// A concrete `PhantomData` of a type that is `Send` but not `ThreadAware`.
#[derive(ThreadAware)]
struct ConcretePhantom {
    tracked: Tracker,
    marker: PhantomData<Arc<i32>>,
}

#[test]
fn concrete_phantom_of_non_thread_aware_type_compiles() {
    let (source, destination) = affinity_pair();

    let mut value = ConcretePhantom {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source, destination);

    assert_eq!(value.tracked.relocations, 1);
}

/// Mixes a relocated generic, a skipped field and a phantom-only generic.
#[derive(ThreadAware)]
struct Mixed<T, U> {
    tracked: T,
    #[thread_aware(skip)]
    skipped: Tracker,
    marker: PhantomData<U>,
}

#[test]
fn relocate_reaches_every_non_skipped_field_and_no_skipped_one() {
    let (source, destination) = affinity_pair();

    let mut value = Mixed::<Tracker, Arc<i32>> {
        tracked: Tracker::default(),
        skipped: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source, destination);

    assert_eq!(value.tracked.relocations, 1, "non-skipped fields must be relocated");
    assert_eq!(value.skipped.relocations, 0, "#[thread_aware(skip)] fields must not be relocated");
}

/// Enum variants carrying phantom-only generics.
#[derive(ThreadAware)]
enum PhantomEnum<T, U> {
    Tracked(T),
    Marked(PhantomData<U>),
}

#[test]
fn enum_with_phantom_only_generic_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    let mut tracked = PhantomEnum::<Tracker, Arc<i32>>::Tracked(Tracker::default());
    tracked.relocate(source, destination);
    match &tracked {
        PhantomEnum::Tracked(t) => assert_eq!(t.relocations, 1),
        PhantomEnum::Marked(_) => unreachable!("constructed as Tracked"),
    }

    let mut marked = PhantomEnum::<Tracker, Arc<i32>>::Marked(PhantomData);
    marked.relocate(source, destination);
}
