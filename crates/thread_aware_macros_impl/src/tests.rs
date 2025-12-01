// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, coverage(off))]

use insta::assert_snapshot;
use quote::quote;

#[cfg(test)]
fn expand(input: proc_macro2::TokenStream) -> String {
    // Use the canonical ::thread_aware root in test snapshots.
    let root: syn::Path = syn::parse_quote!(::thread_aware);
    let ts = super::derive_thread_aware(input, &root);
    // Pretty print if it parses as a file; fall back to raw tokens.
    syn::parse_file(&ts.to_string()).map_or_else(|_| ts.to_string(), |f| prettyplease::unparse(&f))
}

#[test]
fn struct_basic() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct Foo { a: u32, b: String }
    };
    assert_snapshot!(expand(input), @r#"
    impl ::thread_aware::ThreadAware for Foo {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let Self { a, b } = self;
            Self {
                a: ::thread_aware::ThreadAware::relocated(a, source, destination),
                b: ::thread_aware::ThreadAware::relocated(b, source, destination),
            }
        }
    }
    "#);
}

#[test]
fn struct_attr_skip() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct Foo {
            #[thread_aware(skip)] id: u64,
            data: Vec<u8>,
        }
    };
    assert_snapshot!(expand(input), @r#"
    impl ::thread_aware::ThreadAware for Foo {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let Self { id, data } = self;
            Self {
                id: id,
                data: ::thread_aware::ThreadAware::relocated(data, source, destination),
            }
        }
    }
    "#);
}

#[test]
fn tuple_struct_and_enum() {
    let input = quote! {
        #[derive(ThreadAware)]
        enum E {
            A(u32),
            B(String, #[thread_aware(skip)] u8),
            C{ x: u32, y: Vec<u8> }
        }
    };
    assert_snapshot!(expand(input), @r#"
    impl ::thread_aware::ThreadAware for E {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            match self {
                Self::A(_v0) => {
                    Self::A(::thread_aware::ThreadAware::relocated(_v0, source, destination))
                }
                Self::B(_v0, _v1) => {
                    Self::B(
                        ::thread_aware::ThreadAware::relocated(_v0, source, destination),
                        _v1,
                    )
                }
                Self::C { x, y } => {
                    Self::C {
                        x: ::thread_aware::ThreadAware::relocated(x, source, destination),
                        y: ::thread_aware::ThreadAware::relocated(y, source, destination),
                    }
                }
            }
        }
    }
    "#);
}

#[test]
fn generics_add_bounds() {
    // Only T should gain a ThreadAware bound (U appears only inside PhantomData).
    let input = quote! {
        #[derive(ThreadAware)]
        struct Gen<T, U>(T, core::marker::PhantomData<U>);
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ::thread_aware::ThreadAware, U> ::thread_aware::ThreadAware for Gen<T, U> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let Gen(_f0, _f1) = self;
            Gen(::thread_aware::ThreadAware::relocated(_f0, source, destination), _f1)
        }
    }
    "#);
}

#[test]
fn generics_prebound_no_dup() {
    // Ensures no duplicate ThreadAware bound when already present.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PreBound<T: ThreadAware>(T);
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ThreadAware> ::thread_aware::ThreadAware for PreBound<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let PreBound(_f0) = self;
            PreBound(::thread_aware::ThreadAware::relocated(_f0, source, destination))
        }
    }
    "#);
}

#[test]
fn generics_prebound_fq_no_dup() {
    // Ensures no duplicate bound when already present with fully-qualified path.
    let input = quote! {
        #[derive(ThreadAware)]
        struct PreBoundFq<T: ::thread_aware::ThreadAware>(T);
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ::thread_aware::ThreadAware> ::thread_aware::ThreadAware for PreBoundFq<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let PreBoundFq(_f0) = self;
            PreBoundFq(::thread_aware::ThreadAware::relocated(_f0, source, destination))
        }
    }
    "#);
}

#[test]
fn generics_ref_usage_adds_bound() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct RefUse<T>(&'static T);
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ::thread_aware::ThreadAware> ::thread_aware::ThreadAware for RefUse<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let RefUse(_f0) = self;
            RefUse(::thread_aware::ThreadAware::relocated(_f0, source, destination))
        }
    }
    "#);
}

#[test]
fn generics_tuple_usage_adds_bound() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct TupUse<T>((T,));
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ::thread_aware::ThreadAware> ::thread_aware::ThreadAware for TupUse<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let TupUse(_f0) = self;
            TupUse(::thread_aware::ThreadAware::relocated(_f0, source, destination))
        }
    }
    "#);
}

#[test]
fn generics_array_usage_adds_bound() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct ArrUse<T>([T; 2]);
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ::thread_aware::ThreadAware> ::thread_aware::ThreadAware for ArrUse<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let ArrUse(_f0) = self;
            ArrUse(::thread_aware::ThreadAware::relocated(_f0, source, destination))
        }
    }
    "#);
}

