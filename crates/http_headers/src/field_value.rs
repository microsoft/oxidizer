// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owned and borrowed HTTP field values.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::{self, FromStr, Utf8Error};

use bytes::Bytes;

use crate::sink::FieldSensitivity;
use crate::validate;

/// An error produced when bytes cannot form an HTTP field value.
///
/// # Examples
///
/// ```rust
/// use http_headers::{FieldValue, InvalidFieldValue};
///
/// let error: InvalidFieldValue = FieldValue::from_bytes(b"line\r\nbreak").unwrap_err();
/// assert_eq!(error.to_string(), "invalid HTTP field value");
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct InvalidFieldValue;

impl fmt::Display for InvalidFieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid HTTP field value")
    }
}

impl Error for InvalidFieldValue {}

/// An owned, validated HTTP field value.
///
/// Construction validates the [RFC 9110 field-value grammar].
///
/// This type is the `field-value` production itself: the bytes after the colon
/// on a single field line. RFC 9110 additionally uses "field value" for the
/// comma-joined combination of every line sharing a name. [`comma_items`]
/// walks complete items across those lines without materializing them.
/// Combining lines yields bytes that are themselves a valid `field-value`, so
/// one type serves both senses.
///
/// Mark sensitive values to keep their bytes out of [`Debug`] output. The
/// sensitivity marker does not affect equality, ordering, or hashing.
///
/// [RFC 9110 field-value grammar]: https://www.rfc-editor.org/rfc/rfc9110#section-5.5
/// [`comma_items`]: crate::source::FieldLines::comma_items
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldValue;
///
/// let value = FieldValue::from_static("gzip");
/// assert_eq!(value.as_bytes(), b"gzip");
/// assert!(FieldValue::from_bytes(b"line\r\nbreak").is_err());
/// ```
#[derive(Clone)]
pub struct FieldValue {
    repr: Repr,
}

impl Default for FieldValue {
    fn default() -> Self {
        Self::from_static("")
    }
}

/// The longest value stored without allocating.
///
/// Sized from measurement rather than from the shared variant's footprint.
/// Inlining beats sharing for values this short — a copy of a few dozen bytes
/// costs less than the atomic clone and promotion an owner reference needs —
/// and at this length the wider type is still free: no header measured slower
/// against a 38-byte buffer. Past roughly 112 bytes every header starts paying
/// for the larger moves, which is where sharing takes over instead.
const INLINE_CAPACITY: usize = 64;

#[derive(Clone)]
enum Repr {
    Inline {
        len: u8,
        sensitive: bool,
        buf: [u8; INLINE_CAPACITY],
    },
    Shared {
        bytes: Bytes,
        sensitive: bool,
    },
    /// A value retained from an `http::HeaderMap` without copying its bytes.
    ///
    /// `http` keeps the `Bytes` behind a `HeaderValue` private, so sharing its
    /// buffer means holding the `HeaderValue` itself. It is 40 bytes, well
    /// inside what the inline variant already needs, so the extra
    /// representation costs nothing in size. Sensitivity is stored separately
    /// to keep the accessors `const`.
    #[cfg(feature = "http")]
    Http {
        value: http::HeaderValue,
        sensitive: bool,
    },
}

impl Repr {
    /// Stores `bytes` inline when short enough, otherwise in shared storage.
    fn new(bytes: &[u8], sensitive: bool) -> Self {
        if bytes.len() <= INLINE_CAPACITY {
            let mut buf = [0; INLINE_CAPACITY];
            buf[..bytes.len()].copy_from_slice(bytes);
            #[expect(clippy::cast_possible_truncation, reason = "guarded by the INLINE_CAPACITY check above")]
            Self::Inline {
                len: bytes.len() as u8,
                sensitive,
                buf,
            }
        } else {
            Self::Shared {
                bytes: Bytes::copy_from_slice(bytes),
                sensitive,
            }
        }
    }

    /// Stores an `http` value inline when short, and otherwise retains it.
    #[cfg(feature = "http")]
    #[inline]
    fn from_http(value: &http::HeaderValue) -> Self {
        let bytes = value.as_bytes();
        if bytes.len() <= INLINE_CAPACITY {
            Self::new(bytes, value.is_sensitive())
        } else {
            Self::retain_http(value.clone(), value.is_sensitive())
        }
    }

    /// Retains an `http` value, refcounting its buffer rather than copying it.
    #[cfg(feature = "http")]
    #[cold]
    #[inline(never)]
    fn retain_http(value: http::HeaderValue, sensitive: bool) -> Self {
        Self::Http { value, sensitive }
    }

    /// Stores an owner's stable projection inline when short enough.
    fn from_owner_bytes(bytes: Bytes, sensitive: bool) -> Self {
        if bytes.len() <= INLINE_CAPACITY {
            Self::new(&bytes, sensitive)
        } else {
            Self::Shared { bytes, sensitive }
        }
    }

