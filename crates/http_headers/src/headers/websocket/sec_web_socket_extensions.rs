// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::Range;
use std::{fmt, str};

use super::super::shared::FieldLinesIter;
use super::super::{ExtensionValue, invalid_syntax, trim_ows, value_from_bytes};
use super::shared::{CommaItems, validate_header_value_list, validate_list};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Defines the `Sec-WebSocket-Extensions` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.2](https://www.rfc-editor.org/rfc/rfc6455#section-11.3.2).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{SecWebSocketExtensions, SecWebSocketExtensionsOwned};
///
/// let mut map = HeaderMap::new();
/// SecWebSocketExtensions::insert(
///     &mut map,
///     SecWebSocketExtensionsOwned::try_from("permessage-deflate")?,
/// )?;
/// assert!(SecWebSocketExtensions::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct SecWebSocketExtensions {
    _private: (),
}

/// Owned value for the `Sec-WebSocket-Extensions` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.2].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::SecWebSocketExtensionsOwned::try_from("permessage-deflate")?;
/// assert_eq!(value.extensions().count(), 1);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Sec-WebSocket-Extensions: permessage-deflate` offers one extension.
/// `Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits=15`
/// also supplies an extension parameter.
///
/// [RFC 6455 section 11.3.2]: https://www.rfc-editor.org/rfc/rfc6455#section-11.3.2
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SecWebSocketExtensionsOwned {
    values: FieldLinesIter,
}

/// Borrowed value for the `Sec-WebSocket-Extensions` header.
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{
///     SecWebSocketExtensions, SecWebSocketExtensionsOwned, SecWebSocketExtensionsView,
/// };
///
/// let mut map = HeaderMap::new();
/// let value = SecWebSocketExtensionsOwned::try_from("permessage-deflate")?;
/// SecWebSocketExtensions::insert(&mut map, value)?;
/// let view: SecWebSocketExtensionsView<'_> =
///     SecWebSocketExtensions::view(&map)?.expect("header is present");
/// let extension = view
///     .extensions()
///     .next()
///     .transpose()?
///     .expect("one extension");
/// assert_eq!(extension.name(), "permessage-deflate");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct SecWebSocketExtensionsView<'a> {
    values: FieldLines<'a>,
}

/// Builder for one canonical `Sec-WebSocket-Extensions` field value.
#[derive(Clone, Debug, Default)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{SecWebSocketExtensionsBuilder, SecWebSocketExtensionsOwned};
///
/// let builder: SecWebSocketExtensionsBuilder = SecWebSocketExtensionsOwned::builder();
/// let value = builder.extension("permessage-deflate").build()?;
/// assert_eq!(value.extensions().count(), 1);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct SecWebSocketExtensionsBuilder {
    wire: Vec<u8>,
    extension_count: usize,
    pending_error: Option<DecodeErrorKind>,
}

/// One borrowed WebSocket extension.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{SecWebSocketExtensionsOwned, WebSocketExtensionView};
///
/// let value =
///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; client_max_window_bits")?;
/// let extension: WebSocketExtensionView<'_> = value
///     .extensions()
///     .next()
///     .transpose()?
///     .expect("one extension");
/// assert_eq!(extension.name(), "permessage-deflate");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct WebSocketExtensionView<'a> {
    raw: &'a [u8],
    name: &'a str,
    parameter_start: usize,
}

/// One borrowed WebSocket extension parameter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{SecWebSocketExtensionsOwned, WebSocketExtensionParameterView};
///
/// let value =
///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; client_max_window_bits")?;
/// let extension = value
///     .extensions()
///     .next()
///     .transpose()?
///     .expect("one extension");
/// let parameter: WebSocketExtensionParameterView<'_> = extension
///     .parameters()
///     .next()
///     .transpose()?
///     .expect("one parameter");
/// assert_eq!(parameter.name(), "client_max_window_bits");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct WebSocketExtensionParameterView<'a> {
    raw: &'a [u8],
    name: &'a str,
    value: Option<&'a [u8]>,
    quoted: bool,
}