#[test]
fn error_unknown_attr() {
    let input = quote! {
        #[derive(ThreadAware)]
        struct Bad { #[thread_aware(oops)] x: u32 }
    };
    assert_snapshot!(expand(input), @r#"
::core::compile_error! {
    "unknown thread_aware attribute (only 'skip' is supported)"
}
"#);
}

#[test]
fn phantom_data_named_fields() {
    // PhantomData in named fields should be passed through without transformation.
    let input = quote! {
        #[derive(ThreadAware)]
        struct WithPhantom<T> {
            data: Vec<u8>,
            marker: core::marker::PhantomData<T>
        }
    };
    assert_snapshot!(expand(input), @r#"
    impl<T> ::thread_aware::ThreadAware for WithPhantom<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let Self { data, marker } = self;
            Self {
                data: ::thread_aware::ThreadAware::relocated(data, source, destination),
                marker: marker,
            }
        }
    }
    "#);
}

#[test]
fn phantom_data_unnamed_fields() {
    // PhantomData in tuple fields should be passed through without transformation.
    let input = quote! {
        #[derive(ThreadAware)]
        struct TupleWithPhantom<T>(Vec<u8>, core::marker::PhantomData<T>);
    };
    assert_snapshot!(expand(input), @r#"
    impl<T> ::thread_aware::ThreadAware for TupleWithPhantom<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let TupleWithPhantom(_f0, _f1) = self;
            TupleWithPhantom(
                ::thread_aware::ThreadAware::relocated(_f0, source, destination),
                _f1,
            )
        }
    }
    "#);
}

#[test]
fn union_not_supported() {
    let input = quote! {
        #[derive(ThreadAware)]
        union U { a: u32, b: u64 }
    };
    assert_snapshot!(expand(input), @r##"
::core::compile_error! {
    "#[derive(ThreadAware)] does not support unions"
}
"##);
}

#[test]
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
    let ts = super::derive_thread_aware(quote! {#input}, &root);
    let rendered = syn::parse_file(&ts.to_string()).map_or_else(|_| ts.to_string(), |f| prettyplease::unparse(&f));
    assert_snapshot!(rendered, @r#"
    impl<T: ::thread_aware::ThreadAware> ::thread_aware::ThreadAware for GroupUse<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let GroupUse(_f0) = self;
            GroupUse(::thread_aware::ThreadAware::relocated(_f0, source, destination))
        }
    }
    "#);
}

#[test]
fn enum_unnamed_phantom_data() {
    // PhantomData in enum unnamed fields should be passed through without transformation.
    let input = quote! {
        #[derive(ThreadAware)]
        enum EnumUnnamedPhantom<T, U> {
            Variant(String, core::marker::PhantomData<T>),
            Other(u32, core::marker::PhantomData<U>),
        }
    };
    assert_snapshot!(expand(input), @r#"
    impl<T, U> ::thread_aware::ThreadAware for EnumUnnamedPhantom<T, U> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            match self {
                Self::Variant(_v0, _v1) => {
                    Self::Variant(
                        ::thread_aware::ThreadAware::relocated(_v0, source, destination),
                        _v1,
                    )
                }
                Self::Other(_v0, _v1) => {
                    Self::Other(
                        ::thread_aware::ThreadAware::relocated(_v0, source, destination),
                        _v1,
                    )
                }
            }
        }
    }
    "#);
}

#[test]
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
    assert_snapshot!(expand(input), @r#"
    impl<T, U> ::thread_aware::ThreadAware for EnumNamedPhantom<T, U> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            match self {
                Self::Variant { data, marker } => {
                    Self::Variant {
                        data: ::thread_aware::ThreadAware::relocated(
                            data,
                            source,
                            destination,
                        ),
                        marker: marker,
                    }
                }
                Self::Other { value, phantom } => {
                    Self::Other {
                        value: ::thread_aware::ThreadAware::relocated(
                            value,
                            source,
                            destination,
                        ),
                        phantom: phantom,
                    }
                }
            }
        }
    }
    "#);
}

#[test]
fn struct_unit() {
    // Unit structs should simply return self.
    let input = quote! {
        #[derive(ThreadAware)]
        struct UnitStruct;
    };
    assert_snapshot!(expand(input), @r#"
    impl ::thread_aware::ThreadAware for UnitStruct {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            self
        }
    }
    "#);
}

#[test]
fn generics_paren_adds_bound() {
    // Covers Type::Paren case: parenthesized types like `(T)` should add ThreadAware bound.
    let input = quote! {
        #[derive(ThreadAware)]
        struct ParenthesizedType<T> {
            field: (T),
        }
    };
    assert_snapshot!(expand(input), @r#"
    impl<T: ::thread_aware::ThreadAware> ::thread_aware::ThreadAware
    for ParenthesizedType<T> {
        #[allow(
            clippy::redundant_clone,
            reason = "macro generated pattern moves each field once"
        )]
        fn relocated(
            self,
            source: ::thread_aware::MemoryAffinity,
            destination: ::thread_aware::PinnedAffinity,
        ) -> Self {
            let Self { field } = self;
            Self {
                field: ::thread_aware::ThreadAware::relocated(field, source, destination),
            }
        }
    }
    "#);
}
