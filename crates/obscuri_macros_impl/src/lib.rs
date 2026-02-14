// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/obscuri_macros_impl/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/obscuri_macros_impl/favicon.ico"
)]

//! Macros for the [`obscuri`](https://docs.rs/obscuri) crate.

mod enum_template;
pub(crate) mod error;
mod struct_template;
pub(crate) mod template_parser;
mod uri_fragment;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse2};

use crate::enum_template::enum_template;
use crate::struct_template::struct_template;
use crate::uri_fragment::{uri_fragment_impl, uri_unsafe_fragment_impl};

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
#[expect(clippy::too_many_lines, reason = "Macro code")]
#[cfg_attr(test, mutants::skip)] // not relevant for auto-generated proc macros
pub fn templated_paq_impl(attr: &TokenStream, item: TokenStream) -> TokenStream {
    // Parse the item (struct/enum definition)
    let mut input: DeriveInput = match parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    // If attributes were passed via the attribute macro, parse and add them
    if !attr.is_empty() {
        // Create an attribute from the tokens and add it to the input's attributes
        let attr_tokens = quote! { #[templated(#attr)] };
        match syn::parse2::<syn::DeriveInput>(quote! {
            #attr_tokens
            struct Dummy;
        }) {
            Ok(dummy) => {
                if let Some(parsed_attr) = dummy.attrs.into_iter().next() {
                    input.attrs.push(parsed_attr);
                }
            }
            Err(err) => return err.to_compile_error(),
        }
    }

    // Filter out the 'templated' attribute from the output to avoid recursion
    let output_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| !attr.path().is_ident("templated"))
        .collect();

    // Generate the original item definition WITHOUT the templated attribute
    let vis = &input.vis;
    let ident = &input.ident;
    let generics = &input.generics;
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let original = match &input.data {
        syn::Data::Struct(s) => {
            // Filter out templated and unredacted attributes from fields
            let filtered_fields = match &s.fields {
                syn::Fields::Named(fields) => {
                    let fields: Vec<_> = fields
                        .named
                        .iter()
                        .map(|f| {
                            let attrs: Vec<_> = f
                                .attrs
                                .iter()
                                .filter(|attr| {
                                    !attr.path().is_ident("templated")
                                        && !attr.path().is_ident("unredacted")
                                })
                                .collect();
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
                                .filter(|attr| {
                                    !attr.path().is_ident("templated")
                                        && !attr.path().is_ident("unredacted")
                                })
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
    };

    let implementation = match input.data {
        syn::Data::Struct(ref s) => struct_template(input.ident.clone(), s, &input.attrs),
        syn::Data::Enum(ref e) => enum_template(&input.ident, e),
        syn::Data::Union(_) => {
            return syn::Error::new_spanned(
                input.ident,
                "Unions are not supported for TemplatedUri",
            )
            .to_compile_error();
        }
    };

    quote! {
        #original
        #implementation
    }
}

#[must_use]
#[cfg_attr(test, mutants::skip)] // just emits compile error otherwise
pub fn uri_fragment_derive_impl(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    uri_fragment_impl(input)
}

#[must_use]
pub fn uri_unsafe_fragment_derive_impl(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    uri_unsafe_fragment_impl(input)
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

    fn pretty_parse_uri_fragment(input: TokenStream) -> String {
        let output = uri_unsafe_fragment_derive_impl(input);
        prettyplease::unparse(&syn::parse_file(&output.to_string()).unwrap())
    }

    #[test]
    fn test_templated_uri_impl() {
        let attr = quote! { template="/example.com/{param}/{+param2}{/param3,param4}" };
        let item = quote! {
            struct Test {
                param: String,
                param2: UriSafeString,
                param3: String,
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: UriSafeString,
            param3: String,
            param4: String,
        }
        impl obscuri::TemplatedPathAndQuery for Test {
            fn rfc_6570_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn to_uri_string(&self) -> ::std::string::String {
                use ::obscuri::UriFragment;
                use ::obscuri::UriUnsafeFragment;
                let param = self.param.as_uri_safe();
                let param2 = self.param2.as_display();
                let param3 = self.param3.as_uri_safe();
                let param4 = self.param4.as_uri_safe();
                let param: &dyn ::obscuri::UriSafe = &param;
                let param3: &dyn ::obscuri::UriSafe = &param3;
                let param4: &dyn ::obscuri::UriSafe = &param4;
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                obscuri::uri::PathAndQuery,
                obscuri::ValidationError,
            > {
                let uri_string = self.to_uri_string();
                Ok(obscuri::uri::PathAndQuery::try_from(uri_string)?)
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
                ::std::write!(f, "{}", "/example.com/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param, engine, f)?;
                ::std::write!(f, "{}", "/")?;
                <UriSafeString as ::data_privacy::RedactedDisplay>::fmt(
                    &self.param2,
                    engine,
                    f,
                )?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param3, engine, f)?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param4, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for obscuri::uri::TargetPathAndQuery {
            fn from(value: Test) -> Self {
                obscuri::uri::TargetPathAndQuery::TemplatedPathAndQuery(
                    std::sync::Arc::new(value),
                )
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
                param2: UriSafeString,
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
            param2: UriSafeString,
            param3: String,
            param4: String,
        }
        impl obscuri::TemplatedPathAndQuery for Test {
            fn rfc_6570_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn to_uri_string(&self) -> ::std::string::String {
                use ::obscuri::UriFragment;
                use ::obscuri::UriUnsafeFragment;
                let param = self.param.as_uri_safe();
                let param2 = self.param2.as_display();
                let param3 = self.param3.as_uri_safe();
                let param4 = self.param4.as_uri_safe();
                let param: &dyn ::obscuri::UriSafe = &param;
                let param3: &dyn ::obscuri::UriSafe = &param3;
                let param4: &dyn ::obscuri::UriSafe = &param4;
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                obscuri::uri::PathAndQuery,
                obscuri::ValidationError,
            > {
                let uri_string = self.to_uri_string();
                Ok(obscuri::uri::PathAndQuery::try_from(uri_string)?)
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
                ::std::write!(f, "{}", "/example.com/")?;
                ::std::write!(f, "{}", self.param)?;
                ::std::write!(f, "{}", "/")?;
                ::std::write!(f, "{}", self.param2)?;
                ::std::write!(f, "{}", self.param3)?;
                ::std::write!(f, "{}", self.param4)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for obscuri::uri::TargetPathAndQuery {
            fn from(value: Test) -> Self {
                obscuri::uri::TargetPathAndQuery::TemplatedPathAndQuery(
                    std::sync::Arc::new(value),
                )
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
                param2: UriSafeString,
                param3: String,
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: UriSafeString,
            param3: String,
            param4: String,
        }
        impl obscuri::TemplatedPathAndQuery for Test {
            fn rfc_6570_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn to_uri_string(&self) -> ::std::string::String {
                use ::obscuri::UriFragment;
                use ::obscuri::UriUnsafeFragment;
                let param = self.param.as_uri_safe();
                let param2 = self.param2.as_display();
                let param3 = self.param3.as_uri_safe();
                let param4 = self.param4.as_uri_safe();
                let param: &dyn ::obscuri::UriSafe = &param;
                let param3: &dyn ::obscuri::UriSafe = &param3;
                let param4: &dyn ::obscuri::UriSafe = &param4;
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                obscuri::uri::PathAndQuery,
                obscuri::ValidationError,
            > {
                let uri_string = self.to_uri_string();
                Ok(obscuri::uri::PathAndQuery::try_from(uri_string)?)
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
                ::std::write!(f, "{}", "/example.com/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param, engine, f)?;
                ::std::write!(f, "{}", "/")?;
                ::std::write!(f, "{}", self.param2)?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param3, engine, f)?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param4, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for obscuri::uri::TargetPathAndQuery {
            fn from(value: Test) -> Self {
                obscuri::uri::TargetPathAndQuery::TemplatedPathAndQuery(
                    std::sync::Arc::new(value),
                )
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
                param2: UriSafeString,
                param3: String,
                param4: String
            }
        };

        let output_pretty = pretty_parse(attr, item);
        assert_snapshot!(output_pretty, @r#"
        struct Test {
            param: String,
            param2: UriSafeString,
            param3: String,
            param4: String,
        }
        impl obscuri::TemplatedPathAndQuery for Test {
            fn rfc_6570_template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{+param2}{/param3,param4}"
            }
            fn template(&self) -> &'static core::primitive::str {
                "/example.com/{param}/{param2}/{param3}/{param4}"
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                ::core::option::Option::None
            }
            fn to_uri_string(&self) -> ::std::string::String {
                use ::obscuri::UriFragment;
                use ::obscuri::UriUnsafeFragment;
                let param = self.param.as_uri_safe();
                let param2 = self.param2.as_display();
                let param3 = self.param3.as_uri_safe();
                let param4 = self.param4.as_uri_safe();
                let param: &dyn ::obscuri::UriSafe = &param;
                let param3: &dyn ::obscuri::UriSafe = &param3;
                let param4: &dyn ::obscuri::UriSafe = &param4;
                ::std::format!("/example.com/{param}/{param2}/{param3}/{param4}")
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                obscuri::uri::PathAndQuery,
                obscuri::ValidationError,
            > {
                let uri_string = self.to_uri_string();
                Ok(obscuri::uri::PathAndQuery::try_from(uri_string)?)
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
                ::std::write!(f, "{}", "/example.com/")?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param, engine, f)?;
                ::std::write!(f, "{}", "/")?;
                ::std::write!(f, "{}", self.param2)?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param3, engine, f)?;
                <String as ::data_privacy::RedactedDisplay>::fmt(&self.param4, engine, f)?;
                ::std::result::Result::Ok(())
            }
        }
        impl From<Test> for obscuri::uri::TargetPathAndQuery {
            fn from(value: Test) -> Self {
                obscuri::uri::TargetPathAndQuery::TemplatedPathAndQuery(
                    std::sync::Arc::new(value),
                )
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
                param2: UriSafeString,
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
            struct ParseErrorTest;
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
            struct ParseErrorTest;
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
        impl obscuri::TemplatedPathAndQuery for Test {
            fn rfc_6570_template(&self) -> &'static core::primitive::str {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.rfc_6570_template(),
                    Test::SecondTemplate(template_variant) => {
                        template_variant.rfc_6570_template()
                    }
                }
            }
            fn template(&self) -> &'static core::primitive::str {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.template(),
                    Test::SecondTemplate(template_variant) => template_variant.template(),
                }
            }
            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.label(),
                    Test::SecondTemplate(template_variant) => template_variant.label(),
                }
            }
            fn to_uri_string(&self) -> ::std::string::String {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.to_uri_string(),
                    Test::SecondTemplate(template_variant) => template_variant.to_uri_string(),
                }
            }
            fn to_path_and_query(
                &self,
            ) -> ::std::result::Result<
                obscuri::uri::PathAndQuery,
                obscuri::ValidationError,
            > {
                match self {
                    Test::FirstTemplate(template_variant) => template_variant.to_path_and_query(),
                    Test::SecondTemplate(template_variant) => {
                        template_variant.to_path_and_query()
                    }
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
                    Test::FirstTemplate(template_variant) => template_variant.fmt(engine, f)?,
                    Test::SecondTemplate(template_variant) => template_variant.fmt(engine, f)?,
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
        impl From<Test> for obscuri::uri::TargetPathAndQuery {
            fn from(value: Test) -> Self {
                obscuri::uri::TargetPathAndQuery::TemplatedPathAndQuery(
                    std::sync::Arc::new(value),
                )
            }
        }
        "#);
    }

    #[test]
    fn test_uri_fragment_impl() {
        let input = quote! {
            struct MyFragment(String);
        };

        let output_pretty = pretty_parse_uri_fragment(input);
        assert_snapshot!(output_pretty, @r"
        impl ::obscuri::UriUnsafeFragment for MyFragment {
            fn as_display(&self) -> impl ::std::fmt::Display {
                <Self as ::std::string::ToString>::to_string(self)
            }
        }
        ");
    }

    #[test]
    fn test_uri_fragment_with_custom_type() {
        let input = quote! {
            struct CustomFragment(UriSafeString);
        };

        let output_pretty = pretty_parse_uri_fragment(input);
        assert_snapshot!(output_pretty, @r"
        impl ::obscuri::UriUnsafeFragment for CustomFragment {
            fn as_display(&self) -> impl ::std::fmt::Display {
                <Self as ::std::string::ToString>::to_string(self)
            }
        }
        ");
    }

    #[test]
    fn test_uri_fragment_named_fields_error() {
        let input = quote! {
            struct InvalidFragment {
                value: String
            }
        };

        let output_pretty = pretty_parse_uri_fragment(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "UriUnsafeFragment can only be derived for tuple structs (newtype pattern)"
        }
        "#);
    }

    #[test]
    fn test_uri_fragment_multiple_fields_error() {
        let input = quote! {
            struct TooManyFields(String, String);
        };

        let output_pretty = pretty_parse_uri_fragment(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "UriUnsafeFragment requires exactly one field, found 2"
        }
        "#);
    }

    #[test]
    fn test_uri_fragment_enum_error() {
        let input = quote! {
            enum FragmentEnum {
                Variant(String)
            }
        };

        let output_pretty = pretty_parse_uri_fragment(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "UriUnsafeFragment cannot be derived for enums"
        }
        "#);
    }

    #[test]
    fn test_uri_fragment_unit_struct_error() {
        let input = quote! {
            struct UnitFragment;
        };

        let output_pretty = pretty_parse_uri_fragment(input);
        assert_snapshot!(output_pretty, @r#"
        ::core::compile_error! {
            "UriUnsafeFragment can only be derived for tuple structs (newtype pattern)"
        }
        "#);
    }
}
