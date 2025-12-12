// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Module that contains primitives for parsing, formatting, and serializing [`Timestamp`][`crate::Timestamp`].
//!
//! The following types are available:
//!
//! - [`Iso8601`]: Parsing and formatting of timestamps in [ISO 8601](https://en.wikipedia.org/wiki/ISO_8601) format.
//!   For example, `2024-08-06T21:30:00Z`.
//!
//! - [`Rfc2822`]: Parsing and formatting of timestamps in [RFC 2822](https://tools.ietf.org/html/rfc2822#section-3.3) format.
//!   For example, `Tue, 6 Aug 2024 14:30:00 -0000`.
//!
//! - [`UnixSeconds`]: Parsing and formatting of timestamps that are represented as the number of whole seconds since Unix epoch.
//!   For example, `0` represents `Thu, 1 Jan 1970 00:00:00 -0000`.
//!
//! # Examples
//!
//! ```
//! use tick::fmt::{Iso8601, Rfc2822, UnixSeconds};
//!
//! // ISO 8601
//! let time: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! assert_eq!(time.to_string(), "2024-08-06T21:30:00Z");
//!
//! // RFC 2822
//! let time: Rfc2822 = "Tue, 06 Aug 2024 14:30:00 GMT".parse()?;
//! assert_eq!(time.to_string(), "Tue, 06 Aug 2024 14:30:00 GMT");
//!
//! // Unix seconds
//! let time: UnixSeconds = "951786000".parse()?;
//! assert_eq!(time.to_string(), "951786000");
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

// mod iso_8601;
// mod rfc_2822;
mod unix_seconds;
// mod utils;

// pub use iso_8601::Iso8601;
// pub use rfc_2822::Rfc2822;
pub use unix_seconds::UnixSeconds;
