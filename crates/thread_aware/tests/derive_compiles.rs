// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]
#![allow(dead_code, reason = "This is a test module")]

use core::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

use thread_aware::thread::ThreadBuilder;
use thread_aware::{Thread, ThreadAware as _};
use thread_aware_macros::ThreadAware;

fn test_threads(counts: &[usize]) -> Vec<Thread> {
    let builder = ThreadBuilder::default();
    counts
        .iter()
        .enumerate()
        .flat_map(|(numa_node, count)| {
            let builder = builder
                .clone()
                .numa_node(numa_node.try_into().expect("test NUMA node index must fit"));
            (0..*count).map(move |_| {
                let thread_id = thread::spawn(|| thread::current().id()).join().expect("test thread should finish");
                builder.build(thread_id)
            })
        })
        .collect()
}

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
    let mut addrs = test_threads(&[2]);
    let a = addrs.remove(0);
    let b = addrs.remove(0);
    let mut c = Container { val: Inner(5), raw: 10 };
    thread_aware::ThreadAware::relocate(&mut c, Some(&a), &b);
}

/// Counts how many times it was relocated, so tests can assert which fields the
/// generated body actually reaches.
#[derive(Default)]
struct Tracker {
    relocations: usize,
}

impl thread_aware::ThreadAware for Tracker {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        self.relocations += 1;
    }
}

fn thread_pair() -> (Option<Thread>, Thread) {
    let mut threads = test_threads(&[2]);
    (Some(threads.remove(0)), threads.remove(0))
}

/// A generic named only inside `PhantomData` must still compile.
///
/// The parameter takes the ordinary `ThreadAware` bound, like one reached anywhere else.
/// Without that bound the generated impl cannot satisfy the `ThreadAware: Send` supertrait.
#[derive(ThreadAware)]
struct DirectPhantom<T, U>(T, PhantomData<U>);

#[test]
fn phantom_only_generic_compiles_and_relocates() {
    let (source, destination) = thread_pair();

    let mut value = DirectPhantom::<Tracker, Tracker>(Tracker::default(), PhantomData);
    value.relocate(source.as_ref(), &destination);

    assert_eq!(value.0.relocations, 1, "the non-phantom field must be relocated exactly once");
}

/// `PhantomData` nested inside another type is relocated like any other field,
/// which requires a real `ThreadAware` impl for `PhantomData`.
#[derive(ThreadAware)]
struct NestedPhantom<T>((PhantomData<T>,));

#[test]
fn nested_phantom_data_compiles_and_relocates() {
    let (source, destination) = thread_pair();

    let mut value = NestedPhantom::<Tracker>((PhantomData,));
    value.relocate(source.as_ref(), &destination);
}

/// A concrete `PhantomData` of a type that is `Send` but not `ThreadAware`.
#[derive(ThreadAware)]
struct ConcretePhantom {
    tracked: Tracker,
    marker: PhantomData<Arc<i32>>,
}

#[test]
fn concrete_phantom_of_non_thread_aware_type_compiles() {
    let (source, destination) = thread_pair();

    let mut value = ConcretePhantom {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source.as_ref(), &destination);

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
    let (source, destination) = thread_pair();

    let mut value = Mixed::<Tracker, Tracker> {
        tracked: Tracker::default(),
        skipped: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source.as_ref(), &destination);

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
    let (source, destination) = thread_pair();

    let mut tracked = PhantomEnum::<Tracker, Tracker>::Tracked(Tracker::default());
    tracked.relocate(source.as_ref(), &destination);
    match &tracked {
        PhantomEnum::Tracked(t) => assert_eq!(t.relocations, 1),
        PhantomEnum::Marked(_) => unreachable!("constructed as Tracked"),
    }

    let mut marked = PhantomEnum::<Tracker, Tracker>::Marked(PhantomData);
    marked.relocate(source.as_ref(), &destination);
}

// The cases below pin what a marker payload costs. The traversal binds the parameters it
// reaches by `ThreadAware`, which gives `Send` but says nothing about `Sync`, about an
// associated type, or about a payload the traversal cannot reach at all. Where the payload's
// `Send`-ness does not follow, the author writes the bound - the derive does not infer it.

