// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic and bearer `Authorization` header types.

use std::fmt;
use std::io::Write as _;
use std::marker::PhantomData;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use base64::write::EncoderWriter;
use zeroize::Zeroize;

use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField, validate};

/// Caps reusable decoded credential storage at 64 KiB to retain common credentials without
/// indefinitely holding attacker-sized allocations; increasing it trades memory for reuse.
const DEFAULT_CREDENTIAL_RETAIN_LIMIT: usize = 64 * 1024;

#[cfg(test)]
type ZeroizationLog = Arc<Mutex<Vec<(usize, usize, Vec<u8>)>>>;

/// The Bearer authorization scheme.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AuthorizationOwned::<http_headers::headers::Bearer>::bearer(
///     "abc.def",
/// )?;
/// assert_eq!(value.token()?, b"abc.def");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct Bearer;

/// The Basic authorization scheme.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{AuthorizationOwned, Basic};
///
/// let value = AuthorizationOwned::<Basic>::basic(b"Aladdin", b"open sesame")?;
/// assert_eq!(
///     value.encoded_credentials()?,
///     b"QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct Basic;

/// Defines the `Authorization` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 11.6.2](https://www.rfc-editor.org/rfc/rfc9110#section-11.6.2).
/// The Basic and Bearer schemes are defined by
/// [RFC 7617 section 2](https://www.rfc-editor.org/rfc/rfc7617#section-2) and
/// [RFC 6750 section 2.1](https://www.rfc-editor.org/rfc/rfc6750#section-2.1).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Authorization, AuthorizationOwned, Bearer};
///
/// let mut map = HeaderMap::new();
/// Authorization::<Bearer>::insert(&mut map, AuthorizationOwned::<Bearer>::bearer("abc.def")?)?;
/// assert!(Authorization::<Bearer>::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct Authorization<S> {
    _private: PhantomData<fn() -> S>,
}

/// Owned value for the `Authorization` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 11.6.2], with the Basic scheme specified by
/// [RFC 7617 section 2] and the Bearer scheme specified by [RFC 6750 section 2.1].
///
/// # Examples
///
/// ```rust
/// let authorization =
///     http_headers::headers::AuthorizationOwned::<http_headers::headers::Bearer>::bearer(
///         "abc.def",
///     )?;
/// assert_eq!(authorization.token()?, b"abc.def");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==` carries Basic
/// credentials, while the Bearer scheme carries a token68 credential.
///
/// [RFC 9110 section 11.6.2]: https://www.rfc-editor.org/rfc/rfc9110#section-11.6.2
/// [RFC 7617 section 2]: https://www.rfc-editor.org/rfc/rfc7617#section-2
/// [RFC 6750 section 2.1]: https://www.rfc-editor.org/rfc/rfc6750#section-2.1
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct AuthorizationOwned<S> {
    value: FieldValue,
    scheme: PhantomData<S>,
}

/// Borrowed value for the `Authorization` header.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Authorization, AuthorizationView, Basic};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: AuthorizationView<'_, Basic> =
///     <Authorization<Basic> as SingleValueField>::decode_view(FieldValueRef::new(
///         b"Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==",
///     ))?;
/// assert_eq!(view.credentials(), b"QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AuthorizationView<'a, S> {
    value: FieldValueRef<'a>,
    credentials: &'a [u8],
    scheme: PhantomData<S>,
}

/// Reusable storage for decoded Basic credentials.
///
/// Existing credentials are zeroized before every extraction and when this
/// value is dropped. Only initialized credential storage is wiped; retained
/// spare capacity contains no credentials that were not wiped first. Capacity
/// is reused up to a configurable retention limit.
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Authorization, AuthorizationOwned, Basic, BasicCredentials};
///
/// let mut map = HeaderMap::new();
/// Authorization::<Basic>::insert(
///     &mut map,
///     AuthorizationOwned::<Basic>::basic(b"user", b"password")?,
/// )?;
/// let authorization = Authorization::<Basic>::view(&map)?.expect("authorization present");
/// let mut credentials = BasicCredentials::new();
/// let decoded = authorization.extract(&mut credentials)?;
/// assert_eq!(decoded.username(), b"user");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct BasicCredentials {
    bytes: Vec<u8>,
    username_end: usize,
    password_start: usize,
    retain_limit: usize,
    #[cfg(test)]
    zeroization_observer: Option<ZeroizationLog>,
}

impl<S> fmt::Debug for AuthorizationOwned<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizationOwned")
            .field("sensitive", &true)
            .finish_non_exhaustive()
    }
}

impl<S> fmt::Debug for AuthorizationView<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizationView")
            .field("sensitive", &true)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for BasicCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BasicCredentials").field("sensitive", &true).finish_non_exhaustive()
    }
}

impl Default for BasicCredentials {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: 'static> std::str::FromStr for AuthorizationOwned<S>
where
    Authorization<S>: SingleValueField<Owned = Self>,
{
    type Err = DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = FieldValue::from_str(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Authorization))?;
        <Authorization<S> as SingleValueField>::decode_owned(value)
    }
}

impl Drop for BasicCredentials {
    fn drop(&mut self) {
        self.zeroize_initialized();
    }
}

