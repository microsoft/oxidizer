// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process relocation derive contracts.

use quote::quote;
use syn::parse_quote;
use thread_aware_macros_impl::derive_thread_aware;

#[test]
fn gamma_round2_tuple_struct_skip_is_not_relocated() {
    let output = derive_thread_aware(
        quote!(
            struct Example<T, U>(#[thread_aware(skip)] T, U);
        ),
        &parse_quote!(::thread_aware),
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let text = output.to_string();
    assert_eq!(text.matches(":: ThreadAware :: relocate").count(), 1);
    assert!(!text.contains("self . 0"));
    assert!(text.contains("self . 1"));
    assert!(!text.contains(&quote!(T: ::thread_aware::ThreadAware).to_string()));
    assert!(text.contains(&quote!(U: ::thread_aware::ThreadAware).to_string()));
}

#[test]
fn gamma_survivor_skip_and_enum_arms() {
    let root = parse_quote!(::thread_aware);
    let output = derive_thread_aware(
        quote!(
            enum Example<T, U> {
                Empty,
                Named {
                    #[thread_aware(skip)]
                    ignored: U,
                    active: T,
                },
                Tuple(#[thread_aware(skip)] U, T),
            }
        ),
        &root,
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let text = output.to_string();
    for expected in [
        quote!(T: ::thread_aware::ThreadAware),
        quote!(Example<T, U>: ::core::marker::Send),
        quote!(Self::Empty => {}),
        quote!(ignored: _),
    ] {
        assert!(text.contains(&expected.to_string()), "{expected}");
    }
    assert!(!text.contains(&quote!(U: ::thread_aware::ThreadAware).to_string()));
    assert_eq!(text.matches(":: ThreadAware :: relocate").count(), 2);
}

#[test]
fn gamma_survivor_skipped_field_does_not_end_generic_scan() {
    let root = parse_quote!(::thread_aware);
    let output = derive_thread_aware(
        quote!(
            struct Example<T, U> {
                #[thread_aware(skip)]
                first: U,
                second: (Vec<T>, [T; 2]),
            }
        ),
        &root,
    )
    .to_string();
    assert!(output.contains(&quote!(T: ::thread_aware::ThreadAware).to_string()));
    assert!(!output.contains(&quote!(U: ::thread_aware::ThreadAware).to_string()));
    assert!(!output.contains("self . first"));
    assert!(output.contains("self . second"));
}

#[test]
fn gamma_survivor_field_parser_errors_propagate() {
    let root = parse_quote!(::thread_aware);
    for input in [
        quote!(
            struct Example {
                #[thread_aware(unknown)]
                a: u32,
            }
        ),
        quote!(
            struct Example(#[thread_aware(unknown)] u32);
        ),
        quote!(
            enum Example {
                A {
                    #[thread_aware(unknown)]
                    a: u32,
                },
            }
        ),
        quote!(
            enum Example {
                A(#[thread_aware(unknown)] u32),
            }
        ),
        quote!(
            struct Example {
                #[thread_aware(skip =)]
                a: u32,
            }
        ),
    ] {
        let output = derive_thread_aware(input, &root);
        let diagnostic: syn::ItemMacro = syn::parse2(output).unwrap();
        assert_eq!(diagnostic.mac.path.segments.last().unwrap().ident, "compile_error");
        assert!(!syn::parse2::<syn::LitStr>(diagnostic.mac.tokens).unwrap().value().is_empty());
    }
}
