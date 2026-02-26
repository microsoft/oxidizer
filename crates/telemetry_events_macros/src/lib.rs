// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`telemetry_events`](https://docs.rs/telemetry_events) crate.
//!
//! # Provided Derives
//!
//! * `#[derive(Event)]`: Auto-implements the `telemetry_events::Event` trait from
//!   struct-level and field-level `#[telemetry_events(...)]` attributes.

use proc_macro::TokenStream;
use syn::{Path, parse_quote};

#[proc_macro_derive(Event, attributes(telemetry_events))]
#[cfg_attr(test, mutants::skip)]
#[expect(missing_docs, reason = "Documented in the telemetry_events crate's reexport")]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::telemetry_events);
    telemetry_events_macros_impl::derive_event(input.into(), &root_path).into()
}