    /// Shares `owner`'s stable projection when the value is too long to inline.
    #[cfg(feature = "http")]
    fn from_owner<T>(owner: T, sensitive: bool) -> Self
    where
        T: AsRef<[u8]> + Send + 'static,
    {
        Self::from_owner_bytes(Bytes::from_owner(owner), sensitive)
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Inline { len, buf, .. } => &buf[..*len as usize],
            Self::Shared { bytes, .. } => bytes,
            #[cfg(feature = "http")]
            Self::Http { value, .. } => value.as_bytes(),
        }
    }

    #[inline]
    const fn sensitive(&self) -> bool {
        match self {
            Self::Inline { sensitive, .. } | Self::Shared { sensitive, .. } => *sensitive,
            #[cfg(feature = "http")]
            Self::Http { sensitive, .. } => *sensitive,
        }
    }

    #[inline]
    const fn set_sensitive(&mut self, value: bool) {
        match self {
            Self::Inline { sensitive, .. } | Self::Shared { sensitive, .. } => *sensitive = value,
            #[cfg(feature = "http")]
            Self::Http { sensitive, .. } => *sensitive = value,
        }
    }

    /// Consumes the value and returns shared storage, allocating when the
    /// bytes were held inline.
    fn into_shared(self) -> Bytes {
        match self {
            Self::Inline { len, buf, .. } => Bytes::copy_from_slice(&buf[..len as usize]),
            Self::Shared { bytes, .. } => bytes,
            #[cfg(feature = "http")]
            Self::Http { value, .. } => Bytes::from_owner(value),
        }
    }
}

impl FieldValue {
    /// Creates a value from a static string.
    ///
    /// # Panics
    ///
    /// Panics when `value` is not a valid field value. Use
    /// [`FieldValue::try_from_static`] to detect that without panicking.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    ///
    /// assert_eq!(FieldValue::from_static("gzip").as_bytes(), b"gzip");
    /// ```
    #[must_use]
    pub const fn from_static(value: &'static str) -> Self {
        assert!(is_field_value(value.as_bytes()), "invalid static HTTP field value");
        Self {
            repr: Repr::Shared {
                bytes: Bytes::from_static(value.as_bytes()),
                sensitive: false,
            },
        }
    }

    /// Creates a value from a static string without panicking.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a valid field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    ///
    /// assert!(FieldValue::try_from_static("gzip").is_ok());
    /// assert!(FieldValue::try_from_static("\r\n").is_err());
    /// ```
    pub const fn try_from_static(value: &'static str) -> Result<Self, InvalidFieldValue> {
        if is_field_value(value.as_bytes()) {
            Ok(Self::from_static(value))
        } else {
            Err(InvalidFieldValue)
        }
    }

    /// Creates a value from `bytes`.
    ///
    /// # Errors
    ///
    /// Returns an error when `bytes` is not a valid field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    ///
    /// assert_eq!(FieldValue::from_bytes(b"gzip")?.as_bytes(), b"gzip");
    /// # Ok::<(), http_headers::InvalidFieldValue>(())
    /// ```
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, InvalidFieldValue> {
        let bytes = bytes.as_ref();
        if validate::field_value(bytes) {
            Ok(Self {
                repr: Repr::new(bytes, false),
            })
        } else {
            Err(InvalidFieldValue)
        }
    }

