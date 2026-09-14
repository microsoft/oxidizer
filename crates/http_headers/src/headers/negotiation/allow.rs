// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::{ListValues, check_token_value, check_token_values, invalid};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Owned value for the `Allow` header.
///
/// # Specification
///
/// Defined by [RFC 9110 section 10.2.1].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::AllowOwned::try_from("GET, POST")?;
/// assert_eq!(value.items().count(), 2);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Allow: GET, HEAD, OPTIONS` lists supported methods. An empty `Allow:`
/// field is valid and indicates that no methods are currently allowed.
///
/// [RFC 9110 section 10.2.1]: https://www.rfc-editor.org/rfc/rfc9110#section-10.2.1
pub struct AllowOwned {
    values: ListValues,
}

/// Borrowed value for the `Allow` header.
/// # Examples
///
/// ```rust
/// use http_headers::headers::{Allow, AllowView};
/// use http_headers::source::{FieldLines, FieldSource};
/// use http_headers::{Field, FieldName};
///
/// struct Source;
///
/// impl FieldSource for Source {
///     fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
///         (name == &FieldName::Allow).then(|| FieldLines::single(name, b"GET, HEAD, OPTIONS"))
///     }
/// }
///
/// let value: AllowView<'_> = Allow::view(&Source)?.expect("header is present");
/// let items = value.items().collect::<Vec<_>>();
/// assert_eq!(items, [&b"GET"[..], &b"HEAD"[..], &b"OPTIONS"[..]]);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct AllowView<'a> {
    values: FieldLines<'a>,
}

super::shared::list_header!(
    Allow,
    AllowOwned,
    AllowView,
    "Allow",
    "Defined by [RFC 9110 section 10.2.1](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.1).",
    &FieldName::Allow,
    validate_allow_item,
    validate_allow_item,
    check_token_values,
    check_token_value,
    token
);

fn validate_allow_item(bytes: &[u8]) -> Result<(), DecodeError> {
    if validate::token(bytes) {
        Ok(())
    } else {
        Err(invalid(&FieldName::Allow, DecodeErrorKind::InvalidToken))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::validate_allow_item;
    use crate::DecodeErrorKind;

    #[test]
    fn allow_items_are_bare_method_tokens() {
        validate_allow_item(b"PATCH").expect("extension method token");
        assert_eq!(
            validate_allow_item(b"BAD METHOD").expect_err("spaces are not token bytes").kind(),
            DecodeErrorKind::InvalidToken
        );
    }
}
