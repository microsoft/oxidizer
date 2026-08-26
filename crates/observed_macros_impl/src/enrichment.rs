// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of the `#[derive(Enrichment)]` macro.
//!
//! Generates an `Enrichment` trait implementation that converts a struct into
//! a `Vec<EnrichmentEntry>`.
//!
//! Unlike `#[event(...)]`, enrichment fields cannot *be* a metric value;
//! they can only opt in as metric **dimensions** via `#[dimension(metric)]` or
//! `#[dimension(metric = "...")]`.
//!
//! ## Attribute syntax
//!
//! Enrichment uses the **same field-level attributes** as `#[event(...)]`:
//!
//! | Attribute | Example | Description |
//! |-----------|---------|-------------|
//! | `#[dimension]` | `#[dimension]` | Log under the field's own name; not a metric dimension (the explicit default) |
//! | `#[dimension(log = "...")]` | `#[dimension(log = "region")]` | Rename the log key only; metric dimensions stay opt-in |
//! | `#[dimension(metric)]` | `#[dimension(metric)]` | Opt in as a metric dimension keyed by the field's own name |
//! | `#[dimension(log = "...", metric = "...")]` | `#[dimension(log = "http.method", metric = "method")]` | Set the log and/or metric key independently (at least one required) |
//! | `#[dimension(log = exclude)]` | `#[dimension(log = exclude, metric)]` | Exclude the field from the log signal (optionally still a metric dimension) |
//! | `#[data_class(expr)]` | `#[data_class(DataTaxonomy::Euii)]` | Data-classification expression |
//! | `#[unredacted]` | `#[unredacted]` | Bypass redaction; type must implement `Into<Value>` |
//! | `#[if_none(...)]` | `#[if_none(drop)]` | Control how a `None` `Option<T>` is recorded (default: `#[if_none("n/a")]`) |
//!
//! ## Optional fields
//!
//! A field of type `Option<T>` behaves exactly as it does in `#[event(...)]`:
//! when it is `None`, `#[if_none(...)]` decides whether the entry is
//! dropped or filled with a placeholder string (default `"n/a"`).

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt as _;
use syn::{Data, DeriveInput, Error, Field, Fields, Generics, Ident, Result};

use crate::field_attrs::{FieldRedaction, IfNone, LogRouting, SharedFieldAttrs, is_borrowed_str, option_inner_type};

// ================================================================================================
// Intermediate Structs (Parse Phase Output)
// ================================================================================================

/// Parsed definition of an enrichment struct.
struct EnrichmentDef {
    /// The struct identifier.
    ident: Ident,
    /// Generics (including lifetimes) from the struct definition.
    generics: Generics,
    /// Parsed field definitions.
    fields: Vec<EnrichmentFieldDef>,
}

/// Parsed definition of a single enrichment field.
struct EnrichmentFieldDef {
    /// The field identifier.
    ident: Ident,
    /// The field type (used to detect `Option<T>`).
    ty: syn::Type,
    /// Shared field-level attributes (`dimension` + redaction).
    shared: SharedFieldAttrs,
}

// ================================================================================================
// Parse Phase
// ================================================================================================

/// Parses `DeriveInput` into `EnrichmentDef`.
fn parse_enrichment_def(input: &DeriveInput) -> Result<EnrichmentDef> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            Fields::Unnamed(_) | Fields::Unit => {
                return Err(Error::new_spanned(
                    input,
                    "Enrichment can only be derived for structs with named fields",
                ));
            }
        },
        Data::Enum(_) => {
            return Err(Error::new_spanned(input, "Enrichment can only be derived for structs, not enums"));
        }
        Data::Union(_) => {
            return Err(Error::new_spanned(input, "Enrichment can only be derived for structs, not unions"));
        }
    };

    let mut field_defs = Vec::with_capacity(fields.len());
    for field in fields {
        field_defs.push(parse_enrichment_field_def(field)?);
    }

    Ok(EnrichmentDef {
        ident: input.ident.clone(),
        generics: input.generics.clone(),
        fields: field_defs,
    })
}

/// Parses a single field into `EnrichmentFieldDef`.
fn parse_enrichment_field_def(field: &Field) -> Result<EnrichmentFieldDef> {
    let ident = field.ident.clone().expect("named fields should have identifiers");

    let mut shared = SharedFieldAttrs::default();

    for attr in &field.attrs {
        shared.try_parse(attr)?;
    }

    if shared.if_none.is_some() && option_inner_type(&field.ty).is_none() {
        return Err(Error::new_spanned(field, "`#[if_none(...)]` is only valid on `Option<T>` fields"));
    }

    Ok(EnrichmentFieldDef {
        ident,
        ty: field.ty.clone(),
        shared,
    })
}

// ================================================================================================
// Code Generation Phase
// ================================================================================================

/// Entry point: generates the `Enrichment` trait implementation from `DeriveInput`.
pub(crate) fn derive_enrichment(input: &DeriveInput) -> Result<TokenStream> {
    let def = parse_enrichment_def(input)?;
    Ok(generate_enrichment_impl(&def))
}

