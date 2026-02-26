// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`telemetry_events`](https://docs.rs/telemetry_events) crate.

mod codegen;
mod container_attrs;
mod field_attrs;

use proc_macro2::TokenStream as TokenStream2;
use syn::{DeriveInput, Path};

/// Core implementation used by `telemetry_events_macros`.
///
/// This crate is a normal library crate (not `proc-macro`), so we operate purely
/// on `proc_macro2::TokenStream` and let the wrapper perform the conversion.
#[must_use]
pub fn derive_event(input: TokenStream2, root_path: &Path) -> TokenStream2 {
    let parsed: syn::Result<DeriveInput> = syn::parse2(input);
    parsed
        .and_then(|di| codegen::generate(&di, root_path))
        .unwrap_or_else(|e| e.to_compile_error())
}
