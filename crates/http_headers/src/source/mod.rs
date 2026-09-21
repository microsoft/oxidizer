// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Definitions involved in reading fields from field containers.
//!
//! A container implements [`FieldSource`] to expose the field lines stored
//! under a field name, and every typed decode in this crate starts there.
//! [`FieldLines`] is what a source hands back: the one-or-more raw field
//! lines for a single name, borrowed from whatever storage the container
//! uses. [`FieldLinesIter`] walks those lines, and [`DelimitedItems`] walks the
//! comma- or semicolon-separated items within them.
//!
//! The `http` cargo feature implements [`FieldSource`] for
//! `http::HeaderMap`. Implement it yourself only when integrating a different
//! container; reading a known header needs nothing from this module. Typed
//! decodes from custom sources accept at most [`MAX_CUSTOM_FIELD_BYTES`] total
//! bytes and [`MAX_CUSTOM_FIELD_LINES`] lines for one name. Delimited parsing
//! accepts at most [`MAX_CUSTOM_LIST_ITEMS`] items per decode. Exceeding a
//! budget returns [`DecodeErrorKind::SourceLimitExceeded`](crate::DecodeErrorKind::SourceLimitExceeded),
//! not an invalid-syntax error. The validated
//! `http::HeaderMap` adapter is exempt from these custom-source budgets.

mod delimited_items;
mod field_lines;
mod field_source;
mod list_item_count;

#[doc(inline)]
pub use delimited_items::DelimitedItems;
#[doc(inline)]
pub use field_lines::{FieldLines, FieldLinesIter, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES, MAX_CUSTOM_LIST_ITEMS};
#[doc(inline)]
pub use field_source::FieldSource;
pub(crate) use list_item_count::update_list_item_count;
