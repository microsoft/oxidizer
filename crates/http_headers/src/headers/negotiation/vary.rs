// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::{ListValues, check_token_value, check_token_values, invalid};
use super::{FieldNameView, VaryEntryView};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Owned value for the `Vary` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 12.5.5].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::VaryOwned::try_from("accept, origin")?;
/// assert_eq!(value.items().count(), 2);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Vary: Accept-Encoding, Accept-Language` names selection fields, while
/// `Vary: *` means that other aspects of the request influenced selection.
///
/// [RFC 9110 section 12.5.5]: https://www.rfc-editor.org/rfc/rfc9110#section-12.5.5
pub struct VaryOwned {
    values: ListValues,
}

/// Borrowed value for the `Vary` header.
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Vary, VaryView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::Vary).then(|| FieldLines::single(name, b"Accept-Encoding, Origin"))
///     }
/// }
///
/// let value: VaryView<'_> = Vary::view(&Source)?.expect("header is present");
/// let items = value.items().collect::<Vec<_>>();
/// assert_eq!(items, [&b"Accept-Encoding"[..], &b"Origin"[..]]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct VaryView<'a> {
    values: FieldLines<'a>,
}

super::shared::list_header!(
    Vary,
    VaryOwned,
    VaryView,
    "Vary",
    "Defined by [RFC 9110 section 12.5.5](https://www.rfc-editor.org/rfc/rfc9110#section-12.5.5).",
    &FieldName::Vary,
    validate_vary_item,
    validate_vary_item,
    check_token_values,
    check_token_value,
    token
);

impl VaryOwned {
    /// Iterates wildcard or validated-name members in wire order.
    ///
    /// Name comparisons are case-insensitive; original spelling is retained.
    /// Empty list members are ignored, but wildcard members are never hidden.
    #[inline]
    pub fn entries(&self) -> impl Iterator<Item = VaryEntryView<'_>> {
        self.items().map(VaryEntryView::from_validated)
    }

    /// Reports whether any member is `*`, including mixed wildcard/name lists.
    ///
    /// This allocation-free query scans until the first wildcard.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::headers::VaryOwned;
    ///
    /// assert!(VaryOwned::try_from("Origin, *")?.contains_wildcard());
    /// assert!(!VaryOwned::try_from("Origin, X-*")?.contains_wildcard());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    #[must_use]
    #[inline]
    pub fn contains_wildcard(&self) -> bool {
        self.items().any(|item| item == b"*")
    }

    /// Constructs the wildcard field value.
    #[must_use]
    pub fn wildcard() -> Self {
        Self {
            values: ListValues::One(FieldValue::from_static("*")),
        }
    }

    /// Constructs one field line from validated names.
    ///
    /// Order, duplicates, and spelling are preserved. A name equal to `*`
    /// has wildcard semantics. An empty iterator produces an empty field.
    #[must_use]
    pub fn from_field_names<'a>(names: impl IntoIterator<Item = FieldNameView<'a>>) -> Self {
        Self::from_entries(names.into_iter().map(VaryEntryView::from_field_name))
    }

    /// Constructs one field line from typed wildcard and name members.
    ///
    /// Mixed wildcard/name lists are retained without sorting or deduplication.
    #[must_use]
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = VaryEntryView<'a>>) -> Self {
        let mut wire = String::new();
        for entry in entries {
            if !wire.is_empty() {
                wire.push_str(", ");
            }
            wire.push_str(entry.as_str());
        }
        Self {
            values: ListValues::One(FieldValue::from_validated_owned_bytes(wire.into_bytes(), false)),
        }
    }
}

impl<'a> VaryView<'a> {
    /// Iterates wildcard or validated-name members without allocating.
    #[inline]
    pub fn entries(&self) -> impl Iterator<Item = VaryEntryView<'a>> + '_ {
        self.items().map(VaryEntryView::from_validated)
    }

    /// Reports whether any member is `*`, scanning until the first wildcard.
    #[must_use]
    #[inline]
    pub fn contains_wildcard(&self) -> bool {
        self.items().any(|item| item == b"*")
    }
}

fn validate_vary_item(bytes: &[u8]) -> Result<(), DecodeError> {
    if validate::token(bytes) {
        Ok(())
    } else {
        Err(invalid(&FieldName::Vary, DecodeErrorKind::InvalidToken))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::validate_vary_item;
    use crate::DecodeErrorKind;

    #[test]
    fn vary_items_are_bare_field_name_tokens_or_wildcard() {
        validate_vary_item(b"*").expect("wildcard token");
        validate_vary_item(b"x-selection-input").expect("extension field name");
        assert_eq!(
            validate_vary_item(b"bad field").expect_err("spaces are not token bytes").kind(),
            DecodeErrorKind::InvalidToken
        );
    }
}
