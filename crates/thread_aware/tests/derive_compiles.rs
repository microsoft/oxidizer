// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]
#![allow(dead_code, reason = "This is a test module")]

use core::marker::PhantomData;
use std::rc::Rc;
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

/// A real, data-carrying type that merely happens to be named `PhantomData`, named by a
/// qualified path.
///
/// Every field is relocated, whatever its type is called, so this carries no special risk.
/// The name match decides only which predicate the field gets, and here it decides nothing:
/// a qualified path is not the marker, so the traversal descends into the argument as usual.
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

#[derive(ThreadAware)]
struct HoldsLookalike<T>(lookalike::PhantomData<T>);

#[test]
fn qualified_type_named_phantom_data_is_relocated() {
    let (source, destination) = affinity_pair();

    let mut value = HoldsLookalike(lookalike::PhantomData { value: Tracker::default() });
    value.relocate(source, destination);

    assert_eq!(
        value.0.value.relocations, 1,
        "a qualified look-alike is a normal field and must be relocated"
    );
}

/// The same look-alike, imported under the bare name the derive cannot tell apart from the
/// real marker.
///
/// This is the case a syntactic classifier can never get right, and the reason relocation no
/// longer depends on one. The field is relocated because every field without the skip
/// attribute is; the name
/// match only sends the bound to the field's own type, where the look-alike's own impl
/// reduces it correctly.
mod bare_lookalike {
    use thread_aware::affinity::Affinity;
    use thread_aware_macros::ThreadAware;

    pub(crate) struct Marker<T> {
        pub(crate) value: T,
    }

    impl<T: thread_aware::ThreadAware> thread_aware::ThreadAware for Marker<T> {
        fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
            self.value.relocate(source, destination);
        }
    }

    use Marker as PhantomData;

    #[derive(ThreadAware)]
    pub(crate) struct HoldsBareLookalike<T>(pub(crate) PhantomData<T>);
}

#[test]
fn bare_type_named_phantom_data_is_relocated() {
    let (source, destination) = affinity_pair();

    let mut value = bare_lookalike::HoldsBareLookalike(bare_lookalike::Marker { value: Tracker::default() });
    value.relocate(source, destination);

    assert_eq!(
        value.0.value.relocations, 1,
        "a bare look-alike is a normal field and must be relocated"
    );
}

// The two cases below pin the `Send` obligation for types made `Send` by a manual
// `unsafe impl` rather than structurally. A marker whose argument can never be `Send` is
// relocated like any other field, so its own `PhantomData<X>: ThreadAware` predicate would
// reduce to `*const T: Send` or `Rc<()>: Send` - unprovable. `#[thread_aware(skip)]` moves it
// under `where Self: Send`, which the manual impl discharges.

fn assert_thread_aware<X: thread_aware::ThreadAware>() {}

/// The standard raw-pointer variance marker.
#[derive(ThreadAware)]
struct RawMarker<T> {
    len: usize,
    #[thread_aware(skip)]
    marker: PhantomData<*const T>,
}

// SAFETY: test-only. The marker carries no value and `RawMarker` owns nothing but a `usize`.
unsafe impl<T> Send for RawMarker<T> {}

#[test]
fn phantom_raw_pointer_with_manual_send_is_usable() {
    assert_thread_aware::<RawMarker<i32>>();

    let (source, destination) = affinity_pair();
    let mut value = RawMarker::<i32> {
        len: 3,
        marker: PhantomData,
    };
    value.relocate(source, destination);

    assert_eq!(value.len, 3);
}

/// Exactly what `#[thread_aware(skip)]` exists for: a field whose type is not `Send`.
///
/// The field is a capability-free marker rather than a live `Rc`, so the manual `Send`
/// implementation below promises nothing about data the type does not own. An unconditional
/// `unsafe impl Send` over a real `Rc<T>` would be unsound: `skip` only controls whether the
/// generated body calls `relocate`, it does not stop safe code from cloning the `Rc` and then
/// moving the outer value across a thread boundary.
struct NotSendMarker<T>(PhantomData<Rc<T>>);

#[derive(ThreadAware)]
struct SkippedNotSend<T> {
    tracked: Tracker,
    #[thread_aware(skip)]
    cache: NotSendMarker<T>,
}