impl AuthorizationOwned<Bearer> {
    /// Constructs a Bearer authorization value.
    ///
    /// # Errors
    ///
    /// Returns an error when `token` is not valid Bearer token68 syntax.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::AuthorizationOwned::<http_headers::headers::Bearer>::bearer(
    ///     "abc.def",
    /// )?;
    /// assert_eq!(value.token()?, b"abc.def");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn bearer(token: impl AsRef<str>) -> Result<Self, DecodeError> {
        let token = token.as_ref();
        if !token68(token.as_bytes()) {
            return Err(super::invalid_syntax(&FieldName::Authorization));
        }
        build_authorization("Bearer", token.as_bytes())
    }

    /// Returns the Bearer token.
    /// # Errors
    ///
    /// Returns an error if the stored range and wire value disagree.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::AuthorizationOwned::<http_headers::headers::Bearer>::bearer(
    ///     "abc.def",
    /// )?;
    /// assert_eq!(value.token()?, b"abc.def");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn token(&self) -> Result<&[u8], DecodeError> {
        parse_scheme(self.value.as_field_value_ref(), &BEARER).map(|(_start, credentials)| credentials)
    }
}

impl AuthorizationOwned<Basic> {
    /// Constructs Basic credentials from arbitrary username and password bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the username contains `:` or encoded sizing
    /// overflows.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{AuthorizationOwned, Basic};
    ///
    /// let value = AuthorizationOwned::<Basic>::basic(b"Aladdin", b"open sesame")?;
    /// assert_eq!(
    ///     value.encoded_credentials()?,
    ///     b"QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn basic(username: impl AsRef<[u8]>, password: impl AsRef<[u8]>) -> Result<Self, DecodeError> {
        let username = username.as_ref();
        let password = password.as_ref();
        if username.contains(&b':') {
            return Err(super::invalid_syntax(&FieldName::Authorization));
        }
        let total_len = basic_wire_len(username.len(), password.len())?;
        let prefix = b"Basic ";
        let mut wire = Vec::with_capacity(total_len);
        wire.extend_from_slice(prefix);
        encode_basic_credentials(&mut wire, username, password);
        debug_assert_eq!(
            wire.len(),
            total_len,
            "Basic authorization wire length must match the precomputed capacity"
        );
        let mut value = super::value_from_bytes(&FieldName::Authorization, wire)?;
        value.set_sensitive(true);
        Ok(Self {
            value,
            scheme: PhantomData,
        })
    }

    /// Returns the base64-encoded credential bytes.
    /// # Errors
    ///
    /// Returns an error if the stored range and wire value disagree.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{AuthorizationOwned, Basic};
    ///
    /// let value = AuthorizationOwned::<Basic>::basic(b"Aladdin", b"open sesame")?;
    /// assert_eq!(
    ///     value.encoded_credentials()?,
    ///     b"QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn encoded_credentials(&self) -> Result<&[u8], DecodeError> {
        parse_scheme(self.value.as_field_value_ref(), &BASIC).map(|(_start, credentials)| credentials)
    }

    /// Decodes the username and password into reusable credential storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored authorization value is invalid.
    pub fn extract<'a>(&self, output: &'a mut BasicCredentials) -> Result<&'a BasicCredentials, DecodeError> {
        output.fill(self.encoded_credentials()?)
    }
}

impl<S> AuthorizationOwned<S> {
    /// Returns the complete sensitive field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{AuthorizationOwned, Bearer};
    ///
    /// let value = AuthorizationOwned::<Bearer>::bearer("abc.def")?;
    /// assert_eq!(value.as_field_value().as_bytes(), b"Bearer abc.def");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Returns reusable sensitive wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{AuthorizationOwned, Basic};
    ///
    /// let value = AuthorizationOwned::<Basic>::basic(b"Aladdin", b"open sesame")?;
    /// let field_value = value.into_field_value();
    /// assert_eq!(
    ///     field_value.as_bytes(),
    ///     b"Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

impl<S> From<AuthorizationOwned<S>> for FieldValue {
    #[inline]
    fn from(value: AuthorizationOwned<S>) -> Self {
        value.value
    }
}

impl<'a, S> AuthorizationView<'a, S> {
    /// Returns the encoded credential bytes after the authorization scheme.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{Authorization, Basic};
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = <Authorization<Basic> as SingleValueField>::decode_view(FieldValueRef::new(
    ///     b"Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==",
    /// ))?;
    /// assert_eq!(view.credentials(), b"QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn credentials(self) -> &'a [u8] {
        self.credentials
    }

    /// Returns the original field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{Authorization, Bearer};
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let value = FieldValueRef::new(b"Bearer abc.def");
    /// let view = <Authorization<Bearer> as SingleValueField>::decode_view(value)?;
    /// assert_eq!(view.as_field_value().as_bytes(), b"Bearer abc.def");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::wrong_self_convention,
        reason = "borrowed views are Copy and expose value-style accessors consistently"
    )]
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value.with_sensitive(true)
    }
}

impl AuthorizationView<'_, Basic> {
    /// Decodes the username and password into reusable credential storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored authorization value is invalid.
    pub fn extract<'a>(&self, output: &'a mut BasicCredentials) -> Result<&'a BasicCredentials, DecodeError> {
        output.fill(self.credentials)
    }
}

impl<'a> AuthorizationView<'a, Bearer> {
    /// Returns the borrowed Bearer token.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::AuthorizationOwned::<http_headers::headers::Bearer>::bearer(
    ///     "abc.def",
    /// )?;
    /// assert_eq!(value.token()?, b"abc.def");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn token(self) -> &'a [u8] {
        self.credentials
    }
}

