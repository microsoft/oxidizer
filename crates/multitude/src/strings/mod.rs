// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed string builders and frozen string handles.
//!
//! [`String`] builds UTF-8 strings in an arena, while the optional
//! [`Utf16String`] type supports UTF-16.

mod format_macro;
mod from_utf16_error;
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
pub use from_utf16_error::FromUtf16Error;
pub use string::{Drain, String};
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
pub use utf16_string::{Utf16Drain, Utf16String};

#[doc(inline)]
pub use crate::__multitude_format as format;
#[cfg(feature = "utf16")]
#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
#[doc(inline)]
pub use crate::__multitude_format_utf16 as format_utf16;