    /// Creates a value from `value`.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a valid field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    ///
    /// assert_eq!(FieldValue::from_str("gzip")?.as_bytes(), b"gzip");
    /// # Ok::<(), http_headers::InvalidFieldValue>(())
    /// ```
    #[expect(
        clippy::should_implement_trait,
        reason = "the inherent constructor mirrors the conventional field-value API and `FromStr` is implemented as well"
    )]
    pub fn from_str(value: impl AsRef<str>) -> Result<Self, InvalidFieldValue> {
        Self::from_bytes(value.as_ref().as_bytes())
    }

    /// Creates a value from [`Bytes`].
    ///
    /// # Errors
    ///
    /// Returns an error when `bytes` is not a valid field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    ///
    /// let value = FieldValue::from_shared(bytes::Bytes::from_static(b"gzip"))?;
    /// assert_eq!(value.as_bytes(), b"gzip");
    /// # Ok::<(), http_headers::InvalidFieldValue>(())
    /// ```
    pub fn from_shared(bytes: Bytes) -> Result<Self, InvalidFieldValue> {
        if validate::field_value(&bytes) {
            Ok(Self {
                repr: Repr::Shared { bytes, sensitive: false },
            })
        } else {
            Err(InvalidFieldValue)
        }
    }

    /// Creates a value that shares `owner`'s buffer instead of copying it.
    ///
    /// This is the hook for a [`crate::source::FieldSource`] whose storage is already
    /// refcounted. Short values are copied inline, which is cheaper than
    /// sharing; longer ones retain `owner`, so cloning the resulting value
    /// costs a refcount bump rather than a copy of its bytes, whatever the
    /// owning type is.
    ///
    /// # Errors
    ///
    /// Returns an error when `owner` does not hold a valid field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValue;
    ///
    /// let value = FieldValue::from_owner(vec![b'g', b'z', b'i', b'p'])?;
    /// assert_eq!(value.as_bytes(), b"gzip");
    /// # Ok::<(), http_headers::InvalidFieldValue>(())
    /// ```
    pub fn from_owner<T>(owner: T) -> Result<Self, InvalidFieldValue>
    where
        T: AsRef<[u8]> + Send + 'static,
    {
        let bytes = Bytes::from_owner(owner);
        if validate::field_value(&bytes) {
            Ok(Self {
                repr: Repr::from_owner_bytes(bytes, false),
            })
        } else {
            Err(InvalidFieldValue)
        }
    }

    /// Returns the wire bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(
    ///     http_headers::FieldValue::from_static("gzip").as_bytes(),
    ///     b"gzip"
    /// );
    /// ```
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.repr.as_bytes()
    }

    /// Returns a borrowed view of this value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::FieldValue::from_static("gzip");
    /// assert_eq!(value.as_field_value_ref().as_bytes(), b"gzip");
    /// ```
    #[must_use]
    #[inline]
    pub fn as_field_value_ref(&self) -> FieldValueRef<'_> {
        FieldValueRef::new(self.as_bytes()).with_sensitive(self.is_sensitive())
    }

    /// Returns the value as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not UTF-8.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(
    ///     http_headers::FieldValue::from_static("gzip").to_str()?,
    ///     "gzip"
    /// );
    /// # Ok::<(), std::str::Utf8Error>(())
    /// ```
    #[inline]
    pub fn to_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(self.as_bytes())
    }

    /// Returns the number of wire bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(http_headers::FieldValue::from_static("gzip").len(), 4);
    /// ```
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Returns whether the value is empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(http_headers::FieldValue::from_static("").is_empty());
    /// ```
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    /// Returns whether the value was marked as carrying sensitive data.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(!http_headers::FieldValue::from_static("gzip").is_sensitive());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_sensitive(&self) -> bool {
        self.repr.sensitive()
    }

    /// Sets the value's sensitivity classification.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldSensitivity;
    ///
    /// let mut value = http_headers::FieldValue::from_static("secret");
    /// value.set_sensitivity(FieldSensitivity::Sensitive);
    /// assert!(value.is_sensitive());
    /// ```
    #[inline]
    pub const fn set_sensitivity(&mut self, sensitivity: FieldSensitivity) {
        self.repr.set_sensitive(sensitivity.is_sensitive());
    }

    pub(crate) const fn set_sensitive(&mut self, sensitive: bool) {
        self.repr.set_sensitive(sensitive);
    }

    /// Returns the value with its sensitivity classification applied.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldSensitivity;
    ///
    /// let value = http_headers::FieldValue::from_static("secret")
    ///     .with_sensitivity(FieldSensitivity::Sensitive);
    /// assert!(value.is_sensitive());
    /// ```
    #[must_use]
    #[inline]
    pub fn with_sensitivity(mut self, sensitivity: FieldSensitivity) -> Self {
        self.set_sensitivity(sensitivity);
        self
    }

    pub(crate) fn with_sensitive(mut self, sensitive: bool) -> Self {
        self.set_sensitive(sensitive);
        self
    }

    /// Consumes the value and returns its bytes.
    ///
    /// The sensitivity marker is not represented in the returned [`Bytes`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// let bytes = http_headers::FieldValue::from_static("gzip").into_shared();
    /// assert_eq!(bytes.as_ref(), b"gzip");
    /// ```
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> Bytes {
        self.repr.into_shared()
    }
}

/// Returns whether every byte is permitted in an HTTP field value.
///
/// This is the `const` counterpart of [`crate::validate::field_value`], which
/// the accelerated runtime path uses; both accept exactly the same bytes.
const fn is_field_value(bytes: &[u8]) -> bool {
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != b'\t' && (byte < b' ' || byte == 0x7f) {
            return false;
        }
        index += 1;
    }
    true
}

macro_rules! impl_from_integer {
    ($($integer:ty),+ $(,)?) => {
        $(
            impl From<$integer> for FieldValue {
                /// Formats the number, which is always a valid field value.
                fn from(value: $integer) -> Self {
                    let mut buffer = itoa::Buffer::new();
                    let formatted = buffer.format(value);
                    Self {
                        repr: Repr::new(formatted.as_bytes(), false),
                    }
                }
            }
        )+
    };
}

impl_from_integer!(i16, i32, i64, isize, u16, u32, u64, usize);

impl fmt::Debug for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_sensitive() {
            f.write_str("FieldValue(Sensitive)")
        } else {
            write!(f, "FieldValue({:?})", ByteStr(self.as_bytes()))
        }
    }
}

impl PartialEq for FieldValue {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for FieldValue {}

impl Ord for FieldValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl PartialOrd for FieldValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for FieldValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl AsRef<[u8]> for FieldValue {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl FromStr for FieldValue {
    type Err = InvalidFieldValue;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(value.as_bytes())
    }
}

impl TryFrom<&[u8]> for FieldValue {
    type Error = InvalidFieldValue;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(value)
    }
}

impl TryFrom<&str> for FieldValue {
    type Error = InvalidFieldValue;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(value.as_bytes())
    }
}

