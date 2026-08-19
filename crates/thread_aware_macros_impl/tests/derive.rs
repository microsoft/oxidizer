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
    // Only T should gain a ThreadAware bound (U appears only inside PhantomData).
    let input = quote! {
        #[derive(ThreadAware)]
        struct Gen<T, U>(T, core::marker::PhantomData<U>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_prebound_no_dup() {
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
    // PhantomData in named fields should be passed through without transformation.
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
    // PhantomData in tuple fields should be passed through without transformation.
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
    // PhantomData in enum unnamed fields should be passed through without transformation.
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
    // PhantomData in enum named fields should be passed through without transformation.
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
fn phantom_prebound_send_emits_redundant_predicate() {
    // The `Send` obligation is a where-predicate on the phantom argument, so a parameter
    // that already carries `Send` inline picks up a redundant but legal duplicate. That is
    // deliberate: suppressing it required matching the bound by name, which silently
    // dropped the real bound for any unrelated trait also called `Send`.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomPreBound<T: Send>(core::marker::PhantomData<T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_prebound_thread_aware_emits_redundant_predicate() {
    // Likewise for a parameter already bound by `ThreadAware`, which implies `Send`.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomPreBoundTa<T: ThreadAware>(core::marker::PhantomData<T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_unrelated_trait_named_send_still_gets_predicate() {
    // A user trait whose last path segment is `Send` must not be mistaken for
    // `core::marker::Send` and suppress the real obligation.
    let input = quote! {
        #[derive(ThreadAware)]
        struct CustomSend<T: local::Send>(core::marker::PhantomData<T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_concrete_argument_gets_no_predicate() {
    // The argument names no generic parameter, so there is nothing to constrain.
    let input = quote! {
        #[derive(ThreadAware)]
        struct ConcretePhantom<T>(T, core::marker::PhantomData<u32>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_lifetime_and_const_params_untouched() {
    // Only type parameters can carry bounds; lifetimes and const generics are skipped.
    let input = quote! {
        #[derive(ThreadAware)]
        struct Mixed<'a, const N: usize, T>(&'a T, [u8; N], core::marker::PhantomData<T>);
    };
    assert_snapshot!(expand(input));
}
#[test]
#[cfg_attr(miri, ignore)]
fn generics_unused_param_gets_no_bound() {
    // A type parameter that no field mentions is neither relocated nor phantom,
    // so it must be left completely unbound.
    let input = quote! {
        #[derive(ThreadAware)]
        struct UnusedParam<T, U>(T);
    };
    assert_snapshot!(expand(input));
}
#[test]
#[cfg_attr(miri, ignore)]
fn phantom_ref_bounds_the_reference_not_the_parameter() {
    // `&'a T` is `Send` only when `T: Sync`, so the predicate must name the reference
    // itself. Binding `T: Send` here produced an impl that did not compile.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomRef<'a, T: 'a>(core::marker::PhantomData<&'a T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_slice_gets_send_bound() {
    // An unsized slice payload: `[T]: Send` holds exactly when `T: Send`.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomSlice<T>(core::marker::PhantomData<[T]>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_projection_gets_send_bound() {
    // `T: Send` says nothing about `T::Item`, so the projection carries the obligation.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomProjection<T: Iterator>(core::marker::PhantomData<T::Item>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn relocated_and_phantom_param_gets_both_obligations() {
    // A parameter used directly and inside `PhantomData` carries both bounds; treating
    // the two as mutually exclusive dropped the phantom one.
    let input = quote! {
        #[derive(ThreadAware)]
        struct RelocatedAndPhantom<'a, T: 'a>(T, core::marker::PhantomData<&'a T>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn skipped_generic_field_gets_send_bound() {
    // A skipped field is never relocated, so it needs `Send` rather than `ThreadAware`.
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
fn phantom_tuple_generic_gets_send_bound() {
    // A generic reachable only through a tuple inside `PhantomData`.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomTuple<T>(core::marker::PhantomData<(T,)>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_array_generic_gets_send_bound() {
    // A generic reachable only through an array inside `PhantomData`.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomArray<T>(core::marker::PhantomData<[T; 2]>);
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_paren_generic_gets_send_bound() {
    // A generic reachable only through a parenthesized type inside `PhantomData`.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PhantomParen<T> {
            marker: core::marker::PhantomData<(T)>,
        }
    };
    assert_snapshot!(expand(input));
}

#[test]
#[cfg_attr(miri, ignore)]
fn phantom_group_generic_gets_send_bound() {
    // Covers the Type::Group arm inside `PhantomData`, synthesizing the group node
    // that normally appears only after macro expansion (Delimiter::None).
    use syn::{TypeGroup, parse_quote, token};

    let mut input: syn::DeriveInput = parse_quote! {
        #[derive(ThreadAware)]
        pub struct PhantomGroup<T>(core::marker::PhantomData<T>);
    };

    // Wrap the `T` argument of `PhantomData<T>` in a synthetic group.
    if let syn::Data::Struct(ref mut ds) = input.data
        && let syn::Fields::Unnamed(ref mut fs) = ds.fields
    {
        let field = fs.unnamed.first_mut().expect("expected one field");
        if let syn::Type::Path(ref mut tp) = field.ty {
            let segment = tp.path.segments.last_mut().expect("expected a path segment");
            if let syn::PathArguments::AngleBracketed(ref mut ab) = segment.arguments {
                let arg = ab.args.first_mut().expect("expected one type argument");
                if let syn::GenericArgument::Type(ty) = arg {
                    let original = ty.clone();
                    *ty = syn::Type::Group(TypeGroup {
                        attrs: vec![],
                        group_token: token::Group {
                            span: proc_macro2::Span::call_site(),
                        },
                        elem: Box::new(original),
                    });
                } else {
                    panic!("unexpected generic argument shape")
                }
            } else {
                panic!("unexpected path arguments shape")
            }
        } else {
            panic!("unexpected field type shape")
        }
    } else {
        panic!("unexpected data shape")
    }

    let root: syn::Path = syn::parse_quote!(::thread_aware);
    let ts = derive_thread_aware(quote! {#input}, &root);
    let rendered = syn::parse_file(&ts.to_string()).map_or_else(|_| ts.to_string(), |f| prettyplease::unparse(&f));
    assert_snapshot!(rendered);
}