/// Iterator over the parameters of one WebSocket extension.
#[derive(Debug)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{SecWebSocketExtensionsOwned, WebSocketExtensionParameters};
///
/// let value = SecWebSocketExtensionsOwned::try_from(
///     "permessage-deflate; client_max_window_bits; server_max_window_bits=15",
/// )?;
/// let extension = value
///     .extensions()
///     .next()
///     .transpose()?
///     .expect("one extension");
/// let mut parameters: WebSocketExtensionParameters<'_> = extension.parameters();
/// assert_eq!(
///     parameters.next().transpose()?.expect("first").name(),
///     "client_max_window_bits"
/// );
/// assert_eq!(
///     parameters
///         .next()
///         .transpose()?
///         .expect("second")
///         .value_str()?,
///     Some("15")
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct WebSocketExtensionParameters<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl fmt::Debug for SecWebSocketExtensionsOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecWebSocketExtensionsOwned")
            .field("value_count", &self.values.len())
            .finish()
    }
}

impl fmt::Display for SecWebSocketExtensionsOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        super::super::shared::fmt_ascii_values(self.values.iter().map(FieldValue::as_field_value_ref), f)
    }
}

impl fmt::Debug for SecWebSocketExtensionsView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecWebSocketExtensionsView")
            .field("value_count", &self.values.len())
            .finish()
    }
}

impl SecWebSocketExtensionsOwned {
    #[cfg(feature = "serde")]
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.iter().map(FieldValue::as_field_value_ref)
    }

    /// Creates an extension builder.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::builder()
    ///     .extension("permessage-deflate")
    ///     .build()?;
    /// assert_eq!(value.extensions().count(), 1);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn builder() -> SecWebSocketExtensionsBuilder {
        SecWebSocketExtensionsBuilder {
            wire: Vec::new(),
            extension_count: 0,
            pending_error: None,
        }
    }

    /// Iterates extensions in wire order.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidSyntax`] when a stored
    /// extension is malformed. Iteration resumes with the following item.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::try_from(
    ///     "permessage-deflate; client_max_window_bits=15, x-test",
    /// )?;
    /// let names = value
    ///     .extensions()
    ///     .map(|extension| Ok(extension?.name()))
    ///     .collect::<Result<Vec<_>, http_headers::DecodeError>>()?;
    /// assert_eq!(names, ["permessage-deflate", "x-test"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn extensions(&self) -> impl Iterator<Item = Result<WebSocketExtensionView<'_>, DecodeError>> {
        self.values
            .iter()
            .flat_map(|value| CommaItems::new(value.as_bytes()))
            .map(parse_extension)
    }
}

impl SecWebSocketExtensionsView<'_> {
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.repeated()
    }

    /// Iterates extensions in wire order.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidSyntax`] when a stored
    /// extension is malformed. Iteration resumes with the following item.
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http::HeaderMap;
    /// use http_headers::Field;
    /// use http_headers::headers::{SecWebSocketExtensions, SecWebSocketExtensionsOwned};
    ///
    /// let mut map = HeaderMap::new();
    /// let value = SecWebSocketExtensionsOwned::try_from("permessage-deflate, x-test")?;
    /// SecWebSocketExtensions::insert(&mut map, value)?;
    /// let view = SecWebSocketExtensions::view(&map)?.expect("header is present");
    /// let names = view
    ///     .extensions()
    ///     .map(|extension| Ok(extension?.name()))
    ///     .collect::<Result<Vec<_>, http_headers::DecodeError>>()?;
    /// assert_eq!(names, ["permessage-deflate", "x-test"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn extensions(&self) -> impl Iterator<Item = Result<WebSocketExtensionView<'_>, DecodeError>> {
        self.values.comma_items().map(|item| item.and_then(parse_extension))
    }
}

