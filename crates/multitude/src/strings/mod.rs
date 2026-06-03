// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed string builders and frozen string handles.
//!
//! [`String`] builds UTF-8 strings that can be frozen into
//! [`Arc<str>`](crate::Arc) or [`Box<str>`](crate::Box) — both 8-byte
//! thin smart pointers with `Deref<Target = str>` and string-flavored
//! impls (`PartialEq<str>`, `Serialize`, `From<Arc<str>> for Arc<[u8]>`,
//! etc.). With `utf16`, the crate also exposes the parallel UTF-16
//! types ([`ArcUtf16Str`], [`BoxUtf16Str`], [`Utf16String`]) and
//! `format_utf16!`.

mod format_macro;
mod str_impls;
mod string;
mod string_common;

#[cfg(feature = "utf16")]
mod arc_utf16_str;
#[cfg(feature = "utf16")]
mod box_utf16_str;
#[cfg(feature = "utf16")]
mod format_utf16_macro;
#[cfg(feature = "utf16")]
#[macro_use]
mod utf16_str_common;
#[cfg(feature = "utf16")]
mod utf16_string;

#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use arc_utf16_str::ArcUtf16Str;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use box_utf16_str::BoxUtf16Str;
pub use string::String;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use utf16_string::Utf16String;

#[doc(inline)]
pub use crate::__multitude_format as format;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
#[doc(inline)]
pub use crate::__multitude_format_utf16 as format_utf16;
