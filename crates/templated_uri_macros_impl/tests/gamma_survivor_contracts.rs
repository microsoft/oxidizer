// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process URI template expansion contracts.

use proc_macro2::{TokenStream, TokenTree};
use quote::quote;
use templated_uri_macros_impl::{raw_derive_impl, templated_paq_impl, uri_param_derive_impl};

#[test]
fn gamma_round2_struct_wide_unredacted_applies_to_optional_group() {
    let output = templated_paq_impl(
        &quote!(template = "/{a,b}", unredacted),
        quote!(
            struct Example {
                a: Option<String>,
                b: String,
            }
        ),
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let text = output.to_string();
    assert!(text.contains(&quote!(::std::write!(f, "{}", __val)?;).to_string()));
    assert!(!text.contains("as :: templated_uri :: __private :: RedactedDisplay"));
}

#[test]
fn gamma_round2_derive_style_does_not_add_empty_template_attribute() {
    let output = templated_paq_impl(
        &quote!(),
        quote!(
            #[templated(template = "/fixed")]
            struct Example {}
        ),
    );
    assert!(diagnostics(output.clone()).is_empty());
    syn::parse2::<syn::File>(output).unwrap();
}

#[test]
fn gamma_survivor_optional_simple_group_omits_empty_prefix_tokens() {
    let output = templated_paq_impl(
        &quote!(template = "/{a,b}"),
        quote!(
            struct Example {
                a: Option<String>,
                b: String,
            }
        ),
    )
    .to_string();
    assert!(
        output.contains(
            &quote!(if !__first {
                __out.push_str(",");
            })
            .to_string()
        )
    );
    assert!(!output.contains(&quote!(__out.push_str("")).to_string()));
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
fn gamma_survivor_generic_parameter_derives_report_exact_errors() {
    let input = quote!(
        struct Example<T>(T);
    );
    assert_eq!(
        diagnostics(uri_param_derive_impl(input.clone())),
        ["Escape cannot be derived for generic types"]
    );
    assert_eq!(diagnostics(raw_derive_impl(input)), ["Raw cannot be derived for generic types"]);
}

#[test]
fn gamma_survivor_template_field_errors_are_sorted() {
    let missing = templated_paq_impl(
        &quote!(template = "/{z}/{a}/{m}"),
        quote!(
            struct Example {}
        ),
    );
    assert_eq!(diagnostics(missing), ["Missing values in struct: [\"a\", \"m\", \"z\"]"]);
    let excess = templated_paq_impl(
        &quote!(template = "/"),
        quote!(
            struct Example {
                z: u32,
                a: u32,
                m: u32,
            }
        ),
    );
    assert_eq!(diagnostics(excess), ["Excess values in struct: [\"a\", \"m\", \"z\"]"]);
}

#[test]
fn gamma_nov6_missing_template_names_have_exact_lexical_order() {
    let missing = templated_paq_impl(
        &quote!(template = "/{l}/{k}/{j}/{i}/{h}/{g}/{f}/{e}/{d}/{c}/{b}/{a}"),
        quote!(
            struct Example {}
        ),
    );
    assert_eq!(
        diagnostics(missing),
        ["Missing values in struct: [\"a\", \"b\", \"c\", \"d\", \"e\", \"f\", \"g\", \"h\", \"i\", \"j\", \"k\", \"l\"]"]
    );
}

#[test]
fn gamma_survivor_preserves_unrelated_field_attributes() {
    let output = templated_paq_impl(
        &quote!(template = "/{a}"),
        quote!(
            #[derive(Clone)]
            struct Example {
                #[doc = "kept"]
                #[unredacted]
                a: String,
            }
        ),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let syn::Item::Struct(item) = &parsed.items[0] else {
        panic!("must retain the original struct")
    };
    let attrs = &item.fields.iter().next().unwrap().attrs;
    assert_eq!(attrs.len(), 1);
    assert!(attrs[0].path().is_ident("doc"));
}

#[test]
fn gamma_survivor_required_reference_and_optional_raw_receivers() {
    let output = templated_paq_impl(
        &quote!(template = "/{a}/{+b}"),
        quote!(
            struct Example {
                a: &str,
                b: Option<String>,
            }
        ),
    )
    .to_string();
    assert!(output.contains(&quote!(::templated_uri::Escape::escape_into(self.a, __out)).to_string()));
    assert!(output.contains(&quote!(::templated_uri::Raw::raw_into(__val, __out)).to_string()));
    assert!(!output.contains(&quote!(::templated_uri::Escape::escape_into(&self.a, __out)).to_string()));
}

#[test]
fn gamma_survivor_unredacted_container_applies_to_every_field() {
    for ty in [quote!(String), quote!(Option<String>)] {
        let output = templated_paq_impl(
            &quote!(template = "/{a,b}", unredacted),
            quote!(struct Example { a: #ty, b: Option<String> }),
        )
        .to_string();
        assert!(!output.contains("RedactedDisplay :: fmt"), "{output}");
        assert!(output.contains("write !"), "{output}");
    }
}

#[test]
fn gamma_survivor_rejects_stray_close_and_reports_parameter_label() {
    let output = templated_paq_impl(
        &quote!(template = "/literal}"),
        quote!(
            struct Example {}
        ),
    );
    assert!(!diagnostics(output).is_empty());
    let output = templated_paq_impl(
        &quote!(template = "/{?a,1}"),
        quote!(
            struct Example {}
        ),
    );
    let messages = diagnostics(output);
    assert!(messages.iter().any(|message| message.contains("parameter name")), "{messages:?}");
}
