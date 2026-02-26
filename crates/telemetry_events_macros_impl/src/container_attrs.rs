// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::parse::ParseStream;
use syn::{Attribute, Ident, LitInt, LitStr, Token, parenthesized};

/// Parsed container-level `#[telemetry_events(...)]` attributes.
#[derive(Debug)]
pub struct ContainerAttrs {
    pub id: u64,
    pub name: String,
    pub logs: Vec<LogAttr>,
}

/// A single `log(name = "...", message = "...")` entry.
#[derive(Debug)]
pub struct LogAttr {
    pub name: String,
    pub message: String,
}

pub fn parse_container_attrs(attrs: &[Attribute]) -> syn::Result<ContainerAttrs> {
    let mut id: Option<u64> = None;
    let mut name: Option<String> = None;
    let mut logs: Vec<LogAttr> = Vec::new();

    for attr in attrs {
        if !attr.path().is_ident("telemetry_events") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("id") {
                meta.input.parse::<Token![=]>()?;
                let lit: LitInt = meta.input.parse()?;
                id = Some(lit.base10_parse()?);
                return Ok(());
            }

            if meta.path.is_ident("name") {
                meta.input.parse::<Token![=]>()?;
                let lit: LitStr = meta.input.parse()?;
                name = Some(lit.value());
                return Ok(());
            }

            if meta.path.is_ident("log") {
                let content;
                parenthesized!(content in meta.input);
                let log_attr = parse_log_content(&content)?;
                logs.push(log_attr);
                return Ok(());
            }

            Err(meta.error("unknown telemetry_events attribute"))
        })?;
    }

    let id = id.ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), "missing `id` in #[telemetry_events(...)]"))?;
    let name =
        name.ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), "missing `name` in #[telemetry_events(...)]"))?;

    Ok(ContainerAttrs { id, name, logs })
}

fn parse_log_content(input: ParseStream<'_>) -> syn::Result<LogAttr> {
    let mut name: Option<String> = None;
    let mut message: Option<String> = None;

    while !input.is_empty() {
        let ident: Ident = input.parse()?;
        input.parse::<Token![=]>()?;

        if ident == "name" {
            let lit: LitStr = input.parse()?;
            name = Some(lit.value());
        } else if ident == "message" {
            let lit: LitStr = input.parse()?;
            message = Some(lit.value());
        } else {
            return Err(syn::Error::new(ident.span(), "unknown log attribute"));
        }

        if !input.is_empty() {
            input.parse::<Token![,]>()?;
        }
    }

    let name = name.ok_or_else(|| syn::Error::new(input.span(), "missing `name` in log(...)"))?;
    let message = message.ok_or_else(|| syn::Error::new(input.span(), "missing `message` in log(...)"))?;

    Ok(LogAttr { name, message })
}
