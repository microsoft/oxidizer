// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Derive macros for arena-aware deserialization in
//! [`multitude`](https://docs.rs/multitude).
//!
//! The `DeserializeIn` derive accepts arena-specific configuration through
//! `#[multitude(...)]` and Serde configuration through `#[serde(...)]`.

use proc_macro::TokenStream;
use syn::{Path, parse_quote};

#[proc_macro_derive(DeserializeIn, attributes(serde, multitude))]
#[cfg_attr(test, mutants::skip)]
#[expect(missing_docs, reason = "Documented in the multitude crate's reexport")]
pub fn derive_deserialize_in(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::multitude::de);
    multitude_macros_impl::derive_deserialize_in(input.into(), &root_path).into()
}