impl BasicCredentials {
    /// Creates empty reusable credential storage.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_retain_limit(DEFAULT_CREDENTIAL_RETAIN_LIMIT)
    }

    /// Creates credential storage with a custom retained-capacity limit.
    #[must_use]
    pub const fn with_retain_limit(retain_limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            username_end: 0,
            password_start: 0,
            retain_limit,
            #[cfg(test)]
            zeroization_observer: None,
        }
    }

    fn fill(&mut self, encoded: &[u8]) -> Result<&Self, DecodeError> {
        self.clear();
        let result = (|| {
            STANDARD
                .decode_vec(encoded, &mut self.bytes)
                .map_err(|_invalid| super::invalid_syntax(&FieldName::Authorization))?;
            self.bytes
                .iter()
                .position(|byte| *byte == b':')
                .ok_or_else(|| super::invalid_syntax(&FieldName::Authorization))
        })();
        let colon = match result {
            Ok(colon) => colon,
            Err(error) => {
                self.clear();
                return Err(error);
            }
        };
        self.username_end = colon;
        self.password_start = colon + 1;
        Ok(self)
    }

    /// Returns the decoded username bytes.
    #[must_use]
    pub fn username(&self) -> &[u8] {
        &self.bytes[..self.username_end]
    }

    /// Returns the decoded password bytes.
    #[must_use]
    pub fn password(&self) -> &[u8] {
        &self.bytes[self.password_start..]
    }

    /// Zeroizes the decoded credentials and applies the retention limit.
    pub fn clear(&mut self) {
        self.zeroize_initialized();
        self.bytes.clear();
        self.username_end = 0;
        self.password_start = 0;
        if self.bytes.capacity() > self.retain_limit {
            self.bytes.shrink_to(self.retain_limit);
        }
    }

    /// Returns the currently allocated credential capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    // `decode_vec` extends `bytes` before writing, including on errors, so the
    // Vec length covers every initialized credential byte. Reuse always wipes
    // that range before clearing or reallocating, leaving spare capacity free
    // of historical credentials.
    fn zeroize_initialized(&mut self) {
        self.bytes.as_mut_slice().zeroize();
        #[cfg(test)]
        if let Some(observer) = &self.zeroization_observer {
            observer
                .lock()
                .unwrap()
                .push((self.bytes.len(), self.bytes.capacity(), self.bytes.clone()));
        }
    }
}

impl SingleValueField for Authorization<Bearer> {
    type View<'a> = AuthorizationView<'a, Bearer>;
    type Owned = AuthorizationOwned<Bearer>;

    fn name() -> &'static FieldName {
        &FieldName::Authorization
    }

    #[inline]
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let (_credential_start, credentials) = parse_scheme(value, &BEARER)?;
        if !token68(credentials) {
            return Err(super::invalid_syntax(&FieldName::Authorization));
        }
        Ok(AuthorizationView {
            value,
            credentials,
            scheme: PhantomData,
        })
    }

    #[expect(
        clippy::inline_always,
        reason = "measured: Criterion otherwise outlines this conversion while Callgrind inlines it"
    )]
    #[inline(always)]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        let (_credential_start, credentials) = parse_scheme(value.as_field_value_ref(), &BEARER)?;
        if !token68(credentials) {
            return Err(super::invalid_syntax(&FieldName::Authorization));
        }
        Ok(authorization_owned_from_value(value))
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

impl SingleValueField for Authorization<Basic> {
    type View<'a> = AuthorizationView<'a, Basic>;
    type Owned = AuthorizationOwned<Basic>;

    fn name() -> &'static FieldName {
        &FieldName::Authorization
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        let (_credential_start, credentials) = parse_scheme(value, &BASIC)?;
        validate_basic(credentials)?;
        Ok(AuthorizationView {
            value,
            credentials,
            scheme: PhantomData,
        })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        let (_credential_start, credentials) = parse_scheme(value.as_field_value_ref(), &BASIC)?;
        validate_basic(credentials)?;
        Ok(authorization_owned_from_value(value))
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

fn encode_basic_credentials(wire: &mut Vec<u8>, username: &[u8], password: &[u8]) {
    let mut encoder = EncoderWriter::new(wire, &STANDARD);
    encoder
        .write_all(username)
        .and_then(|()| encoder.write_all(b":"))
        .and_then(|()| encoder.write_all(password))
        .and_then(|()| encoder.finish().map(|_wire| ()))
        .expect("base64 encoding into a Vec cannot fail");
}

#[inline]
fn basic_wire_len(username_len: usize, password_len: usize) -> Result<usize, DecodeError> {
    let input_len = username_len
        .checked_add(1)
        .and_then(|length| length.checked_add(password_len))
        .ok_or_else(authorization_size_error)?;
    let encoded_len = base64::encoded_len(input_len, true).ok_or_else(authorization_size_error)?;
    let total_len = b"Basic ".len().checked_add(encoded_len).ok_or_else(authorization_size_error)?;
    Ok(total_len)
}

#[inline]
fn authorization_wire_len(scheme_len: usize, credentials_len: usize) -> Result<usize, DecodeError> {
    scheme_len
        .checked_add(1)
        .and_then(|length| length.checked_add(credentials_len))
        .ok_or_else(authorization_size_error)
}

#[cold]
fn authorization_size_error() -> DecodeError {
    DecodeError::new(&FieldName::Authorization, DecodeErrorKind::InvalidNumber)
}

fn authorization_owned_from_value<S>(value: FieldValue) -> AuthorizationOwned<S> {
    let mut value = value;
    value.set_sensitive(true);
    AuthorizationOwned {
        value,
        scheme: PhantomData,
    }
}

fn build_authorization(scheme: &str, credentials: &[u8]) -> Result<AuthorizationOwned<Bearer>, DecodeError> {
    let total_len = authorization_wire_len(scheme.len(), credentials.len())?;
    let mut wire = Vec::with_capacity(total_len);
    wire.extend_from_slice(scheme.as_bytes());
    wire.push(b' ');
    wire.extend_from_slice(credentials);
    let mut value = super::value_from_bytes(&FieldName::Authorization, wire)?;
    value.set_sensitive(true);
    Ok(AuthorizationOwned {
        value,
        scheme: PhantomData,
    })
}

/// A case-insensitive scheme prefix matched eight bytes at a time.
struct Scheme {
    name: &'static [u8],
    /// Lowercases the scheme bytes of a little-endian eight-byte load.
    lower: u64,
    /// Retains the scheme bytes plus the delimiting space.
    keep: u64,
    /// The lowercased scheme followed by the delimiting space.
    expected: u64,
}

impl Scheme {
    const fn new(name: &'static [u8]) -> Self {
        assert!(name.len() < 8, "scheme plus its space must fit in a word");
        let mut lower = [0_u8; 8];
        let mut keep = [0_u8; 8];
        let mut expected = [0_u8; 8];
        let mut index = 0;
        while index < name.len() {
            lower[index] = 0x20;
            keep[index] = 0xff;
            expected[index] = name[index];
            index += 1;
        }
        keep[index] = 0xff;
        expected[index] = b' ';
        Self {
            name,
            lower: u64::from_le_bytes(lower),
            keep: u64::from_le_bytes(keep),
            expected: u64::from_le_bytes(expected),
        }
    }
}

const BASIC: Scheme = Scheme::new(b"basic");
const BEARER: Scheme = Scheme::new(b"bearer");

/// Length at which the accelerated `token68` validator beats the local scalar
/// loop, matching the dispatch threshold of the acceleration crate.
const TOKEN68_SIMD_THRESHOLD: usize = 32;

/// Maps every byte to zero when it is `token68` data and `0xff` otherwise.
const TOKEN68_INVALID: [u8; 256] = {
    let mut table = [0xff_u8; 256];
    let mut index = 0;
    while index < 256 {
        #[expect(clippy::cast_possible_truncation, reason = "index is below 256")]
        let byte = index as u8;
        if matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'+' | b'/'
        ) {
            table[index] = 0;
        }
        index += 1;
    }
    table
};

