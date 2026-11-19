// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process error shape and diagnostic contracts.

use ohno_macros_impl::{derive_error, enrich_err, error};
use proc_macro2::{TokenStream, TokenTree};
use quote::quote;

#[test]
fn gamma_round2_injected_unit_core_is_a_terminated_tuple_declaration() {
    let output = error(
        quote!(),
        quote!(
            pub struct UnitError;
        ),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let syn::Item::Struct(item) = &parsed.items[0] else {
        panic!("expected struct")
    };
    assert!(matches!(item.fields, syn::Fields::Unnamed(_)));
    assert!(item.semi_token.is_some());
}

#[test]
fn gamma_survivor_unrelated_docs_do_not_disable_constructors_or_select_core() {
    let output = derive_error(quote!(
        #[doc = "unrelated"]
        struct Example {
            #[doc = "field docs"]
            a: u32,
            core: OhnoCore,
        }
    ));
    let parsed: syn::File = syn::parse2(output).unwrap();
    let new = parsed
        .items
        .iter()
        .find_map(|item| {
            let syn::Item::Impl(item) = item else { return None };
            item.items.iter().find_map(|item| {
                let syn::ImplItem::Fn(function) = item else { return None };
                (function.sig.ident == "new").then_some(function)
            })
        })
        .unwrap();
    assert_eq!(new.sig.inputs.len(), 1);
    let args = &new.sig.inputs;
    assert!(quote!(#args).to_string().contains("Into < u32 >"));
}

#[test]
fn gamma_survivor_associated_ohno_core_is_not_selected_by_spelling() {
    let messages = diagnostics(derive_error(quote!(
        struct Example {
            core: <T as Trait>::OhnoCore,
        }
    )));
    assert!(
        messages.iter().any(|message| message.starts_with("No field holds the OhnoCore")),
        "{messages:?}"
    );
}

#[cfg(test)]
fn diagnostics(tokens: TokenStream) -> Vec<String> {
    let mut messages = Vec::new();
    let mut iter = tokens.into_iter();
    while let Some(token) = iter.next() {
        match token {
            TokenTree::Ident(ident) if ident == "compile_error" => {
                assert!(matches!(iter.next(), Some(TokenTree::Punct(punct)) if punct.as_char() == '!'));
                let Some(TokenTree::Group(group)) = iter.next() else {
                    panic!("diagnostic must carry a message")
                };
                messages.push(syn::parse2::<syn::LitStr>(group.stream()).unwrap().value());
            }
            TokenTree::Group(group) => messages.extend(diagnostics(group.stream())),
            _ => {}
        }
    }
    messages
}

#[test]
fn gamma_survivor_attribute_injects_unique_core_and_unit_semicolon() {
    let output = error(
        quote!(),
        quote!(
            struct Example;
        ),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let syn::Item::Struct(item) = &parsed.items[0] else {
        panic!("must produce a struct")
    };
    assert!(item.semi_token.is_some());
    assert_eq!(item.fields.len(), 1);
    let output = error(
        quote!(),
        quote!(
            struct Example {
                ohno_core: u32,
                ohno_core_1: u32,
            }
        ),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let syn::Item::Struct(item) = &parsed.items[0] else {
        panic!("must produce a struct")
    };
    assert_eq!(item.fields.len(), 3);
    assert_eq!(item.fields.iter().next_back().unwrap().ident.as_ref().unwrap(), "ohno_core_2");
    let output = error(
        quote!(),
        quote!(
            struct SparseNames {
                ohno_core: u32,
                ohno_core_2: u32,
            }
        ),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let syn::Item::Struct(item) = &parsed.items[0] else {
        panic!("must produce a struct")
    };
    assert_eq!(item.fields.iter().next_back().unwrap().ident.as_ref().unwrap(), "ohno_core_1");
    assert!(
        !diagnostics(error(
            quote!(),
            quote!(
                struct Example {
                    #[error]
                    core: Core,
                }
            )
        ))
        .is_empty()
    );
}

#[test]
fn gamma_survivor_error_marker_must_be_bare() {
    let output = derive_error(quote!(
        struct Example {
            #[error(value)]
            core: Core,
        }
    ));
    let messages = diagnostics(output);
    assert!(messages.iter().any(|message| message.contains("`#[error]`")), "{messages:?}");
    assert!(!messages.iter().any(|message| message.is_empty() || message.contains("xyzzy")));
}

#[test]
fn gamma_survivor_conversion_unknown_key_lists_all_fields() {
    let output = derive_error(quote!(
        #[from(Source(missing: 7))]
        struct Example {
            core: OhnoCore,
            a: u32,
            b: String,
        }
    ));
    assert_eq!(
        diagnostics(output),
        ["unknown field `missing` in `#[from(...)]`, available fields: `a`, `b`"]
    );
    let output = derive_error(quote!(
        #[from(Source(8: 7))]
        struct Example(OhnoCore, u32);
    ));
    assert_eq!(diagnostics(output), ["unknown field `8` in `#[from(...)]`, available fields: `1`"]);
}

#[test]
fn gamma_survivor_malformed_conversion_and_message_arguments() {
    for args in [quote!(Source(a)), quote!(Source(a:)), quote!(Source(: 3))] {
        assert!(
            !diagnostics(derive_error(quote!(
                #[from(#args)]
                struct Example {
                    core: OhnoCore,
                    a: u32,
                }
            )))
            .is_empty()
        );
    }
    for args in [quote!("{}",), quote!("{{"), quote!("}}")] {
        let output = enrich_err(
            args,
            quote!(
                fn example() -> Result<(), Error> {
                    Ok(())
                }
            ),
        )
        .to_string();
        assert!(output.contains(":: std :: format !"), "{output}");
    }
    assert!(
        !diagnostics(enrich_err(
            quote!("{}", +),
            quote!(
                fn example() -> Result<(), Error> {
                    Ok(())
                }
            )
        ))
        .is_empty()
    );
}