/// A shared-reference payload, carried through a fn pointer so it is `Send` for every `T`.
#[derive(ThreadAware)]
struct PhantomRef<'a, T: 'a>(Tracker, PhantomData<fn(&'a T)>);

#[test]
fn phantom_shared_reference_compiles_and_relocates() {
    let (source, destination) = thread_pair();

    let mut value = PhantomRef::<'_, Rc<()>>(Tracker::default(), PhantomData);
    value.relocate(source.as_ref(), &destination);

    assert_eq!(value.0.relocations, 1);
}

/// An unsized slice payload, which the traversal does not enter, so nothing is bound for it.
#[derive(ThreadAware)]
struct PhantomSlice<T>(Tracker, PhantomData<fn(&[T])>);

#[test]
fn phantom_slice_compiles_and_relocates() {
    let (source, destination) = thread_pair();

    let mut value = PhantomSlice::<Rc<()>>(Tracker::default(), PhantomData);
    value.relocate(source.as_ref(), &destination);

    assert_eq!(value.0.relocations, 1);
}

/// An associated-type projection: a bound on `T` says nothing about `T::Item`, and binding
/// `T: ThreadAware` to reach it is both too strong and beside the point. Skip it.
#[derive(ThreadAware)]
struct PhantomProjection<T: Iterator>(Tracker, PhantomData<fn(T::Item)>);

#[test]
fn phantom_projection_compiles_and_relocates() {
    let (source, destination) = thread_pair();

    let mut value = PhantomProjection::<std::vec::IntoIter<i32>>(Tracker::default(), PhantomData);
    value.relocate(source.as_ref(), &destination);

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
    let (source, destination) = thread_pair();

    // `Arc<i32>` is `Send` but deliberately not `ThreadAware`; a skipped field must not
    // force the stronger bound.
    let mut value = SkippedGeneric {
        tracked: Tracker::default(),
        skipped: Arc::new(1_i32),
    };
    value.relocate(source.as_ref(), &destination);

    assert_eq!(value.tracked.relocations, 1);
    assert_eq!(*value.skipped, 1, "the skipped field is left untouched");
}

/// A real, data-carrying type that merely happens to be named `PhantomData`, named by a
/// qualified path.
///
/// The derive does no name matching on field types at all, so this is an ordinary field:
/// relocated, and traversed for the parameters it contains.
mod lookalike {
    use thread_aware::Thread;

    pub(crate) struct PhantomData<T> {
        pub(crate) value: T,
    }

    impl<T: thread_aware::ThreadAware> thread_aware::ThreadAware for PhantomData<T> {
        fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
            self.value.relocate(source, destination);
        }
    }
}

#[derive(ThreadAware)]
struct HoldsLookalike<T>(lookalike::PhantomData<T>);

#[test]
fn qualified_type_named_phantom_data_is_relocated() {
    let (source, destination) = thread_pair();

    let mut value = HoldsLookalike(lookalike::PhantomData { value: Tracker::default() });
    value.relocate(source.as_ref(), &destination);

    assert_eq!(
        value.0.value.relocations, 1,
        "a qualified look-alike is a normal field and must be relocated"
    );
}

/// The same look-alike, imported under the bare name.
///
/// The derive matches no name on a field type, so this is an ordinary field: relocated and
/// traversed like any other, with the look-alike's own impl deciding what its bound reduces
/// to. A syntactic classifier could not tell this shape apart from the real marker.
mod bare_lookalike {
    use thread_aware::Thread;
    use thread_aware_macros::ThreadAware;

    pub(crate) struct Marker<T> {
        pub(crate) value: T,
    }

    impl<T: thread_aware::ThreadAware> thread_aware::ThreadAware for Marker<T> {
        fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
            self.value.relocate(source, destination);
        }
    }

    use Marker as PhantomData;

    #[derive(ThreadAware)]
    pub(crate) struct HoldsBareLookalike<T>(pub(crate) PhantomData<T>);
}

#[test]
fn bare_type_named_phantom_data_is_relocated() {
    let (source, destination) = thread_pair();

    let mut value = bare_lookalike::HoldsBareLookalike(bare_lookalike::Marker { value: Tracker::default() });
    value.relocate(source.as_ref(), &destination);

    assert_eq!(
        value.0.value.relocations, 1,
        "a bare look-alike is a normal field and must be relocated"
    );
}

// A marker only needs a `Send` payload, and one can always be written: a function-pointer
// payload carries the parameter for variance while staying `Send` whatever the parameter is.
// A type that genuinely must not be `Send` cannot implement `ThreadAware` at all, since the
// trait requires it - so no marker shape needs `#[thread_aware(skip)]` or a manual
// `unsafe impl Send` to get through the derive.