// SAFETY: test-only. `NotSendMarker` owns no value and exposes no destructor, dereference or
// safe accessor, so `SkippedNotSend` carries no thread-affine capability to transfer.
#[expect(clippy::non_send_fields_in_send_ty, reason = "deliberate: this is the shape `skip` exists for")]
unsafe impl<T> Send for SkippedNotSend<T> {}

#[test]
fn skipped_non_send_field_with_manual_send_is_usable() {
    assert_thread_aware::<SkippedNotSend<i32>>();

    let (source, destination) = affinity_pair();
    let mut value = SkippedNotSend {
        tracked: Tracker::default(),
        cache: NotSendMarker::<i32>(PhantomData),
    };
    value.relocate(source, destination);

    assert_eq!(value.tracked.relocations, 1, "the skipped field emits no relocation");
}

/// A named enum variant whose fields are called `source` and `destination`, and one whose
/// fields reuse the names the generated method's parameters actually have.
///
/// The generated arm binds every relocated field to a name of the derive's own choosing, so no
/// field name can reach the relocation call. The `source`/`destination` variant is the shape
/// that failed before the parameters were renamed; `Generated` is the shape that would fail
/// today if the rebinding were removed.
#[derive(ThreadAware)]
enum ShadowingFieldNames<T> {
    Both {
        source: Tracker,
        destination: Tracker,
    },
    Marker {
        source: PhantomData<T>,
    },
    Generated {
        __thread_aware_source: Tracker,
        __thread_aware_destination: Tracker,
    },
}

/// Constants named exactly like the bindings the derive used to generate.
///
/// A binding cannot shadow a constant in scope: the identifier is read as a pattern referring
/// to the constant rather than as a binding, so a generated name that a caller might plausibly
/// declare breaks the derive. The generated names are obscure for that reason; `_v0` was the
/// old spelling and is exactly the kind of name a caller could reasonably use.
#[expect(
    non_upper_case_globals,
    reason = "deliberately spelled like the binding the derive used to generate"
)]
const _v0: usize = 0;

#[expect(
    non_upper_case_globals,
    reason = "deliberately spelled like the binding the derive used to generate"
)]
const _v1: usize = 1;

#[derive(ThreadAware)]
enum ConstCapture {
    Tuple(Tracker, Tracker),
    Named { first: Tracker, second: Tracker },
}

/// A caller type shadowing a name the generated signature uses.
///
/// The derive spells every path it emits in full, so a local `Option` cannot change what the
/// generated `relocate` signature means.
mod shadowed_prelude {
    use thread_aware_macros::ThreadAware;

    use super::Tracker;

    pub(crate) enum Option<T> {
        Only(T),
    }

    #[derive(ThreadAware)]
    pub(crate) struct Holder {
        pub(crate) tracked: Tracker,
    }

    #[derive(ThreadAware)]
    pub(crate) enum Wrapped {
        V(Tracker),
    }

    pub(crate) fn only(value: &Option<u8>) -> u8 {
        match value {
            Option::Only(v) => *v,
        }
    }
}

#[test]
fn a_shadowed_prelude_name_does_not_break_the_derive() {
    let (source, destination) = affinity_pair();

    let mut holder = shadowed_prelude::Holder {
        tracked: Tracker::default(),
    };
    holder.relocate(source, destination);
    assert_eq!(holder.tracked.relocations, 1);

    let mut wrapped = shadowed_prelude::Wrapped::V(Tracker::default());
    wrapped.relocate(source, destination);
    match &wrapped {
        shadowed_prelude::Wrapped::V(t) => assert_eq!(t.relocations, 1),
    }

    assert_eq!(shadowed_prelude::only(&shadowed_prelude::Option::Only(7)), 7);
}

/// Constants and const parameters named exactly like parameters of the generated method.
///
/// A function parameter does not shadow either: a `const` or const parameter of the same name
/// is read as a pattern referring to that item, and a `static` may not be shadowed at all.
/// Both fail to compile, so the generated method names its parameters obscurely, as the field
/// bindings do.
///
/// Scoped to its own module for two reasons: a `const destination` at file scope would be
/// matched against by every `let (source, destination) = ...` in the tests below, and the
/// lint exemption these deliberate spellings need should not apply to the whole file.
mod colliding_parameter_names {
    #![expect(non_upper_case_globals, reason = "deliberately spelled like the generated parameters")]

    use thread_aware_macros::ThreadAware;

    use super::Tracker;

    pub(crate) const destination: usize = 0;

