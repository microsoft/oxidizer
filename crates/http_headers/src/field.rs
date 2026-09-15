// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Core traits connecting fields, borrowed views, and wire encoding.

use crate::sink::{EncodedValues, FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, FieldName, FieldValue, FieldValueRef};

/// Controls how field values are validated.
///
/// Strict decoding follows the field grammar exactly. Relaxed decoding is
/// opt-in and permits only documented, field-specific interoperability
/// deviations; fields without such deviations behave exactly as in strict
/// mode.
///
/// Relaxed decoding accepts these interoperability deviations:
///
/// - flexible quality-value whitespace and precision for `Accept`,
///   `Accept-Encoding`, and `Accept-Language`;
/// - lowercase `w/` entity-tag prefixes;
/// - optional whitespace around `Content-Type`, `Range`, and `Content-Range`
///   delimiters;
/// - `UTC`, outer whitespace, and one-digit components in IMF-style HTTP
///   dates;
/// - UTF-8 internationalized `Host` names that pass IDNA conversion; and
/// - backslashes normalized to slashes while validating `Location`.
///
/// Original field bytes are preserved. Relaxed mode still enforces numeric
/// bounds, token and list structure, entity-tag contents, range ordering,
/// valid IDNA and ports, and every framing, authorization, CORS, security, and
/// WebSocket invariant.
///
/// # Examples
///
/// ```rust
/// use http_headers::DecodeMode;
///
/// assert_eq!(DecodeMode::default(), DecodeMode::Strict);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum DecodeMode {
    /// Requires the field's specified grammar.
    #[default]
    Strict,
    /// Permits the explicitly documented interoperability relaxations.
    Relaxed,
}

/// Reads and writes one typed HTTP field.
///
/// Prefer [`Field::view`] when the result can borrow from the source. Use
/// [`Field::owned`] when the result must outlive the source borrow.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::headers::{UserAgent, UserAgentOwned};
///
/// let mut map = HeaderMap::new();
/// UserAgent::insert(&mut map, UserAgentOwned::try_from_static("client/1")?)?;
/// assert!(UserAgent::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub trait Field: Sized + 'static {
    /// The borrowed value type returned by [`Field::view`].
    type View<'a>
    where
        Self: 'a;

    /// The owned value type returned by [`Field::owned`].
    type Owned: 'static;

    /// Returns the field name.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::UserAgent;
    /// use http_headers::{Field, FieldName};
    ///
    /// assert_eq!(<UserAgent as Field>::name(), &FieldName::UserAgent);
    /// ```
    fn name() -> &'static FieldName;

    /// Reads a borrowed typed view from `source`.
    ///
    /// This is the preferred read operation when the result does not need to
    /// outlive `source`. Returns `Ok(None)` when the field is absent.
    ///
    /// # Errors
    ///
    /// Returns an error when a present field is malformed.
    #[expect(
        clippy::inline_always,
        reason = "the strict-mode convenience must disappear from typed decode hot paths"
    )]
    #[inline(always)]
    fn view<S>(source: &S) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, DecodeMode::Strict)
    }

    /// Reads a borrowed typed view under an explicit validation policy.
    ///
    /// Returns `Ok(None)` when the field is absent.
    ///
    /// # Errors
    ///
    /// Returns an error when a present field violates the selected policy.
    fn view_with<S>(source: &S, mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized;

    /// Reads an independently owned field from `source`.
    ///
    /// Prefer [`Field::view`] unless the result must be retained after
    /// `source` is released. Returns `Ok(None)` when the field is absent.
    ///
    /// # Errors
    ///
    /// Returns an error when a present field is malformed.
    #[expect(
        clippy::inline_always,
        reason = "the strict-mode convenience must disappear from typed decode hot paths"
    )]
    #[inline(always)]
    fn owned<S>(source: &S) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::owned_with(source, DecodeMode::Strict)
    }

    /// Reads an independently owned field under an explicit validation policy.
    ///
    /// Returns `Ok(None)` when the field is absent.
    ///
    /// # Errors
    ///
    /// Returns an error when a present field violates the selected policy.
    fn owned_with<S>(source: &S, mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized;

    /// Inserts a field, replacing every existing value with that name.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when it cannot hold the
    /// encoded values.
    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized;

    /// Removes every field value stored for this field.
    #[inline]
    fn remove<S>(sink: &mut S)
    where
        S: FieldSink + ?Sized,
    {
        sink.remove_values(Self::name());
    }
}

