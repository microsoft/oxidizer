// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Implementation of the `internity` interner-aware serialization and
//! deserialization derives.

use proc_macro2::TokenStream as TokenStream2;
use syn::{DeriveInput, Error, Path};

mod attrs;
mod deserialize;
mod hygiene;
mod roots;
mod serialize;
mod shared;

#[cfg(test)]
use syn::parse_quote;

#[cfg(test)]
use crate::attrs::{ContainerAttrs, DefaultValue, FieldAttrs, parse_container, parse_field};
use crate::deserialize::expand_deserialize;
#[cfg(test)]
use crate::roots::{append_module, resolve_de_root};
use crate::serialize::expand_serialize;
#[cfg(test)]
use crate::shared::{field_seed, missing_value_expr, validate_transparent_container, with_seed_def};

/// Generates an implementation of `DeserializeIn` using `root_path` as the
/// `internity` crate root.
#[must_use]
pub fn derive_deserialize_in(input: TokenStream2, root_path: &Path) -> TokenStream2 {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| expand_deserialize(&input, root_path))
        .unwrap_or_else(Error::into_compile_error)
}

/// Generates an implementation of `SerializeIn` using `root_path` as the
/// `internity` crate root.
#[must_use]
pub fn derive_serialize_in(input: TokenStream2, root_path: &Path) -> TokenStream2 {
    syn::parse2::<DeriveInput>(input)
        .and_then(|input| expand_serialize(&input, root_path))
        .unwrap_or_else(Error::into_compile_error)
}

/// The default path to the `internity` crate root used by tests.
#[cfg(test)]
fn default_root() -> Path {
    parse_quote!(::internity)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests;
