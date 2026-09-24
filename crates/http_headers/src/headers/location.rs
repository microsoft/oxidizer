// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Validated URI-reference support for the `Location` header.

use std::hash::{Hash, Hasher};
use std::{fmt, str};

use fluent_uri::Uri;

use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

mod component;
mod construction;
mod metadata;
mod uri_authority;
mod uri_reference;

use metadata::ComponentRanges;
pub use uri_authority::UriAuthority;
pub use uri_reference::UriReference;

/// Defines the `Location` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.2.2](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.2)
/// using the URI-reference grammar from
/// [RFC 3986 section 4](https://www.rfc-editor.org/rfc/rfc3986#section-4).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{Location, LocationOwned};
///
/// let mut map = HeaderMap::new();
/// Location::insert(&mut map, LocationOwned::try_from("/next")?)?;
/// assert!(Location::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct Location {
    _private: (),
}

/// Owned value for the `Location` header.
///
/// # Specification
///
/// The field is defined by [RFC 9110 section 10.2.2] and its URI-reference
/// grammar by [RFC 3986 section 4].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::LocationOwned::try_from("../people?tab=1#profile")?;
/// assert_eq!(value.as_str()?, "../people?tab=1#profile");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Location: https://example.com/people` is absolute,
/// `Location: /accounts/12345` is relative, and `Location: #profile` is a
/// fragment-only reference. An empty field value is also a valid reference.
///
/// [RFC 9110 section 10.2.2]: https://www.rfc-editor.org/rfc/rfc9110#section-10.2.2
/// [RFC 3986 section 4]: https://www.rfc-editor.org/rfc/rfc3986#section-4
#[derive(Clone)]
pub struct LocationOwned {
    value: FieldValue,
    component_ranges: ComponentRanges,
    normalized: Option<String>,
}

/// Borrowed value for the `Location` header.
///
/// Ordinary views borrow without allocation. A relaxed value containing
/// backslashes also retains an owned, slash-normalized semantic spelling.
/// Cloning such a view clones that buffer; raw access always borrows the
/// original source and structured access borrows this view.
#[derive(Clone)]
/// # Examples
///
/// ```
/// use http_headers::headers::{Location, LocationView};
/// use http_headers::{FieldValue, SingleValueField};
///
/// let field = FieldValue::from_static("/next");
/// let view: LocationView<'_> =
///     <Location as SingleValueField>::decode_view(field.as_field_value_ref())?;
/// assert_eq!(view.as_str()?, "/next");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct LocationView<'a> {
    text: &'a str,
    component_ranges: ComponentRanges,
    normalized: Option<String>,
}

impl fmt::Debug for LocationOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocationOwned").field("redacted", &true).finish_non_exhaustive()
    }
}

impl fmt::Debug for LocationView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocationView").field("redacted", &true).finish_non_exhaustive()
    }
}

impl LocationOwned {
    /// Constructs a URI-reference from validated authority and encoded components.
    ///
    /// Components are not percent-encoded automatically. A missing component is
    /// distinct from `Some("")`. The authority must already satisfy its grammar,
    /// while the remaining components and their contextual constraints are
    /// validated here. The assembled reference is not parsed again.
    ///
    /// # Errors
    ///
    /// Returns [`crate::DecodeErrorKind::InvalidSyntax`] for invalid components
    /// or combinations: an authority requires an empty or slash-prefixed path;
    /// without an authority the path cannot start with `//`; without a scheme,
    /// a relative path's first segment cannot contain a colon.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::{LocationOwned, UriAuthority};
    ///
    /// let authority = UriAuthority::new(None, "example.com", Some("443"))?;
    /// let location =
    ///     LocationOwned::from_components(Some("https"), Some(authority), "/a%2Fb", Some(""), None)?;
    /// assert_eq!(location.as_str()?, "https://example.com:443/a%2Fb?");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn from_components(
        scheme: Option<&str>,
        authority: Option<UriAuthority<'_>>,
        path: &str,
        query: Option<&str>,
        fragment: Option<&str>,
    ) -> Result<Self, DecodeError> {
        construction::from_components(scheme, authority, path, query, fragment)
    }

    /// Returns retained URI components without repeating URI grammar validation.
    ///
    /// In relaxed mode these describe the backslash-normalized spelling, while
    /// [`Self::as_str`] and forwarding retain the original wire spelling.
    #[inline]
    #[must_use]
    pub fn uri_reference(&self) -> UriReference<'_> {
        UriReference::from_component_ranges(self.semantic_text(), &self.component_ranges)
    }

    fn semantic_text(&self) -> &str {
        self.normalized.as_deref().unwrap_or_else(|| {
            self.as_str()
                .expect("all Location constructors validate UTF-8 and storage is immutable")
        })
    }

    /// Whether relaxed decoding replaced backslashes in the semantic spelling.
    #[inline]
    #[must_use]
    pub const fn was_normalized(&self) -> bool {
        self.normalized.is_some()
    }

    /// Returns the URI-reference bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::LocationOwned;
    ///
    /// let value = LocationOwned::try_from("/next")?;
    /// assert_eq!(value.as_bytes(), b"/next");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(&self) -> &[u8] {
        self.value.as_bytes()
    }

    /// Returns the URI-reference as UTF-8.
    /// # Errors
    ///
    /// Returns an error if the stored wire value is unexpectedly non-UTF-8.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::LocationOwned::try_from("/next")?;
    /// assert_eq!(value.as_str()?, "/next");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_str(&self) -> Result<&str, DecodeError> {
        str::from_utf8(self.value.as_bytes()).map_err(|_invalid| invalid())
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::LocationOwned;
    ///
    /// let value = LocationOwned::try_from("https://example.com/people")?;
    /// let field = value.into_field_value();
    /// assert_eq!(field.as_bytes(), b"https://example.com/people");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::shared::impl_field_value_conversion!(LocationOwned, |value| value.value);

