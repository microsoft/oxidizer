// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recipes and patterns for building URIs with [`templated_uri`](crate).
//!
//! # Prefer templated paths over string formatting
//!
//! Building a path with [`format!`] and parsing the result into an
//! [`http::uri::PathAndQuery`] works, but it is **less safe** and
//! **slower** than describing the path with a [`templated`] struct.
//!
//! ```
//! use templated_uri::{Uri, UriError, templated};
//!
//! #[templated(template = "/users/{user_id}/profile", unredacted)]
//! struct UserProfilePath {
//!     user_id: u32,
//! }
//!
//! // A handler that doesn't care how the URI was built, only that it parses.
//! fn consume_uri(uri: impl TryInto<Uri, Error: Into<UriError>>) -> Result<Uri, UriError> {
//!     uri.try_into().map_err(Into::into)
//! }
//!
//! let user_id: u32 = 42;
//!
//! // `format!` allocates a `String`, then `Uri::try_from` validates and
//! // copies it again, two allocations and two passes per call.
//! let from_format = consume_uri(format!("/users/{user_id}/profile"))?;
//!
//! // The templated struct converts straight into a `Uri` without
//! // intermediate parsing.
//! let from_template = consume_uri(UserProfilePath { user_id })?;
//!
//! assert_eq!(
//!     from_format.to_string().declassify_ref(),
//!     from_template.to_string().declassify_ref(),
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Because [`Uri`] implements `TryFrom<&str>` / `TryFrom<String>` *and*
//! `From<T> for Uri where T: PathAndQueryTemplate`, the same `consume_uri`
//! signature accepts plain strings and templated structs interchangeably.
//!
//! The templated version also wins on **safety** and **observability**:
//!
//! - **Type safety.** Only types implementing [`Escape`] (such as [`u32`],
//!   [`Uuid`](uuid::Uuid), or [`EscapedString`]) can be used as variables, so
//!   reserved characters cannot leak into the rendered URI. With `format!`,
//!   anything that implements [`Display`](std::fmt::Display) is accepted and
//!   the caller has to remember to escape.
//! - **Compile-time checks.** Field names must match the placeholders in the
//!   template literal; mistakes are caught by `cargo check`, not at runtime.
//! - **Telemetry.** The original template (`/users/{user_id}/profile`) is
//!   exposed through [`PathAndQueryTemplate::template`] so logs, traces, and
//!   metrics can group requests by route instead of by unique URL.
//! - **Compliance.** Variables can be wrapped in [`Sensitive`] (or any
//!   [`DataClass`]) and the rendered [`PathAndQuery`] integrates with
//!   [`data_privacy`] so values are redacted by default in logs and error
//!   messages. A raw `format!`-built string carries no classification, which
//!   makes it easy to leak personal or otherwise regulated data into telemetry.
//!
//! # Use domain types directly as template variables
//!
//! In real applications, identifiers usually have richer meaning than `u32` or
//! `String`. Wrap them in newtypes that derive [`Escape`] (and optionally
//! attach a [`#[classified]`](data_privacy::classified) data class), and they
//! can be dropped straight into a [`templated`] struct, no manual conversion
//! at every call site.
//!
//! ```
//! use data_privacy::{classified, taxonomy};
//! use templated_uri::{Escape, Uri, UriError, templated};
//!
//! #[taxonomy(example_taxonomy)]
//! enum ExampleTaxonomy {
//!     /// End User Pseudonymous Identifier.
//!     Eupi,
//! }
//!
//! #[classified(ExampleTaxonomy::Eupi)]
//! #[derive(Escape)]
//! struct UserId(u32);
//!
//! #[templated(template = "/users/{user_id}/profile")]
//! struct UserProfilePath {
//!     user_id: UserId,
//! }
//!
//! fn consume_uri(uri: impl TryInto<Uri, Error: Into<UriError>>) -> Result<Uri, UriError> {
//!     uri.try_into().map_err(Into::into)
//! }
//!
//! let _uri = consume_uri(UserProfilePath { user_id: UserId(42) })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The newtype owns its own validation rules and data classification, so the
//! template definition stays declarative and the compiler refuses to mix up,
//! say, a `UserId` with another numeric id. The classification carried by
//! `UserId` flows through into the rendered URI, so logging the path through
//! [`RedactedDisplay`] redacts the right components without any per-call-site
//! ceremony.
//!
//! See [`examples/classified_templating.rs`](https://github.com/microsoft/oxidizer/blob/main/crates/templated_uri/examples/classified_templating.rs)
//! for an end-to-end version that wires up a [`RedactionEngine`].

#[expect(unused_imports, reason = "simplifies the docs")]
use crate::*;
#[expect(unused_imports, reason = "simplifies the docs")]
use ::data_privacy::{DataClass, RedactedDisplay, RedactionEngine, Sensitive};
#[expect(unused_imports, reason = "simplifies the docs")]
use ::http::*;
