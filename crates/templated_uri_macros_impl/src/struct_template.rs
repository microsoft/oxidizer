// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::option_if_let_else, reason = "Darling's macro expansion currently uses this pattern")]
#![expect(
    clippy::needless_continue,
    reason = "Darling's macro expansion triggers this lint until next version gets released (https://github.com/TedDriggs/darling/pull/402)"
)]

use std::collections::HashSet;

use darling::{FromAttributes, FromField};
use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Attribute, DataStruct, Field};

use crate::template_parser::{TemplatePart, UriTemplate};

#[derive(Debug, FromAttributes)]
#[darling(attributes(templated))]
pub struct Opts {
    #[darling(rename = "template")]
    pub input_template: String,
    #[darling(default)]
    pub unredacted: bool,
    /// Optional label for telemetry. When provided, this label is used in metrics
    /// instead of the full template string, which is useful for complex templates.
    #[darling(default)]
    pub label: Option<String>,
}

#[derive(Debug, FromField)]
#[darling(attributes(templated))]
pub struct FieldOpts {
    pub ident: Option<Ident>,
    #[darling(default)]
    pub unredacted: bool,
}

/// Represents the fields of a struct with their options parsed from attributes.
struct Fields {
    fields: Vec<FieldOpts>,
}

impl Fields {
    /// Constructs a new `Fields` instance by parsing a slice of `Field`.
    fn from_fields(fields: &[&Field]) -> darling::Result<Self> {
        let fields = fields
            .iter()
            .map(|&f| {
                let mut opts = FieldOpts::from_field(f)?;
                // Also check for standalone #[unredacted] attribute
                if !opts.unredacted {
                    opts.unredacted = f.attrs.iter().any(|attr| attr.path().is_ident("unredacted"));
                }
                Ok(opts)
            })
            .collect::<darling::Result<Vec<_>>>()?;
        Ok(Self { fields })
    }

    /// Returns the names of the fields in the struct
    fn field_names(&self) -> HashSet<String> {
        self.fields
            .iter()
            .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
            .collect()
    }
}

// #[proc_macro_derive(TemplatedPathAndQuery, attributes(templated, unredacted))]
pub fn struct_template(ident: Ident, data: &DataStruct, attrs: &[Attribute]) -> TokenStream {
    if !matches!(data.fields, syn::Fields::Named(_)) {
        crate::bail!(ident, "#[templated] can only be applied to structs with named fields");
    }

    // Parse the derive input using the Opts struct with custom parsing
    let struct_name = ident.to_string();
    let Opts {
        input_template,
        unredacted,
        label,
    } = match Opts::from_attributes(attrs) {
        Ok(opts) => opts,
        Err(err) => return err.write_errors(),
    };

    let template = match UriTemplate::parse(&input_template) {
        Ok(template) => template,
        Err(err) => {
            return err.to_compile_error(ident.span());
        }
    };

    let format_template = template.format_template();

    let template_params: Vec<_> = template.params().collect();
    let struct_fields: Vec<&Field> = data.fields.iter().collect();

    let fields = match Fields::from_fields(struct_fields.as_slice()) {
        Ok(fields) => fields,
        Err(err) => return err.write_errors(),
    };

    let struct_field_names = fields.field_names();
    let template_param_names = template.param_names();

    // Compare the template parameters with the struct fields and errors if there are mismatches.
    let mut excess_values: Vec<_> = struct_field_names.difference(&template_param_names).collect();
    excess_values.sort();

    let mut missing_values: Vec<_> = template_param_names.difference(&struct_field_names).collect();
    missing_values.sort();

    if !missing_values.is_empty() {
        crate::bail!(ident, "Missing values in struct: {missing_values:?}")
    }

    if !excess_values.is_empty() {
        crate::bail!(ident, "Excess values in struct: {excess_values:?}")
    }

    // Determine which parameters are unrestricted (Can contain any value) and which are restricted (Must be `Escaped`).
    let unrestricted_params: HashSet<String> = template_params
        .iter()
        .filter(|p| p.is_unrestricted)
        .map(|p| p.name.to_owned())
        .collect();

    let is_restricted = |p: &Ident| !unrestricted_params.contains(&p.to_string());

    let collect_params: Vec<_> = struct_fields
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().expect("struct fields must be named");
            let is_restricted = is_restricted(ident);
            let ty_span = f.ty.span();

            // Restricted fields use .escape(), unrestricted use .raw()
            if is_restricted {
                quote_spanned! { ty_span => let #ident = ::templated_uri::Escape::escape(&self.#ident); }
            } else {
                quote_spanned! { ty_span => let #ident = ::templated_uri::Raw::raw(&self.#ident); }
            }
        })
        .collect();

    let redacted_display = construct_redacted_display(&template, &struct_fields, &fields, unredacted);

    let label_impl = label.as_ref().map_or_else(
        || quote! { ::core::option::Option::None },
        |l| quote! { ::core::option::Option::Some(#l) },
    );

    quote! {
        impl ::templated_uri::PathAndQueryTemplate for #ident {
            fn template(&self) -> &'static core::primitive::str {
                #input_template
            }

            fn format_template(&self) -> &'static core::primitive::str {
                #format_template
            }

            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                #label_impl
            }

            fn render(&self) -> ::std::string::String {
                #(#collect_params)*

                ::std::format!(#format_template)
            }

            fn to_path_and_query(&self) -> ::std::result::Result<::templated_uri::http::uri::PathAndQuery, ::templated_uri::UriError> {
                Ok(::templated_uri::http::uri::PathAndQuery::try_from(::templated_uri::PathAndQueryTemplate::render(self))?)
            }
        }

        impl ::std::fmt::Debug for #ident {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_tuple(#struct_name)
                    .field(&#input_template)
                    .finish()
            }
        }

        impl ::data_privacy::RedactedDisplay for #ident {
            fn fmt(&self, engine: &::data_privacy::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                #redacted_display
            }
        }

        impl From<#ident> for ::templated_uri::PathAndQuery {
            fn from(value: #ident) -> Self {
                ::templated_uri::PathAndQuery::from_template(value)
            }
        }
    }
}

