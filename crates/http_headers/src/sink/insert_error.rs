// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The error reported when a field cannot be encoded or stored.

use std::error::Error;
use std::fmt;

/// An error produced when a field cannot be stored.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{UserAgent, UserAgentOwned};
/// use http_headers::sink::InsertError;
///
/// let mut map = HeaderMap::new();
/// let stored: Result<(), InsertError> =
///     UserAgent::insert(&mut map, UserAgentOwned::try_from_static("client/1")?);
/// assert!(stored.is_ok());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InsertError;

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("field could not be encoded or stored")
    }
}

impl Error for InsertError {}