/// Returns whether `bytes` is an RFC 9110 `token68` value.
///
/// Equivalent to [`validate::token68`], but short credentials skip the
/// dispatch preamble and validate through a branchless table scan.
fn token68(bytes: &[u8]) -> bool {
    if bytes.len() >= TOKEN68_SIMD_THRESHOLD {
        return token68_accelerated(bytes);
    }
    if all_token68_data(bytes) {
        return !bytes.is_empty();
    }
    token68_with_padding(bytes)
}

/// Keeps the vector implementation out of line so that short credentials
/// validate without its register pressure and stack frame.
#[inline(never)]
fn token68_accelerated(bytes: &[u8]) -> bool {
    validate::token68(bytes)
}

/// Validates values that carry trailing `=` padding.
#[inline(never)]
fn token68_with_padding(bytes: &[u8]) -> bool {
    let Some(last_data) = bytes.iter().rposition(|byte| *byte != b'=') else {
        return false;
    };
    all_token68_data(&bytes[..=last_data])
}

/// Returns whether every byte of `bytes` is `token68` data.
///
/// The scan walks eight-byte windows and finishes with a window aligned to the
/// end of the slice. That final window overlaps the previous one, which is
/// harmless because the scan only accumulates rejection bits.
#[expect(
    clippy::inline_always,
    reason = "measured: folding the short token68 scan into Bearer decoding saves 17 Ir"
)]
#[inline(always)]
fn all_token68_data(bytes: &[u8]) -> bool {
    let Some(head) = bytes.first_chunk::<8>() else {
        let mut invalid = 0_u8;
        for byte in bytes {
            invalid |= TOKEN68_INVALID[usize::from(*byte)];
        }
        return invalid == 0;
    };
    let mut invalid = token68_invalid_bits(head);
    let mut rest = &bytes[8..];
    while let Some((chunk, tail)) = rest.split_first_chunk::<8>() {
        invalid |= token68_invalid_bits(chunk);
        rest = tail;
    }
    if !rest.is_empty() {
        let tail = bytes.last_chunk::<8>().unwrap_or(head);
        invalid |= token68_invalid_bits(tail);
    }
    invalid == 0
}

#[inline]
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "copying the window costs more instructions than indexing it in place"
)]
fn token68_invalid_bits(chunk: &[u8; 8]) -> u8 {
    let mut invalid = 0_u8;
    for byte in chunk {
        invalid |= TOKEN68_INVALID[usize::from(*byte)];
    }
    invalid
}

fn parse_scheme<'a>(value: FieldValueRef<'a>, scheme: &Scheme) -> Result<(usize, &'a [u8]), DecodeError> {
    let bytes = value.as_bytes();
    let Some(head) = bytes.first_chunk::<8>() else {
        return parse_short_scheme(bytes, scheme);
    };
    if (u64::from_le_bytes(*head) | scheme.lower) & scheme.keep != scheme.expected {
        return Err(super::invalid_syntax(&FieldName::Authorization));
    }
    credentials_at(bytes, scheme.name.len() + 1)
}

/// Matches schemes in values too short for the eight-byte load.
#[cold]
fn parse_short_scheme<'a>(bytes: &'a [u8], scheme: &Scheme) -> Result<(usize, &'a [u8]), DecodeError> {
    let scheme_length = scheme.name.len();
    if bytes.len() <= scheme_length || bytes[scheme_length] != b' ' || !bytes[..scheme_length].eq_ignore_ascii_case(scheme.name) {
        return Err(super::invalid_syntax(&FieldName::Authorization));
    }
    credentials_at(bytes, scheme_length + 1)
}

/// Skips optional whitespace at `start` and returns the credential bytes.
#[inline]
fn credentials_at(bytes: &[u8], start: usize) -> Result<(usize, &[u8]), DecodeError> {
    let rest = &bytes[start..];
    let offset = rest
        .iter()
        .position(|byte| *byte != b' ')
        .ok_or_else(|| super::invalid_syntax(&FieldName::Authorization))?;
    Ok((start + offset, &rest[offset..]))
}