fn assert_thread_aware<X: thread_aware::ThreadAware>() {}

/// The standard raw-pointer variance marker, written in its `Send` form.
///
/// `PhantomData<*const T>` would make the whole struct `!Send`; `PhantomData<fn(*const T)>`
/// carries the same parameter, keeps the variance intent, and is `Send` for every `T`.
#[derive(ThreadAware)]
struct RawMarker<T> {
    len: usize,
    marker: PhantomData<fn(*const T)>,
}

#[test]
fn raw_pointer_variance_marker_needs_no_escape_hatch() {
    // No `skip`, no `unsafe impl`, and the payload is not `Send` on its own.
    assert_thread_aware::<RawMarker<*const u8>>();

    let (source, destination) = thread_pair();
    let mut value = RawMarker::<i32> {
        len: 3,
        marker: PhantomData,
    };
    value.relocate(source.as_ref(), &destination);

    assert_eq!(value.len, 3);
}

/// A payload that is neither `Send` nor `ThreadAware`, carried the same way.
#[derive(ThreadAware)]
struct HoldsNonSendPayload<T> {
    tracked: Tracker,
    marker: PhantomData<fn(Rc<T>)>,
}

#[test]
fn a_non_send_payload_still_derives_through_a_fn_marker() {
    assert_thread_aware::<HoldsNonSendPayload<Rc<()>>>();

    let (source, destination) = thread_pair();
    let mut value = HoldsNonSendPayload::<i32> {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source.as_ref(), &destination);

    assert_eq!(value.tracked.relocations, 1);
}

/// A named enum variant whose fields are called `source` and `destination`, and one whose
/// fields reuse the names the generated method's parameters actually have.
///
/// The generated arm binds every relocated field to a name of the derive's own choosing, so no
/// field name can reach the relocation call. Without that rebinding either variant shadows a
/// parameter of the generated `relocate` and passes a field where a `Thread` is expected.
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

/// Constants spelled like a binding a derive might plausibly generate.
///
/// A binding cannot shadow a constant in scope: the identifier is read as a pattern referring
/// to the constant rather than as a new binding, so any generated name a caller might also
/// declare breaks the derive. `_v0` is exactly such a name, which is why fields bind to
/// `__thread_aware_field_{i}` instead.
#[expect(
    non_upper_case_globals,
    reason = "deliberately spelled like a binding a derive might plausibly generate"
)]
const _v0: usize = 0;

#[expect(
    non_upper_case_globals,
    reason = "deliberately spelled like a binding a derive might plausibly generate"
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
    let (source, destination) = thread_pair();

    let mut holder = shadowed_prelude::Holder {
        tracked: Tracker::default(),
    };
    holder.relocate(source.as_ref(), &destination);
    assert_eq!(holder.tracked.relocations, 1);

    let mut wrapped = shadowed_prelude::Wrapped::V(Tracker::default());
    wrapped.relocate(source.as_ref(), &destination);
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
    let (source, destination) = thread_pair();

    let mut value = colliding_parameter_names::ConstParamNamedSource::<3> {
        tracked: Tracker::default(),
    };
    value.relocate(source.as_ref(), &destination);
    assert_eq!(value.tracked.relocations, 1);

    let mut other = colliding_parameter_names::NamedAfterConst {
        tracked: Tracker::default(),
    };
    other.relocate(source.as_ref(), &destination);
    assert_eq!(other.tracked.relocations, 1);

    let colliding = colliding_parameter_names::destination;
    assert_eq!(colliding, 0, "the colliding constant is untouched");
}

#[test]
fn caller_constants_do_not_collide_with_generated_bindings() {
    let (source, destination) = thread_pair();

    let mut tuple = ConstCapture::Tuple(Tracker::default(), Tracker::default());
    tuple.relocate(source.as_ref(), &destination);
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
    named.relocate(source.as_ref(), &destination);
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
    let (source, destination) = thread_pair();

    let mut value = ShadowingFieldNames::<i32>::Both {
        source: Tracker::default(),
        destination: Tracker::default(),
    };
    value.relocate(source.as_ref(), &destination);

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
    generated.relocate(source.as_ref(), &destination);
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
    marker.relocate(source.as_ref(), &destination);
}