    #[derive(ThreadAware)]
    pub(crate) struct ConstParamNamedSource<const source: usize> {
        pub(crate) tracked: Tracker,
    }

    #[derive(ThreadAware)]
    pub(crate) struct NamedAfterConst {
        pub(crate) tracked: Tracker,
    }
}

#[test]
fn caller_constants_do_not_collide_with_generated_parameters() {
    let (source, destination) = affinity_pair();

    let mut value = colliding_parameter_names::ConstParamNamedSource::<3> {
        tracked: Tracker::default(),
    };
    value.relocate(source, destination);
    assert_eq!(value.tracked.relocations, 1);

    let mut other = colliding_parameter_names::NamedAfterConst {
        tracked: Tracker::default(),
    };
    other.relocate(source, destination);
    assert_eq!(other.tracked.relocations, 1);

    let colliding = colliding_parameter_names::destination;
    assert_eq!(colliding, 0, "the colliding constant is untouched");
}

#[test]
fn caller_constants_do_not_collide_with_generated_bindings() {
    let (source, destination) = affinity_pair();

    let mut tuple = ConstCapture::Tuple(Tracker::default(), Tracker::default());
    tuple.relocate(source, destination);
    match &tuple {
        ConstCapture::Tuple(a, b) => {
            assert_eq!(a.relocations, 1);
            assert_eq!(b.relocations, 1);
        }
        ConstCapture::Named { .. } => unreachable!(),
    }

    let mut named = ConstCapture::Named {
        first: Tracker::default(),
        second: Tracker::default(),
    };
    named.relocate(source, destination);
    match &named {
        ConstCapture::Named { first, second } => {
            assert_eq!(first.relocations, 1);
            assert_eq!(second.relocations, 1);
        }
        ConstCapture::Tuple(..) => unreachable!(),
    }

    assert_eq!(_v0 + _v1, 1, "the colliding constants are untouched");
}

#[test]
fn variant_fields_named_after_relocate_parameters_compile() {
    let (source, destination) = affinity_pair();

    let mut value = ShadowingFieldNames::<i32>::Both {
        source: Tracker::default(),
        destination: Tracker::default(),
    };
    value.relocate(source, destination);

    match &value {
        ShadowingFieldNames::Both { source: a, destination: b } => {
            assert_eq!(a.relocations, 1, "a field named `source` is still relocated");
            assert_eq!(b.relocations, 1, "a field named `destination` is still relocated");
        }
        ShadowingFieldNames::Marker { .. } | ShadowingFieldNames::Generated { .. } => unreachable!(),
    }

    let mut generated = ShadowingFieldNames::<i32>::Generated {
        __thread_aware_source: Tracker::default(),
        __thread_aware_destination: Tracker::default(),
    };
    generated.relocate(source, destination);
    match &generated {
        ShadowingFieldNames::Generated {
            __thread_aware_source: a,
            __thread_aware_destination: b,
        } => {
            assert_eq!(a.relocations, 1, "a field named like a generated parameter is relocated");
            assert_eq!(b.relocations, 1, "a field named like a generated parameter is relocated");
        }
        _ => unreachable!(),
    }

    let mut marker = ShadowingFieldNames::<i32>::Marker { source: PhantomData };
    marker.relocate(source, destination);
}

/// An enum with no variants has nothing to relocate.
///
/// The derive emits no body for it. `match self {}` would be rejected as non-exhaustive,
/// because a `&mut` reference is inhabited however uninhabited its referent is.
#[derive(ThreadAware)]
enum Uninhabited {}

#[test]
fn empty_enum_derives_a_usable_impl() {
    assert_thread_aware::<Uninhabited>();
}

// `ManualSendMarker` below pins the one case that still needs `Self: Send`: a skipped field
// whose type can never be `Send` structurally. The two after it pin the opposite - that a
// marker's obligation must name the marker type rather than scan for generic-parameter
// tokens, since each has a `Send`-ness no such scan can see.

