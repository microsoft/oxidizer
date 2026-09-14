// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Derive macros for arena-aware deserialization in
//! [`multitude`](https://docs.rs/multitude).
//!
//! The `DeserializeIn` derive accepts arena-specific configuration through
//! `#[multitude(...)]` and Serde configuration through `#[serde(...)]`.
//!
//! Macro invocation belongs in a crate that depends on the `multitude` facade,
//! because expansion resolves that facade crate. See `multitude` for an
//! example.
//!
//! ```
//! use multitude_macros::DeserializeIn;
//! ```

use proc_macro::TokenStream;
use syn::{Path, parse_quote};

/// Derives arena-aware deserialization through the `multitude` facade.
///
/// Invalid derive input or unsupported helper attributes are emitted as
/// `compile_error!` tokens.
#[proc_macro_derive(DeserializeIn, attributes(serde, multitude))]
#[cfg_attr(test, mutants::skip)]
pub fn derive_deserialize_in(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::multitude::de);
    multitude_macros_impl::derive_deserialize_in(input.into(), &root_path).into()
}