impl SecWebSocketExtensionsBuilder {
    /// Starts another extension for validation by [`Self::build`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::builder()
    ///     .extension("permessage-deflate")
    ///     .extension("x-test")
    ///     .build()?;
    /// let names = value
    ///     .extensions()
    ///     .map(|extension| Ok(extension?.name()))
    ///     .collect::<Result<Vec<_>, http_headers::DecodeError>>()?;
    /// assert_eq!(names, ["permessage-deflate", "x-test"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn extension(mut self, name: impl AsRef<str>) -> Self {
        let name = name.as_ref();
        if !validate::token(name.as_bytes()) {
            self.record_error(DecodeErrorKind::InvalidToken);
        }
        if self.extension_count != 0 {
            self.wire.extend_from_slice(b", ");
        }
        self.wire.extend_from_slice(name.as_bytes());
        if let Some(count) = self.extension_count.checked_add(1) {
            self.extension_count = count;
        } else {
            self.record_error(DecodeErrorKind::InvalidNumber);
        }
        self
    }

    /// Adds a flag parameter for validation by [`Self::build`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::builder()
    ///     .extension("permessage-deflate")
    ///     .parameter_flag("client_max_window_bits")
    ///     .build()?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter = extension
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert_eq!(parameter.name(), "client_max_window_bits");
    /// assert_eq!(parameter.value(), None);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn parameter_flag(self, name: impl AsRef<str>) -> Self {
        self.parameter(name, ExtensionValue::Flag)
    }

    /// Adds a parameter for validation by [`Self::build`].
    ///
    /// A value-bearing parameter must contain an HTTP token. Validation,
    /// including whether an extension precedes the parameter, occurs in
    /// [`Self::build`].
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ExtensionValue, SecWebSocketExtensionsOwned};
    ///
    /// let value = SecWebSocketExtensionsOwned::builder()
    ///     .extension("permessage-deflate")
    ///     .parameter("server_max_window_bits", ExtensionValue::Value("15"))
    ///     .build()?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter = extension
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert_eq!(parameter.name(), "server_max_window_bits");
    /// assert_eq!(parameter.value_str()?, Some("15"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn parameter(mut self, name: impl AsRef<str>, value: ExtensionValue<'_>) -> Self {
        let name = name.as_ref();
        if self.extension_count == 0 || !validate::token(name.as_bytes()) {
            self.record_error(DecodeErrorKind::InvalidToken);
        }
        self.wire.extend_from_slice(b"; ");
        self.wire.extend_from_slice(name.as_bytes());
        match value {
            ExtensionValue::Flag => {}
            ExtensionValue::Value(value) => {
                if !validate::token(value.as_bytes()) {
                    self.record_error(DecodeErrorKind::InvalidToken);
                }
                self.wire.push(b'=');
                self.wire.extend_from_slice(value.as_bytes());
            }
        }
        self
    }

    /// Adds a token-valued parameter to the most recently added extension.
    #[must_use]
    pub fn parameter_value(self, name: &str, value: &str) -> Self {
        self.parameter(name, ExtensionValue::Value(value))
    }

    /// Adds a quoted parameter for validation by [`Self::build`].
    ///
    /// WebSocket extension quoted strings must decode to an HTTP token.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::builder()
    ///     .extension("permessage-deflate")
    ///     .quoted_parameter("mode", "fast")
    ///     .build()?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter = extension
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert_eq!(parameter.name(), "mode");
    /// assert!(parameter.is_quoted());
    /// assert_eq!(parameter.value(), Some(&b"\"fast\""[..]));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    pub fn quoted_parameter(mut self, name: &str, value: &str) -> Self {
        if self.extension_count == 0 || !validate::token(name.as_bytes()) || !validate::token(value.as_bytes()) {
            self.record_error(DecodeErrorKind::InvalidToken);
        }
        self.wire.extend_from_slice(b"; ");
        self.wire.extend_from_slice(name.as_bytes());
        self.wire.extend_from_slice(b"=\"");
        self.wire.extend_from_slice(value.as_bytes());
        self.wire.push(b'"');
        self
    }

    /// Builds a nonempty extension list.
    ///
    /// # Errors
    ///
    /// Returns an error when no extension was added, an item is malformed, a
    /// parameter has no preceding extension, or item counting overflowed.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let empty = SecWebSocketExtensionsOwned::builder().build();
    /// assert!(empty.is_err());
    ///
    /// let value = SecWebSocketExtensionsOwned::builder()
    ///     .extension("permessage-deflate")
    ///     .build()?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// assert_eq!(extension.name(), "permessage-deflate");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn build(self) -> Result<SecWebSocketExtensionsOwned, DecodeError> {
        if let Some(kind) = self.pending_error {
            return Err(DecodeError::new(&FieldName::SecWebSocketExtensions, kind));
        }
        if self.extension_count == 0 {
            return Err(invalid_syntax(&FieldName::SecWebSocketExtensions));
        }
        let value = value_from_bytes(&FieldName::SecWebSocketExtensions, self.wire)?;
        SecWebSocketExtensionsOwned::try_from(value)
    }

    fn record_error(&mut self, kind: DecodeErrorKind) {
        if self.pending_error.is_none() {
            self.pending_error = Some(kind);
        }
    }
}

