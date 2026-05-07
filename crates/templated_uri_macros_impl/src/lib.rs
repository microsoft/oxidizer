// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
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
    use insta::assert_snapshot;
    use quote::quote;

    use super::*;

    #[expect(clippy::needless_pass_by_value, reason = "Test code")]
    fn pretty_parse(attr: TokenStream, item: TokenStream) -> String {
        let output = templated_paq_impl(&attr, item);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    fn pretty_parse_raw(input: TokenStream) -> String {
        let output = raw_derive_impl(input);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    fn pretty_parse_uri_param(input: TokenStream) -> String {
        let output = uri_param_derive_impl(input);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    #[test]
    fn test_templated_uri_impl() {
        let attr = quote! { template="/example.com/{param}/{+param2}{/param3,param4}" };
        let item = quote! {
            struct Test {
                param: String,
                param2: EscapedString,
                param3: String,
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: EscapedString,
            param3: String,
            param4: String,
        }
        impl ::templated_uri::PathAndQueryTemplate for Test {
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn format_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn render(&self) -> ::std::string::String {
                let param = ::templated_uri::Escape::escape(&self.param);
                let param2 = ::templated_uri::Raw::raw(&self.param2);
                let param3 = ::templated_uri::Escape::escape(&self.param3);
                let param4 = ::templated_uri::Escape::escape(&self.param4);
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                ::templated_uri::http::uri::PathAndQuery,
                ::templated_uri::UriError,
            > {
                Ok(
                    ::templated_uri::http::uri::PathAndQuery::try_from(
                        ::templated_uri::PathAndQueryTemplate::render(self),
                    )?,
                )
            }
        }
        impl ::std::fmt::Debug for Test {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple("Test")
                    .field(&"/example.com/{param}/{+param2}{/param3,param4}")
                    .finish()
            }
        }
        impl ::data_privacy::RedactedDisplay for Test {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                f.write_str("/example.com/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param, engine, f)?;
                f.write_str("/")?;
                <EscapedString as ::data_privacy::RedactedDisplay>::fmt(
                    &self.param2,
                    engine,
                    f,
                )?;
                f.write_str("/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param3, engine, f)?;
                f.write_str("/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param4, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for ::templated_uri::PathAndQuery {
            fn from(value: Test) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
        "#);
    }

    #[test]
    fn test_templated_unredacted_uri_impl() {
        let attr = quote! { template="/example.com/{param}/{+param2}{/param3,param4}", unredacted };
        let item = quote! {
            struct Test {
                // #[templated(classify=Public)]
                param: String,
                param2: EscapedString,
                // #[templated(classify=Restricted)]
                param3: String,
                // #[templated(classify=Public)]
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: EscapedString,
            param3: String,
            param4: String,
        }
        impl ::templated_uri::PathAndQueryTemplate for Test {
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn format_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn render(&self) -> ::std::string::String {
                let param = ::templated_uri::Escape::escape(&self.param);
                let param2 = ::templated_uri::Raw::raw(&self.param2);
                let param3 = ::templated_uri::Escape::escape(&self.param3);
                let param4 = ::templated_uri::Escape::escape(&self.param4);
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                ::templated_uri::http::uri::PathAndQuery,
                ::templated_uri::UriError,
            > {
                Ok(
                    ::templated_uri::http::uri::PathAndQuery::try_from(
                        ::templated_uri::PathAndQueryTemplate::render(self),
                    )?,
                )
            }
        }
        impl ::std::fmt::Debug for Test {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple("Test")
                    .field(&"/example.com/{param}/{+param2}{/param3,param4}")
                    .finish()
            }
        }
        impl ::data_privacy::RedactedDisplay for Test {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                f.write_str("/example.com/")?;
                ::std::write!(f, "{}", self.param)?;
                f.write_str("/")?;
                ::std::write!(f, "{}", self.param2)?;
                f.write_str("/")?;
                ::std::write!(f, "{}", self.param3)?;
                f.write_str("/")?;
                ::std::write!(f, "{}", self.param4)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for ::templated_uri::PathAndQuery {
            fn from(value: Test) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
        "#);
    }

    #[test]
    fn test_field_level_unredacted() {
        let attr = quote! { template="/example.com/{param}/{+param2}{/param3,param4}" };
        let item = quote! {
            struct Test {
                param: String,
                #[templated(unredacted)]
                param2: EscapedString,
                param3: String,
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: EscapedString,
            param3: String,
            param4: String,
        }
        impl ::templated_uri::PathAndQueryTemplate for Test {
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn format_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn render(&self) -> ::std::string::String {
                let param = ::templated_uri::Escape::escape(&self.param);
                let param2 = ::templated_uri::Raw::raw(&self.param2);
                let param3 = ::templated_uri::Escape::escape(&self.param3);
                let param4 = ::templated_uri::Escape::escape(&self.param4);
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                ::templated_uri::http::uri::PathAndQuery,
                ::templated_uri::UriError,
            > {
                Ok(
                    ::templated_uri::http::uri::PathAndQuery::try_from(
                        ::templated_uri::PathAndQueryTemplate::render(self),
                    )?,
                )
            }
        }
        impl ::std::fmt::Debug for Test {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple("Test")
                    .field(&"/example.com/{param}/{+param2}{/param3,param4}")
                    .finish()
            }
        }
        impl ::data_privacy::RedactedDisplay for Test {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                f.write_str("/example.com/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param, engine, f)?;
                f.write_str("/")?;
                ::std::write!(f, "{}", self.param2)?;
                f.write_str("/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param3, engine, f)?;
                f.write_str("/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param4, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for ::templated_uri::PathAndQuery {
            fn from(value: Test) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
        "#);
    }

    #[test]
    fn test_standalone_unredacted() {
        let attr = quote! { template="/example.com/{param}/{+param2}{/param3,param4}" };
        let item = quote! {
            struct Test {
                param: String,
                #[unredacted]
                param2: EscapedString,
                param3: String,
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: EscapedString,
            param3: String,
            param4: String,
        }
        impl ::templated_uri::PathAndQueryTemplate for Test {
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn format_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn render(&self) -> ::std::string::String {
                let param = ::templated_uri::Escape::escape(&self.param);
                let param2 = ::templated_uri::Raw::raw(&self.param2);
                let param3 = ::templated_uri::Escape::escape(&self.param3);
                let param4 = ::templated_uri::Escape::escape(&self.param4);
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                ::templated_uri::http::uri::PathAndQuery,
                ::templated_uri::UriError,
            > {
                Ok(
                    ::templated_uri::http::uri::PathAndQuery::try_from(
                        ::templated_uri::PathAndQueryTemplate::render(self),
                    )?,
                )
            }
        }
        impl ::std::fmt::Debug for Test {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple("Test")
                    .field(&"/example.com/{param}/{+param2}{/param3,param4}")
                    .finish()
            }
        }
        impl ::data_privacy::RedactedDisplay for Test {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                f.write_str("/example.com/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param, engine, f)?;
                f.write_str("/")?;
                ::std::write!(f, "{}", self.param2)?;
                f.write_str("/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param3, engine, f)?;
                f.write_str("/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param4, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for ::templated_uri::PathAndQuery {
            fn from(value: Test) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
        "#);
    }

    #[test]
    fn test_query_param_is_kv_expansion() {
        let attr = quote! { template="/api/{resource}{?page,limit}" };
        let item = quote! {
            struct QueryTest {
                resource: String,
                page: String,
                limit: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        // Verify the generated RedactedDisplay emits "key=" for query params
        assert_snapshot!(output_pretty, @r#"
        struct QueryTest {
            resource: String,
            page: String,
            limit: String,
        }
        impl ::templated_uri::PathAndQueryTemplate for QueryTest {
            fn template(&self) -> &'static core::primitive::str {
                "/api/{resource}{?page,limit}"
            }
            fn format_template(&self) -> &'static core::primitive::str {
                "/api/{resource}?page={page}&limit={limit}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn render(&self) -> ::std::string::String {
                let resource = ::templated_uri::Escape::escape(&self.resource);
                let page = ::templated_uri::Escape::escape(&self.page);
                let limit = ::templated_uri::Escape::escape(&self.limit);
                ::std::format!("/api/{resource}?page={page}&limit={limit}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                ::templated_uri::http::uri::PathAndQuery,
                ::templated_uri::UriError,
            > {
                Ok(
                    ::templated_uri::http::uri::PathAndQuery::try_from(
                        ::templated_uri::PathAndQueryTemplate::render(self),
                    )?,
                )
            }
        }
        impl ::std::fmt::Debug for QueryTest {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple("QueryTest").field(&"/api/{resource}{?page,limit}").finish()
            }
        }
        impl ::data_privacy::RedactedDisplay for QueryTest {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                f.write_str("/api/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.resource, engine, f)?;
                f.write_str("?")?;
                f.write_str("page")?;
                f.write_str("=")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.page, engine, f)?;
                f.write_str("&")?;
                f.write_str("limit")?;
                f.write_str("=")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.limit, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<QueryTest> for ::templated_uri::PathAndQuery {
            fn from(value: QueryTest) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
        "#);
    }

    #[test]
    fn test_excessive_template_impl() {
        let attr = quote! { template="/example.com/{param}/{+param2}{/param3,param4}" };
        let item = quote! {
            struct ExcessiveTemplate {
                param: String,
                param2: EscapedString,
                param3: String,
                param4: String,
                extra_param: String, // This should cause an error
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert!(
            output_pretty.contains("::core::compile_error!"),
            "Output should contain compile_error: {output_pretty}"
        );
        assert!(
            output_pretty.contains("Excess values in struct"),
            "Output should contain error message: {output_pretty}"
        );
    }

    #[test]
    fn test_insufficient_template_impl() {
        let attr = quote! { template="/{param}/{param2}" };
        let item = quote! {
            struct InsufficientTemplate {
                param: String,
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct InsufficientTemplate {
            param: String,
        }
        ::core::compile_error! {
            "Missing values in struct: [\"param2\"]"
        }
        "#);
    }

    #[test]
    fn test_parse_error() {
        let attr = quote! { template="/example.com/{param" };
        let item = quote! {
            struct ParseErrorTest {
                param: String,
            }
        };

        let output_pretty = pretty_parse(attr, item);

        assert!(
            output_pretty.contains("::core::compile_error!"),
            "Output should contain compile_error: {output_pretty}"
        );
        assert!(
            output_pretty.contains("Failed to parse URI"),
            "Output should contain error message: {output_pretty}"
        );

        let attr = quote! { template="/example.com/{>param}" };
        let item = quote! {
            struct ParseErrorTest {
                param: String,
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert!(
            output_pretty.contains("::core::compile_error!"),
            "Output should contain compile_error: {output_pretty}"
        );
        assert!(
            output_pretty.contains("Failed to parse URI"),
            "Output should contain error message: {output_pretty}"
        );
    }

    #[test]
    fn test_tuple_struct_rejected() {
        let attr = quote! { template = "/test/{param}" };
        let item = quote! {
            struct TupleTest(String);
        };

        let output_pretty = pretty_parse(attr, item);
        assert!(
            output_pretty.contains("can only be applied to structs with named fields"),
            "Output should reject tuple structs: {output_pretty}"
        );
    }

    #[test]
    fn test_enum_struct_item_error() {
        let attr = quote! {};
        let item = quote! {
            enum TestEnum {
                Variant1 { param: String },
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        enum TestEnum {
            Variant1 { param: String },
        }
        ::core::compile_error! {
            "Only unnamed fields (tuples) are supported in enum variants for TemplatedUri"
        }
        "#);
    }

    #[test]
    fn test_enum_single_item_only_error() {
        let attr = quote! {};
        let item = quote! {
            enum TestEnum {
                Variant1(String, String),
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        enum TestEnum {
            Variant1(String, String),
        }
        ::core::compile_error! {
            "TemplatedUri enum variants must have exactly one field containing a TemplatedUri struct"
        }
        "#);
    }

    #[test]
    fn test_template_enum_impl() {
        let attr = quote! {};
        let item = quote! {
            enum Test {
                FirstTemplate(First),
                SecondTemplate(Second),
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        enum Test {
            FirstTemplate(First),
            SecondTemplate(Second),
        }
        impl ::templated_uri::PathAndQueryTemplate for Test {
            fn template(&self) -> &'static core::primitive::str {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.template(),
                    Test::SecondTemplate(template_variant) => template_variant.template(),
                }
            }
            fn format_template(&self) -> &'static core::primitive::str {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.format_template(),
                    Test::SecondTemplate(template_variant) => template_variant.format_template(),
                }
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.label(),
                    Test::SecondTemplate(template_variant) => template_variant.label(),
                }
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                ::templated_uri::http::uri::PathAndQuery,
                ::templated_uri::UriError,
            > {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.to_path_and_query(),
                    Test::SecondTemplate(template_variant) => {
                        template_variant.to_path_and_query()
                    }
                }
            }
            fn render(&self) -> ::std::string::String {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.render(),
                    Test::SecondTemplate(template_variant) => template_variant.render(),
                }
            }
        }
        impl ::std::fmt::Debug for Test {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    Test::FirstTemplate(template_variant) => {
                        f.debug_tuple("Test").field(&template_variant).finish()
                    }
                    Test::SecondTemplate(template_variant) => {
                        f.debug_tuple("Test").field(&template_variant).finish()
                    }
                }
            }
        }
        impl ::data_privacy::RedactedDisplay for Test {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                match self {
                    Test::FirstTemplate(template_variant) => {
                        ::data_privacy::RedactedDisplay::fmt(template_variant, engine, f)?
                    }
                    Test::SecondTemplate(template_variant) => {
                        ::data_privacy::RedactedDisplay::fmt(template_variant, engine, f)?
                    }
                }
                Ok(())
            }
        }
        impl ::std::convert::From<First> for Test {
            fn from(template_variant: First) -> Self {
                Self::FirstTemplate(template_variant)
            }
        }
        impl ::std::convert::From<Second> for Test {
            fn from(template_variant: Second) -> Self {
                Self::SecondTemplate(template_variant)
            }
        }
        impl From<Test> for ::templated_uri::PathAndQuery {
            fn from(value: Test) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
        "#);
    }

    #[test]
    fn test_raw_impl() {
        let input = quote! {
            struct MyFragment(String);
        };

        let output_pretty = pretty_parse_raw(input);
        assert_snapshot!(output_pretty, @r"
        impl ::templated_uri::Raw for MyFragment {
            fn raw(&self) -> impl ::std::fmt::Display {
                &self.0
            }
        }
        ");
    }

    #[test]
    fn test_raw_with_custom_type() {
        let input = quote! {
            struct CustomFragment(EscapedString);
        };

        let output_pretty = pretty_parse_raw(input);
        assert_snapshot!(output_pretty, @r"
        impl ::templated_uri::Raw for CustomFragment {
            fn raw(&self) -> impl ::std::fmt::Display {
                &self.0
            }
        }
        ");
    }

    #[test]
    fn test_raw_named_fields_error() {
        let input = quote! {
            struct InvalidFragment {
                value: String
            }
        };

        let output_pretty = pretty_parse_raw(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Raw can only be derived for tuple structs (newtype pattern)"
        }
        "#);
    }

    #[test]
    fn test_raw_multiple_fields_error() {
        let input = quote! {
            struct TooManyFields(String, String);
        };

        let output_pretty = pretty_parse_raw(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Raw requires exactly one field, found 2"
        }
        "#);
    }

    #[test]
    fn test_raw_enum_error() {
        let input = quote! {
            enum FragmentEnum {
                Variant(String)
            }
        };

        let output_pretty = pretty_parse_raw(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Raw cannot be derived for enums"
        }
        "#);
    }

    #[test]
    fn test_raw_union_error() {
        let input = quote! {
            union UnsafeFragmentUnion {
                value: u32
            }
        };

        let output_pretty = pretty_parse_raw(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Raw cannot be derived for unions"
        }
        "#);
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
        let input = quote! {
            struct SafeFragment(String);
        };

        let output_pretty = pretty_parse_uri_param(input);
        assert_snapshot!(output_pretty, @r"
        impl ::templated_uri::Escape for SafeFragment {
            fn escape(&self) -> ::templated_uri::Escaped<impl ::std::fmt::Display> {
                self.0.escape()
            }
        }
        ");
    }

    #[test]
    fn test_uri_param_named_fields_error() {
        let input = quote! {
            struct InvalidSafeFragment {
                value: String
            }
        };

        let output_pretty = pretty_parse_uri_param(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Escape can only be derived for tuple structs (newtype pattern)"
        }
        "#);
    }

    #[test]
    fn test_uri_param_enum_error() {
        let input = quote! {
            enum SafeFragmentEnum {
                Variant(String)
            }
        };

        let output_pretty = pretty_parse_uri_param(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Escape cannot be derived for enums"
        }
        "#);
    }

    #[test]
    fn test_uri_param_union_error() {
        let input = quote! {
            union SafeFragmentUnion {
                value: u32
            }
        };

        let output_pretty = pretty_parse_uri_param(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Escape cannot be derived for unions"
        }
        "#);
    }

    #[test]
    fn test_uri_param_multiple_fields_error() {
        let input = quote! {
            struct TooManySafeFields(String, String);
        };

        let output_pretty = pretty_parse_uri_param(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Escape requires exactly one field, found 2"
        }
        "#);
    }

    #[test]
    fn test_uri_param_zero_fields_error() {
        let input = quote! {
            struct NoFields();
        };

        let output_pretty = pretty_parse_uri_param(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Escape requires exactly one field, found 0"
        }
        "#);
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
        let attr = quote! { template="/{param}" };
        let item = quote! {
            union TestUnion {
                field1: u32,
                field2: i32,
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Unions are not supported for TemplatedUri"
        }
        "#);
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
        let attr = quote! { template="/{param}" };
        let item = quote! {
            struct GenericTemplate<T> {
                param: T,
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "Generic types are not supported for #[templated]"
        }
        "#);
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