impl<'a> LocationView<'a> {
    /// Returns retained components, borrowing any normalized backing from this view.
    ///
    /// Ordinary decoding borrows the source without allocation. Relaxed
    /// backslash normalization retains one owned semantic buffer; reading these
    /// components neither allocates nor normalizes again.
    #[inline]
    #[must_use]
    pub fn uri_reference(&self) -> UriReference<'_> {
        UriReference::from_component_ranges(self.normalized.as_deref().unwrap_or(self.text), &self.component_ranges)
    }

    /// Whether relaxed decoding replaced backslashes in the semantic spelling.
    #[inline]
    #[must_use]
    pub const fn was_normalized(&self) -> bool {
        self.normalized.is_some()
    }

    /// Returns the URI-reference bytes.
    #[must_use]
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::Location;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let field = FieldValue::from_static("../people?tab=1#profile");
    /// let view = <Location as SingleValueField>::decode_view(field.as_field_value_ref())?;
    /// assert_eq!(view.as_bytes(), b"../people?tab=1#profile");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn as_bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    /// Returns the URI-reference as UTF-8.
    ///
    /// # Errors
    ///
    /// This validated view always returns `Ok`; the fallible signature is
    /// retained for API compatibility with the owned form.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::LocationOwned::try_from("/next")?;
    /// assert_eq!(value.as_str()?, "/next");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fallible signature intentionally matches the owned representation"
    )]
    pub const fn as_str(&self) -> Result<&'a str, DecodeError> {
        Ok(self.text)
    }
}

impl SingleValueField for Location {
    type View<'a> = LocationView<'a>;
    type Owned = LocationOwned;

    fn name() -> &'static FieldName {
        &FieldName::Location
    }

    #[inline]
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        validate(value.as_bytes())
    }

    #[inline]
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        LocationOwned::try_from(value)
    }

    fn decode_view_with(value: FieldValueRef<'_>, mode: crate::DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        validate_with(value.as_bytes(), mode)
    }

    fn decode_owned_with(mut value: FieldValue, mode: crate::DecodeMode) -> Result<Self::Owned, DecodeError> {
        let view = validate_with(value.as_bytes(), mode)?;
        let component_ranges = view.component_ranges;
        let normalized = view.normalized;
        value.set_sensitive(true);
        Ok(LocationOwned {
            value,
            component_ranges,
            normalized,
        })
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

impl TryFrom<&str> for LocationOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut value = FieldValue::from_str(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))?;
        value.set_sensitive(true);
        Self::try_from(value)
    }
}

impl TryFrom<String> for LocationOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut value = FieldValue::try_from(value).map_err(|_invalid| super::invalid_syntax(&FieldName::Location))?;
        value.set_sensitive(true);
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for LocationOwned {
    type Error = DecodeError;

    fn try_from(mut value: FieldValue) -> Result<Self, Self::Error> {
        let component_ranges = validate(value.as_bytes())?.component_ranges;
        value.set_sensitive(true);
        Ok(Self {
            value,
            component_ranges,
            normalized: None,
        })
    }
}

#[inline]
fn validate(bytes: &[u8]) -> Result<LocationView<'_>, DecodeError> {
    if let Some(text) = is_simple_reference(bytes) {
        return Ok(LocationView {
            text,
            component_ranges: ComponentRanges::from_simple(text),
            normalized: None,
        });
    }
    let text = str::from_utf8(bytes).map_err(|_invalid| invalid())?;
    let component_ranges = validate_general_reference(text)?;
    Ok(LocationView {
        text,
        component_ranges,
        normalized: None,
    })
}

