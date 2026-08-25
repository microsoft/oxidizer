// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]

use insta::assert_snapshot;
use quote::quote;
use thread_aware_macros_impl::derive_thread_aware;

fn expand(input: proc_macro2::TokenStream) -> String {
    // Use the canonical ::thread_aware root in test snapshots.
    let root: syn::Path = syn::parse_quote!(::thread_aware);
    let ts = derive_thread_aware(input, &root);
    // Pretty print if it parses as a file; fall back to raw tokens.
    syn::parse_file(&ts.to_string()).map_or_else(|_| ts.to_string(), |f| prettyplease::unparse(&f))
}

#[test]
#[cfg_attr(miri, ignore)]
fn struct_basic() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct Foo { a: u32, b: String }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn struct_attr_skip() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct Foo {
            #[thread_aware(skip)] id: u64,
            data: Vec<u8>,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn tuple_struct_and_enum() {
    let input = quote! {
        #[derive(ThreadAware)]
        enum E {
            A(u32),
            B(String, #[thread_aware(skip)] u8),
            C{ x: u32, y: Vec<u8> }
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_add_bounds() {
    // Both parameters gain a ThreadAware bound: the traversal reaches U through the marker's
    // type argument exactly as it reaches T directly.
    let input = quote! {
        #[derive(ThreadAware)]
        struct Gen<T, U>(T, core::marker::PhantomData<U>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_prebound_bare_no_dup() {
    // Ensures no duplicate ThreadAware bound when already present.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PreBound<T: ThreadAware>(T);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_prebound_fq_no_dup() {
    // Ensures no duplicate bound when already present with fully-qualified path.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PreBoundFq<T: ::thread_aware::ThreadAware>(T);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_ref_usage_adds_bound() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct RefUse<T>(&'static T);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_tuple_usage_adds_bound() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct TupUse<T>((T,));
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_array_usage_adds_bound() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct ArrUse<T>([T; 2]);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn error_unknown_attr() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct Bad { #[thread_aware(oops)] x: u32 }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_data_named_fields() {
    // PhantomData in named fields is relocated through its own no-op impl, and the parameter
    // inside it takes the ordinary bound.
    let input = quote! {
        #[derive(ThreadAware)]
        struct WithPhantom<T> {
            data: Vec<u8>,
            marker: core::marker::PhantomData<T>
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_data_unnamed_fields() {
    // PhantomData in tuple fields is relocated through its own no-op impl, and the parameter
    // inside it takes the ordinary bound.
    let input = quote! {
        #[derive(ThreadAware)]
        struct TupleWithPhantom<T>(Vec<u8>, core::marker::PhantomData<T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn union_not_supported() {
    let input = quote! {
        #[derive(ThreadAware)]
        union U { a: u32, b: u64 }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_group_usage_adds_bound() {
    // Covers Type::Group case by synthetically wrapping a type in a group node.
    use syn::{TypeGroup, parse_quote, token};

    // Start from an ordinary parsed tuple struct.
    let mut input: syn::DeriveInput = parse_quote! {
        #[derive(ThreadAware)]
        pub struct GroupUse<T>(T);
    };

    // Locate the single field and replace its type with a synthetic Type::Group wrapper
    // to exercise the Type::Group match arm (normally produced only after macro expansion
    // with a Delimiter::None group).
    if let syn::Data::Struct(ref mut ds) = input.data {
        if let syn::Fields::Unnamed(ref mut fs) = ds.fields {
            let field = fs.unnamed.first_mut().expect("expected one field");
            let original = field.ty.clone();
            field.ty = syn::Type::Group(TypeGroup {
                attrs: vec![],
                group_token: token::Group {
                    span: proc_macro2::Span::call_site(),
                },
                elem: Box::new(original),
            });
        } else {
            panic!("unexpected field shape")
        }
    } else {
        panic!("unexpected data shape")
    }

    let root: syn::Path = syn::parse_quote!(::thread_aware);
    let ts = derive_thread_aware(quote! {#input}, &root);
    let rendered = syn::parse_file(&ts.to_string()).map_or_else(|_| ts.to_string(), |f| prettyplease::unparse(&f));
    assert_snapshot!(rendered);
}

#[test]
#[cfg_attr(miri, ignore)]
fn enum_unnamed_phantom_data() {
    // PhantomData in enum unnamed fields is relocated like any other field.
    let input = quote! {
        #[derive(ThreadAware)]
        enum EnumUnnamedPhantom<T, U> {
            Variant(String, core::marker::PhantomData<T>),
            Other(u32, core::marker::PhantomData<U>),
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn enum_named_phantom_data() {
    // PhantomData in enum named fields is relocated like any other field, so the binding is
    // used and no `field: _` pattern is needed.
    let input = quote! {
        #[derive(ThreadAware)]
        enum EnumNamedPhantom<T, U> {
            Variant {
                data: Vec<u8>,
                marker: core::marker::PhantomData<T>,
            },
            Other {
                value: String,
                phantom: core::marker::PhantomData<U>,
            },
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn struct_unit() {
    // Unit structs should simply return self.
    let input = quote! {
        #[derive(ThreadAware)]
        struct UnitStruct;
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_paren_adds_bound() {
    // Covers Type::Paren case: parenthesized types like `(T)` should add ThreadAware bound.
    let input = quote! {
        #[derive(ThreadAware)]
        struct ParenthesizedType<T> {
            field: (T),
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_only_generic_gets_thread_aware_bound() {
    // The original defect: a parameter named only inside `PhantomData` used to gain no bound
    // at all, so the impl could not satisfy the `ThreadAware: Send` supertrait. It now takes
    // the ordinary bound, like a parameter reached anywhere else.
    let input = quote! {
        #[derive(ThreadAware)]
        struct DirectPhantom<T, U>(T, core::marker::PhantomData<U>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_reference_binds_its_parameter() {
    // The traversal reaches `T` through the reference and binds it by `ThreadAware`. That
    // gives `Send`, not `Sync`, so `&'a T: Send` is the author's to state - the derive does
    // not infer it.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomRef<'a, T: 'a + Sync>(core::marker::PhantomData<&'a T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn skipped_raw_pointer_marker_gets_only_self_send() {
    // A raw-pointer marker can never satisfy `PhantomData<*const T>: ThreadAware`, since that
    // reduces to `*const T: Send`, which no instantiation can prove. Skipping it moves the
    // obligation to `Self: Send`, which the manual `unsafe impl Send` such types carry
    // discharges.
    let input = quote! {
        #[derive(ThreadAware)]
        struct RawMarker<T>(usize, #[thread_aware(skip)] core::marker::PhantomData<*const T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn nested_phantom_binds_its_parameter() {
    // A marker nested in a relocated field is traversed like any other type argument, so the
    // parameter inside it takes the ordinary bound.
    let input = quote! {
        #[derive(ThreadAware)]
        struct NestedPhantom<T>((core::marker::PhantomData<T>,));
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn skipped_generic_field_gets_self_send_predicate() {
    // A skipped field is never relocated, so it needs no `ThreadAware` bound - only the
    // `Self: Send` predicate that keeps the supertrait satisfied.
    let input = quote! {
        #[derive(ThreadAware)]
        struct SkippedGeneric<T> {
            #[thread_aware(skip)]
            skipped: T,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn relocated_and_phantom_param_shares_one_bound() {
    // A parameter reached both directly and through a marker's type argument takes a single
    // `ThreadAware` bound - the two traversal paths converge on the same parameter.
    let input = quote! {
        #[derive(ThreadAware)]
        struct RelocatedAndPhantom<'a, T: 'a + Sync>(T, core::marker::PhantomData<&'a T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn unrelated_trait_named_thread_aware_does_not_suppress_the_bound() {
    // Matching only the final path segment treated `local::ThreadAware` as the real trait and
    // dropped the bound the generated body needs.
    let input = quote! {
        #[derive(ThreadAware)]
        struct CustomTa<T: local::ThreadAware>(T);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn prebound_bare_thread_aware_assumed_real() {
    // The real trait, written by the user, must still suppress the generated duplicate -
    // otherwise clippy reports the redundancy against the user's own source line.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PreBoundTa<T: ThreadAware>(T);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn no_skipped_field_means_no_self_send_predicate() {
    // Every field is relocated, so `Self: Send` follows from the per-parameter bounds.
    let input = quote! {
        #[derive(ThreadAware)]
        struct AllRelocated<T, U>(T, U);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn user_where_clause_is_preserved() {
    // Generated bounds land inline on the impl generics, so the author's own `where` clause
    // has to survive untouched beside them.
    let input = quote! {
        #[derive(ThreadAware)]
        struct WithWhere<T, U>
        where
            T: Clone,
        {
            value: T,
            marker: core::marker::PhantomData<U>,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_lifetime_and_const_params_untouched() {
    // Only type parameters can carry bounds; lifetimes and const generics are skipped.
    // The field shape must be one the crate can actually relocate; pinning an expansion
    // that cannot compile is the blind spot this PR exists to close.
    let input = quote! {
        #[derive(ThreadAware)]
        struct Mixed<'a, const N: usize, T: Sync>(Tracker, core::marker::PhantomData<(&'a T, [u8; N])>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn enum_variant_nested_phantom_binds_its_parameter() {
    // Exercises merging parameter bounds across enum variants.
    let input = quote! {
        #[derive(ThreadAware)]
        enum NestedPhantomEnum<T, U> {
            Wrapped((core::marker::PhantomData<T>,)),
            Also((core::marker::PhantomData<U>,)),
            Plain(U),
        }
    };
    assert_snapshot!(expand(input));
}