/// Defines a custom field represented by exactly one validated field value.
///
/// Implementing this trait also implements [`Field`], including source
/// lookup, rejection of multiple field lines, strict and relaxed reads,
/// insertion, and removal. Applications using built-in headers normally use
/// [`Field`] instead.
///
/// The blanket [`Field`] implementation enforces custom-source byte and line
/// budgets. It cannot infer whether a downstream-defined grammar is
/// list-valued, so such implementations must enforce
/// [`MAX_CUSTOM_LIST_ITEMS`](crate::source::MAX_CUSTOM_LIST_ITEMS) in both
/// borrowed and owned decoding before accepting further parsed items.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::SingleValueField;
/// use http_headers::headers::{UserAgent, UserAgentOwned};
///
/// let mut map = HeaderMap::new();
/// let value = UserAgentOwned::try_from_static("client/1")?;
/// UserAgent::insert(&mut map, value)?;
/// assert_eq!(
///     <UserAgent as SingleValueField>::name().as_str(),
///     "user-agent"
/// );
/// assert!(
///     UserAgent::view(&map)
///         .expect("stored header is valid")
///         .is_some()
/// );
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub trait SingleValueField: Sized + 'static {
    /// The borrowed view type.
    type View<'a>
    where
        Self: 'a;

    /// The owned value type.
    type Owned: 'static;

    /// Returns the field name.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::SingleValueField;
    /// use http_headers::headers::UserAgent;
    ///
    /// assert_eq!(
    ///     <UserAgent as SingleValueField>::name().as_str(),
    ///     "user-agent"
    /// );
    /// ```
    fn name() -> &'static FieldName;

    /// Validates and constructs a borrowed view.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` violates this field's grammar.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::UserAgent;
    /// use http_headers::{FieldValueRef, SingleValueField};
    ///
    /// let view = <UserAgent as SingleValueField>::decode_view(FieldValueRef::new(b"client/1"))?;
    /// assert_eq!(view.as_str()?, "client/1");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError>;

    /// Validates and constructs a borrowed view under an explicit policy.
    ///
    /// Fields without documented interoperability relaxations apply strict
    /// validation in either mode.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` violates the selected policy.
    fn decode_view_with(value: FieldValueRef<'_>, _mode: DecodeMode) -> Result<Self::View<'_>, DecodeError> {
        Self::decode_view(value)
    }

    /// Validates and constructs the owned value from one field value.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` violates this field's grammar.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::UserAgent;
    /// use http_headers::{FieldValue, SingleValueField};
    ///
    /// let owned = <UserAgent as SingleValueField>::decode_owned(FieldValue::from_static("client/1"))?;
    /// assert_eq!(
    ///     <UserAgent as SingleValueField>::as_field_value(&owned),
    ///     "client/1"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError>;

    /// Validates and constructs an owned value under an explicit policy.
    ///
    /// Fields without documented interoperability relaxations apply strict
    /// validation in either mode.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` violates the selected policy.
    fn decode_owned_with(value: FieldValue, _mode: DecodeMode) -> Result<Self::Owned, DecodeError> {
        Self::decode_owned(value)
    }

    /// Returns the stored field value.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::SingleValueField;
    /// use http_headers::headers::{UserAgent, UserAgentOwned};
    ///
    /// let agent = UserAgentOwned::try_from_static("client/1")?;
    /// assert_eq!(
    ///     <UserAgent as SingleValueField>::as_field_value(&agent),
    ///     "client/1"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    fn as_field_value(value: &Self::Owned) -> &FieldValue;

    /// Consumes the field and returns its stored field value.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::SingleValueField;
    /// use http_headers::headers::{UserAgent, UserAgentOwned};
    ///
    /// let agent = UserAgentOwned::try_from_static("client/1")?;
    /// assert_eq!(
    ///     <UserAgent as SingleValueField>::into_field_value(agent),
    ///     "client/1"
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    fn into_field_value(value: Self::Owned) -> FieldValue;
}

fn validate_single_value_list_limit<T: SingleValueField>(lines: &FieldLines<'_>) -> Result<(), DecodeError> {
    if T::name() == &FieldName::Range {
        lines.validate_list_item_limit(b',', true)
    } else if T::name() == &FieldName::StrictTransportSecurity {
        lines.validate_list_item_limit(b';', false)
    } else {
        Ok(())
    }
}

fn validate_single_value_view<T: SingleValueField>(lines: &FieldLines<'_>) -> Result<(), DecodeError> {
    if T::name() == &FieldName::Range || T::name() == &FieldName::StrictTransportSecurity {
        validate_single_value_list_limit::<T>(lines)
    } else {
        lines.validate_custom_source()
    }
}