impl<'a> WebSocketExtensionView<'a> {
    /// Returns the extension token.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value =
    ///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; client_max_window_bits")?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// assert_eq!(extension.name(), "permessage-deflate");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the complete extension bytes after surrounding OWS trimming.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value =
    ///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; client_max_window_bits")?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// assert_eq!(
    ///     extension.as_bytes(),
    ///     b"permessage-deflate; client_max_window_bits"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_bytes(self) -> &'a [u8] {
        self.raw
    }

    /// Iterates extension parameters.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::try_from(
    ///     "permessage-deflate; client_max_window_bits; server_max_window_bits=15",
    /// )?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter_names = extension
    ///     .parameters()
    ///     .map(|parameter| Ok(parameter?.name()))
    ///     .collect::<Result<Vec<_>, http_headers::DecodeError>>()?;
    /// assert_eq!(
    ///     parameter_names,
    ///     ["client_max_window_bits", "server_max_window_bits"]
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn parameters(self) -> WebSocketExtensionParameters<'a> {
        WebSocketExtensionParameters {
            bytes: self.raw,
            position: self.parameter_start,
        }
    }
}

impl<'a> WebSocketExtensionParameterView<'a> {
    /// Returns the parameter name.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value =
    ///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; server_max_window_bits=15")?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter = extension
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert_eq!(parameter.name(), "server_max_window_bits");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the raw token or quoted-string value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value = SecWebSocketExtensionsOwned::try_from(
    ///     "permessage-deflate; client_max_window_bits; server_max_window_bits=15",
    /// )?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let mut parameters = extension.parameters();
    /// let flag = parameters.next().transpose()?.expect("flag parameter");
    /// assert_eq!(flag.value(), None);
    /// let valued = parameters.next().transpose()?.expect("valued parameter");
    /// assert_eq!(valued.value(), Some(&b"15"[..]));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn value(self) -> Option<&'a [u8]> {
        self.value
    }

    /// Returns whether the value is a quoted string.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let token_value = SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode=fast")?;
    /// let token_parameter = token_value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension")
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert!(!token_parameter.is_quoted());
    ///
    /// let quoted_value = SecWebSocketExtensionsOwned::try_from("permessage-deflate; mode=\"fast\"")?;
    /// let quoted_parameter = quoted_value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension")
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert!(quoted_parameter.is_quoted());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn is_quoted(self) -> bool {
        self.quoted
    }

    /// Returns the value as UTF-8 without unescaping quoted strings.
    ///
    /// # Errors
    ///
    /// Returns an error when the value contains non-UTF-8 `obs-text`.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value =
    ///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; server_max_window_bits=15")?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter = extension
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert_eq!(parameter.value_str()?, Some("15"));
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn value_str(self) -> Result<Option<&'a str>, DecodeError> {
        self.value
            .map(str::from_utf8)
            .transpose()
            .map_err(|_invalid| DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidUtf8))
    }

    /// Returns the complete parameter bytes after surrounding OWS trimming.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketExtensionsOwned;
    ///
    /// let value =
    ///     SecWebSocketExtensionsOwned::try_from("permessage-deflate; client_max_window_bits")?;
    /// let extension = value
    ///     .extensions()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one extension");
    /// let parameter = extension
    ///     .parameters()
    ///     .next()
    ///     .transpose()?
    ///     .expect("one parameter");
    /// assert_eq!(parameter.as_bytes(), b"client_max_window_bits");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_bytes(self) -> &'a [u8] {
        self.raw
    }
}

impl<'a> Iterator for WebSocketExtensionParameters<'a> {
    type Item = Result<WebSocketExtensionParameterView<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        match take_extension_parameter(self.bytes, &mut self.position) {
            Ok(Some(parameter)) => Some(Ok(parameter)),
            Ok(None) => None,
            Err(error) => {
                self.position = self.bytes.len();
                Some(Err(error))
            }
        }
    }
}

impl Field for SecWebSocketExtensions {
    type View<'a> = SecWebSocketExtensionsView<'a>;
    type Owned = SecWebSocketExtensionsOwned;

    fn name() -> &'static FieldName {
        &FieldName::SecWebSocketExtensions
    }

    fn view_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        validate_extensions(&lines)?;
        Ok(Some(SecWebSocketExtensionsView { values: lines }))
    }

    fn owned_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b',', true)?;
        lines.validate_list_item_limit(b';', false)?;
        let mut copied = FieldLinesIter::empty();
        for (value_index, (value, owned)) in lines.repeated_owned()?.enumerate() {
            validate_extension_header_value(value).map_err(|error| {
                if error.kind() == DecodeErrorKind::UnterminatedQuote {
                    error.at_value(value_index)
                } else {
                    error
                }
            })?;
            copied.push(owned);
        }
        Ok(Some(SecWebSocketExtensionsOwned { values: copied }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), value.values.into_encoded())
    }
}