/// Sentinel bit marking a byte that is not part of the base64 alphabet.
const NOT_SEXTET: u8 = 0x80;

/// Maps every byte to its base64 sextet, or to [`NOT_SEXTET`].
const BASE64_SEXTET: [u8; 256] = {
    let mut table = [NOT_SEXTET; 256];
    let mut index = 0;
    while index < 256 {
        #[expect(clippy::cast_possible_truncation, reason = "index is below 256")]
        let byte = index as u8;
        table[index] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => NOT_SEXTET,
        };
        index += 1;
    }
    table
};

fn validate_basic(encoded: &[u8]) -> Result<(), DecodeError> {
    if encoded.is_empty() || !encoded.len().is_multiple_of(4) {
        return Err(super::invalid_syntax(&FieldName::Authorization));
    }
    let (body, last) = encoded.split_at(encoded.len() - 4);
    let mut colons = 0_u32;
    for chunk in body.chunks_exact(4) {
        let &[first, second, third, fourth] = <&[u8; 4]>::try_from(chunk).expect("chunks_exact(4) always yields four bytes");
        let first = BASE64_SEXTET[usize::from(first)];
        let second = BASE64_SEXTET[usize::from(second)];
        let third = BASE64_SEXTET[usize::from(third)];
        let fourth = BASE64_SEXTET[usize::from(fourth)];
        if (first | second | third | fourth) & NOT_SEXTET != 0 {
            return Err(super::invalid_syntax(&FieldName::Authorization));
        }
        let group = (u32::from(first) << 18) | (u32::from(second) << 12) | (u32::from(third) << 6) | u32::from(fourth);
        colons |= colon_marks(group);
    }
    validate_basic_last(last, colons != 0)
}

/// Marks which of the three bytes packed into `group` decode to `:`.
#[inline]
const fn colon_marks(group: u32) -> u32 {
    let difference = group ^ 0x003a_3a3a;
    difference.wrapping_sub(0x0001_0101) & !difference & 0x0080_8080
}