/// An enum with no variants has nothing to relocate.
///
/// The derive emits `match *self {}`, which is accepted because `*self` is a place of an
/// uninhabited type. `match self {}` would not be: a `&mut` reference is inhabited however
/// uninhabited its referent is.
#[derive(ThreadAware)]
enum Uninhabited {}

#[test]
fn empty_enum_derives_a_usable_impl() {
    assert_thread_aware::<Uninhabited>();
}

// The two below carry payloads the traversal cannot reason about - one behind a const
// parameter, one behind a type macro. Written in the `Send` marker form, neither needs
// `#[thread_aware(skip)]` or a manual `unsafe impl Send`.

/// A payload whose `Send`-ness would depend on a const argument.
struct ConstPayload<const N: usize>(PhantomData<*const ()>);

/// The derive binds nothing for a const parameter; the fn-pointer marker means it need not.
#[derive(ThreadAware)]
struct ConstDependent<const N: usize> {
    tracked: Tracker,
    marker: PhantomData<fn(ConstPayload<N>)>,
}

macro_rules! hidden_generic {
    () => {
        T
    };
}

/// A generic hidden behind a type macro, so no `T` token is visible before expansion.
///
/// No syntactic traversal can reach it, and none needs to.
#[derive(ThreadAware)]
struct MacroHidden<T> {
    tracked: Tracker,
    marker: PhantomData<fn(hidden_generic!())>,
}

#[test]
fn payloads_invisible_to_a_syntactic_scan_need_no_escape_hatch() {
    let (source, destination) = thread_pair();

    // Both instantiated with arguments that are neither `Send` nor `ThreadAware`.
    assert_thread_aware::<ConstDependent<7>>();
    assert_thread_aware::<MacroHidden<Rc<()>>>();

    let mut value = ConstDependent::<0> {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    value.relocate(source.as_ref(), &destination);
    assert_eq!(value.tracked.relocations, 1);

    let mut hidden = MacroHidden::<i32> {
        tracked: Tracker::default(),
        marker: PhantomData,
    };
    hidden.relocate(source.as_ref(), &destination);
    assert_eq!(hidden.tracked.relocations, 1);
}

/// A trait of the user's own that happens to be called `ThreadAware`, named by a qualified
/// path.
///
/// Comparing only the final path segment would treat this as the real trait and drop the bound
/// the generated body needs, leaving an impl that cannot compile. A bare `ThreadAware` is
/// inherently ambiguous and is assumed to be the real trait - see `is_same_trait`.
mod own_thread_aware {
    use thread_aware::Thread;
    use thread_aware_macros::ThreadAware as DeriveThreadAware;

    pub(crate) mod inner {
        pub(crate) trait ThreadAware {}
    }

    pub(crate) struct Inner;

    impl inner::ThreadAware for Inner {}

    impl thread_aware::ThreadAware for Inner {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[derive(DeriveThreadAware)]
    pub(crate) struct Holder<T: inner::ThreadAware>(pub(crate) T);
}

#[test]
fn user_trait_named_thread_aware_does_not_suppress_the_real_bound() {
    let (source, destination) = thread_pair();

    let mut value = own_thread_aware::Holder(own_thread_aware::Inner);
    value.relocate(source.as_ref(), &destination);
}

/// A named enum variant carrying a skipped field.
///
/// The generated match arm emits no statement for a skipped field, so binding it by name would
/// leave an unused variable; the `Skipped` variant pins the `field: _` binding. The `Marked`
/// variant is bound and relocated like any other. `deny(warnings)` is deliberate: it is the
/// gate a downstream crate applies to the generated code.
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

    let (source, destination) = thread_pair();

    let mut marked = NamedVariants::<i32, u8>::Marked {
        value: Tracker::default(),
        marker: PhantomData,
    };
    marked.relocate(source.as_ref(), &destination);
    match &marked {
        NamedVariants::Marked { value, .. } => assert_eq!(value.relocations, 1),
        NamedVariants::Skipped { .. } => unreachable!("constructed as Marked"),
    }

    let mut skipped = NamedVariants::<i32, u8>::Skipped {
        value: Tracker::default(),
        ignored: 7,
    };
    skipped.relocate(source.as_ref(), &destination);
    match &skipped {
        NamedVariants::Skipped { value, ignored } => {
            assert_eq!(value.relocations, 1);
            assert_eq!(*ignored, 7, "the skipped field is left untouched");
        }
        NamedVariants::Marked { .. } => unreachable!("constructed as Skipped"),
    }
}
