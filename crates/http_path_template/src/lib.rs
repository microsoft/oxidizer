// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_path_template/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_path_template/favicon.ico"
)]

//! A parser for the [`google.api.http`] path-template grammar.
//!
//! A path template is the pattern that appears in a `google.api.http`
//! annotation, for example `/shelves/{shelf}/books/{book=**}:archive`. This crate
//! turns such a string into a validated, structured [`PathTemplate`] — an
//! abstract syntax tree of [`Segment`]s (literals, `*`, `**`, and
//! `{field.path=sub-template}` [`Variable`] bindings) plus an optional custom
//! `:verb`.
//!
//! A template must begin with `/` and is a `/`-separated sequence of segments —
//! each `/` delimits one segment. Literal segments, variable field names, and the
//! custom `:verb` are stored and compared **verbatim**, so the grammar is
//! case-sensitive; the parser performs no case folding.
//!
//! The grammar mirrors the reference [`google.api.HttpRule`] path syntax:
//!
//! - a **literal** segment (`shelves`) must match verbatim;
//! - **`*`** ([`Segment::Single`]) matches exactly one non-empty segment;
//! - **`**`** ([`Segment::Rest`]) matches the remaining segments and may only
//!   appear as the final element;
//! - **`{field.path=sub-template}`** ([`Segment::Variable`]) captures the portion
//!   of the path matched by its sub-template into a dotted message field; the
//!   shorthand `{field}` is `{field=*}` and nested variables are rejected;
//! - a trailing **`:verb`** declares a custom method verb.
//!
//! # Extended grammar
//!
//! [`PathTemplate::parse`] takes a [`Grammar`] argument. The default grammar is
//! the strict `google.api.http` syntax above; passing a [`Grammar`] with
//! [`Grammar::with_segment_affixes`] enabled additionally allows **intra-segment
//! prefix/suffix parameters**: a single segment may wrap one `{field.path}`
//! variable in literal text, for example `/files/{name}.json`, `/v{version}/x`,
//! or `/img-{id}.png`. Such a segment parses to a [`Segment::Affix`]. The strict
//! grammar rejects this syntax.
//!
//! # Examples
//!
//! Parsing `/shelves/{shelf}/books/{book=**}:archive` yields four top-level
//! [`Segment`]s plus the custom verb `archive`:
//!
//! - `shelves` — a [`Segment::Literal`];
//! - `{shelf}` — a [`Segment::Variable`] binding field `shelf` to a single
//!   segment (`*`, i.e. [`Segment::Single`]);
//! - `books` — a [`Segment::Literal`];
//! - `{book=**}` — a [`Segment::Variable`] binding field `book` to the remaining
//!   segments (`**`, i.e. [`Segment::Rest`]).
//!
//! ```
//! use http_path_template::{Grammar, PathTemplate, Segment};
//!
//! # fn main() -> Result<(), http_path_template::ParseError> {
//! let template =
//!     PathTemplate::parse("/shelves/{shelf}/books/{book=**}:archive", Grammar::default())?;
//!
//! assert_eq!(template.segments().len(), 4);
//! assert_eq!(template.verb(), Some("archive"));
//!
//! assert_eq!(template.segments()[0], Segment::Literal("shelves".to_owned()));
//! assert_eq!(template.segments()[2], Segment::Literal("books".to_owned()));
//!
//! let Segment::Variable(shelf) = &template.segments()[1] else {
//!     panic!("expected variable")
//! };
//! assert_eq!(shelf.field_path(), &[String::from("shelf")]);
//! assert_eq!(shelf.segments(), &[Segment::Single]);
//!
//! let Segment::Variable(book) = &template.segments()[3] else {
//!     panic!("expected variable")
//! };
//! assert_eq!(book.field_path(), &[String::from("book")]);
//! assert_eq!(book.segments(), &[Segment::Rest]);
//! # Ok(())
//! # }
//! ```
//!
//! [`google.api.http`]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto
//! [`google.api.HttpRule`]: https://github.com/googleapis/googleapis/blob/master/google/api/http.proto

mod error;
mod grammar;
mod path_template;
mod segment;
mod variable;

#[doc(inline)]
pub use error::ParseError;
#[doc(inline)]
pub use grammar::Grammar;
#[doc(inline)]
pub use path_template::PathTemplate;
#[doc(inline)]
pub use segment::Segment;
#[doc(inline)]
pub use variable::Variable;
