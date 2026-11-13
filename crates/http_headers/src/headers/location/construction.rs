// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Component validation and assembly without parsing the assembled reference again.

use std::num::NonZeroUsize;

use super::component::Component;
use super::{ComponentRanges, LocationOwned, UriAuthority, invalid, is_simple_reference, validate_general_reference_len};
use crate::{DecodeError, FieldValue};

pub(super) fn from_components(
    scheme: Option<&str>,
    authority: Option<UriAuthority<'_>>,
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> Result<LocationOwned, DecodeError> {
    validate_components(scheme, authority, path, query, fragment)?;

    let length = component_capacity(
        // #[gamma::skip(expr.increment, tag = "equivalent", reason = "This adds one spare byte only to String's initial capacity; text assembly and component ranges use actual appended lengths")]
        path.len(),
        // #[gamma::skip(collection.reverse_array, tag = "equivalent", reason = "Permuting these lengths only changes String's initial capacity estimate by separator slack; assembly appends the original components independently")]
        [
            scheme.map(str::len),
            authority.map(|value| value.host().len()),
            authority.and_then(UriAuthority::userinfo).map(str::len),
            authority.and_then(UriAuthority::port).map(str::len),
            query.map(str::len),
            fragment.map(str::len),
        ],
    )?;
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
    // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "Every component has passed its component validator or the UriAuthority constructor, and assembly adds only field-value-safe ASCII delimiters")]
    let mut value = FieldValue::try_from(text).map_err(|_invalid| invalid())?;
    value.set_sensitive(true);
    Ok(LocationOwned {
        value,
        component_ranges: ComponentRanges {
            scheme_end,
            authority_host,
            path: path_start..path_end,
            query_end,
            fragment_start,
        },
        normalized: None,
    })
}

fn component_capacity(path_len: usize, components: [Option<usize>; 6]) -> Result<usize, DecodeError> {
    #[cfg(test)]
    if tests::FAIL_COMPONENT_CAPACITY.replace(false) {
        return Err(invalid());
    }
    let [scheme, host, userinfo, port, query, fragment] = components;
    let mut length = path_len;
    for (component, separator_length) in
        // #[gamma::skip(collection.reverse_array, tag = "equivalent", reason = "This loop only sums nonnegative component and separator lengths; checked addition reports overflow independently of order, and text is appended separately")]
        [(scheme, 1), (host, 2), (userinfo, 1), (port, 1), (query, 1), (fragment, 1)]
    {
        if let Some(component) = component {
            length = length
                .checked_add(component)
                .and_then(|length| length.checked_add(separator_length))
                .ok_or_else(invalid)?;
        }
    }
    Ok(length)
}

fn validate_constructed_reference_len(length: usize, bytes: &[u8]) -> Result<(), DecodeError> {
    // Only general references have the dependency's existing i32 length bound.
    // #[gamma::skip(logical.and_remove_left, tag = "equivalent", reason = "For any in-bound length the called general-reference length validator returns Ok, so invoking it for additional nonsimple references cannot change the result")]
    if !super::general_reference_len_fits(length) && is_simple_reference(bytes).is_none() {
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
    } else {
        // #[gamma::skip(logical.and_remove_right, literal.char_to_nul, tag = "equivalent", reason = "A leading slash makes the first slash-delimited segment empty and therefore colon-free; NUL was already rejected by Component::Path")]
        let relative = scheme.is_none() && !path.starts_with('/');
        if path.starts_with("//") || (relative && path.split('/').next().is_some_and(|segment| segment.contains(':'))) {
            return Err(invalid());
        }
    }
    for component in
        // #[gamma::skip(collection.reverse_array, tag = "equivalent", reason = "Both optional components use the same pure validator and identical error kind; construction retains their original query and fragment order")]
        [query, fragment].into_iter().flatten()
    {
        Component::QueryFragment.validate(component)?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    std::thread_local! {
        pub(super) static FAIL_COMPONENT_CAPACITY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[test]
    fn gamma_nov6_component_constructor_propagates_capacity_failure() {
        FAIL_COMPONENT_CAPACITY.set(true);
        let result = super::from_components(None, None, "/", None, None);
        assert!(!FAIL_COMPONENT_CAPACITY.replace(false));
        assert_eq!(result.unwrap_err().kind(), crate::DecodeErrorKind::InvalidSyntax);
    }

    use super::validate_constructed_reference_len;
    use crate::{DecodeErrorKind, FieldName};

    #[test]
    fn gamma_afternoon_component_capacity_counts_every_delimiter_and_propagates_overflow() {
        assert_eq!(super::component_capacity(3, [None; 6]), Ok(3));
        for (index, separator) in [1, 2, 1, 1, 1, 1].into_iter().enumerate() {
            let mut components = [None; 6];
            components[index] = Some(5);
            assert_eq!(super::component_capacity(3, components), Ok(8 + separator));
        }
        assert_eq!(super::component_capacity(2, [Some(0); 6]), Ok(9));
        assert_eq!(super::component_capacity(usize::MAX - 7, [Some(0); 6]), Ok(usize::MAX));
        for components in [[Some(0), None, None, None, None, None], [Some(1), None, None, None, None, None]] {
            assert_eq!(
                super::component_capacity(usize::MAX, components).unwrap_err().kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }

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
