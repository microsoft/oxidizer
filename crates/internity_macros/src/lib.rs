// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Derive macros for interner-aware serialization and deserialization in
//! [`internity`](https://docs.rs/internity).
//!
//! The [`DeserializeIn`](https://docs.rs/internity/latest/internity/derive.DeserializeIn.html)
//! and [`SerializeIn`](https://docs.rs/internity/latest/internity/derive.SerializeIn.html)
//! derives thread a reader or lexicon through Serde so [`Sym`] fields are
//! encoded and decoded through the interner.
//!
//! [`Sym`]: https://docs.rs/internity/latest/internity/struct.Sym.html

use proc_macro::TokenStream;
use syn::{Path, parse_quote};

/// Derive interner-aware deserialization for a struct.
///
/// The macro generates an implementation of `internity::de::DeserializeIn`,
/// threading the interner through Serde so `Sym` fields are decoded through it.
///
/// # Supported shapes
///
/// Non-generic structs only: named-field, tuple, newtype, and unit structs, plus
/// `#[serde(transparent)]` newtypes. Enums, unions, and generic types are
/// rejected with a compile error.
///
/// # Attributes
///
/// * `#[internity(crate = "path")]` on the container renames the `internity`
///   crate root (for re-exports or renamed dependencies).
/// * `#[internity(via_serde)]` on a field decodes it with its ordinary
///   [`serde::Deserialize`](https://docs.rs/serde) implementation instead of the
///   interner-aware path.
/// * Container `#[serde(...)]` attributes honored: `rename`, `rename_all`,
///   `deny_unknown_fields`, `default`, `transparent`, and `expecting`.
/// * Field `#[serde(...)]` attributes honored: `rename`, `alias`, `default`,
///   `skip`/`skip_deserializing`, and `with`/`deserialize_with`.
///
/// # Rejected attributes
///
/// `#[serde(tag/content/untagged/remote)]` are rejected on any container because
/// they change the wire shape in ways the interner-aware codegen cannot honor.
/// Because this derive controls the deserialize direction, `#[serde(from)]` and
/// `#[serde(try_from)]` are also rejected; `#[serde(into)]` is ignored (it only
/// affects serialization).
#[proc_macro_derive(DeserializeIn, attributes(internity, serde))]
#[cfg_attr(test, mutants::skip)] // `proc_macro::TokenStream` is only usable by rustc.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn derive_deserialize_in(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::internity);
    internity_macros_impl::derive_deserialize_in(input.into(), &root_path).into()
}

/// Derive interner-aware serialization for a struct.
///
/// The macro generates an implementation of `internity::se::SerializeIn`,
/// threading a reader through Serde so `Sym` fields are encoded through it.
///
/// # Supported shapes
///
/// Non-generic structs only: named-field, tuple, newtype, and unit structs, plus
/// `#[serde(transparent)]` newtypes. Enums, unions, and generic types are
/// rejected with a compile error.
///
/// # Attributes
///
/// * `#[internity(crate = "path")]` on the container renames the `internity`
///   crate root (for re-exports or renamed dependencies).
/// * `#[internity(via_serde)]` on a field encodes it with its ordinary
///   [`serde::Serialize`](https://docs.rs/serde) implementation instead of the
///   interner-aware path.
/// * Container `#[serde(...)]` attributes honored: `rename`, `rename_all`,
///   `transparent`, and their `serialize_`-prefixed forms.
/// * Field `#[serde(...)]` attributes honored: `rename`, `skip`/
///   `skip_serializing`, and `serialize_with`.
///
/// # Rejected attributes
///
/// `#[serde(tag/content/untagged/remote)]` are rejected on any container because
/// they change the wire shape in ways the interner-aware codegen cannot honor.
/// Because this derive controls the serialize direction, `#[serde(into)]` is
/// rejected; `#[serde(from)]` and `#[serde(try_from)]` are ignored (they only
/// affect deserialization). `#[serde(skip_serializing_if)]` is rejected because
/// the interner-aware encoder cannot evaluate the predicate mid-stream.
#[proc_macro_derive(SerializeIn, attributes(internity, serde))]
#[cfg_attr(test, mutants::skip)] // `proc_macro::TokenStream` is only usable by rustc.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn derive_serialize_in(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::internity);
    internity_macros_impl::derive_serialize_in(input.into(), &root_path).into()
}
