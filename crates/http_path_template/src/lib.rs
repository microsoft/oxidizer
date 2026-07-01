// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

//! A parser for the [`google.api.http`] path-template grammar.
//!
//! A path template is the pattern that appears in a `google.api.http`
//! annotation, for example `shelves/{shelf}/books/{book=**}:archive`. This crate
//! turns such a string into a validated, structured [`PathTemplate`] — an
//! abstract syntax tree of [`Segment`]s (literals, `*`, `**`, and
//! `{field.path=sub-template}` [`Variable`] bindings) plus an optional custom
//! `:verb`.
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
//! This crate is purely a *parser*: it validates a template and exposes its
//! structure ([`PathTemplate::segments`] / [`PathTemplate::verb`]). It performs
//! no request matching and pulls in no dependencies. Build-time code generators
//! (such as `rest_over_grpc_build`) consume the parsed structure to emit a static
//! router.
//!
//! # Examples
//!
//! ```
//! use http_path_template::{PathTemplate, Segment};
//!
//! # fn main() -> Result<(), http_path_template::ParseError> {
//! let template = PathTemplate::parse("/shelves/{shelf}/books/{book=**}:archive")?;
//! assert_eq!(template.verb(), Some("archive"));
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
mod segment;
mod template;
mod variable;

#[doc(inline)]
pub use error::{ParseError, ParseErrorKind};
#[doc(inline)]
pub use segment::Segment;
#[doc(inline)]
pub use template::PathTemplate;
#[doc(inline)]
pub use variable::Variable;
