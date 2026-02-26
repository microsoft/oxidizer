// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, Fields, Path, parse_quote};

use crate::container_attrs::{ContainerAttrs, parse_container_attrs};
use crate::field_attrs::{FieldAttrs, is_into_type, parse_field_attrs};

/// Parsed information about a single struct field.
struct FieldInfo {
    ident: syn::Ident,
    index: usize,
    ty: syn::Type,
    attrs: FieldAttrs,
}

pub fn generate(input: &DeriveInput, root_path: &Path) -> syn::Result<TokenStream2> {
    let data = match &input.data {
        syn::Data::Struct(s) => s,
        syn::Data::Enum(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "#[derive(Event)] does not support enums",
            ));
        }
        syn::Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "#[derive(Event)] does not support unions",
            ));
        }
    };

    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Event)] only supports structs with named fields",
        ));
    };

    let container = parse_container_attrs(&input.attrs)?;

    let fields: Vec<FieldInfo> = named
        .named
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let attrs = parse_field_attrs(&f.attrs)?;
            Ok(FieldInfo {
                ident: f.ident.clone().expect("named field"),
                index: i,
                ty: f.ty.clone(),
                attrs,
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let description_tokens = gen_description(&container, &fields, root_path);
    let value_tokens = gen_value(&fields, root_path);
    let instructions_tokens = gen_default_instructions(&container, &fields, root_path);

    let mut event_path = root_path.clone();
    event_path.segments.push(parse_quote!(Event));

    Ok(quote! {
        impl #impl_generics #event_path for #name #ty_generics #where_clause {
            #description_tokens
            #instructions_tokens
            #value_tokens
        }
    })
}

fn gen_description(container: &ContainerAttrs, fields: &[FieldInfo], root_path: &Path) -> TokenStream2 {
    let event_name = &container.name;
    let event_id = container.id;

    let mut event_desc_path = root_path.clone();
    event_desc_path.segments.push(parse_quote!(EventDescription));

    let mut field_desc_path = root_path.clone();
    field_desc_path.segments.push(parse_quote!(FieldDescription));

    let field_entries: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let fname = f.ident.to_string();
            let findex = f.index as u64;
            quote! {
                #field_desc_path {
                    name: #fname,
                    index: #findex,
                }
            }
        })
        .collect();

    quote! {
        const DESCRIPTION: #event_desc_path = #event_desc_path {
            name: #event_name,
            id: #event_id,
            fields: &[#(#field_entries),*],
        };
    }
}

fn gen_value(fields: &[FieldInfo], root_path: &Path) -> TokenStream2 {
    let mut field_desc_path = root_path.clone();
    field_desc_path.segments.push(parse_quote!(FieldDescription));

    let mut tsv_path = root_path.clone();
    tsv_path.segments.push(parse_quote!(TelemetrySafeValue));

    let arms: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let idx = f.index as u64;
            let ident = &f.ident;
            if is_into_type(&f.ty) {
                quote! { #idx => self.#ident.into() }
            } else {
                quote! { #idx => #tsv_path::from_redacted(&self.#ident, redactor) }
            }
        })
        .collect();

    quote! {
        fn value(&self, field: &#field_desc_path, redactor: &::data_privacy::RedactionEngine) -> #tsv_path {
            match field.index {
                #(#arms,)*
                _ => panic!("Unknown field index"),
            }
        }
    }
}

fn gen_default_instructions(container: &ContainerAttrs, fields: &[FieldInfo], root_path: &Path) -> TokenStream2 {
    let mut pi_path = root_path.clone();
    pi_path.segments.push(parse_quote!(ProcessingInstructions));

    let mut gpi_path = root_path.clone();
    gpi_path.segments.push(parse_quote!(GenericProcessingInstructions));

    let mut lpi_path = root_path.clone();
    lpi_path.segments.push(parse_quote!(LogProcessingInstructions));

    let mut mpi_path = root_path.clone();
    mpi_path.segments.push(parse_quote!(MetricProcessingInstructions));

    // Generate log instructions
    let log_entries: Vec<TokenStream2> = container
        .logs
        .iter()
        .map(|log| {
            let log_name = &log.name;
            let message = &log.message;

            // Collect field indices that should be included in this log
            let included_indices: Vec<usize> = fields
                .iter()
                .filter(|f| f.attrs.include_in_logs || f.attrs.include_in_log.contains(log_name))
                .map(|f| f.index)
                .collect();

            let field_refs: Vec<TokenStream2> = included_indices
                .iter()
                .map(|&i| quote! { Self::DESCRIPTION.fields[#i] })
                .collect();

            quote! {
                #lpi_path {
                    logger_name: #log_name,
                    included_fields: vec![#(#field_refs),*],
                    message_template: #message,
                }
            }
        })
        .collect();

    // Generate metric instructions
    let metric_entries: Vec<TokenStream2> = fields
        .iter()
        .filter_map(|f| {
            let metric_attr = f.metric_attr()?;
            let meter_name = &container.name;
            let instrument_name = &metric_attr.name;
            let kind = &metric_attr.kind;
            let field_index = f.index;

            // Collect dimension field indices
            let dimension_indices: Vec<usize> = fields
                .iter()
                .filter(|dim_f| dim_f.attrs.include_in_metrics || dim_f.attrs.include_in_metric.contains(instrument_name))
                .map(|dim_f| dim_f.index)
                .collect();

            let dim_refs: Vec<TokenStream2> = dimension_indices
                .iter()
                .map(|&i| quote! { Self::DESCRIPTION.fields[#i] })
                .collect();

            Some(quote! {
                #mpi_path {
                    meter_name: #meter_name,
                    instrument_name: #instrument_name,
                    included_dimensions: vec![#(#dim_refs),*],
                    metric_field: Self::DESCRIPTION.fields[#field_index],
                    instrument_kind: #kind,
                }
            })
        })
        .collect();

    quote! {
        fn default_instructions() -> #pi_path<Self> {
            #pi_path {
                generic_instructions: #gpi_path {
                    log_instructions: vec![#(#log_entries),*],
                    metric_instructiosns: vec![#(#metric_entries),*],
                },
                additional_processing: None,
            }
        }
    }
}

impl FieldInfo {
    fn metric_attr(&self) -> Option<&crate::field_attrs::MetricAttr> {
        self.attrs.metric.as_ref()
    }
}
