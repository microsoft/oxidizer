// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros_impl/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros_impl/favicon.ico"
)]

//! Macros for the [`templated_uri`](https://docs.rs/templated_uri) crate.

mod enum_template;
pub(crate) mod error;
mod struct_template;
pub(crate) mod template_parser;
mod uri_param;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, DeriveInput, Field, parse_quote, parse2};

use crate::enum_template::enum_template;
use crate::struct_template::struct_template;
use crate::uri_param::{raw_impl, uri_param_impl};

macro_rules! bail {
    ($span:ident, $msg:expr) => {
        crate::bail!($span, $msg, )
    };
    ($span:ident, $msg:expr, $($args:tt),*) => {
        return syn::Error::new_spanned($span, format!($msg, $($args,)*)).to_compile_error()
    };
}

pub(crate) use bail;

#[must_use]
#[cfg_attr(test, mutants::skip)] // not relevant for auto-generated proc macros
pub fn templated_paq_impl(attr: &TokenStream, item: TokenStream) -> TokenStream {
    // Parse the item (struct/enum definition)
    let mut input: DeriveInput = match parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    // Generic types are not supported
    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(&input.generics, "Generic types are not supported for #[templated]").to_compile_error();
    }

    // If attributes were passed via the attribute macro, parse and add them
    if !attr.is_empty() {
        // Create an attribute from the tokens and add it to the input's attributes
        let attribute = parse_quote! { #[templated(#attr)] };
        input.attrs.push(attribute);
    }

    let original = filter_original(&input);

    let implementation = match input.data {
        syn::Data::Struct(ref s) => struct_template(input.ident.clone(), s, &input.attrs),
        syn::Data::Enum(ref e) => enum_template(&input.ident, e),
        syn::Data::Union(_) => {
            return syn::Error::new_spanned(input.ident, "Unions are not supported for TemplatedUri").to_compile_error();
        }
    };

    quote! {
        #original
        #implementation
    }
}