/// Validates the final base64 quantum, which alone may carry `=` padding.
fn validate_basic_last(chunk: &[u8], mut has_colon: bool) -> Result<(), DecodeError> {
    let &[first, second, third, fourth] = <&[u8; 4]>::try_from(chunk).expect("the final quantum always holds four bytes");
    let first_sextet = BASE64_SEXTET[usize::from(first)];
    let second_sextet = BASE64_SEXTET[usize::from(second)];
    if (first_sextet | second_sextet) & NOT_SEXTET != 0 {
        return Err(super::invalid_syntax(&FieldName::Authorization));
    }
    has_colon |= (first_sextet << 2) | (second_sextet >> 4) == b':';

    let third_sextet = BASE64_SEXTET[usize::from(third)];
    if third_sextet & NOT_SEXTET != 0 {
        if third != b'=' || fourth != b'=' || second_sextet & 0x0f != 0 {
            return Err(super::invalid_syntax(&FieldName::Authorization));
        }
    } else {
        has_colon |= (second_sextet << 4) | (third_sextet >> 2) == b':';

        let fourth_sextet = BASE64_SEXTET[usize::from(fourth)];
        if fourth_sextet & NOT_SEXTET != 0 {
            if fourth != b'=' || third_sextet & 0x03 != 0 {
                return Err(super::invalid_syntax(&FieldName::Authorization));
            }
        } else {
            has_colon |= (third_sextet << 6) | fourth_sextet == b':';
        }
    }

    if has_colon {
        Ok(())
    } else {
        Err(super::invalid_syntax(&FieldName::Authorization))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use std::sync::{Arc, Mutex};

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    use super::{
        Authorization, AuthorizationOwned, AuthorizationView, BASIC, BEARER, Basic, BasicCredentials, Bearer, Scheme,
        TOKEN68_SIMD_THRESHOLD, all_token68_data, authorization_owned_from_value, authorization_wire_len, basic_wire_len,
        build_authorization, credentials_at, parse_scheme, parse_short_scheme, token68, token68_invalid_bits, token68_with_padding,
        validate_basic, validate_basic_last,
    };
    use crate::sink::FieldSink;
    use crate::source::FieldSource;
    use crate::{DecodeErrorKind, FieldName, FieldValue, SingleValueField, TestSink};

    type ZeroizationObserver = Arc<Mutex<Vec<(usize, usize, Vec<u8>)>>>;

    fn observed_credentials() -> (BasicCredentials, ZeroizationObserver) {
        let observer = Arc::new(Mutex::new(Vec::new()));
        let mut credentials = BasicCredentials::new();
        credentials.zeroization_observer = Some(Arc::clone(&observer));
        (credentials, observer)
    }

    fn assert_zeroized(snapshot: &(usize, usize, Vec<u8>), expected_len: usize, lifecycle: &str) {
        assert_eq!(snapshot.0, expected_len, "{lifecycle} observed an unexpected initialized length");
        assert!(snapshot.0 <= snapshot.1, "{lifecycle} initialized length exceeds capacity");
        let snapshot = &snapshot.2;
        assert_eq!(
            snapshot.len(),
            expected_len,
            "{lifecycle} must cover all initialized credential bytes"
        );
        assert!(snapshot.iter().all(|byte| *byte == 0), "{lifecycle} left credential bytes");
    }

    #[test]
    fn basic_validator_matches_the_decoder_and_colon_oracle() {
        let valid = b"dXNlcjpwYXNzd29yZA==";
        for index in 0..valid.len() - 4 {
            for byte in u8::MIN..=u8::MAX {
                let mut candidate = valid.to_vec();
                candidate[index] = byte;
                let oracle = STANDARD.decode(&candidate).is_ok_and(|decoded| decoded.contains(&b':'));
                assert_eq!(validate_basic(&candidate).is_ok(), oracle, "index {index}, byte {byte:#04x}");
            }
        }
    }

    #[test]
    fn bearer_construction_parsing_and_views_cover_token68_paths() {
        for token in ["a", "abc.def", "YWJj=", "abcdefghijklmnopqrstuvwxyz0123456789"] {
            let owned = AuthorizationOwned::<Bearer>::bearer(token).expect("valid bearer token");
            assert_eq!(owned.token(), Ok(token.as_bytes()));
            assert!(owned.as_field_value().is_sensitive());
            assert!(format!("{owned:?}").contains("sensitive"));

            let parsed = owned
                .as_field_value()
                .try_as_str()
                .expect("ASCII authorization")
                .parse::<AuthorizationOwned<Bearer>>()
                .expect("FromStr accepts constructed value");
            assert_eq!(parsed.token(), Ok(token.as_bytes()));

            let mut table = TestSink::new();
            Authorization::<Bearer>::insert(&mut table, owned).expect("table accepts bearer");
            let view = Authorization::<Bearer>::view(&table).expect("valid bearer").expect("present");
            assert_eq!(view.token(), token.as_bytes());
            assert_eq!(view.credentials(), token.as_bytes());
            assert_eq!(
                view.as_field_value().as_bytes(),
                table
                    .lines(&FieldName::Authorization)
                    .expect("authorization stored")
                    .repeated()
                    .next()
                    .expect("one value")
                    .as_bytes()
            );
            assert!(format!("{view:?}").contains("sensitive"));
            assert_eq!(
                Authorization::<Bearer>::owned(&table)
                    .expect("valid owned bearer")
                    .expect("present")
                    .token(),
                Ok(token.as_bytes())
            );
            table.remove_values(&FieldName::Authorization);
            assert!(Authorization::<Bearer>::view(&table).expect("absence is valid").is_none());
        }

        for token in ["", "=", "==", "ab=c", "ab c"] {
            assert!(AuthorizationOwned::<Bearer>::bearer(token).is_err(), "{token:?}");
        }
        assert!(token68(b"short"));
        assert!(!token68(b""));
        assert!(token68(b"data=="));
        assert!(!token68(b"===="));
        assert!(!token68(b"data=x"));
        assert!(token68(&[b'a'; TOKEN68_SIMD_THRESHOLD]));
        assert!(!token68(&[b' '; TOKEN68_SIMD_THRESHOLD]));
        assert!(all_token68_data(b"1234567"));
        assert!(all_token68_data(b"12345678"));
        assert!(all_token68_data(b"123456789"));
        assert!(all_token68_data(b"1234567890123456"));
        assert!(!all_token68_data(b"1234567 "));
        assert_eq!(token68_invalid_bits(b"12345678"), 0);
        assert_ne!(token68_invalid_bits(b"1234567 "), 0);
    }

    #[test]
    fn basic_credentials_cover_construction_extraction_errors_and_retention() {
        assert!(AuthorizationOwned::<Basic>::basic(b"user:name", b"secret").is_err());
        let owned = AuthorizationOwned::<Basic>::basic(b"user", b"password").expect("valid credentials");
        assert_eq!(owned.encoded_credentials(), Ok(b"dXNlcjpwYXNzd29yZA==".as_slice()));
        assert!(owned.as_field_value().is_sensitive());
        assert!(format!("{owned:?}").contains("sensitive"));
        let parsed = owned
            .as_field_value()
            .try_as_str()
            .expect("ASCII authorization")
            .parse::<AuthorizationOwned<Basic>>()
            .expect("basic FromStr");
        assert_eq!(parsed.encoded_credentials(), owned.encoded_credentials());
        assert!(parsed.into_field_value().is_sensitive());

        let mut credentials = BasicCredentials::default();
        let extracted = owned.extract(&mut credentials).expect("valid base64");
        assert_eq!(extracted.username(), b"user");
        assert_eq!(extracted.password(), b"password");
        assert!(format!("{extracted:?}").contains("sensitive"));
        assert!(credentials.capacity() >= b"user:password".len());
        credentials.clear();
        assert_eq!(credentials.username(), b"");
        assert_eq!(credentials.password(), b"");

        let mut table = TestSink::new();
        Authorization::<Basic>::insert(&mut table, owned).expect("table accepts basic");
        let view = Authorization::<Basic>::view(&table).expect("valid basic").expect("present");
        assert_eq!(view.credentials(), b"dXNlcjpwYXNzd29yZA==");
        let extracted = view.extract(&mut credentials).expect("valid view credentials");
        assert_eq!(extracted.username(), b"user");
        assert_eq!(extracted.password(), b"password");
        assert!(Authorization::<Basic>::owned(&table).expect("valid owned basic").is_some());

        let mut bounded = BasicCredentials::with_retain_limit(0);
        AuthorizationOwned::<Basic>::basic([b'a'; 256], b"")
            .expect("valid large credentials")
            .extract(&mut bounded)
            .expect("valid extraction");
        assert!(bounded.capacity() >= 257);
        bounded.clear();
        assert_eq!(bounded.capacity(), 0);

        for encoded in [b"!!!!".as_slice(), b"bm9jb2xvbg=="] {
            assert!(bounded.fill(encoded).is_err());
            assert_eq!(bounded.username(), b"");
            assert_eq!(bounded.password(), b"");
        }
    }

    #[test]
    fn basic_credential_capacity_tracks_decoded_output_instead_of_encoded_input() {
        let username = [b'a'; 4_096];
        let authorization = AuthorizationOwned::<Basic>::basic(username, b"").unwrap();
        let encoded_len = authorization.encoded_credentials().unwrap().len();
        let mut credentials = BasicCredentials::new();
        authorization.extract(&mut credentials).unwrap();

        assert!(credentials.capacity() > username.len());
        assert!(credentials.capacity() < encoded_len);
    }

    #[test]
    fn basic_credential_clear_zeroizes_the_initialized_range() {
        let (mut credentials, observer) = observed_credentials();
        AuthorizationOwned::<Basic>::basic([b'a'; 1_024], [b'b'; 1_024])
            .unwrap()
            .extract(&mut credentials)
            .unwrap();
        let credential_len = credentials.username().len() + 1 + credentials.password().len();
        observer.lock().unwrap().clear();

        credentials.clear();

        let snapshots = observer.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_zeroized(&snapshots[0], credential_len, "clear");
    }

    #[test]
    fn failed_basic_credential_decode_zeroizes_old_and_partial_output() {
        let (mut credentials, observer) = observed_credentials();
        AuthorizationOwned::<Basic>::basic([b'a'; 1_024], b"secret")
            .unwrap()
            .extract(&mut credentials)
            .unwrap();
        let old_len = credentials.username().len() + 1 + credentials.password().len();
        observer.lock().unwrap().clear();

        let malformed = [STANDARD.encode([b'x'; 1_024]), "!".into()].concat();
        assert!(credentials.fill(malformed.as_bytes()).is_err());

        let snapshots = observer.lock().unwrap();
        assert_eq!(snapshots.len(), 2, "failed fill must erase old storage and decoder output");
        assert_zeroized(&snapshots[0], old_len, "failed decode old credentials");
        assert!(!snapshots[1].2.is_empty());
        assert_zeroized(&snapshots[1], snapshots[1].0, "failed decode partial output");
    }

    #[test]
    fn failed_basic_credential_parse_zeroizes_decoded_output() {
        let (mut credentials, observer) = observed_credentials();

        assert!(credentials.fill(b"bm9jb2xvbg==").is_err());

        let snapshots = observer.lock().unwrap();
        assert_eq!(snapshots.len(), 2, "failed parse must erase decoded output");
        assert_zeroized(&snapshots[0], 0, "failed parse empty input");
        assert_zeroized(&snapshots[1], b"nocolon".len(), "failed parse decoded output");
    }

    #[test]
    fn basic_credential_reuse_bounds_later_wipes_to_the_short_output() {
        let (mut credentials, observer) = observed_credentials();
        AuthorizationOwned::<Basic>::basic([b'a'; 2_048], [b'b'; 2_048])
            .unwrap()
            .extract(&mut credentials)
            .unwrap();
        let old_len = credentials.username().len() + 1 + credentials.password().len();
        let old_capacity = credentials.capacity();
        observer.lock().unwrap().clear();

        credentials.fill(b"dTpw").unwrap();

        {
            let snapshots = observer.lock().unwrap();
            assert_eq!(snapshots.len(), 1);
            assert_zeroized(&snapshots[0], old_len, "reuse");
            assert!(snapshots[0].0 < old_capacity, "reuse must not wipe spare capacity");
        }
        assert_eq!(credentials.username(), b"u");
        assert_eq!(credentials.password(), b"p");

        observer.lock().unwrap().clear();
        credentials.clear();
        let snapshots = observer.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_zeroized(&snapshots[0], 3, "short reuse");
        assert_eq!(snapshots[0].1, old_capacity, "reuse must retain the high-water allocation");
    }

    #[test]
    fn basic_credential_growth_wipes_before_reallocation_and_on_drop() {
        let observer = {
            let (mut credentials, observer) = observed_credentials();
            credentials.fill(b"dTpw").unwrap();
            let old_capacity = credentials.capacity();
            observer.lock().unwrap().clear();

            let decoded = [b"expanded:".as_slice(), &[b'x'; 8_192]].concat();
            let encoded = STANDARD.encode(&decoded);
            credentials.fill(encoded.as_bytes()).unwrap();
            assert!(credentials.capacity() > old_capacity, "larger credentials must reallocate");

            {
                let snapshots = observer.lock().unwrap();
                assert_eq!(snapshots.len(), 1);
                assert_zeroized(&snapshots[0], 3, "pre-reallocation");
                assert_eq!(snapshots[0].1, old_capacity);
            }
            observer.lock().unwrap().clear();
            observer
        };

        let snapshots = observer.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_zeroized(&snapshots[0], b"expanded:".len() + 8_192, "post-reallocation drop");
    }

    #[test]
    fn dropping_basic_credentials_zeroizes_the_initialized_range() {
        let observer = {
            let (mut credentials, observer) = observed_credentials();
            AuthorizationOwned::<Basic>::basic([b'a'; 1_024], b"secret")
                .unwrap()
                .extract(&mut credentials)
                .unwrap();
            observer.lock().unwrap().clear();
            observer
        };

        let snapshots = observer.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].2.is_empty());
        assert_zeroized(&snapshots[0], snapshots[0].0, "drop");
    }

    #[test]
    fn scheme_and_basic_quantum_validation_cover_boundary_cases() {
        let short = FieldValue::from_static("Basic X");
        assert_eq!(
            parse_scheme(short.as_field_value_ref(), &BASIC).expect("short scheme matches").1,
            b"X"
        );
        for (wire, scheme) in [("Basic", &BASIC), ("Basic ", &BASIC), ("Bearer", &BEARER), ("Digest x", &BASIC)] {
            assert!(
                parse_scheme(FieldValue::from_str(wire).expect("legal field value").as_field_value_ref(), scheme).is_err(),
                "{wire}"
            );
        }

        for valid in [b"Og==".as_slice(), b"YTo=", b"YWI6", b"YWJjOg=="] {
            assert!(validate_basic(valid).is_ok(), "{valid:?}");
        }
        for invalid in [b"".as_slice(), b"abc", b"Oh==", b"YTp=", b"YWJj", b"YWJj====", b"!!!!"] {
            assert!(validate_basic(invalid).is_err(), "{invalid:?}");
        }

        let malformed = FieldValue::from_static("Basic bm9jb2xvbg==");
        assert_eq!(
            <Authorization<Basic> as SingleValueField>::decode_owned(malformed)
                .expect_err("decoded credentials require a colon")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        let malformed = FieldValue::from_static("Bearer ab=c");
        assert!(<Authorization<Bearer> as SingleValueField>::decode_view(malformed.as_field_value_ref()).is_err());
        assert!(<Authorization<Bearer> as SingleValueField>::decode_owned(malformed).is_err());
        assert!("\n".parse::<AuthorizationOwned<Basic>>().is_err());
        assert!("\n".parse::<AuthorizationOwned<Bearer>>().is_err());

        let bearer = FieldValue::from_static("Bearer abc");
        let bearer_owned = <Authorization<Bearer> as SingleValueField>::decode_owned(bearer.clone()).expect("valid owned bearer");
        assert_eq!(<Authorization<Bearer> as SingleValueField>::as_field_value(&bearer_owned), &bearer);
        let basic = FieldValue::from_static("Basic dTpw");
        let basic_view = <Authorization<Basic> as SingleValueField>::decode_view(basic.as_field_value_ref()).expect("valid borrowed basic");
        assert_eq!(basic_view.credentials(), b"dTpw");
        let basic_owned = <Authorization<Basic> as SingleValueField>::decode_owned(basic.clone()).expect("valid owned basic");
        assert_eq!(<Authorization<Basic> as SingleValueField>::as_field_value(&basic_owned), &basic);
    }

    #[test]
    fn private_scheme_and_base64_helpers_run_directly() {
        let runtime_name = std::hint::black_box(b"basic".as_slice());
        let runtime_scheme = Scheme::new(runtime_name);
        assert_eq!(runtime_scheme.name, b"basic");
        assert_eq!(
            parse_short_scheme(b"BaSiC credential", &runtime_scheme).expect("runtime scheme").1,
            b"credential"
        );
        assert_eq!(
            credentials_at(b"Basic   credential", 6)
                .expect("credentials after optional spaces")
                .1,
            b"credential"
        );
        assert!(credentials_at(b"Basic   ", 6).is_err());

        assert!(token68_with_padding(b"abc=="));
        assert!(!token68_with_padding(b"==="));
        assert!(!token68_with_padding(b"ab=!="));

        assert!(validate_basic_last(b"Og==", false).is_ok());
        assert!(validate_basic_last(b"YTo=", false).is_ok());
        assert!(validate_basic_last(b"YWI6", false).is_ok());
        assert!(validate_basic_last(b"YQ==", true).is_ok());
        assert!(validate_basic_last(b"YQ=A", false).is_err());
        assert!(validate_basic_last(b"YWF=", false).is_err());
    }

    #[test]
    fn private_owned_paths_and_credential_reuse_clear_failures() {
        let custom = build_authorization("Custom", b"credential").expect("valid wire value");
        assert_eq!(custom.as_field_value().as_bytes(), b"Custom credential");
        assert!(custom.as_field_value().is_sensitive());
        assert!(custom.token().is_err());

        let wrapped = authorization_owned_from_value::<Bearer>(FieldValue::from_static("Bearer x"));
        assert_eq!(wrapped.token(), Ok(b"x".as_slice()));
        let raw = <Authorization<Bearer> as SingleValueField>::into_field_value(wrapped);
        assert!(raw.is_sensitive());

        let empty = AuthorizationOwned::<Basic>::basic(b"", b"").expect("empty username and password");
        let mut credentials = BasicCredentials::new();
        let decoded = empty.extract(&mut credentials).expect("colon-only credentials");
        assert_eq!(decoded.username(), b"");
        assert_eq!(decoded.password(), b"");

        let binary = AuthorizationOwned::<Basic>::basic(b"user", [0, 0xff]).expect("basic credentials accept arbitrary password bytes");
        let decoded = binary.extract(&mut credentials).expect("binary password decodes");
        assert_eq!(decoded.username(), b"user");
        assert_eq!(decoded.password(), [0, 0xff]);
        let retained_capacity = credentials.capacity();
        assert!(credentials.fill(b"!!!!").is_err());
        assert_eq!(credentials.username(), b"");
        assert_eq!(credentials.password(), b"");
        assert_eq!(credentials.capacity(), retained_capacity);

        let malformed = AuthorizationOwned::<Basic> {
            value: FieldValue::from_static("Bearer x"),
            scheme: std::marker::PhantomData,
        };
        assert!(malformed.encoded_credentials().is_err());
        assert!(malformed.extract(&mut credentials).is_err());

        let direct = FieldValue::from_static("Bearer direct");
        let view = AuthorizationView::<Bearer> {
            value: direct.as_field_value_ref(),
            credentials: b"direct",
            scheme: std::marker::PhantomData,
        };
        assert_eq!(view.token(), b"direct");
    }

    #[test]
    fn wire_length_helpers_cover_boundaries_without_allocating() {
        assert_eq!(authorization_wire_len(6, 5), Ok(12));
        assert!(authorization_wire_len(usize::MAX, 0).is_err());
        assert!(authorization_wire_len(usize::MAX - 1, 1).is_err());

        assert_eq!(basic_wire_len(0, 0), Ok(10));
        assert!(basic_wire_len(usize::MAX, 0).is_err());
        assert!(basic_wire_len(usize::MAX - 1, 1).is_err());
        assert!(basic_wire_len(usize::MAX / 4 * 3, 0).is_err());
    }
}
