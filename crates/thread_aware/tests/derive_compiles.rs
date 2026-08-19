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

// The cases below pin the `Send` obligation for phantom payloads whose `Send`-ness does
// not follow from `T: Send`. Bounding the type parameters instead of the field type made
// each of these fail to compile, and the snapshot tests could not catch it because they
// never build the expansion.

/// `&'a T` is `Send` only when `T: Sync`, so a per-parameter `T: Send` bound is wrong here.
#[derive(ThreadAware)]
struct PhantomRef<'a, T: 'a>(Tracker, PhantomData<&'a T>);

#[test]
fn phantom_shared_reference_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    // `i32` is `Sync`, which is what `&'a i32: Send` actually requires.
    let mut value = PhantomRef::<'_, i32>(Tracker::default(), PhantomData);
    value.relocate(source, destination);

    assert_eq!(value.0.relocations, 1);
}

/// `Arc<T>` is `Send` only when `T: Send + Sync`.
#[derive(ThreadAware)]
struct PhantomArc<T>(Tracker, PhantomData<Arc<T>>);

#[test]
fn phantom_arc_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    let mut value = PhantomArc::<i32>(Tracker::default(), PhantomData);
    value.relocate(source, destination);

    assert_eq!(value.0.relocations, 1);
}

/// An unsized slice payload: reached through no `Type::Slice` arm, so it used to get no bound.
#[derive(ThreadAware)]
struct PhantomSlice<T>(Tracker, PhantomData<[T]>);

#[test]
fn phantom_slice_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    let mut value = PhantomSlice::<i32>(Tracker::default(), PhantomData);
    value.relocate(source, destination);

    assert_eq!(value.0.relocations, 1);
}

/// An associated-type projection: `T: Send` says nothing about `T::Item`.
#[derive(ThreadAware)]
struct PhantomProjection<T: Iterator>(Tracker, PhantomData<T::Item>);

#[test]
fn phantom_projection_compiles_and_relocates() {
    let (source, destination) = affinity_pair();

    let mut value = PhantomProjection::<std::vec::IntoIter<i32>>(Tracker::default(), PhantomData);
    value.relocate(source, destination);

    assert_eq!(value.0.relocations, 1);
}

/// A parameter that is both relocated and named inside `PhantomData` carries both
/// obligations; classifying it as one or the other drops the second.
#[derive(ThreadAware)]
struct RelocatedAndPhantom<'a, T: 'a>(T, PhantomData<&'a T>);

#[test]
fn parameter_that_is_both_relocated_and_phantom_compiles() {
    let (source, destination) = affinity_pair();

    let mut value = RelocatedAndPhantom::<'_, Tracker>(Tracker::default(), PhantomData);
    value.relocate(source, destination);

    assert_eq!(value.0.relocations, 1);
}

/// A skipped field is never relocated, so it needs `Send` rather than `ThreadAware`.
#[derive(ThreadAware)]
struct SkippedGeneric<T> {
    tracked: Tracker,
    #[thread_aware(skip)]
    skipped: T,
}

#[test]
fn skipped_generic_field_needs_only_send() {
    let (source, destination) = affinity_pair();

    // `Arc<i32>` is `Send` but deliberately not `ThreadAware`; a skipped field must not
    // force the stronger bound.
    let mut value = SkippedGeneric {
        tracked: Tracker::default(),
        skipped: Arc::new(1_i32),
    };
    value.relocate(source, destination);

    assert_eq!(value.tracked.relocations, 1);
    assert_eq!(*value.skipped, 1, "the skipped field is left untouched");
}

/// A real, data-carrying type that merely happens to be named `PhantomData`.
mod lookalike {
    use thread_aware::affinity::Affinity;

    pub(crate) struct PhantomData<T> {
        pub(crate) value: T,
    }

    impl<T: thread_aware::ThreadAware> thread_aware::ThreadAware for PhantomData<T> {
        fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
            self.value.relocate(source, destination);
        }
    }
}

/// The body generator no longer decides what to relocate by matching the type's name, so a
/// look-alike carrying real data is relocated like any other field instead of being
/// silently dropped from the generated body.
#[derive(ThreadAware)]
struct HoldsLookalike<T: thread_aware::ThreadAware>(lookalike::PhantomData<T>);

#[test]
fn type_named_phantom_data_is_relocated_not_skipped() {
    let (source, destination) = affinity_pair();

    let mut value = HoldsLookalike(lookalike::PhantomData { value: Tracker::default() });
    value.relocate(source, destination);

    assert_eq!(value.0.value.relocations, 1, "a look-alike must be relocated, not skipped by name");
}