impl<T> Field for T
where
    T: SingleValueField,
{
    type View<'a>
        = T::View<'a>
    where
        T: 'a;
    type Owned = T::Owned;

    #[inline]
    fn name() -> &'static FieldName {
        T::name()
    }

    #[expect(
        clippy::inline_always,
        reason = "the single-value adapter must disappear from borrowed decode hot paths"
    )]
    #[inline(always)]
    fn view_with<S>(source: &S, mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(T::name()) else {
            return Ok(None);
        };
        validate_single_value_view::<T>(&lines)?;
        let value = lines.exactly_one()?;
        T::decode_view_with(value, mode).map(Some)
    }

    #[expect(
        clippy::inline_always,
        reason = "the single-value adapter must disappear from owned decode hot paths"
    )]
    #[inline(always)]
    fn owned_with<S>(source: &S, mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(T::name()) else {
            return Ok(None);
        };
        validate_single_value_list_limit::<T>(&lines)?;
        let owned = lines.exactly_one_owned()?;
        T::decode_owned_with(owned, mode).map(Some)
    }

    #[inline]
    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(T::name(), EncodedValues::single(T::into_field_value(value)))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{DecodeMode, Field, SingleValueField};
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, TestSink};

    struct FixtureHeader;

    impl SingleValueField for FixtureHeader {
        type View<'a> = FieldValueRef<'a>;
        type Owned = FieldValue;

        fn name() -> &'static FieldName {
            &FieldName::UserAgent
        }

        fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
            if value.as_bytes() == b"strict" {
                Ok(value)
            } else {
                Err(DecodeError::new(<Self as SingleValueField>::name(), DecodeErrorKind::InvalidSyntax))
            }
        }

        fn decode_view_with(value: FieldValueRef<'_>, mode: DecodeMode) -> Result<Self::View<'_>, DecodeError> {
            if mode == DecodeMode::Relaxed {
                Ok(value)
            } else {
                Self::decode_view(value)
            }
        }

        fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
            if value.as_bytes() == b"strict" {
                Ok(value)
            } else {
                Err(DecodeError::new(<Self as SingleValueField>::name(), DecodeErrorKind::InvalidSyntax))
            }
        }

        fn decode_owned_with(value: FieldValue, mode: DecodeMode) -> Result<Self::Owned, DecodeError> {
            if mode == DecodeMode::Relaxed {
                Ok(value)
            } else {
                Self::decode_owned(value)
            }
        }

        fn as_field_value(value: &Self::Owned) -> &FieldValue {
            value
        }

        fn into_field_value(value: Self::Owned) -> FieldValue {
            value
        }
    }

    struct Source {
        values: Vec<FieldValue>,
    }

    impl FieldSource for Source {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            FieldLines::from_slice(name, &self.values)
        }
    }

    #[test]
    fn single_value_adapter_handles_absence_modes_cardinality_insert_and_remove() {
        let absent = Source { values: vec![] };
        assert_eq!(FixtureHeader::view(&absent).expect("absence is valid"), None);
        assert_eq!(FixtureHeader::owned(&absent).expect("absence is valid"), None);

        let strict = Source {
            values: vec![FieldValue::from_static("strict")],
        };
        assert_eq!(
            FixtureHeader::view(&strict)
                .expect("strict value decodes")
                .expect("strict value is present"),
            "strict"
        );
        assert_eq!(
            FixtureHeader::owned(&strict)
                .expect("strict value decodes")
                .expect("strict value is present"),
            "strict"
        );

        let relaxed = Source {
            values: vec![FieldValue::from_static("relaxed")],
        };
        assert_eq!(
            FixtureHeader::view(&relaxed)
                .expect_err("strict view rejects relaxed fixture")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            FixtureHeader::owned(&relaxed)
                .expect_err("strict owned decode rejects relaxed fixture")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            FixtureHeader::view_with(&relaxed, DecodeMode::Relaxed)
                .expect("relaxed view decodes")
                .expect("relaxed value is present"),
            "relaxed"
        );
        assert_eq!(
            FixtureHeader::owned_with(&relaxed, DecodeMode::Relaxed)
                .expect("relaxed owned decode succeeds")
                .expect("relaxed value is present"),
            "relaxed"
        );

        let multiple = Source {
            values: vec![FieldValue::from_static("strict"), FieldValue::from_static("strict")],
        };
        assert_eq!(
            FixtureHeader::view(&multiple).expect_err("multiple values are rejected").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(
            FixtureHeader::owned(&multiple)
                .expect_err("multiple owned values are rejected")
                .kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );

        let mut sink = TestSink::new();
        let owned = FieldValue::from_static("strict");
        assert_eq!(<FixtureHeader as SingleValueField>::as_field_value(&owned), "strict");
        FixtureHeader::insert(&mut sink, owned).expect("insertion succeeds");
        assert!(sink.contains(&FieldName::UserAgent));
        FixtureHeader::remove(&mut sink);
        assert!(!sink.contains(&FieldName::UserAgent));
    }
}