impl TryFrom<&str> for SecWebSocketExtensionsOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| invalid_syntax(&FieldName::SecWebSocketExtensions))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for SecWebSocketExtensionsOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| invalid_syntax(&FieldName::SecWebSocketExtensions))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for SecWebSocketExtensionsOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        validate_extension_header_value(value.as_field_value_ref())?;
        Ok(Self {
            values: FieldLinesIter::one(value),
        })
    }
}

fn validate_extensions(values: &FieldLines<'_>) -> Result<(), DecodeError> {
    values.validate_list_item_limit(b',', true)?;
    values.validate_list_item_limit(b';', false)?;
    let mut present = false;
    for value in values.repeated() {
        let Some(line_present) = validate_plain_extension_line(value.as_bytes())? else {
            return validate_list(values, &mut validate_extension);
        };
        present |= line_present;
    }
    present
        .then_some(())
        .ok_or_else(|| DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::MissingValue))
}

fn validate_extension_header_value(value: FieldValueRef<'_>) -> Result<(), DecodeError> {
    match validate_plain_extension_line(value.as_bytes())? {
        Some(true) => Ok(()),
        Some(false) => Err(invalid_syntax(&FieldName::SecWebSocketExtensions)),
        None => validate_header_value_list(value, &FieldName::SecWebSocketExtensions, validate_extension),
    }
}

/// Validates an unquoted extension list in one pass.
///
/// `None` selects the structured parser when quoted-string syntax appears.
fn validate_plain_extension_line(bytes: &[u8]) -> Result<Option<bool>, DecodeError> {
    if matches!(
        bytes,
        b"permessage-deflate"
            | b"permessage-deflate; client_max_window_bits"
            | b"permessage-deflate; server_no_context_takeover; client_no_context_takeover"
    ) {
        return Ok(Some(true));
    }
    if bytes.contains(&b'"') {
        return Ok(None);
    }
    let mut position = 0_usize;
    let mut present = false;
    loop {
        skip_ows(bytes, &mut position);
        while bytes.get(position) == Some(&b',') {
            position += 1;
            skip_ows(bytes, &mut position);
        }
        if position == bytes.len() {
            break;
        }
        if matches!(bytes.get(position), Some(b'"' | b'\\')) {
            return Ok(None);
        }
        if !take_plain_extension_token(bytes, &mut position) {
            return Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidToken));
        }
        present = true;

        loop {
            skip_ows(bytes, &mut position);
            match bytes.get(position) {
                None => return Ok(Some(present)),
                Some(b',') => {
                    position += 1;
                    break;
                }
                Some(b'"' | b'\\') => return Ok(None),
                Some(b';') => {
                    position += 1;
                    skip_ows(bytes, &mut position);
                    if matches!(bytes.get(position), Some(b'"' | b'\\')) {
                        return Ok(None);
                    }
                    if !take_plain_extension_token(bytes, &mut position) {
                        return Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidToken));
                    }
                    if !take_plain_parameter_value(bytes, &mut position)? {
                        return Ok(None);
                    }
                }
                Some(_) => return Err(invalid_syntax(&FieldName::SecWebSocketExtensions)),
            }
        }
    }
    Ok(Some(present))
}

fn take_plain_parameter_value(bytes: &[u8], position: &mut usize) -> Result<bool, DecodeError> {
    if bytes.get(*position) != Some(&b'=') {
        return Ok(true);
    }
    *position += 1;
    if matches!(bytes.get(*position), Some(b'"' | b'\\')) {
        return Ok(false);
    }
    if !take_plain_extension_token(bytes, position) {
        return Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidToken));
    }
    Ok(true)
}

fn validate_extension(bytes: &[u8]) -> Result<(), DecodeError> {
    parse_extension(bytes).map(drop)
}

#[inline]
fn take_plain_extension_token(bytes: &[u8], position: &mut usize) -> bool {
    let start = *position;
    while bytes.get(*position).is_some_and(|byte| validate::token_byte(*byte)) {
        *position += 1;
    }
    start != *position
}

fn parse_extension(bytes: &[u8]) -> Result<WebSocketExtensionView<'_>, DecodeError> {
    let mut position = 0_usize;
    let name_range = take_token(bytes, &mut position)?;
    let parameter_start = position;
    while take_extension_parameter(bytes, &mut position)?.is_some() {}
    let name = str::from_utf8(&bytes[name_range]).expect("HTTP token parsing guarantees an in-bounds ASCII range");
    Ok(WebSocketExtensionView {
        raw: bytes,
        name,
        parameter_start,
    })
}