impl TryFrom<Vec<u8>> for FieldValue {
    type Error = InvalidFieldValue;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        Self::from_shared(Bytes::from(value))
    }
}

impl TryFrom<String> for FieldValue {
    type Error = InvalidFieldValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_shared(Bytes::from(value))
    }
}

impl TryFrom<Bytes> for FieldValue {
    type Error = InvalidFieldValue;

    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        Self::from_shared(value)
    }
}

impl From<FieldValue> for Bytes {
    fn from(value: FieldValue) -> Self {
        value.into_shared()
    }
}

/// A borrowed HTTP field value.
///
/// This is the borrowed counterpart of [`FieldValue`]. Field `*View` types
/// use it to expose field bytes for as long as the source remains borrowed.
///
/// Like [`FieldValue`], it can be marked sensitive. The marker is preserved
/// when converting to an owned value and does not affect equality, ordering,
/// or hashing. [`Debug`] never includes the borrowed bytes, regardless of the
/// marker.
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldValueRef;
///
/// let value = FieldValueRef::new(b"gzip");
/// assert_eq!(value.as_str()?, "gzip");
/// # Ok::<(), std::str::Utf8Error>(())
/// ```
#[derive(Clone, Copy, Default)]
pub struct FieldValueRef<'a> {
    bytes: &'a [u8],
    sensitive: bool,
}

impl<'a> FieldValueRef<'a> {
    /// Creates a borrowed field value.
    ///
    /// This constructor accepts arbitrary bytes. Use
    /// [`FieldValueRef::try_to_field_value`] when converting untrusted bytes
    /// into a validated [`FieldValue`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(
    ///     http_headers::FieldValueRef::new(b"gzip").as_bytes(),
    ///     b"gzip"
    /// );
    /// ```
    #[must_use]
    #[inline]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, sensitive: false }
    }

    /// Returns the value with its sensitivity classification applied.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldSensitivity;
    ///
    /// let value =
    ///     http_headers::FieldValueRef::new(b"secret").with_sensitivity(FieldSensitivity::Sensitive);
    /// assert!(value.is_sensitive());
    /// ```
    #[must_use]
    #[inline]
    pub const fn with_sensitivity(mut self, sensitivity: FieldSensitivity) -> Self {
        self.sensitive = sensitivity.is_sensitive();
        self
    }

    pub(crate) const fn with_sensitive(mut self, sensitive: bool) -> Self {
        self.sensitive = sensitive;
        self
    }

    /// Returns whether the value was marked as carrying sensitive data.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(!http_headers::FieldValueRef::new(b"gzip").is_sensitive());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_sensitive(self) -> bool {
        self.sensitive
    }

    /// Returns the wire bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(
    ///     http_headers::FieldValueRef::new(b"gzip").as_bytes(),
    ///     b"gzip"
    /// );
    /// ```
    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Returns the value as UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not UTF-8.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(http_headers::FieldValueRef::new(b"gzip").as_str()?, "gzip");
    /// # Ok::<(), std::str::Utf8Error>(())
    /// ```
    #[inline]
    pub const fn as_str(self) -> Result<&'a str, Utf8Error> {
        str::from_utf8(self.bytes)
    }

    /// Alias for [`FieldValueRef::as_str`].
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not UTF-8.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(http_headers::FieldValueRef::new(b"gzip").to_str()?, "gzip");
    /// # Ok::<(), std::str::Utf8Error>(())
    /// ```
    #[inline]
    pub const fn to_str(self) -> Result<&'a str, Utf8Error> {
        self.as_str()
    }

    /// Returns the number of wire bytes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert_eq!(http_headers::FieldValueRef::new(b"gzip").len(), 4);
    /// ```
    #[must_use]
    #[inline]
    pub const fn len(self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the value is empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(http_headers::FieldValueRef::new(b"").is_empty());
    /// ```
    #[must_use]
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    /// Creates an owned value from the borrowed bytes.
    ///
    /// The sensitivity flag travels with the bytes.
    ///
    /// # Panics
    ///
    /// Panics if a caller constructed this reference from bytes that are not
    /// a valid HTTP field value. [`FieldValueRef::try_to_field_value`] is the
    /// fallible form, and is the recommended conversion for a reference whose
    /// provenance a caller cannot vouch for.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let owned = http_headers::FieldValueRef::new(b"gzip").to_field_value();
    /// assert_eq!(owned.as_bytes(), b"gzip");
    /// ```
    #[must_use]
    #[inline]
    pub fn to_field_value(self) -> FieldValue {
        self.try_to_field_value()
            .expect("FieldValueRef must contain a valid HTTP field value")
    }

    /// Creates a validated owned value from the borrowed bytes.
    ///
    /// The sensitivity flag travels with the bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the borrowed bytes are not a valid HTTP field
    /// value. [`FieldValueRef::new`] accepts arbitrary bytes, so a reference
    /// a caller built by hand can hold bytes this validated owned type
    /// rejects.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldValueRef;
    ///
    /// let owned = FieldValueRef::new(b"gzip").try_to_field_value()?;
    /// assert_eq!(owned.as_bytes(), b"gzip");
    /// assert!(
    ///     FieldValueRef::new(b"bad\nvalue")
    ///         .try_to_field_value()
    ///         .is_err()
    /// );
    /// # Ok::<(), http_headers::InvalidFieldValue>(())
    /// ```
    #[inline]
    pub fn try_to_field_value(self) -> Result<FieldValue, InvalidFieldValue> {
        FieldValue::from_bytes(self.bytes).map(|owned| owned.with_sensitive(self.sensitive))
    }
}