#[cfg_attr(test, mutants::skip)] // not relevant for auto-generated proc macros
fn filter_original(input: &DeriveInput) -> TokenStream {
    // Generate the original item definition WITHOUT the templated attribute
    let vis = &input.vis;
    let ident = &input.ident;
    let generics = &input.generics;
    let (impl_generics, _, where_clause) = generics.split_for_impl();

    // Filter out the 'templated' attribute from the output to avoid recursion
    let output_attrs: Vec<_> = input.attrs.iter().filter(|attr| !attr.path().is_ident("templated")).collect();

    match &input.data {
        syn::Data::Struct(s) => {
            // Filter out templated and unredacted attributes from fields
            let filtered_fields = match &s.fields {
                syn::Fields::Named(fields) => {
                    let fields: Vec<_> = fields
                        .named
                        .iter()
                        .map(|f| {
                            let attrs = filter_attributes(f);
                            let vis = &f.vis;
                            let ident = &f.ident;
                            let ty = &f.ty;
                            quote! { #(#attrs)* #vis #ident: #ty }
                        })
                        .collect();
                    quote! { { #(#fields),* } }
                }
                syn::Fields::Unnamed(fields) => {
                    let fields: Vec<_> = fields
                        .unnamed
                        .iter()
                        .map(|f| {
                            let attrs: Vec<_> = f
                                .attrs
                                .iter()
                                .filter(|attr| !attr.path().is_ident("templated") && !attr.path().is_ident("unredacted"))
                                .collect();
                            let vis = &f.vis;
                            let ty = &f.ty;
                            quote! { #(#attrs)* #vis #ty }
                        })
                        .collect();
                    quote! { ( #(#fields),* ) }
                }
                syn::Fields::Unit => quote! {},
            };
            match &s.fields {
                syn::Fields::Named(_) => {
                    quote! {
                        #(#output_attrs)*
                        #vis struct #ident #impl_generics #filtered_fields #where_clause
                    }
                }
                syn::Fields::Unnamed(_) => {
                    quote! {
                        #(#output_attrs)*
                        #vis struct #ident #impl_generics #filtered_fields #where_clause;
                    }
                }
                syn::Fields::Unit => {
                    quote! {
                        #(#output_attrs)*
                        #vis struct #ident #impl_generics #where_clause;
                    }
                }
            }
        }
        syn::Data::Enum(e) => {
            let variants = &e.variants;
            quote! {
                #(#output_attrs)*
                #vis enum #ident #impl_generics #where_clause {
                    #variants
                }
            }
        }
        syn::Data::Union(u) => {
            let fields = &u.fields;
            quote! {
                #(#output_attrs)*
                #vis union #ident #impl_generics #fields #where_clause
            }
        }
    }
}

fn filter_attributes(f: &Field) -> Vec<&Attribute> {
    let attrs: Vec<_> = f
        .attrs
        .iter()
        .filter(|attr| !attr.path().is_ident("templated") && !attr.path().is_ident("unredacted"))
        .collect();
    attrs
}

#[must_use]
#[cfg_attr(test, mutants::skip)] // just emits compile error otherwise
pub fn uri_param_derive_impl(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    uri_param_impl(input)
}

#[must_use]
pub fn raw_derive_impl(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    raw_impl(input)
}

#[cfg(not(miri))] // Insta can't work with Miri
#[cfg(test)]
mod tests {
    //! Snapshot tests for the proc-macro codegen.
    //!
    //! Each `assert_snapshot!(output)` is backed by a file in `src/snapshots/` named
    //! `templated_uri_macros_impl__tests__<test_name>.snap`, where `<test_name>` is
    //! the test function name with its leading `test_` prefix stripped (this is the
    //! default insta behavior). So `fn test_templated_uri_impl` is backed by
    //! `templated_uri_macros_impl__tests__templated_uri_impl.snap`.
    //!
    //! After intentionally changing the generated code, run `cargo insta review`
    //! (or `cargo insta accept`) to update the snapshot files.

    use quote::quote;

    use super::*;

    /// Renders the `#[templated(<attr>)] <item>` derive output, pretty-printed.
    ///
    /// Used by `assert_paq_snapshot!` and a few non-snapshot error tests; new tests
    /// should prefer the macro for consistency with the snapshot-file naming pattern.
    #[expect(clippy::needless_pass_by_value, reason = "Test code")]
    fn pretty_parse(attr: TokenStream, item: TokenStream) -> String {
        let output = templated_paq_impl(&attr, item);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    /// Renders the `#[derive(Raw)]` output for `input`, pretty-printed.
    fn pretty_parse_raw(input: TokenStream) -> String {
        let output = raw_derive_impl(input);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    /// Renders the `#[derive(Escape)]` output for `input`, pretty-printed.
    fn pretty_parse_uri_param(input: TokenStream) -> String {
        let output = uri_param_derive_impl(input);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    /// Asserts the codegen for `#[templated(<attr>)] <item>` matches its snapshot.
    ///
    /// `$attr` is a bracket-wrapped token-tree (e.g. `[template = "/foo"]`) that's
    /// forwarded to `quote!`; the leading `#[...]` form is mimicked because attribute
    /// arguments are the natural reading here. The bracket may be omitted for the
    /// no-attribute case. `$item` is a Rust item declaration (struct/enum/union)
    /// passed directly without any wrapping. The snapshot file is derived from the
    /// calling test function name because the inner `assert_snapshot!` call inherits
    /// the test's source location.
    ///
    /// The macro binds the rendered output to a local named `output_pretty` before
    /// calling `assert_snapshot!`, so the `expression: output_pretty` field stored
    /// in each `.snap` file stays stable across re-accepts (otherwise insta would
    /// rewrite it to the full `pretty_parse(::quote::quote![...], ...)` expression).
    macro_rules! assert_paq_snapshot {
        ($attr:tt, $item:item $(,)?) => {{
            let output_pretty = pretty_parse(::quote::quote!$attr, ::quote::quote! { $item });
            ::insta::assert_snapshot!(output_pretty);
        }};
        ($item:item $(,)?) => {{
            let output_pretty = pretty_parse(::quote::quote! {}, ::quote::quote! { $item });
            ::insta::assert_snapshot!(output_pretty);
        }};
    }

    /// Asserts the `#[derive(Raw)]` codegen for `$item` matches its snapshot.
    ///
    /// See [`assert_paq_snapshot`] for the rationale behind the `output_pretty` local.
    macro_rules! assert_raw_snapshot {
        ($item:item $(,)?) => {{
            let output_pretty = pretty_parse_raw(::quote::quote! { $item });
            ::insta::assert_snapshot!(output_pretty);
        }};
    }

    /// Asserts the `#[derive(Escape)]` codegen for `$item` matches its snapshot.
    ///
    /// See [`assert_paq_snapshot`] for the rationale behind the `output_pretty` local.
    macro_rules! assert_uri_param_snapshot {
        ($item:item $(,)?) => {{
            let output_pretty = pretty_parse_uri_param(::quote::quote! { $item });
            ::insta::assert_snapshot!(output_pretty);
        }};
    }

    /// Asserts the `#[templated]` codegen for `<attr> <item>` produces a
    /// `compile_error!` invocation whose rendered output contains `$expected_msg`.
    ///
    /// Used for tests that pin down macro error paths via substring inspection
    /// instead of snapshots (typically because the message wording is short and
    /// stable, so a full snapshot would be more boilerplate than signal). The
    /// `$attr` argument may be omitted for the no-attribute case.
    macro_rules! assert_paq_compile_error {
        ($attr:tt, $item:item, $expected_msg:expr $(,)?) => {{
            let output = pretty_parse(::quote::quote!$attr, ::quote::quote!{ $item });
            assert!(
                output.contains("compile_error"),
                "expected compile_error! invocation in proc-macro output:\n{output}",
            );
            assert!(
                output.contains($expected_msg),
                "expected diagnostic message {:?} in proc-macro output:\n{}",
                $expected_msg,
                output,
            );
        }};
        ($item:item, $expected_msg:expr $(,)?) => {
            assert_paq_compile_error!([], $item, $expected_msg)
        };
    }

    #[test]
    fn test_templated_uri_impl() {
        assert_paq_snapshot!(
            [template = "/example.com/{param}/{+param2}{/param3,param4}"],
            struct Test {
                param: String,
                param2: EscapedString,
                param3: String,
                param4: String,
            }
        );
    }

    #[test]
    fn test_templated_unredacted_uri_impl() {
        assert_paq_snapshot!(
            [template = "/example.com/{param}/{+param2}{/param3,param4}", unredacted],
            struct Test {
                param: String,
                param2: EscapedString,
                param3: String,
                param4: String,
            }
        );
    }

    #[test]
    fn test_field_level_unredacted() {
        assert_paq_snapshot!(
            [template = "/example.com/{param}/{+param2}{/param3,param4}"],
            struct Test {
                param: String,
                #[templated(unredacted)]
                param2: EscapedString,
                param3: String,
                param4: String,
            }
        );
    }

    /// Locks in that the bare `#[unredacted]` field attribute and the namespaced
    /// `#[templated(unredacted)]` form produce **byte-identical** codegen, instead
    /// of duplicating the full codegen snapshot under two test names. The reference
    /// snapshot itself lives in `test_field_level_unredacted`; if that snapshot
    /// changes, this parity check still ensures both attribute forms move together.
    #[test]
    fn test_standalone_unredacted_matches_templated_form() {
        let templated_form = pretty_parse(
            quote! { template = "/example.com/{param}/{+param2}{/param3,param4}" },
            quote! {
                struct Test {
                    param: String,
                    #[templated(unredacted)]
                    param2: EscapedString,
                    param3: String,
                    param4: String,
                }
            },
        );
        let standalone_form = pretty_parse(
            quote! { template = "/example.com/{param}/{+param2}{/param3,param4}" },
            quote! {
                struct Test {
                    param: String,
                    #[unredacted]
                    param2: EscapedString,
                    param3: String,
                    param4: String,
                }
            },
        );
        assert_eq!(
            templated_form, standalone_form,
            "`#[unredacted]` shorthand must produce identical codegen to `#[templated(unredacted)]`",
        );
    }

    #[test]
    fn test_query_param_is_kv_expansion() {
        assert_paq_snapshot!(
            [template = "/api/{resource}{?page,limit}"],
            struct QueryTest {
                resource: String,
                page: String,
                limit: String,
            }
        );
    }

    #[test]
    fn test_optional_field_codegen() {
        // Mixes a required-only group `{id}` with a query group `{?filter,limit}` that
        // contains an `Option<u32>`. Locks in:
        //   * the all-required render path (flat `push_str` / `write!` for `{id}`),
        //   * the optional-aware render path with `__first` tracking for `{?filter,limit}`,
        //   * the matching `RedactedDisplay` paths,
        //   * `if let Some(ref __val) = self.limit` extraction of the inner `u32`.
        assert_paq_snapshot!(
            [template = "/items/{id}{?filter,limit}"],
            struct OptionalTest {
                id: u32,
                filter: String,
                limit: Option<u32>,
            }
        );
    }

    #[test]
    fn test_optional_field_with_unredacted_codegen() {
        // Pins down the `unredacted || field_unredacted` semantics inside the
        // optional-aware redacted-display path. With a `#[unredacted]` field, the
        // generated code MUST call `::std::write!(f, "{}", self.field)` (Display),
        // not the `RedactedDisplay::fmt` trait path. Flipping the `||` to `&&` in
        // `redacted_display_group_with_optional` produces a different TokenStream
        // and this snapshot diff will catch it.
        assert_paq_snapshot!(
            [template = "/items{?filter,limit}"],
            struct OptionalUnredactedTest {
                #[unredacted]
                filter: String,
                #[unredacted]
                limit: Option<u32>,
            }
        );
    }

    #[test]
    fn test_optional_reference_field_codegen() {
        // For `Option<&T>`, `Some(ref __val)` binds `__val: &&T`. The macro must peel one
        // reference so the generated trait calls resolve against `T: Escape`/`T: RedactedDisplay`
        // rather than the much rarer `&T` impls. This snapshot pins down:
        //   * the render path emits `Escape::escape(*__val)` (not `__val`),
        //   * the redacted-display path emits `<str as RedactedDisplay>::fmt(*__val, ...)`
        //     with the inner reference peeled from the UFCS receiver type.
        assert_paq_snapshot!(
            [template = "/items{?name}"],
            struct ReferenceOptional {
                name: Option<&'static str>,
            }
        );
    }

    #[test]
    fn test_excessive_template_impl() {
        assert_paq_compile_error!(
            [template = "/example.com/{param}/{+param2}{/param3,param4}"],
            struct ExcessiveTemplate {
                param: String,
                param2: EscapedString,
                param3: String,
                param4: String,
                extra_param: String,
            },
            "Excess values in struct"
        );
    }

    #[test]
    fn test_insufficient_template_impl() {
        assert_paq_snapshot!(
            [template = "/{param}/{param2}"],
            struct InsufficientTemplate {
                param: String,
            }
        );
    }

    #[test]
    fn test_parse_error() {
        // Unterminated `{`.
        assert_paq_compile_error!(
            [template = "/example.com/{param"],
            struct ParseErrorTest {
                param: String,
            },
            "Failed to parse URI",
        );

        // Invalid operator character `>`.
        assert_paq_compile_error!(
            [template = "/example.com/{>param}"],
            struct ParseErrorTest {
                param: String,
            },
            "Failed to parse URI"
        );
    }

    #[test]
    fn test_tuple_struct_rejected() {
        assert_paq_compile_error!(
            [template = "/test/{param}"],
            struct TupleTest(String);,
            "can only be applied to structs with named fields"
        );
    }

    #[test]
    fn test_enum_struct_item_error() {
        assert_paq_snapshot!(
            enum TestEnum {
                Variant1 { param: String },
            }
        );
    }

    #[test]
    fn test_enum_single_item_only_error() {
        assert_paq_snapshot!(
            enum TestEnum {
                Variant1(String, String),
            }
        );
    }

    #[test]
    fn test_template_enum_impl() {
        assert_paq_snapshot!(
            enum Test {
                FirstTemplate(First),
                SecondTemplate(Second),
            }
        );
    }

    #[test]
    fn test_raw_impl() {
        assert_raw_snapshot!(
            struct MyFragment(String);
        );
    }

    #[test]
    fn test_raw_with_custom_type() {
        assert_raw_snapshot!(
            struct CustomFragment(EscapedString);
        );
    }

    #[test]
    fn test_raw_named_fields_error() {
        assert_raw_snapshot!(
            struct InvalidFragment {
                value: String,
            }
        );
    }

    #[test]
    fn test_raw_multiple_fields_error() {
        assert_raw_snapshot!(
            struct TooManyFields(String, String);
        );
    }

    #[test]
    fn test_raw_enum_error() {
        assert_raw_snapshot!(
            enum FragmentEnum {
                Variant(String),
            }
        );
    }

    #[test]
    fn test_raw_union_error() {
        assert_raw_snapshot!( union UnsafeFragmentUnion {
            value: u32,
        });
    }

    #[test]
    fn test_template_attribute_parsing_error() {
        // Test error handling for Opts::from_attributes in struct_template.rs
        let attr = quote! { invalid_attribute_name="value" };
        let item = quote! {
            struct TestStruct {
                param: String,
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert!(
            output_pretty.contains("compile_error") || output_pretty.contains("error"),
            "Output should contain error for invalid attribute: {output_pretty}"
        );
    }

    #[test]
    fn test_field_attribute_parsing_error() {
        // Test error handling for Fields::from_fields in struct_template.rs
        let attr = quote! { template="/{param}" };
        let item = quote! {
            struct TestStruct {
                #[templated(invalid_field_attr)]
                param: String,
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert!(
            output_pretty.contains("compile_error") || output_pretty.contains("error"),
            "Output should contain error for invalid field attribute: {output_pretty}"
        );
    }

    #[test]
    fn test_uri_param_impl() {
        assert_uri_param_snapshot!(
            struct SafeFragment(String);
        );
    }

    #[test]
    fn test_uri_param_named_fields_error() {
        assert_uri_param_snapshot!(
            struct InvalidSafeFragment {
                value: String,
            }
        );
    }

    #[test]
    fn test_uri_param_enum_error() {
        assert_uri_param_snapshot!(
            enum SafeFragmentEnum {
                Variant(String),
            }
        );
    }

    #[test]
    fn test_uri_param_union_error() {
        assert_uri_param_snapshot!( union SafeFragmentUnion {
            value: u32,
        });
    }

    #[test]
    fn test_uri_param_multiple_fields_error() {
        assert_uri_param_snapshot!(
            struct TooManySafeFields(String, String);
        );
    }

    #[test]
    fn test_uri_param_zero_fields_error() {
        assert_uri_param_snapshot!(
            struct NoFields();
        );
    }

    #[test]
    fn test_invalid_item_syntax_error() {
        // Test error handling when item cannot be parsed as DeriveInput
        let attr = quote! { template="/{param}" };
        let item = quote! {
            // Invalid syntax - not a valid struct/enum/union
            impl SomeTrait for SomeType {}
        };

        let output_pretty = pretty_parse(attr, item);
        assert!(
            output_pretty.contains("compile_error") || output_pretty.contains("error"),
            "Output should contain error for invalid item syntax: {output_pretty}"
        );
    }

    #[test]
    fn test_union_not_supported_error() {
        // Test that unions are not supported for TemplatedUri
        assert_paq_snapshot!([ template = "/{param}" ], union TestUnion {
            field1: u32,
            field2: i32,
        });
    }

    #[test]
    fn test_filter_attributes() {
        use syn::Field;

        // Create a field with multiple attributes including templated and unredacted
        let field: Field = syn::parse_quote! {
            #[serde(rename = "test")]
            #[templated(unredacted)]
            #[unredacted]
            #[doc = "Test field"]
            pub test_field: String
        };

        let filtered = super::filter_attributes(&field);

        // Should only keep serde and doc attributes, filtering out templated and unredacted
        assert_eq!(filtered.len(), 2);
        assert!(filtered[0].path().is_ident("serde"));
        assert!(filtered[1].path().is_ident("doc"));
    }

    #[test]
    fn test_raw_derive_impl_parse_error() {
        // Test error handling when input cannot be parsed as DeriveInput
        // Pass invalid tokens that cannot be parsed as a struct/enum/union
        let input = quote! {
            fn not_a_struct() {}
        };

        let output = raw_derive_impl(input);
        let output_str = output.to_string();

        // Should produce a compile error
        assert!(
            output_str.contains("compile_error") || output_str.contains("expected"),
            "Output should contain error for invalid input: {output_str}"
        );
    }

    #[test]
    fn test_uri_param_derive_impl_parse_error() {
        // Test error handling when input cannot be parsed as DeriveInput
        // Pass invalid tokens that cannot be parsed as a struct/enum/union
        let input = quote! {
            fn not_a_struct() {}
        };

        let output = uri_param_derive_impl(input);
        let output_str = output.to_string();

        // Should produce a compile error
        assert!(
            output_str.contains("compile_error") || output_str.contains("expected"),
            "Output should contain error for invalid input: {output_str}"
        );
    }

    #[test]
    fn test_filter_original_unnamed_fields() {
        use syn::DeriveInput;

        // Create a tuple struct with various attributes including templated and unredacted
        let input: DeriveInput = syn::parse_quote! {
            #[derive(Debug, Clone)]
            #[templated(template = "/test")]
            pub struct TestTuple(
                #[serde(rename = "field1")]
                #[templated(unredacted)]
                pub String,
                #[unredacted]
                #[doc = "Field 2"]
                pub i32,
                pub u64
            );
        };

        let filtered = super::filter_original(&input);
        let filtered_str = filtered.to_string();

        // Should keep derive and omit templated attribute from struct
        assert!(
            filtered_str.contains("derive") && filtered_str.contains("Debug") && filtered_str.contains("Clone"),
            "Output should contain derive with Debug and Clone: {filtered_str}"
        );
        assert!(
            !filtered_str.contains("templated"),
            "Output should not contain templated: {filtered_str}"
        );

        // Should keep serde and doc attributes, but filter out templated and unredacted from fields
        assert!(filtered_str.contains("serde"), "Output should contain serde: {filtered_str}");
        assert!(filtered_str.contains("doc"), "Output should contain doc: {filtered_str}");
        assert!(
            !filtered_str.contains("unredacted"),
            "Output should not contain unredacted: {filtered_str}"
        );

        // Should maintain structure as tuple struct
        assert!(
            filtered_str.contains("pub struct TestTuple"),
            "Output should contain struct declaration: {filtered_str}"
        );
        assert!(filtered_str.contains("String"), "Output should contain String type: {filtered_str}");
        assert!(filtered_str.contains("i32"), "Output should contain i32 type: {filtered_str}");
        assert!(filtered_str.contains("u64"), "Output should contain u64 type: {filtered_str}");
    }

    #[test]
    fn test_generic_struct_rejected() {
        assert_paq_snapshot!(
            [template = "/{param}"],
            struct GenericTemplate<T> {
                param: T,
            }
        );
    }

    #[test]
    fn test_generic_uri_param_rejected() {
        let input = quote! {
            struct Wrapper<T>(T);
        };

        let output = raw_derive_impl(input);
        let output_str = output.to_string();
        assert!(output_str.contains("compile_error"), "Should reject generic Raw: {output_str}");
    }

    #[test]
    fn test_generic_uri_safe_param_rejected() {
        let input = quote! {
            struct Wrapper<T>(T);
        };

        let output = uri_param_derive_impl(input);
        let output_str = output.to_string();
        assert!(output_str.contains("compile_error"), "Should reject generic Escape: {output_str}");
    }

    #[test]
    fn test_filter_original_unit_struct() {
        use syn::DeriveInput;

        let input: DeriveInput = syn::parse_quote! {
            #[derive(Debug)]
            #[templated(template = "/test")]
            pub struct UnitStruct;
        };

        let filtered = super::filter_original(&input);
        let filtered_str = filtered.to_string();

        assert!(
            filtered_str.contains("pub struct UnitStruct"),
            "Should contain struct: {filtered_str}"
        );
        assert!(filtered_str.contains("derive"), "Should keep derive: {filtered_str}");
        assert!(!filtered_str.contains("templated"), "Should filter templated: {filtered_str}");
    }
}