/// Generates the complete `Enrichment` trait implementation.
fn generate_enrichment_impl(def: &EnrichmentDef) -> TokenStream {
    let struct_ident = &def.ident;

    // Spell out the obligations the generated `into_entries` body relies on, so
    // a generic enrichment reports them against the caller's type rather than
    // failing inside the expansion.
    let mut generics = def.generics.clone();
    let params = crate::field_attrs::type_param_idents(&def.generics);
    let predicates: Vec<syn::WherePredicate> = def
        .fields
        .iter()
        .flat_map(|field| {
            crate::field_attrs::enrichment_field_predicates(
                &field.ty,
                field.shared.redaction.as_ref().unwrap_or(&FieldRedaction::Default),
                &params,
            )
        })
        .collect();
    crate::field_attrs::extend_where_clause(&mut generics, predicates);

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let entry_stmts: Vec<TokenStream> = def.fields.iter().map(generate_entry_stmt).collect();
    let field_count = def.fields.len();

    quote! {
        const _: () = {
            impl #impl_generics ::observed::enrichment::Enrichment for #struct_ident #ty_generics #where_clause {
                fn into_entries(self) -> ::std::vec::Vec<::observed::__private::EnrichmentEntry> {
                    let mut entries = ::std::vec::Vec::with_capacity(#field_count);
                    #(#entry_stmts)*
                    entries
                }
            }
        };
    }
}

/// Generates a statement that pushes this field's `EnrichmentEntry` onto `entries`.
///
/// `Option<T>` fields behave like in `#[event(...)]`: on `None`,
/// `#[if_none(...)]` decides whether the entry is dropped or filled with
/// a placeholder string (default `"n/a"`).
fn generate_entry_stmt(field: &EnrichmentFieldDef) -> TokenStream {
    let field_ident = &field.ident;
    // Unraw-ed so a field written as `r#type` is exported under the domain key
    // `type`; `field_ident` keeps the raw form because it addresses the field.
    let own_name = field.ident.unraw().to_string();
    let redaction = field.shared.redaction.as_ref().unwrap_or(&FieldRedaction::Default);

    // The unified `#[dimension(...)]` attribute controls both the log key /
    // exclusion and the metric-dimension opt-in. Absent it, the field is logged
    // under its own name and is not a metric dimension.
    let dimension = field.shared.dimension.as_ref();
    let log = dimension.map(|d| &d.log);
    let exclude = matches!(log, Some(LogRouting::Exclude));
    let metric_key = dimension.and_then(|d| d.metric.resolve_key(&own_name));
    let key = match log {
        Some(LogRouting::Rename(name)) => name.clone(),
        _ => own_name,
    };

    let borrowed_str = is_borrowed_str(option_inner_type(&field.ty).unwrap_or(&field.ty));

    if option_inner_type(&field.ty).is_none() {
        let entry = entry_ctor(
            &key,
            redaction,
            &quote! { self.#field_ident },
            exclude,
            metric_key.as_deref(),
            borrowed_str,
        );
        return quote! { entries.push(#entry); };
    }

    let some_entry = entry_ctor(&key, redaction, &quote! { __val }, exclude, metric_key.as_deref(), borrowed_str);

    // On `None`, `#[if_none(...)]` decides whether the entry is dropped
    // or filled with a placeholder string (default `"n/a"`).
    match field.shared.if_none.clone().unwrap_or_default() {
        IfNone::Drop => quote! {
            if let ::core::option::Option::Some(__val) = self.#field_ident {
                entries.push(#some_entry);
            }
        },
        IfNone::Fill(placeholder) => {
            let fill_entry = entry_ctor(
                &key,
                &FieldRedaction::Unredacted,
                &quote! { #placeholder },
                exclude,
                metric_key.as_deref(),
                // The placeholder is a literal, so it is stored without copying.
                false,
            );
            quote! {
                match self.#field_ident {
                    ::core::option::Option::Some(__val) => entries.push(#some_entry),
                    ::core::option::Option::None => entries.push(#fill_entry),
                }
            }
        }
    }
}

/// Builds an `EnrichmentEntry` constructor for `value` under the given redaction
/// path, appending `.exclude_from_logs()` and `.with_metric_dimension(...)` when
/// requested.
fn entry_ctor(
    key: &str,
    redaction: &FieldRedaction,
    value: &TokenStream,
    exclude: bool,
    metric_key: Option<&str>,
    borrowed_str: bool,
) -> TokenStream {
    let constructor = match redaction {
        // A borrowed `&str` cannot be stored by reference, so the macro spells
        // out the copy that `Value` has no `From<&str>` impl to hide.
        FieldRedaction::Unredacted if borrowed_str => quote! {
            ::observed::__private::EnrichmentEntry::unclassified(
                #key,
                ::observed::Value::from(::std::sync::Arc::<str>::from(#value)),
            )
        },
        // Unredacted: bypass redaction, type must impl `Into<Value>`.
        FieldRedaction::Unredacted => quote! {
            ::observed::__private::EnrichmentEntry::unclassified(#key, #value)
        },
        // `data_class`: wrap in `Sensitive` before storing.
        FieldRedaction::DataClass(data_class_expr) => quote! {
            ::observed::__private::EnrichmentEntry::new(
                #key,
                ::observed::__private::Sensitive::new(#value, #data_class_expr),
            )
        },
        // Default: type must impl `RedactedDisplay`.
        FieldRedaction::Default => quote! {
            ::observed::__private::EnrichmentEntry::new(#key, #value)
        },
    };
    let exclude_chain = exclude.then(|| quote! { .exclude_from_logs() });
    let metric_chain = metric_key.map(|mk| quote! { .with_metric_dimension(#mk) });
    quote! { #constructor #exclude_chain #metric_chain }
}