impl fmt::Debug for FieldValueRef<'_> {
    /// Formats the value without exposing its bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.sensitive {
            f.write_str("FieldValueRef(Sensitive)")
        } else {
            write!(f, "FieldValueRef({} bytes)", self.bytes.len())
        }
    }
}

impl PartialEq for FieldValueRef<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for FieldValueRef<'_> {}

impl Ord for FieldValueRef<'_> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.bytes.cmp(other.bytes)
    }
}

impl PartialOrd for FieldValueRef<'_> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for FieldValueRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl AsRef<[u8]> for FieldValueRef<'_> {
    fn as_ref(&self) -> &[u8] {
        self.bytes
    }
}

impl<'a> From<&'a FieldValue> for FieldValueRef<'a> {
    fn from(value: &'a FieldValue) -> Self {
        value.as_field_value_ref()
    }
}

impl<'a> From<&'a [u8]> for FieldValueRef<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Self::new(bytes)
    }
}

impl TryFrom<FieldValueRef<'_>> for FieldValue {
    type Error = InvalidFieldValue;

    /// Creates a validated owned value from the borrowed bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the borrowed bytes are not a valid HTTP field
    /// value.
    fn try_from(value: FieldValueRef<'_>) -> Result<Self, Self::Error> {
        value.try_to_field_value()
    }
}

/// Formats bytes as a string when possible and as an escaped list otherwise.
struct ByteStr<'a>(&'a [u8]);

impl fmt::Debug for ByteStr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match str::from_utf8(self.0) {
            Ok(text) => fmt::Debug::fmt(text, f),
            Err(_invalid) => fmt::Debug::fmt(self.0, f),
        }
    }
}

macro_rules! impl_eq_bytes {
    ($owner:ty, $other:ty) => {
        impl PartialEq<$other> for $owner {
            #[inline]
            fn eq(&self, other: &$other) -> bool {
                AsRef::<[u8]>::as_ref(self) == AsRef::<[u8]>::as_ref(other)
            }
        }
    };
}