fn construct_redacted_display(template: &UriTemplate, struct_fields: &[&Field], fields: &Fields, unredacted: bool) -> TokenStream {
    // Build a map from field name to field for lookup
    let field_map: std::collections::HashMap<String, &Field> = struct_fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|ident| (ident.to_string(), *f)))
        .collect();

    // Build a map from field name to field options for checking unredacted attribute
    let field_opts_map: std::collections::HashMap<String, &FieldOpts> = fields
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(|ident| (ident.to_string(), f)))
        .collect();

    let statements: Vec<TokenStream> = template
        .template_parts()
        .iter()
        .flat_map(|part| match part {
            TemplatePart::Content(content) => {
                // For static content, just write it to the formatter
                vec![quote! { f.write_str(#content)?; }]
            }
            TemplatePart::ParamGroup(group) => {
                let prefix = group.prefix().unwrap_or_default();
                let separator = group.separator();
                let is_kv = group.is_kv();
                let param_names = group.param_names();

                let mut stmts = Vec::new();
                for (i, param_name) in param_names.iter().enumerate() {
                    // Emit prefix (first param) or separator (subsequent params)
                    let delim = if i == 0 { prefix } else { separator };
                    if !delim.is_empty() {
                        stmts.push(quote! { f.write_str(#delim)?; });
                    }

                    // Emit key= for KV expansions (e.g. ?key=value, ;key=value)
                    if is_kv {
                        let key = *param_name;
                        stmts.push(quote! { f.write_str(#key)?; f.write_str("=")?; });
                    }

                    let field = field_map.get(*param_name).expect("Field should exist (validated earlier)");
                    let field_ident = field.ident.as_ref().expect("struct fields must be named");
                    let field_type = &field.ty;

                    // Check if this specific field is marked as unredacted
                    let field_unredacted = field_opts_map.get(*param_name).is_some_and(|opts| opts.unredacted);

                    if unredacted || field_unredacted {
                        stmts.push(quote! {
                            ::std::write!(f, "{}", self.#field_ident)?;
                        });
                    } else {
                        stmts.push(quote! {
                            <#field_type as ::data_privacy::RedactedDisplay>::fmt(&self.#field_ident, engine, f)?;
                        });
                    }
                }
                stmts
            }
        })
        .collect();

    quote! {
        #(#statements)*
        ::std::result::Result::Ok(())
    }
}
