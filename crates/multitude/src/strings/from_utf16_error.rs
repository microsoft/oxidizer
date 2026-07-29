// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error type for the byte-oriented UTF-16 string constructors.

use core::fmt;

/// Error from the byte-oriented UTF-16 string constructors.
///
/// Returned by
/// [`Arena::alloc_string_from_utf16le`](crate::Arena::alloc_string_from_utf16le)
/// and friends when the input byte slice is not valid UTF-16 — either it had
/// an odd length, or it contained an unpaired surrogate. The arena-bound
/// analog of [`std::string::FromUtf16Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// ```
/// use multitude::Arena;
/// use multitude::strings::FromUtf16Error;
///
/// let arena = Arena::new();
/// let result: Result<_, FromUtf16Error> = arena.alloc_string_from_utf16le([0]);
/// assert!(result.is_err());
/// ```
pub struct FromUtf16Error(());

impl FromUtf16Error {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self(())
    }
}

impl fmt::Display for FromUtf16Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid UTF-16: odd byte length or unpaired surrogate")
    }
}

impl core::error::Error for FromUtf16Error {}