impl_eq_bytes!(FieldValue, str);
impl_eq_bytes!(FieldValue, &str);
impl_eq_bytes!(FieldValue, String);
impl_eq_bytes!(FieldValue, [u8]);
impl_eq_bytes!(FieldValue, &[u8]);
impl_eq_bytes!(FieldValue, Vec<u8>);
impl_eq_bytes!(FieldValue, FieldValueRef<'_>);
impl_eq_bytes!(FieldValueRef<'_>, str);
impl_eq_bytes!(FieldValueRef<'_>, &str);
impl_eq_bytes!(FieldValueRef<'_>, String);
impl_eq_bytes!(FieldValueRef<'_>, [u8]);
impl_eq_bytes!(FieldValueRef<'_>, &[u8]);
impl_eq_bytes!(FieldValueRef<'_>, Vec<u8>);
impl_eq_bytes!(FieldValueRef<'_>, FieldValue);

impl PartialEq<FieldValue> for str {
    #[inline]
    fn eq(&self, other: &FieldValue) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<FieldValueRef<'_>> for str {
    #[inline]
    fn eq(&self, other: &FieldValueRef<'_>) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<FieldValue> for &str {
    #[inline]
    fn eq(&self, other: &FieldValue) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<FieldValueRef<'_>> for &str {
    #[inline]
    fn eq(&self, other: &FieldValueRef<'_>) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

#[cfg(feature = "http")]
mod http_conversions {
    use http::HeaderValue;

    use super::{FieldValue, Repr};

    impl From<&HeaderValue> for FieldValue {
        /// Converts an `http` field value while preserving its sensitivity marker.
        ///
        /// A value too long to inline shares the `HeaderValue`'s buffer rather
        /// than copying it. `http` keeps its `Bytes` private, so the retained
        /// owner is a clone of the `HeaderValue` itself — a refcount bump.
        fn from(value: &HeaderValue) -> Self {
            Self {
                repr: Repr::from_http(value),
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::error::Error;
    use std::hash::{Hash, Hasher};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use bytes::Bytes;

    use super::{FieldValue, FieldValueRef, INLINE_CAPACITY, InvalidFieldValue, Repr};

    #[test]
    fn short_values_are_stored_without_allocating_and_do_not_grow_the_type() {
        assert_eq!(size_of::<FieldValue>(), 72);

        let inline = FieldValue::from_bytes([b'a'; INLINE_CAPACITY]).expect("valid");
        assert!(matches!(inline.repr, Repr::Inline { .. }));
        assert_eq!(inline.as_bytes(), &[b'a'; INLINE_CAPACITY]);

        let shared = FieldValue::from_bytes([b'a'; INLINE_CAPACITY + 1]).expect("valid");
        assert!(matches!(shared.repr, Repr::Shared { .. }));
        assert_eq!(shared.as_bytes(), &[b'a'; INLINE_CAPACITY + 1]);

        // Sensitivity and byte round-tripping survive both representations.
        for mut value in [inline, shared] {
            let bytes = value.as_bytes().to_vec();
            value.set_sensitive(true);
            assert!(value.is_sensitive());
            assert_eq!(value.clone().into_shared(), bytes);
        }
    }

    #[test]
    fn constructors_accessors_sensitivity_and_debug_validate_real_bytes() {
        let error = InvalidFieldValue;
        assert_eq!(error.to_string(), "invalid HTTP field value");
        let error: &dyn Error = &error;
        assert!(error.source().is_none());

        let default = FieldValue::default();
        assert!(default.is_empty());
        assert_eq!(default.len(), 0);
        assert_eq!(FieldValue::try_from_static("gzip").expect("valid"), "gzip");
        FieldValue::try_from_static("\r\n").expect_err("CRLF is invalid");
        std::panic::catch_unwind(|| FieldValue::from_static("\n")).expect_err("invalid static value panics");

        let copied = FieldValue::from_bytes(b"\t visible \xff").expect("valid field bytes");
        assert_eq!(copied.as_bytes(), b"\t visible \xff");
        copied.to_str().expect_err("obs-text is not UTF-8");
        assert_eq!(
            format!("{copied:?}"),
            "FieldValue([9, 32, 118, 105, 115, 105, 98, 108, 101, 32, 255])"
        );
        FieldValue::from_bytes(b"\0").expect_err("NUL is invalid");
        FieldValue::from_bytes(b"\x7f").expect_err("DEL is invalid");
        FieldValue::from_str("line\nbreak").expect_err("newline is invalid");

        let shared = FieldValue::from_shared(Bytes::from_static(b"shared")).expect("valid");
        assert_eq!(shared.to_str().expect("UTF-8"), "shared");
        FieldValue::from_shared(Bytes::from_static(b"\r")).expect_err("carriage return is invalid");
        assert_eq!(shared.into_shared(), Bytes::from_static(b"shared"));

        let mut sensitive = FieldValue::from_static("secret");
        sensitive.set_sensitive(true);
        assert!(sensitive.is_sensitive());
        assert_eq!(format!("{sensitive:?}"), "FieldValue(Sensitive)");
        sensitive.set_sensitive(false);
        assert!(!sensitive.is_sensitive());
        assert!(sensitive.with_sensitive(true).is_sensitive());
    }

    #[test]
    fn numeric_conversion_comparison_hashing_and_owned_conversions_are_consistent() {
        assert_eq!(FieldValue::from(i16::MIN), i16::MIN.to_string());
        assert_eq!(FieldValue::from(i32::MIN), i32::MIN.to_string());
        assert_eq!(FieldValue::from(i64::MIN), i64::MIN.to_string());
        assert_eq!(FieldValue::from(isize::MIN), isize::MIN.to_string());
        assert_eq!(FieldValue::from(u16::MAX), u16::MAX.to_string());
        assert_eq!(FieldValue::from(u32::MAX), u32::MAX.to_string());
        assert_eq!(FieldValue::from(u64::MAX), u64::MAX.to_string());
        assert_eq!(FieldValue::from(usize::MAX), usize::MAX.to_string());

        let value = FieldValue::from_static("abc");
        let same = "abc".parse::<FieldValue>().expect("valid");
        let later = FieldValue::from_static("abd");
        assert_eq!(value, same);
        assert!(value < later);
        assert_eq!(value.partial_cmp(&later), Some(std::cmp::Ordering::Less));
        assert_eq!(value.as_ref(), b"abc");

        let mut first_hash = DefaultHasher::new();
        value.hash(&mut first_hash);
        let mut second_hash = DefaultHasher::new();
        same.with_sensitive(true).hash(&mut second_hash);
        assert_eq!(first_hash.finish(), second_hash.finish());

        assert_eq!(FieldValue::try_from(b"abc".as_slice()).expect("valid"), "abc");
        assert_eq!(FieldValue::try_from("abc").expect("valid"), "abc");
        assert_eq!(FieldValue::try_from(b"abc".to_vec()).expect("valid"), "abc");
        assert_eq!(FieldValue::try_from(String::from("abc")).expect("valid"), "abc");
        assert_eq!(FieldValue::try_from(Bytes::from_static(b"abc")).expect("valid"), "abc");
        let bytes: Bytes = FieldValue::from_static("abc").into();
        assert_eq!(bytes, Bytes::from_static(b"abc"));
    }

    #[test]
    fn borrowed_values_and_symmetric_comparisons_match_wire_bytes() {
        let owned = FieldValue::from_static("abc");
        let borrowed = FieldValueRef::from(&owned);
        assert_eq!(borrowed.as_bytes(), b"abc");
        assert_eq!(borrowed.as_str().expect("UTF-8"), "abc");
        assert_eq!(borrowed.to_str().expect("UTF-8"), "abc");
        assert_eq!(borrowed.len(), 3);
        assert!(!borrowed.is_empty());
        assert_eq!(borrowed.as_ref(), b"abc");
        assert_eq!(format!("{borrowed:?}"), "FieldValueRef(3 bytes)");

        let empty = FieldValueRef::default();
        assert!(empty.is_empty());
        let from_bytes = FieldValueRef::from(b"abc".as_slice());
        assert_eq!(from_bytes.try_to_field_value().expect("valid bytes"), owned);
        assert_eq!(FieldValue::try_from(from_bytes).expect("valid bytes"), owned);
        FieldValueRef::new(b"\xff").as_str().expect_err("obs-text is not UTF-8");
        assert_eq!(format!("{:?}", FieldValueRef::new(b"\xff")), "FieldValueRef(1 bytes)");

        let string = String::from("abc");
        let vector = b"abc".to_vec();
        let slice = b"abc".as_slice();
        assert!(PartialEq::<str>::eq(&owned, "abc"));
        assert!(PartialEq::<&str>::eq(&owned, &"abc"));
        assert!(PartialEq::<String>::eq(&owned, &string));
        assert!(PartialEq::<[u8]>::eq(&owned, b"abc"));
        assert!(PartialEq::<&[u8]>::eq(&owned, &slice));
        assert!(PartialEq::<Vec<u8>>::eq(&owned, &vector));
        assert!(PartialEq::<FieldValueRef<'_>>::eq(&owned, &borrowed));
        assert!(PartialEq::<str>::eq(&borrowed, "abc"));
        assert!(PartialEq::<&str>::eq(&borrowed, &"abc"));
        assert!(PartialEq::<String>::eq(&borrowed, &string));
        assert!(PartialEq::<[u8]>::eq(&borrowed, b"abc"));
        assert!(PartialEq::<&[u8]>::eq(&borrowed, &slice));
        assert!(PartialEq::<Vec<u8>>::eq(&borrowed, &vector));
        assert!(PartialEq::<FieldValue>::eq(&borrowed, &owned));
        assert!("abc".eq(&owned));
        assert!("abc".eq(&borrowed));
        assert!((&"abc").eq(&owned));
        assert!((&"abc").eq(&borrowed));
    }

    #[test]
    fn borrowed_values_redact_debug_and_carry_sensitivity_into_owned_values() {
        let secret = FieldValue::from_static("Bearer credential").with_sensitive(true);
        let borrowed = secret.as_field_value_ref();
        assert!(borrowed.is_sensitive());
        assert_eq!(format!("{borrowed:?}"), "FieldValueRef(Sensitive)");

        let owned = borrowed.try_to_field_value().expect("valid bytes");
        assert!(owned.is_sensitive());
        assert_eq!(format!("{owned:?}"), "FieldValue(Sensitive)");
        assert_eq!(owned.as_bytes(), b"Bearer credential");

        let plain = FieldValueRef::new(b"Bearer credential");
        assert!(!plain.is_sensitive());
        let rendered = format!("{plain:?}");
        assert!(
            !rendered.contains("credential"),
            "a borrowed value must never render its bytes: {rendered}"
        );
        assert!(plain.with_sensitive(true).is_sensitive());

        assert_eq!(plain, borrowed);
        assert!(plain <= borrowed);
        let mut plain_hash = DefaultHasher::new();
        plain.hash(&mut plain_hash);
        let mut sensitive_hash = DefaultHasher::new();
        borrowed.hash(&mut sensitive_hash);
        assert_eq!(plain_hash.finish(), sensitive_hash.finish());
    }

    #[test]
    fn borrowed_to_owned_conversion_reports_invalid_bytes() {
        let invalid = FieldValueRef::new(b"bad\nvalue");
        assert_eq!(invalid.try_to_field_value().expect_err("newline is invalid"), InvalidFieldValue);
        FieldValue::try_from(invalid).expect_err("newline is invalid");
        assert_eq!(
            FieldValue::try_from(FieldValueRef::new(b"good value")).expect("valid"),
            "good value"
        );

        let valid = FieldValueRef::new(b"good value").with_sensitive(true);
        let owned = valid.to_field_value();
        assert_eq!(owned, "good value");
        assert!(owned.is_sensitive());
        std::panic::catch_unwind(|| invalid.to_field_value()).expect_err("the infallible form still documents its panic");
    }

    #[cfg(feature = "http")]
    #[test]
    fn http_conversions_preserve_bytes_and_sensitivity() {
        let mut http = http::HeaderValue::from_static("secret");
        http.set_sensitive(true);
        let borrowed_owned = FieldValue::from(&http);
        assert_eq!(borrowed_owned, "secret");
        assert!(borrowed_owned.is_sensitive());

        let moved_owned = FieldValue::from(http);
        let round_trip = http::HeaderValue::try_from(moved_owned).expect("valid");
        assert_eq!(round_trip, "secret");
        assert!(round_trip.is_sensitive());

        let borrowed = FieldValueRef::from(&round_trip);
        assert_eq!(borrowed, "secret");
        assert!(borrowed.is_sensitive());
        assert_eq!(format!("{borrowed:?}"), "FieldValueRef(Sensitive)");
        let converted = http::HeaderValue::try_from(borrowed).expect("valid");
        assert_eq!(converted, "secret");
        assert!(converted.is_sensitive());
        assert!(borrowed.try_to_field_value().expect("valid").is_sensitive());
        http::HeaderValue::try_from(FieldValueRef::new(b"\n")).expect_err("newline is invalid");
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_long_http_value_is_shared_rather_than_copied() {
        let long = "a".repeat(INLINE_CAPACITY + 1);
        let http = http::HeaderValue::from_str(&long).expect("valid");
        let shared = FieldValue::from(&http);
        assert_eq!(shared.as_bytes(), long.as_bytes());
        assert!(
            std::ptr::eq(shared.as_bytes().as_ptr(), http.as_bytes().as_ptr()),
            "a value too long to inline must retain the source buffer, not copy it"
        );
        assert_eq!(shared.into_shared().as_ref(), long.as_bytes());

        let short = http::HeaderValue::from_static("client/1");
        let inlined = FieldValue::from(&short);
        assert_eq!(inlined.as_bytes(), b"client/1");
        assert!(
            !std::ptr::eq(inlined.as_bytes().as_ptr(), short.as_bytes().as_ptr()),
            "a short value is copied inline, which costs less than sharing"
        );
    }

    #[test]
    fn an_owner_is_retained_only_when_the_value_is_too_long_to_inline() {
        let long = vec![b'a'; INLINE_CAPACITY + 1];
        let address = long.as_ptr();
        let shared = FieldValue::from_owner(long).expect("valid");
        assert!(std::ptr::eq(shared.as_bytes().as_ptr(), address));
        assert!(matches!(shared.repr, Repr::Shared { .. }));

        let short = vec![b'a'; INLINE_CAPACITY];
        let inlined = FieldValue::from_owner(short).expect("valid");
        assert!(matches!(inlined.repr, Repr::Inline { .. }));
        assert_eq!(inlined.as_bytes(), &[b'a'; INLINE_CAPACITY]);

        FieldValue::from_owner(vec![b'\n']).expect_err("a newline is not a field value");
    }

    #[test]
    fn owner_validation_and_storage_use_one_stable_projection() {
        struct StatefulOwner {
            valid: Vec<u8>,
            invalid: Vec<u8>,
            calls: Arc<AtomicUsize>,
        }

        impl AsRef<[u8]> for StatefulOwner {
            fn as_ref(&self) -> &[u8] {
                if self.calls.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    &self.valid
                } else {
                    &self.invalid
                }
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let owner = StatefulOwner {
            valid: vec![b'a'; INLINE_CAPACITY + 1],
            invalid: {
                let mut bytes = vec![b'a'; INLINE_CAPACITY + 1];
                bytes[INLINE_CAPACITY] = b'\n';
                bytes
            },
            calls: Arc::clone(&calls),
        };
        let value = FieldValue::from_owner(owner).expect("the captured projection is valid");
        assert_eq!(value.as_bytes(), &[b'a'; INLINE_CAPACITY + 1]);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    }
}

#[cfg(feature = "http")]
mod http_conversions_owned {
    use http::HeaderValue;

    use super::{FieldValue, FieldValueRef, InvalidFieldValue, Repr};

    impl From<HeaderValue> for FieldValue {
        /// Converts an `http` field value while preserving its sensitivity marker.
        ///
        /// A value too long to inline is retained rather than copied.
        fn from(value: HeaderValue) -> Self {
            let sensitive = value.is_sensitive();
            Self {
                repr: Repr::from_owner(value, sensitive),
            }
        }
    }

    impl TryFrom<FieldValue> for HeaderValue {
        type Error = InvalidFieldValue;

        /// Converts the value while preserving its sensitivity marker.
        ///
        /// A value retained from an `http::HeaderMap` is handed back as it
        /// was, without rebuilding or revalidating it.
        fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
            let sensitive = value.is_sensitive();
            let mut converted = match value.repr {
                Repr::Http { value, .. } => value,
                repr => Self::from_maybe_shared(repr.into_shared()).map_err(|_invalid| InvalidFieldValue)?,
            };
            converted.set_sensitive(sensitive);
            Ok(converted)
        }
    }

    impl TryFrom<FieldValueRef<'_>> for HeaderValue {
        type Error = InvalidFieldValue;

        fn try_from(value: FieldValueRef<'_>) -> Result<Self, Self::Error> {
            let mut converted = Self::from_bytes(value.as_bytes()).map_err(|_invalid| InvalidFieldValue)?;
            converted.set_sensitive(value.is_sensitive());
            Ok(converted)
        }
    }

    impl<'a> From<&'a HeaderValue> for FieldValueRef<'a> {
        /// Borrows the value's bytes, carrying its sensitivity flag along.
        fn from(value: &'a HeaderValue) -> Self {
            Self::new(value.as_bytes()).with_sensitive(value.is_sensitive())
        }
    }
}