/// A marker-only type whose argument is not `Send`, made `Send` by hand.
#[derive(ThreadAware)]
struct ManualSendMarker(#[thread_aware(skip)] PhantomData<Rc<()>>);

// SAFETY: test-only. The marker holds no value.
unsafe impl Send for ManualSendMarker {}

/// A const-parameterised type that owns no data: no pointee, no destructor, no dereference and
/// no safe accessor. The raw pointer inside the marker exists only to stop the compiler from
/// deriving `Send` automatically.
struct MaybeSend<const N: usize>(PhantomData<*const ()>);

// SAFETY: test-only. The type holds no value, so moving one transfers no capability at all.
// `N == 0` is singled out purely to exercise `Send`-ness that depends on a const parameter.
unsafe impl Send for MaybeSend<0> {}

/// `Send`-ness depending on a const parameter, which a type-parameter scan never sees.
#[derive(ThreadAware)]
struct ConstDependent<const N: usize> {
    tracked: Tracker,
    marker: PhantomData<MaybeSend<N>>,
}

macro_rules! hidden_generic {
    () => {
        T
    };
}

/// A generic hidden behind a type macro, so no `T` token is visible before expansion.
#[derive(ThreadAware)]
struct MacroHidden<T> {
    tracked: Tracker,
    marker: PhantomData<hidden_generic!()>,
}

#[test]
fn send_ness_invisible_to_a_syntactic_scan_still_compiles() {
    let (source, destination) = affinity_pair();

    assert_thread_aware::<ManualSendMarker>();
    assert_thread_aware::<ConstDependent<0>>();
    assert_thread_aware::<MacroHidden<i32>>();

    let mut value = ConstDependent::<0> {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source, destination);
    assert_eq!(value.tracked.relocations, 1);

    let mut hidden = MacroHidden::<i32> {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    hidden.relocate(source, destination);
    assert_eq!(hidden.tracked.relocations, 1);
}

/// A trait of the user's own that happens to be called `ThreadAware`, named by a qualified
/// path.
///
/// Matching only the final path segment treated this as the real trait and dropped the bound
/// the generated body needs, so the impl could not compile. A bare `ThreadAware` stays
/// ambiguous and is still assumed to be the real trait - see `is_same_trait`.
mod own_thread_aware {
    use thread_aware::affinity::Affinity;
    use thread_aware_macros::ThreadAware as DeriveThreadAware;

    pub(crate) mod inner {
        pub(crate) trait ThreadAware {}
    }

    pub(crate) struct Inner;

    impl inner::ThreadAware for Inner {}

    impl thread_aware::ThreadAware for Inner {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
    }

    #[derive(DeriveThreadAware)]
    pub(crate) struct Holder<T: inner::ThreadAware>(pub(crate) T);
}

#[test]
fn user_trait_named_thread_aware_does_not_suppress_the_real_bound() {
    let (source, destination) = affinity_pair();

    let mut value = own_thread_aware::Holder(own_thread_aware::Inner);
    value.relocate(source, destination);
}

/// A named enum variant carrying a skipped field.
///
/// The generated match arm emits no statement for a skipped field, so binding it by name left
/// an unused variable; the `Skipped` variant is what pins the `field: _` binding. The `Marked`
/// variant is bound and relocated like any other. `deny(warnings)` is deliberate: it is what a
/// downstream crate would hit, and it is what this shape used to fail.
mod enum_named_bindings {
    #![deny(warnings)]

    use core::marker::PhantomData;

    use thread_aware_macros::ThreadAware;

    #[derive(ThreadAware)]
    pub(crate) enum NamedVariants<T, U> {
        Marked {
            value: super::Tracker,
            marker: PhantomData<T>,
        },
        Skipped {
            value: super::Tracker,
            #[thread_aware(skip)]
            ignored: U,
        },
    }
}

#[test]
fn enum_named_variant_bindings_do_not_warn() {
    use enum_named_bindings::NamedVariants;

    let (source, destination) = affinity_pair();

    let mut marked = NamedVariants::<i32, u8>::Marked {
        value: Tracker::default(),
        marker: PhantomData,
    };
    marked.relocate(source, destination);
    match &marked {
        NamedVariants::Marked { value, .. } => assert_eq!(value.relocations, 1),
        NamedVariants::Skipped { .. } => unreachable!("constructed as Marked"),
    }

    let mut skipped = NamedVariants::<i32, u8>::Skipped {
        value: Tracker::default(),
        ignored: 7,
    };
    skipped.relocate(source, destination);
    match &skipped {
        NamedVariants::Skipped { value, ignored } => {
            assert_eq!(value.relocations, 1);
            assert_eq!(*ignored, 7, "the skipped field is left untouched");
        }
        NamedVariants::Marked { .. } => unreachable!("constructed as Skipped"),
    }
}