fn take_extension_parameter<'a>(bytes: &'a [u8], position: &mut usize) -> Result<Option<WebSocketExtensionParameterView<'a>>, DecodeError> {
    skip_ows(bytes, position);
    if *position == bytes.len() {
        return Ok(None);
    }
    if bytes.get(*position) != Some(&b';') {
        return Err(invalid_syntax(&FieldName::SecWebSocketExtensions));
    }
    let raw_start = *position;
    *position += 1;
    skip_ows(bytes, position);
    let name_range = take_token(bytes, position)?;
    let (value, quoted) = if bytes.get(*position) == Some(&b'=') {
        *position += 1;
        if bytes.get(*position) == Some(&b'"') {
            (Some(take_quoted_string(bytes, position)?), true)
        } else {
            (Some(take_token(bytes, position)?), false)
        }
    } else {
        (None, false)
    };
    let raw_end = *position;
    let name = str::from_utf8(&bytes[name_range]).expect("HTTP token parsing guarantees an in-bounds ASCII range");
    let value = value.map(|range| &bytes[range]);
    Ok(Some(WebSocketExtensionParameterView {
        raw: trim_ows(&bytes[raw_start + 1..raw_end]),
        name,
        value,
        quoted,
    }))
}

fn take_token(bytes: &[u8], position: &mut usize) -> Result<Range<usize>, DecodeError> {
    let start = *position;
    while bytes.get(*position).is_some_and(|byte| validate::token_byte(*byte)) {
        *position += 1;
    }
    if start == *position {
        Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidToken))
    } else {
        Ok(start..*position)
    }
}

fn take_quoted_string(bytes: &[u8], position: &mut usize) -> Result<Range<usize>, DecodeError> {
    let start = *position;
    *position += 1;
    let mut escaped = false;
    let mut value_length = 0_usize;
    while let Some(byte) = bytes.get(*position).copied() {
        *position += 1;
        if escaped {
            if !validate::token_byte(byte) {
                return Err(invalid_syntax(&FieldName::SecWebSocketExtensions));
            }
            escaped = false;
            value_length += 1;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return if value_length == 0 {
                Err(DecodeError::new(&FieldName::SecWebSocketExtensions, DecodeErrorKind::InvalidToken))
            } else {
                Ok(start..*position)
            };
        } else if !validate::token_byte(byte) {
            return Err(invalid_syntax(&FieldName::SecWebSocketExtensions));
        } else {
            value_length += 1;
        }
    }
    Err(DecodeError::new(
        &FieldName::SecWebSocketExtensions,
        DecodeErrorKind::UnterminatedQuote,
    ))
}

