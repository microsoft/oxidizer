// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{DecodeError, FieldName, FieldValue, FieldValueRef, SingleValueField};

/// Defines the `X-Content-Type-Options` header.
///
/// # Specification
///
/// Defined by the Fetch standard's
/// [X-Content-Type-Options section](https://fetch.spec.whatwg.org/#x-content-type-options-header).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{XContentTypeOptions, XContentTypeOptionsOwned};
///
/// let mut map = HeaderMap::new();
/// XContentTypeOptions::insert(&mut map, XContentTypeOptionsOwned::nosniff())?;
/// assert!(XContentTypeOptions::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct XContentTypeOptions {
    _private: (),
}

/// Owned value for the `X-Content-Type-Options` header.
///
/// # Specification
///
/// Defined by the Fetch standard's [X-Content-Type-Options section].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::XContentTypeOptionsOwned::nosniff();
/// assert_eq!(value.as_field_value(), "nosniff");
/// ```
///
/// `X-Content-Type-Options: nosniff` is the only supported value.
///
/// [X-Content-Type-Options section]: https://fetch.spec.whatwg.org/#x-content-type-options-header
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct XContentTypeOptionsOwned {
    value: FieldValue,
}

/// Borrowed value for the `X-Content-Type-Options` header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{XContentTypeOptions, XContentTypeOptionsView};
/// use http_headers::{FieldValueRef, SingleValueField};
///
/// let view: XContentTypeOptionsView<'_> =
///     XContentTypeOptions::decode_view(FieldValueRef::new(b"nosniff"))?;
/// assert_eq!(view.as_field_value().as_bytes(), b"nosniff");
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct XContentTypeOptionsView<'a> {
    value: FieldValueRef<'a>,
}

impl XContentTypeOptionsOwned {
    /// Constructs the only valid value, `nosniff`.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::XContentTypeOptionsOwned;
    ///
    /// let value = XContentTypeOptionsOwned::nosniff();
    /// assert_eq!(value.as_field_value().as_bytes(), b"nosniff");
    /// ```
    pub fn nosniff() -> Self {
        Self {
            value: FieldValue::from_static("nosniff"),
        }
    }

    /// Returns the stored field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::XContentTypeOptionsOwned;
    ///
    /// let value = XContentTypeOptionsOwned::nosniff();
    /// assert_eq!(value.as_field_value().as_bytes(), b"nosniff");
    /// assert!(XContentTypeOptionsOwned::try_from("NoSniff").is_err());
    /// ```
    pub fn as_field_value(&self) -> &FieldValue {
        &self.value
    }

    /// Returns reusable wire storage.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::XContentTypeOptionsOwned;
    ///
    /// let value = XContentTypeOptionsOwned::nosniff().into_field_value();
    /// assert_eq!(value.as_bytes(), b"nosniff");
    /// ```
    pub fn into_field_value(self) -> FieldValue {
        self.into()
    }
}

super::super::shared::impl_field_value_conversion!(XContentTypeOptionsOwned, |value| value.value);

impl<'a> XContentTypeOptionsView<'a> {
    /// Returns the original `nosniff` field value.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::XContentTypeOptions;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = XContentTypeOptions::decode_view(FieldValueRef::new(b"nosniff"))?;
    /// assert_eq!(view.as_field_value().as_bytes(), b"nosniff");
    /// assert!(XContentTypeOptions::decode_view(FieldValueRef::new(b"NoSniff")).is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_field_value(self) -> FieldValueRef<'a> {
        self.value
    }
}

impl Default for XContentTypeOptionsOwned {
    fn default() -> Self {
        Self::nosniff()
    }
}

impl SingleValueField for XContentTypeOptions {
    type View<'a> = XContentTypeOptionsView<'a>;
    type Owned = XContentTypeOptionsOwned;

    fn name() -> &'static FieldName {
        &FieldName::XContentTypeOptions
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        validate_nosniff(value)?;
        Ok(XContentTypeOptionsView { value })
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        XContentTypeOptionsOwned::try_from(value)
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.value
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.value
    }
}

impl TryFrom<&str> for XContentTypeOptionsOwned {
    type Error = DecodeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = FieldValue::from_str(value).map_err(|_invalid| super::super::invalid_syntax(&FieldName::XContentTypeOptions))?;
        Self::try_from(value)
    }
}

impl TryFrom<String> for XContentTypeOptionsOwned {
    type Error = DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let value = FieldValue::try_from(value).map_err(|_invalid| super::super::invalid_syntax(&FieldName::XContentTypeOptions))?;
        Self::try_from(value)
    }
}

impl TryFrom<FieldValue> for XContentTypeOptionsOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        validate_nosniff(value.as_field_value_ref())?;
        Ok(Self { value })
    }
}

fn validate_nosniff(value: FieldValueRef<'_>) -> Result<(), DecodeError> {
    if value.as_bytes() == b"nosniff" {
        Ok(())
    } else {
        Err(super::super::invalid_syntax(&FieldName::XContentTypeOptions))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{XContentTypeOptions, XContentTypeOptionsOwned};
    use crate::{DecodeErrorKind, FieldValue, FieldValueRef, TestSink};

    #[test]
    fn constructors_accessors_and_header_round_trip_use_exact_nosniff_value() {
        let value = XContentTypeOptionsOwned::nosniff();
        assert_eq!(value.as_field_value().as_bytes(), b"nosniff");
        assert_eq!(value.clone().into_field_value().as_bytes(), b"nosniff");
        assert_eq!(value, XContentTypeOptionsOwned::default());
        assert_eq!(
            <XContentTypeOptions as crate::SingleValueField>::as_field_value(&value).as_bytes(),
            b"nosniff"
        );
        assert_eq!(
            <XContentTypeOptions as crate::SingleValueField>::into_field_value(value.clone()).as_bytes(),
            b"nosniff"
        );
        assert_eq!(
            <XContentTypeOptions as crate::SingleValueField>::decode_owned(FieldValue::from_static("nosniff"),)
                .expect("direct owned decode"),
            value
        );
        assert_eq!(
            <XContentTypeOptions as crate::SingleValueField>::decode_view(FieldValueRef::new(b"NoSniff",))
                .expect_err("borrowed value is case-sensitive")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        let mut table = TestSink::new();
        assert!(XContentTypeOptions::view(&table).expect("absent header succeeds").is_none());
        XContentTypeOptions::insert(&mut table, value).expect("header inserts");
        let view = XContentTypeOptions::view(&table).expect("view decodes").expect("header is present");
        assert_eq!(view.as_field_value().as_bytes(), b"nosniff");
        let owned = XContentTypeOptions::owned(&table)
            .expect("owned value decodes")
            .expect("header is present");
        assert_eq!(owned.as_field_value().as_bytes(), b"nosniff");
    }

    #[test]
    fn conversions_reject_case_changes_invalid_syntax_and_bad_field_strings() {
        for value in [
            XContentTypeOptionsOwned::try_from("nosniff"),
            XContentTypeOptionsOwned::try_from(String::from("nosniff")),
            XContentTypeOptionsOwned::try_from(FieldValue::from_static("nosniff")),
        ] {
            assert_eq!(value.expect("exact value is valid").as_field_value().as_bytes(), b"nosniff");
        }
        for raw in ["NoSniff", "nosniff ", ""] {
            assert_eq!(
                XContentTypeOptionsOwned::try_from(raw)
                    .expect_err("only exact nosniff is valid")
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
        assert_eq!(
            XContentTypeOptionsOwned::try_from(String::from("nosniff\n"))
                .expect_err("line break is not a field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            XContentTypeOptionsOwned::try_from("nosniff\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}