fn validate_with(bytes: &[u8], mode: crate::DecodeMode) -> Result<LocationView<'_>, DecodeError> {
    if mode == crate::DecodeMode::Strict {
        return validate(bytes);
    }
    if let Some(text) = is_simple_reference(bytes) {
        return Ok(LocationView {
            text,
            component_ranges: ComponentRanges::from_simple(text),
            normalized: None,
        });
    }
    let text = str::from_utf8(bytes).map_err(|_invalid| invalid())?;
    if let Ok(component_ranges) = validate_general_reference(text) {
        return Ok(LocationView {
            text,
            component_ranges,
            normalized: None,
        });
    }
    if !text.contains('\\') {
        return Err(invalid());
    }
    let normalized = text.replace('\\', "/");
    let component_ranges = validate_general_reference(&normalized)?;
    Ok(LocationView {
        text,
        component_ranges,
        normalized: Some(normalized),
    })
}

/// Validates everything the origin-relative scanner declines to recognize.
///
/// The recognized subset already covers the shapes a redirect normally
/// carries, so keeping the RFC 3986 parse cold and behind a call leaves
/// [`validate`] small enough to fold into its callers without changing
/// validation semantics.
#[cold]
#[inline(never)]
fn validate_general_reference(value: &str) -> Result<ComponentRanges, DecodeError> {
    validate_general_reference_len(value.len())?;
    Uri::parse(value)
        .map(|parsed| ComponentRanges::from_parsed(&parsed))
        .map_err(|_invalid| invalid())
}

#[inline]
fn validate_general_reference_len(length: usize) -> Result<(), DecodeError> {
    if i32::try_from(length).is_ok() { Ok(()) } else { Err(invalid()) }
}

/// Accepts the references that need no RFC 3986 parse.
///
/// The scanner recognizes a `path-absolute` with an optional query and one
/// optional fragment written only with `unreserved`, `sub-delims`, `:`, `@`,
/// `/`, and `?`, and the same shape behind a `scheme "://" host [":" port]`
/// prefix. Anything else, including every percent escape, userinfo, and
/// IP-literal, returns `false` and is handed to the full parser unchanged.
fn is_simple_reference(bytes: &[u8]) -> Option<&str> {
    http_headers_simd::as_simple_uri_reference(bytes)
}

fn invalid() -> DecodeError {
    super::invalid_syntax(&FieldName::Location)
}

