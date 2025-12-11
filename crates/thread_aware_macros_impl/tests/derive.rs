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
