// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed UTF-8 and UTF-16 string builders.
//!
//! [`String`] stores valid UTF-8 and mirrors the common mutation operations of
//! [`std::string::String`]. Create one through
//! [`Arena::alloc_string`](crate::Arena::alloc_string),
//! [`Arena::alloc_string_with_capacity`](crate::Arena::alloc_string_with_capacity),
//! or [`format!`]. The optional
//! `utf16` feature adds [`Utf16String`], [`Utf16Str`], and [`format_utf16!`] for
//! native UTF-16 construction without an intermediate UTF-8 allocation.
//!
//! Both builders borrow their arena and grow using arena-backed vectors.
//! Operations preserve their encoding invariant; APIs that accept indices
//! require UTF-8 byte boundaries or UTF-16 code-unit boundaries as documented.
//! Invalid boundaries panic, while fallible growth methods return
//! [`crate::AllocError`]. Freezing a builder into an arena smart pointer can
//! reuse its storage without copying.
//!
//! # UTF-8 example
//!
//! ```
//! use core::fmt::Write as _;
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//! let mut message = multitude::strings::format!(in &arena, "request {}", 41);
//! write!(message, " -> {}", 42)?;
//! assert_eq!(message.as_str(), "request 41 -> 42");
//! # Ok::<(), core::fmt::Error>(())
//! ```
//!
//! # UTF-16 example
//!
//! ```
//! # #[cfg(feature = "utf16")] {
//! use multitude::Arena;
//! use widestring::utf16str;
//!
//! let arena = Arena::new();
//! let mut message = arena.alloc_utf16_string();
//! message.push_str(utf16str!("hello"));
//! message.push(' ');
//! message.push_str(utf16str!("world"));
//! assert_eq!(message.as_utf16_str(), utf16str!("hello world"));
//! # }
//! ```

mod format_macro;
mod from_utf16_error;
mod str_impls;
mod string;
mod string_common;

#[cfg(feature = "utf16")]
mod format_utf16_macro;
#[cfg(feature = "utf16")]
mod utf16_str;
#[cfg(feature = "utf16")]
mod utf16_str_impls;
#[cfg(feature = "utf16")]
mod utf16_string;

pub use from_utf16_error::FromUtf16Error;
pub use string::{Drain, String};
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use utf16_str::Utf16Str;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use utf16_string::{Utf16Drain, Utf16String};

#[doc(inline)]
pub use crate::__multitude_format as format;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
#[doc(inline)]
pub use crate::__multitude_format_utf16 as format_utf16;