impl PartialEq for LocationOwned {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for LocationOwned {}

impl Hash for LocationOwned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl PartialEq for LocationView<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for LocationView<'_> {}

impl Hash for LocationView<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "tests classify parser outcomes without needing successful values"
    )]

    use super::{Location, LocationOwned, validate, validate_general_reference, validate_general_reference_len, validate_with};
    use crate::sink::FieldSink;
    use crate::{DecodeErrorKind, DecodeMode, FieldName, FieldValue, SingleValueField, TestSink};

    #[test]
    #[should_panic(expected = "all Location constructors validate UTF-8 and storage is immutable")]
    fn owned_semantic_projection_checks_its_utf8_invariant() {
        let malformed = LocationOwned {
            value: FieldValue::try_from(vec![0xff]).unwrap(),
            component_ranges: validate(b"").unwrap().component_ranges,
            normalized: None,
        };
        assert_eq!(malformed.as_str().unwrap_err().kind(), DecodeErrorKind::InvalidSyntax);
        let _uri = malformed.uri_reference();
    }

    #[test]
    fn constructors_accessors_debug_and_round_trip() {
        for wire in ["", "/next?tab=1#profile", "../next", "https://example.com/a"] {
            let owned = LocationOwned::try_from(wire).expect("valid URI reference");
            assert_eq!(owned.as_bytes(), wire.as_bytes());
            assert_eq!(owned.as_str(), Ok(wire));
            assert!(format!("{owned:?}").contains("redacted"));
            assert!(owned.clone().into_field_value().is_sensitive());

            let mut table = TestSink::new();
            Location::insert(&mut table, owned).expect("table accepts location");
            let view = Location::view(&table).expect("valid location").expect("present");
            assert_eq!(view.as_bytes(), wire.as_bytes());
            assert_eq!(view.as_str(), Ok(wire));
            assert!(format!("{view:?}").contains("redacted"));
            assert!(
                Location::owned(&table)
                    .expect("valid owned location")
                    .expect("present")
                    .into_field_value()
                    .is_sensitive()
            );
            table.remove_values(&FieldName::Location);
            assert!(Location::view(&table).expect("absence is valid").is_none());
        }

        assert_eq!(
            LocationOwned::try_from(String::from("/owned")).expect("valid owned URI").as_str(),
            Ok("/owned")
        );
        assert_eq!(
            "/parsed".parse::<LocationOwned>().expect("shared FromStr implementation").as_str(),
            Ok("/parsed")
        );
    }

    /// The fast path replaces the RFC 3986 parse outright, so anything it
    /// accepts must be something the general parser would also accept.
    #[test]
    fn the_recognized_subset_agrees_with_the_general_parser() {
        let alphabet = b"aZ0:@/?#%[].+-_~!$&'()*,;=\\ \t";
        let radix = alphabet.len();
        let case_count = radix.pow(if cfg!(miri) { 2 } else { 4 });
        for encoded in 0..case_count {
            let mut value = encoded;
            let mut indices = [0; 4];
            for index in &mut indices {
                *index = value % radix;
                value /= radix;
            }
            if cfg!(miri) {
                // Pairwise seeds still place every alphabet byte in every position.
                indices[2] = (indices[0] + indices[1]) % radix;
                indices[3] = (indices[0] + 2 * indices[1]) % radix;
            }
            let bytes = indices.map(|index| alphabet[index]);
            for prefix in [b"".as_slice(), b"https://h.example".as_slice()] {
                let mut candidate = prefix.to_vec();
                candidate.extend_from_slice(&bytes);
                let Some(text) = super::is_simple_reference(&candidate) else {
                    continue;
                };
                assert_eq!(
                    super::ComponentRanges::from_simple(text),
                    validate_general_reference(text).unwrap(),
                    "recognized {text:?} with different components"
                );
            }
        }
    }

    #[test]
    fn strict_and_relaxed_validation_cover_fallbacks() {
        assert!(validate(b"/simple/path?query#fragment").is_ok());
        assert!(validate(b"https://example.com/docs?x=1#top").is_ok());
        assert!(validate(b"https://example.com:8443/docs").is_ok());
        assert!(validate(b"https://user@example.com/docs").is_ok());
        assert!(validate(b"https://example.com/caf%C3%A9").is_ok());
        assert!(validate(b"https://example.com/a b").is_err());
        assert!(validate_general_reference("mailto:user@example.com").is_ok());
        assert!(validate(b"%zz").is_err());
        assert!(validate(b"has space").is_err());
        assert!(validate(&[0xff]).is_err());

        assert!(validate_with(b"/a\\b", DecodeMode::Strict).is_err());
        assert_eq!(validate_with(b"/simple", DecodeMode::Relaxed).unwrap().as_str(), Ok("/simple"));
        assert_eq!(validate_with(b"/a\\b", DecodeMode::Relaxed).unwrap().as_str(), Ok("/a\\b"));
        assert!(validate_with(b"bad\\%zz", DecodeMode::Relaxed).is_err());
        assert!(validate_with(b"%zz", DecodeMode::Relaxed).is_err());
        assert!(validate_with(&[0xff], DecodeMode::Relaxed).is_err());

        let relaxed = FieldValue::from_static("/a\\b");
        assert!(<Location as SingleValueField>::decode_view(relaxed.as_field_value_ref()).is_err());
        assert!(<Location as SingleValueField>::decode_view_with(relaxed.as_field_value_ref(), DecodeMode::Relaxed).is_ok());
        let owned = <Location as SingleValueField>::decode_owned_with(relaxed, DecodeMode::Relaxed).expect("relaxed owned location");
        assert!(owned.into_field_value().is_sensitive());

        let strict = FieldValue::from_static("/strict");
        let strict_view = <Location as SingleValueField>::decode_view(strict.as_field_value_ref()).expect("valid strict location");
        assert_eq!(strict_view.as_str(), Ok("/strict"));
        let strict_owned = <Location as SingleValueField>::decode_owned(strict.clone()).expect("valid strict owned location");
        assert_eq!(<Location as SingleValueField>::as_field_value(&strict_owned), &strict);

        let invalid = FieldValue::try_from(vec![0xff]).expect("obs-text field value");
        assert_eq!(
            LocationOwned::try_from(invalid.clone())
                .expect_err("location requires UTF-8")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert!(LocationOwned::try_from("\n").is_err());
        assert!(LocationOwned::try_from(String::from("\n")).is_err());
        assert!(validate_general_reference_len(i32::MAX as usize).is_ok());
        assert!(validate_general_reference_len(i32::MAX as usize + 1).is_err());
        let malformed = LocationOwned {
            value: invalid,
            component_ranges: validate(b"").unwrap().component_ranges,
            normalized: None,
        };
        assert_eq!(
            malformed.as_str().expect_err("malformed private storage is not UTF-8").kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}
