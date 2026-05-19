// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed string builders and frozen string handles.
//!
//! [`String`] builds UTF-8 strings that can be frozen into [`RcStr`],
//! [`ArcStr`], or [`BoxStr`]. With `utf16`, the crate also exposes the
//! parallel UTF-16 types and `format_utf16!`.

mod arc_str;
mod box_str;
mod format_macro;
mod rc_str;
mod string;

#[cfg(feature = "utf16")]
mod arc_utf16_str;
#[cfg(feature = "utf16")]
mod box_utf16_str;
#[cfg(feature = "utf16")]
mod format_utf16_macro;
#[cfg(feature = "utf16")]
mod rc_utf16_str;
#[cfg(feature = "utf16")]
mod utf16_string;

pub use arc_str::ArcStr;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use arc_utf16_str::ArcUtf16Str;
pub use box_str::BoxStr;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use box_utf16_str::BoxUtf16Str;
pub use rc_str::RcStr;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use rc_utf16_str::RcUtf16Str;
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
