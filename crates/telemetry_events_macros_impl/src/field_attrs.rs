// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream as TokenStream2;
use syn::{Attribute, Ident, LitStr, Token, Type, TypePath, parenthesized};

/// Parsed field-level `#[telemetry_events(...)]` attributes.
#[derive(Debug, Default)]
pub struct FieldAttrs {
    /// Include this field in all logs.
    pub include_in_logs: bool,
    /// Include this field in specific logs (by name).
    pub include_in_log: Vec<String>,
    /// Include this field as a dimension in all metrics.
    pub include_in_metrics: bool,
    /// Include this field as a dimension in specific metrics (by name).
    pub include_in_metric: Vec<String>,
    /// This field defines a metric value.
    pub metric: Option<MetricAttr>,
}

/// A `metric(name = "...", kind = InstrumentKind::X)` field attribute.
#[derive(Debug)]
pub struct MetricAttr {
    pub name: String,
    pub kind: TokenStream2,
}

pub fn parse_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut result = FieldAttrs::default();

    for attr in attrs {
        if !attr.path().is_ident("telemetry_events") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("include_in_logs") {
                result.include_in_logs = true;
                return Ok(());
            }

            if meta.path.is_ident("include_in_log") {
                let content;
                parenthesized!(content in meta.input);
                let lit: LitStr = content.parse()?;
                result.include_in_log.push(lit.value());
                return Ok(());
            }

            if meta.path.is_ident("include_in_metrics") {
                result.include_in_metrics = true;
                return Ok(());
            }

            if meta.path.is_ident("include_in_metric") {
                let content;
                parenthesized!(content in meta.input);
                let lit: LitStr = content.parse()?;
                result.include_in_metric.push(lit.value());
                return Ok(());
            }

            if meta.path.is_ident("metric") {
                let content;
                parenthesized!(content in meta.input);
                let metric_attr = parse_metric_content(&content)?;
                result.metric = Some(metric_attr);
                return Ok(());
            }

            Err(meta.error("unknown telemetry_events field attribute"))
        })?;
    }

    Ok(result)
}

fn parse_metric_content(input: syn::parse::ParseStream<'_>) -> syn::Result<MetricAttr> {
    let mut name: Option<String> = None;
    let mut kind: Option<TokenStream2> = None;

    while !input.is_empty() {
        let ident: Ident = input.parse()?;
        input.parse::<Token![=]>()?;

        if ident == "name" {
            let lit: LitStr = input.parse()?;
            name = Some(lit.value());
        } else if ident == "kind" {
            // Parse as a path expression (e.g. InstrumentKind::Histogram)
            let path: syn::Path = input.parse()?;
            kind = Some(quote::quote! { #path });
        } else {
            return Err(syn::Error::new(ident.span(), "unknown metric attribute"));
        }

        if !input.is_empty() {
            input.parse::<Token![,]>()?;
        }
    }

    let name = name.ok_or_else(|| syn::Error::new(input.span(), "missing `name` in metric(...)"))?;
    let kind = kind.ok_or_else(|| syn::Error::new(input.span(), "missing `kind` in metric(...)"))?;

    Ok(MetricAttr { name, kind })
}

/// Returns true if the type is one of the predefined types for which `From` is implemented
/// for `TelemetrySafeValue` (i.e., should use `.into()` instead of `from_redacted`).
pub fn is_into_type(ty: &Type) -> bool {
    let Type::Path(TypePath { path, .. }) = ty else {
        return false;
    };
    let Some(last) = path.segments.last() else {
        return false;
    };
    matches!(last.ident.to_string().as_str(), "i64" | "f64" | "Duration")
}
