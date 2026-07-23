// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed parsing and production of URL query strings.
//!
//! Derive [`FromQuery`] for inbound query parameters and [`ToQuery`] for
//! canonical production. Parsing follows `application/x-www-form-urlencoded`
//! rules: `%XX` escapes are decoded and `+` represents a space.
//!
//! Fields may be scalar values, [`Option`] values, or repeated
//! [`Vec`](alloc::vec::Vec) values.
//! Values other than strings are decoded through [`core::str::FromStr`] and
//! encoded through [`core::fmt::Display`]. The `query` derive attributes
//! support renaming, aliases, defaults, flattened query types, skipped fields,
//! and rejecting unknown parameters. Names claimed by more than one direct or
//! flattened schema are rejected as ambiguous.
//! Common compatible
//! `serde` field and container attributes are also accepted.
//!
//! Parsing and production apply [`QueryLimits`] so untrusted inputs cannot
//! force unbounded work or output. Use [`FromQuery::from_query_with`] or
//! [`ToQuery::to_query_string_with`] or [`ToQuery::write_query_with`] to supply
//! application-specific limits.
//!
//! The additive `form` feature reuses [`FromQuery`] for bounded
//! `routerama::route::form::Form` bodies. URI query extraction may borrow from
//! its request URI, while form extraction deliberately requires a schema that
//! can decode for every input lifetime so references into its temporary body
//! buffer cannot escape.
//!
//! # Derive helper attributes
//!
//! `#[derive(FromQuery)]` and `#[derive(ToQuery)]` register `#[query(...)]` as a
//! derive helper attribute. It is not a standalone attribute macro, so rustdoc
//! documents its options on the [`FromQuery`](macro@FromQuery) and
//! [`ToQuery`](macro@ToQuery) derive pages rather than creating a separate
//! `query` attribute page.
//!
//! Container attributes control field renaming with `rename_all` and decoding
//! of unknown parameters with `deny_unknown_fields`. Field attributes provide
//! `rename`, repeatable `alias`, `default`, `flatten`, and `skip`. A
//! [`Vec`](alloc::vec::Vec) field always represents a repeated parameter. See
//! the derive pages for complete semantics and restrictions.
//!
//! # Examples
//!
//! ```
//! use routerama::query::{FromQuery, ToQuery};
//!
//! #[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
//! #[query(deny_unknown_fields)]
//! struct SearchQuery {
//!     q: String,
//!     page: Option<usize>,
//!     #[query(rename = "tag")]
//!     tags: Vec<String>,
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let parsed = SearchQuery::from_query("q=rust+language&page=2&tag=fast&tag=safe")?;
//! assert_eq!(parsed.q, "rust language");
//! assert_eq!(parsed.tags, ["fast", "safe"]);
//!
//! let encoded = parsed.to_query_string()?;
//! assert_eq!(encoded, "q=rust+language&page=2&tag=fast&tag=safe");
//! # Ok(())
//! # }
//! ```

mod decode_fields;
mod decoded;
mod encode_fields;
mod encoder;
mod error;
mod error_kind;
mod from_query;
mod parser;
mod query_decoder;
mod query_limits;
mod scan;
mod to_query;

use decode_fields::DecodeFields;
use decoded::Decoded;
use encode_fields::EncodeFields;
use encoder::Encoder;
pub use error::Error;
pub use error_kind::ErrorKind;
pub use from_query::FromQuery;
use query_decoder::QueryDecoder;
pub use query_limits::QueryLimits;
/// Derives direct query-string decoding for a named-field struct.
///
/// # Example
///
/// ```
/// use routerama::query::FromQuery;
///
/// #[derive(Debug, PartialEq, FromQuery)]
/// #[query(rename_all = "camelCase", deny_unknown_fields)]
/// struct Search<'q> {
///     search_term: &'q str,
///     #[query(alias = "limit")]
///     max_results: Option<u32>,
///     #[query(rename = "tag")]
///     tags: Vec<String>,
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let value = Search::from_query("searchTerm=rust&limit=10&tag=fast&tag=safe")?;
/// assert_eq!(value.search_term, "rust");
/// assert_eq!(value.max_results, Some(10));
/// assert_eq!(value.tags, ["fast", "safe"]);
/// # Ok(())
/// # }
/// ```
///
/// The obsolete `repeated` marker is rejected because [`Vec`] alone expresses
/// repeated parameters:
///
/// ```compile_fail
/// #[derive(routerama::query::FromQuery)]
/// struct Unsupported {
///     #[query(repeated)]
///     values: Vec<String>,
/// }
/// ```
///
/// Borrowing through distinct query lifetimes is rejected:
///
/// ```compile_fail
/// #[derive(routerama::query::FromQuery)]
/// struct Unsupported<'a, 'b> {
///     first: &'a str,
///     second: &'b str,
/// }
/// ```
///
/// [`Vec`]: alloc::vec::Vec
///
/// # Reference
pub use routerama_macros::FromQuery;
/// Derives direct query-string encoding for a named-field struct.
///
/// # Example
///
/// ```
/// use routerama::query::ToQuery;
///
/// #[derive(ToQuery)]
/// #[query(rename_all = "camelCase")]
/// struct Search<'q> {
///     search_term: &'q str,
///     #[query(rename = "tag")]
///     tags: Vec<&'q str>,
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let value = Search {
///     search_term: "rust language",
///     tags: vec!["fast", "safe"],
/// };
/// assert_eq!(
///     value.to_query_string()?,
///     "searchTerm=rust+language&tag=fast&tag=safe"
/// );
/// # Ok(())
/// # }
/// ```
///
/// # Reference
pub use routerama_macros::ToQuery;
pub use to_query::ToQuery;

/// Runtime support referenced by generated query codecs.
#[doc(hidden)]
pub mod __private {
    /// Repeated query-field storage used by generated decoders.
    pub type Repeated<T> = alloc::vec::Vec<T>;

    pub use super::decode_fields::DecodeFields;
    pub use super::decoded::{Decoded, parse_borrowed, parse_cow, parse_owned, parse_value};
    pub use super::encode_fields::EncodeFields;
    pub use super::encoder::Encoder;
    pub use super::error::Error;
    pub use super::query_decoder::QueryDecoder;
    pub use super::query_limits::QueryLimits;
}