fn skip_ows(bytes: &[u8], position: &mut usize) {
    while bytes.get(*position).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
        *position += 1;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        SecWebSocketExtensions, SecWebSocketExtensionsOwned, WebSocketExtensionParameterView, WebSocketExtensionParameters,
        WebSocketExtensionView, parse_extension, take_quoted_string, validate_plain_extension_line,
    };
    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::FieldSource;
    use crate::{DecodeErrorKind, Field, FieldValue, TestSink};

    #[test]
    fn builder_and_accessors_cover_flags_tokens_quotes_and_multiple_extensions() {
        let extensions = SecWebSocketExtensionsOwned::builder()
            .extension("permessage-deflate")
            .parameter_flag("client_max_window_bits")
            .parameter_value("server_max_window_bits", "15")
            .quoted_parameter("mode", "fast")
            .extension("x-test")
            .build()
            .expect("nonempty extension list");
        assert_eq!(format!("{extensions:?}"), "SecWebSocketExtensionsOwned { value_count: 1 }");

        let parsed = extensions
            .extensions()
            .collect::<Result<Vec<_>, _>>()
            .expect("built extensions parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name(), "permessage-deflate");
        assert_eq!(
            parsed[0].as_bytes(),
            b"permessage-deflate; client_max_window_bits; server_max_window_bits=15; mode=\"fast\""
        );
        let parameters = parsed[0].parameters().collect::<Result<Vec<_>, _>>().expect("parameters parse");
        assert_eq!(parameters[0].name(), "client_max_window_bits");
        assert_eq!(parameters[0].value(), None);
        assert!(!parameters[0].is_quoted());
        assert_eq!(parameters[0].value_str(), Ok(None));
        assert_eq!(parameters[0].as_bytes(), b"client_max_window_bits");
        assert_eq!(parameters[1].name(), "server_max_window_bits");
        assert_eq!(parameters[1].value(), Some(b"15".as_slice()));
        assert_eq!(parameters[1].value_str(), Ok(Some("15")));
        assert!(!parameters[1].is_quoted());
        assert_eq!(parameters[2].name(), "mode");
        assert_eq!(parameters[2].value(), Some(b"\"fast\"".as_slice()));
        assert_eq!(parameters[2].value_str(), Ok(Some("\"fast\"")));
        assert!(parameters[2].is_quoted());
        assert_eq!(parsed[1].name(), "x-test");
        assert_eq!(parsed[1].parameters().next(), None);
    }

    #[test]
    fn builder_rejects_missing_extensions_and_invalid_tokens() {
        assert_eq!(
            SecWebSocketExtensionsOwned::builder().build().expect_err("empty builder").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketExtensionsOwned::builder()
                .parameter_flag("flag")
                .build()
                .expect_err("parameter needs extension")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            SecWebSocketExtensionsOwned::builder()
                .extension("not valid")
                .build()
                .expect_err("invalid extension token")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        let builder = SecWebSocketExtensionsOwned::builder().extension("valid");
        assert_eq!(
            builder
                .clone()
                .parameter_flag("bad name")
                .build()
                .expect_err("invalid parameter name")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            builder
                .clone()
                .parameter_value("mode", "bad value")
                .build()
                .expect_err("invalid parameter value")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            builder
                .quoted_parameter("mode", "bad value")
                .build()
                .expect_err("quoted value must decode to token")
                .kind(),
            DecodeErrorKind::InvalidToken
        );

        let overflow = super::SecWebSocketExtensionsBuilder {
            wire: Vec::new(),
            extension_count: usize::MAX,
            pending_error: None,
        };
        assert_eq!(
            overflow.extension("x").build().expect_err("extension count overflow").kind(),
            DecodeErrorKind::InvalidNumber
        );

        let invalid_wire = super::SecWebSocketExtensionsBuilder {
            wire: vec![b'\n'],
            extension_count: 1,
            pending_error: None,
        };
        assert_eq!(
            invalid_wire.build().expect_err("invalid private builder wire").kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn conversions_views_owned_and_insert_preserve_repeated_field_lines() {
        for value in [
            SecWebSocketExtensionsOwned::try_from("permessage-deflate; client_max_window_bits=15"),
            SecWebSocketExtensionsOwned::try_from(String::from("permessage-deflate; client_max_window_bits=15")),
            SecWebSocketExtensionsOwned::try_from(FieldValue::from_static("permessage-deflate; client_max_window_bits=15")),
        ] {
            assert_eq!(
                value
                    .expect("valid extension")
                    .extensions()
                    .next()
                    .expect("one extension")
                    .expect("extension parses")
                    .name(),
                "permessage-deflate"
            );
        }
        assert_eq!(
            SecWebSocketExtensionsOwned::try_from(String::from("extension\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketExtensionsOwned::try_from("extension\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketExtensionsOwned::try_from(FieldValue::from_static("x; =bad"))
                .expect_err("invalid stored parameter")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            SecWebSocketExtensionsOwned::try_from(FieldValue::from_static(",,,"))
                .expect_err("empty stored list")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let mut table = TestSink::new();
        assert!(SecWebSocketExtensions::view(&table).expect("absent view succeeds").is_none());
        assert!(SecWebSocketExtensions::owned(&table).expect("absent owned succeeds").is_none());
        table
            .set_values(
                SecWebSocketExtensions::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("permessage-deflate"),
                    FieldValue::from_static("x-test; mode=\"fast\", x-other"),
                ]),
            )
            .expect("table accepts extensions");
        let view = SecWebSocketExtensions::view(&table)
            .expect("view decodes")
            .expect("header is present");
        assert_eq!(view.field_values().count(), 2);
        assert_eq!(
            view.extensions()
                .map(|extension| extension.map(WebSocketExtensionView::name))
                .collect::<Result<Vec<_>, _>>(),
            Ok(vec!["permessage-deflate", "x-test", "x-other"])
        );
        assert_eq!(format!("{view:?}"), "SecWebSocketExtensionsView { value_count: 2 }");

        let owned = SecWebSocketExtensions::owned(&table)
            .expect("owned decode succeeds")
            .expect("header is present");
        let mut output = TestSink::new();
        SecWebSocketExtensions::insert(&mut output, owned).expect("extensions insert");
        assert_eq!(output.lines(SecWebSocketExtensions::name()).expect("inserted extensions").len(), 2);
    }

    #[test]
    fn plain_and_structured_parsers_classify_errors_and_quotes() {
        assert_eq!(validate_plain_extension_line(b"x-test; flag"), Ok(Some(true)));
        assert_eq!(validate_plain_extension_line(b"permessage-deflate"), Ok(Some(true)));
        assert_eq!(validate_plain_extension_line(b",, \t"), Ok(Some(false)));
        assert_eq!(validate_plain_extension_line(b"x; p=\"v\""), Ok(None));
        assert_eq!(validate_plain_extension_line(b"\\x"), Ok(None));
        assert_eq!(validate_plain_extension_line(b"x; \\p"), Ok(None));
        assert_eq!(validate_plain_extension_line(b"x; p=\\v"), Ok(None));
        assert_eq!(validate_plain_extension_line(b"x\\"), Ok(None));
        assert_eq!(validate_plain_extension_line(b"x,y"), Ok(Some(true)));
        assert_eq!(
            validate_plain_extension_line(b"=x").expect_err("missing extension name").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            validate_plain_extension_line(b"x; =v").expect_err("missing parameter name").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            validate_plain_extension_line(b"x; p=").expect_err("missing parameter value").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            validate_plain_extension_line(b"x p").expect_err("missing delimiter").kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let quoted = parse_extension(b"x; mode=\"fa\\st\"").expect("escaped token parses");
        let parameter = quoted.parameters().next().expect("one parameter").expect("parameter parses");
        assert!(parameter.is_quoted());
        assert_eq!(parameter.value(), Some(b"\"fa\\st\"".as_slice()));

        for raw in [
            b"".as_slice(),
            b"x; mode=\"\"".as_slice(),
            b"x; mode=\"bad value\"".as_slice(),
            b"x; mode=\"unterminated".as_slice(),
            b"x; mode=\"trail\\".as_slice(),
            b"x; mode=\"bad\\ \"".as_slice(),
            b"x; mode=".as_slice(),
        ] {
            let _error = parse_extension(raw).expect_err("malformed extension must fail");
        }

        let mut position = 0;
        let _range = take_quoted_string(b"\"ok\"", &mut position).expect("quoted token is valid");

        let invalid_utf8 = WebSocketExtensionParameterView {
            raw: b"mode=\xff",
            name: "mode",
            value: Some(&[0xff]),
            quoted: false,
        };
        assert_eq!(
            invalid_utf8.value_str().expect_err("non-UTF-8 value").kind(),
            DecodeErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn parameter_iterator_stops_after_error_and_owned_quote_errors_keep_line_index() {
        let mut parameters = WebSocketExtensionParameters {
            bytes: b"x; =bad",
            position: 1,
        };
        assert_eq!(
            parameters.next().expect("one error").expect_err("missing name").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert!(parameters.next().is_none());

        let mut parameters = WebSocketExtensionParameters {
            bytes: b"x bad",
            position: 1,
        };
        assert_eq!(
            parameters
                .next()
                .expect("one error")
                .expect_err("parameter delimiter is required")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let mut table = TestSink::new();
        table
            .set_values(
                SecWebSocketExtensions::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("x"),
                    FieldValue::from_static("y; mode=\"unterminated"),
                ]),
            )
            .expect("table accepts raw extensions");
        let error = SecWebSocketExtensions::owned(&table).expect_err("unterminated quote is rejected");
        assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
        assert_eq!(error.value_index(), Some(1));

        table
            .set_values(
                SecWebSocketExtensions::name(),
                EncodedValues::single(FieldValue::from_static("x; =bad")),
            )
            .expect("table accepts malformed extension");
        assert_eq!(
            SecWebSocketExtensions::owned(&table)
                .expect_err("invalid parameter is rejected")
                .kind(),
            DecodeErrorKind::InvalidToken
        );

        table
            .set_values(
                SecWebSocketExtensions::name(),
                EncodedValues::single(FieldValue::from_static(",,, \t")),
            )
            .expect("table accepts empty list");
        assert_eq!(
            SecWebSocketExtensions::view(&table).expect_err("empty list is rejected").kind(),
            DecodeErrorKind::MissingValue
        );

        table
            .set_values(
                SecWebSocketExtensions::name(),
                EncodedValues::single(FieldValue::from_static("x; =bad")),
            )
            .expect("table accepts invalid plain extension");
        assert_eq!(
            SecWebSocketExtensions::view(&table)
                .expect_err("plain parser error propagates")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
    }
}
