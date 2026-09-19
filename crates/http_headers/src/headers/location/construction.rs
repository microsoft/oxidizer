// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Component validation and assembly without parsing the assembled reference again.

use std::num::NonZeroUsize;

use super::component::Component;
use super::{LocationOwned, Metadata, UriAuthority, invalid, is_simple_reference, validate_general_reference_len};
use crate::{DecodeError, FieldValue};

pub(super) fn from_components(
    scheme: Option<&str>,
    authority: Option<UriAuthority<'_>>,
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> Result<LocationOwned, DecodeError> {
    validate_components(scheme, authority, path, query, fragment)?;

    let mut length = path.len();
    for (component, separator_length) in [
        (scheme, 1),
        (authority.map(UriAuthority::host), 2),
        (authority.and_then(UriAuthority::userinfo), 1),
        (authority.and_then(UriAuthority::port), 1),
        (query, 1),
        (fragment, 1),
    ] {
        if let Some(component) = component {
            length = length
                .checked_add(component.len())
                .and_then(|length| length.checked_add(separator_length))
                .ok_or_else(invalid)?;
        }
    }
    let mut text = String::with_capacity(length);
    let scheme_end = scheme.and_then(|scheme| {
        text.push_str(scheme);
        let end = NonZeroUsize::new(text.len());
        text.push(':');
        end
    });
    let authority_host = authority.map(|authority| {
        text.push_str("//");
        if let Some(userinfo) = authority.userinfo() {
            text.push_str(userinfo);
            text.push('@');
        }
        let host_start = text.len();
        text.push_str(authority.host());
        let host_end = text.len();
        if let Some(port) = authority.port() {
            text.push(':');
            text.push_str(port);
        }
        host_start..host_end
    });
    let path_start = text.len();
    text.push_str(path);
    let path_end = text.len();
    let query_end = query.and_then(|query| {
        text.push('?');
        text.push_str(query);
        NonZeroUsize::new(text.len())
    });
    let fragment_start = fragment.and_then(|fragment| {
        text.push('#');
        let start = NonZeroUsize::new(text.len());
        text.push_str(fragment);
        start
    });

    validate_constructed_reference_len(text.len(), text.as_bytes())?;
    let mut value = FieldValue::try_from(text).map_err(|_invalid| invalid())?;
    value.set_sensitive(true);
    Ok(LocationOwned {
        value,
        metadata: Metadata {
            scheme_end,
            authority_host,
            path: path_start..path_end,
            query_end,
            fragment_start,
        },
        normalized: None,
    })
}

fn validate_constructed_reference_len(length: usize, bytes: &[u8]) -> Result<(), DecodeError> {
    // Only general references have the dependency's existing i32 length bound.
    if i32::try_from(length).is_err() && is_simple_reference(bytes).is_none() {
        validate_general_reference_len(length)?;
    }
    Ok(())
}

fn validate_components(
    scheme: Option<&str>,
    authority: Option<UriAuthority<'_>>,
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> Result<(), DecodeError> {
    if let Some(scheme) = scheme {
        if !scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
            return Err(invalid());
        }
        Component::Scheme.validate(scheme)?;
    }
    Component::Path.validate(path)?;
    if authority.is_some() {
        if !path.is_empty() && !path.starts_with('/') {
            return Err(invalid());
        }
    } else if path.starts_with("//")
        || (scheme.is_none() && !path.starts_with('/') && path.split('/').next().is_some_and(|segment| segment.contains(':')))
    {
        return Err(invalid());
    }
    for component in [query, fragment].into_iter().flatten() {
        Component::QueryFragment.validate(component)?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::validate_constructed_reference_len;
    use crate::{DecodeErrorKind, FieldName};

    #[test]
    fn constructed_references_accept_the_general_length_boundary() {
        let maximum = usize::try_from(i32::MAX).unwrap();
        for length in [0, 1, maximum - 1, maximum] {
            for bytes in [b"/next?x#y".as_slice(), b"../next", b"/next%20path"] {
                assert_eq!(validate_constructed_reference_len(length, bytes), Ok(()));
            }
        }
    }

    #[test]
    fn oversized_constructed_references_only_accept_simple_shapes() {
        let maximum = usize::try_from(i32::MAX).unwrap();
        for length in [maximum + 1, usize::MAX] {
            for bytes in [b"/next?x#y".as_slice(), b"https://example.com/path"] {
                assert_eq!(validate_constructed_reference_len(length, bytes), Ok(()));
            }
            for bytes in [b"../next".as_slice(), b"/next%20path"] {
                let error = validate_constructed_reference_len(length, bytes).unwrap_err();
                assert_eq!(error.header(), &FieldName::Location);
                assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
                assert_eq!(error.value_index(), None);
            }
        }
    }
}
