// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// An error signaled by general-purpose extension logic.
///
/// This is used for helper logic that is not related to a specific implementation of [`Read`] or [`Write`].
/// It may either represent a logical error (e.g. unexpected end of stream) or be a wrapper for an inner error
/// that came from underlying implementation-specific logic.
///
/// [`Read`]: crate::Read
/// [`Write`]: crate::Write
#[ohno::error]
pub struct Error {}

/// A `Result` that may contain an [`Error`] from this crate.
pub type Result<T> = std::result::Result<T, Error>;
