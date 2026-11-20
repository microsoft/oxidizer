// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process telemetry schema validation.

use observed_macros_impl::{derive_enrichment, event};
use quote::quote;

#[test]
fn gamma_survivor_enrichment_copies_borrowed_strings_only() {
    let borrowed = derive_enrichment(quote!(
        struct Example<'a> {
            #[unredacted]
            value: &'a str,
        }
    ))
    .unwrap()
    .to_string();
    let owned = derive_enrichment(quote!(
        struct Example {
            #[unredacted]
            value: String,
        }
    ))
    .unwrap()
    .to_string();
    assert!(borrowed.contains(&quote!(::std::sync::Arc::<str>::from(self.value)).to_string()));
    assert!(!owned.contains("Arc :: < str >"));
}

#[test]
fn gamma_survivor_message_scan_visits_every_placeholder() {
    for message in ["{{literal}} {missing}", "{a} {missing}", "{{ {a} {missing}"] {
        let error = event(
            quote!("example"),
            quote!(
                #[info(#message)]
                struct Example {
                    a: u32,
                }
            ),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "log message references `{missing}` but no log attribute with that name exists; available attributes: [a]"
        );
    }
    for message in ["literal", "{a}", "{{literal}} {a}", "{a} {unterminated"] {
        syn::parse2::<syn::File>(
            event(
                quote!("example"),
                quote!(
                    #[info(#message)]
                    struct Example {
                        a: u32,
                    }
                ),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn gamma_survivor_malformed_event_and_dimension_arguments() {
    for args in [quote!(), quote!("example", 1), quote!("example", =)] {
        let error = event(
            args,
            quote!(
                struct Example {
                    a: u32,
                }
            ),
        )
        .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
    for attr in [
        quote!(#[info("message" name = "override")]),
        quote!(#[info(name =)]),
        quote!(#[info(name "override")]),
        quote!(#[info(true = "override")]),
        quote!(#[counter(a name = "override")]),
        quote!(#[counter(name =)]),
        quote!(#[counter(desc = "d", unit =)]),
    ] {
        assert!(event(quote!("example"), quote!(#attr struct Example { a: u32 })).is_err(), "{attr}");
    }
    for attrs in [
        quote!(#[dimension("old")]),
        quote!(#[dimension(log "name")]),
        quote!(#[dimension(log =)]),
        quote!(#[dimension(log = other)]),
        quote!(#[dimension(metric =)]),
        quote!(#[dimension(1)]),
        quote!(#[if_none(1)]),
        quote!(#[data_class()]),
    ] {
        let input = quote!(struct Example { #attrs a: Option<u32> });
        assert!(event(quote!("example"), input.clone()).is_err(), "{attrs}");
        assert!(derive_enrichment(input).is_err(), "{attrs}");
    }
}

#[test]
fn gamma_survivor_helper_attributes_are_consumed() {
    for attrs in [
        quote!(#[dimension(log = "renamed", metric = "key")]),
        quote!(#[dimension(log = exclude, metric)]),
        quote!(#[unredacted]),
        quote!(#[data_class(Privacy::Secret)]),
        quote!(#[if_none(drop)]),
        quote!(#[if_none("empty")]),
    ] {
        let output = event(quote!("example"), quote!(#[info] struct Example { #attrs a: Option<u32> })).unwrap();
        let parsed: syn::File = syn::parse2(output).unwrap();
        let syn::Item::Struct(item) = &parsed.items[0] else {
            panic!("first item must retain the event struct")
        };
        assert!(item.fields.iter().all(|field| field.attrs.is_empty()), "{attrs}");
    }
}

#[test]
fn gamma_survivor_bare_counter_has_no_description_or_unit() {
    event(
        quote!("example"),
        quote!(
            #[counter()]
            struct Example {}
        ),
    )
    .unwrap();
    let output = event(
        quote!("example"),
        quote!(
            #[counter]
            struct Example {}
        ),
    )
    .unwrap()
    .to_string();
    assert!(!output.contains(&quote!(Some("")).to_string()));
    assert!(output.contains(&quote!(::core::option::Option::None).to_string()));
}

#[test]
fn gamma_survivor_if_none_rejects_non_option_and_qualified_self() {
    for ty in [quote!(Vec<u32>), quote!(<T as Trait>::Option<u32>)] {
        let input = quote!(struct Example<T> { #[if_none(drop)] a: #ty });
        assert_eq!(
            event(quote!("example"), input.clone()).unwrap_err().to_string(),
            "`#[if_none(...)]` is only valid on `Option<T>` fields"
        );
        assert_eq!(
            derive_enrichment(input).unwrap_err().to_string(),
            "`#[if_none(...)]` is only valid on `Option<T>` fields"
        );
    }
}

#[test]
fn gamma_survivor_generic_bounds_follow_signal_routing() {
    let output = event(
        quote!("example"),
        quote!(
            #[counter]
            struct Example<T> {
                ignored: Vec<T>,
            }
        ),
    )
    .unwrap()
    .to_string();
    assert!(output.contains(&quote!(Vec<T>: ::core::marker::Send + ::core::marker::Sync).to_string()));
    assert!(!output.contains("RedactedDisplay"));
    let output = event(
        quote!("example"),
        quote!(
            #[info]
            struct Example<T> {
                value: Option<(T, u32)>,
            }
        ),
    )
    .unwrap()
    .to_string();
    assert!(output.contains(&quote!((T, u32): ::observed::__private::RedactedDisplay).to_string()));
    let output = event(
        quote!("example"),
        quote!(
            #[info]
            struct Example {
                a: u32,
            }
        ),
    )
    .unwrap()
    .to_string();
    assert!(!output.contains("where"));
    assert!(!output.contains("Example < >"));
}
