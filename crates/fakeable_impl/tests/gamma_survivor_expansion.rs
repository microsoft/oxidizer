// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macro diagnostics and generated wrapper contracts.

use fakeable_impl::fakeable_impl;
use quote::quote;

#[test]
fn gamma_survivor_invalid_arguments_return_diagnostics_not_panics() {
    for args in [
        quote!(= "test"),
        quote!(fake_impl Fake),
        quote!(fakes_feature =),
        quote!(fake_impl =),
        quote!(fake_constructor = 7),
        quote!(generate_mockall_fake = true extra),
        quote!(mockall_fake_module = 7),
        quote!(mockall_fake_visibility = 7),
    ] {
        let output = fakeable_impl(
            args,
            quote!(
                struct Service;
            ),
        )
        .to_string();
        assert!(output.contains("compile_error"), "{output}");
    }
}

#[test]
fn gamma_survivor_tuple_fields_are_visible_to_generated_constructor() {
    let output = fakeable_impl(
        quote!(fake_impl = Fake),
        quote!(
            struct Service(u32, String);
        ),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let module = parsed
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Mod(module) => Some(module),
            _ => None,
        })
        .unwrap();
    let real = module
        .content
        .as_ref()
        .unwrap()
        .1
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert_eq!(real.fields.len(), 2);
    for field in &real.fields {
        assert!(matches!(field.vis, syn::Visibility::Restricted(_)));
    }
}

#[test]
fn gamma_survivor_qualified_targets_are_rejected_but_plain_targets_expand() {
    for input in [
        quote!(impl ::Service { pub fn value(&self) -> u32 { 1 } }),
        quote!(
            impl<T> ::Service {
                pub fn value(&self) -> u32 {
                    1
                }
            }
        ),
        quote!(
            impl <T as Trait>::Service {
                pub fn value(&self) -> u32 {
                    1
                }
            }
        ),
        quote!(impl module::Service { pub fn value(&self) -> u32 { 1 } }),
    ] {
        let output = fakeable_impl(quote!(), input).to_string();
        assert!(output.contains("qualified impl targets are not supported"), "{output}");
    }
    let output = fakeable_impl(quote!(), quote!(impl Service { pub fn value(&self) -> u32 { 1 } })).to_string();
    assert!(!output.contains("compile_error"), "{output}");
    assert!(!output.contains(":: < >"), "{output}");
}

#[test]
fn gamma_survivor_cfg_conditions_are_not_treated_as_applied_attributes() {
    let output = fakeable_impl(
        quote!(fake_impl = Fake),
        quote!(
            #[cfg_attr(derive, doc = "description")]
            #[cfg_attr(cfg, doc = "another")]
            struct Service;
        ),
    )
    .to_string();
    assert!(!output.contains("compile_error"), "{output}");
    let output = fakeable_impl(
        quote!(fake_impl = Fake),
        quote!(
            #[doc(repr(C))]
            struct Service;
        ),
    )
    .to_string();
    assert!(!output.contains("compile_error"), "{output}");
}

#[test]
fn gamma_survivor_nested_expect_rewrite_preserves_conditions_and_non_cfg_payloads() {
    let output = fakeable_impl(
        quote!(fake_impl = Fake),
        quote!(
            #[cfg_attr(expect(condition), cfg_attr(feature = "x", expect(dead_code)), doc(expect(payload)))]
            struct Service;
        ),
    )
    .to_string();
    assert!(output.contains("expect (condition)"), "{output}");
    assert!(output.contains("allow (dead_code)"), "{output}");
    assert!(output.contains("doc (expect (payload))"), "{output}");
}

#[test]
fn gamma_survivor_mutable_borrowed_receivers_are_accepted_but_mut_value_receivers_are_not() {
    let output = fakeable_impl(quote!(), quote!(impl Service { pub fn update(&mut self, value: u32) {} })).to_string();
    assert!(!output.contains("compile_error"), "{output}");
    let output = fakeable_impl(quote!(), quote!(impl Service { pub fn update(mut self) {} })).to_string();
    assert!(output.contains("mut self receivers are not supported"), "{output}");
    let output = fakeable_impl(
        quote!(),
        quote!(impl Service { pub fn unsupported(self: Box<Self>) {} pub fn later(&self) {} }),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert!(
        matches!(&parsed.items[0], syn::Item::Macro(item) if item.mac.path.segments.last().is_some_and(|segment| segment.ident == "compile_error"))
    );
}

#[cfg(feature = "mockall")]
#[test]
fn gamma_survivor_mockall_validates_public_mutable_receivers_and_continues_after_gated_errors() {
    let args = quote!(generate_mockall_fake = true);
    let output = fakeable_impl(args.clone(), quote!(impl Service { pub fn update(&mut self, value: u32) {} }));
    let parsed: syn::File = syn::parse2(output).unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert!(
        matches!(&parsed.items[0], syn::Item::Macro(item) if item.mac.path.segments.last().is_some_and(|segment| segment.ident == "compile_error"))
    );

    let output = fakeable_impl(args.clone(), quote!(impl Service { fn private(self) {} pub fn later(&self) {} })).to_string();
    assert!(!output.contains("compile_error"), "{output}");
    let output = fakeable_impl(
        args,
        quote!(impl Service {
            #[cfg(feature = "disabled")]
            pub fn unsupported(self) {}
            pub fn later(&self) {}
        }),
    );
    let parsed: syn::File = syn::parse2(output).unwrap();
    let module = parsed
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Mod(module) if module.ident == "fakes" => Some(module),
            _ => None,
        })
        .unwrap();
    let mock = module
        .content
        .as_ref()
        .unwrap()
        .1
        .iter()
        .find_map(|item| match item {
            syn::Item::Macro(item) if item.mac.path.segments.last().unwrap().ident == "mock" => Some(item),
            _ => None,
        })
        .unwrap();
    assert!(mock.mac.tokens.to_string().contains("fn later"), "{:?}", mock.mac.tokens);
}
