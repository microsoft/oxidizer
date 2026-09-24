// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::quality::QualityView;
use super::shared::{ListValues, invalid_syntax};
use crate::source::{MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_LIST_ITEMS};
use crate::{DecodeError, DecodeErrorKind, FieldName, FieldValue, validate};

pub(super) fn validated_member(item: Result<&[u8], DecodeError>) -> &[u8] {
    item.expect("header decoding validated every list member before constructing the immutable header")
}

pub(super) fn weighted_parts(bytes: &[u8]) -> (&[u8], Option<QualityView<'_>>) {
    let Some(semicolon) = bytes.iter().position(|byte| *byte == b';') else {
        return (bytes, None);
    };
    let parameter = &bytes[semicolon + 1..];
    let equals = parameter
        .iter()
        .position(|byte| *byte == b'=')
        .expect("validated weights contain an equals sign");
    (
        validate::trim_ows(&bytes[..semicolon]),
        Some(QualityView::from_validated(validate::trim_ows(&parameter[equals + 1..]))),
    )
}

pub(super) fn checked_len(lengths: impl IntoIterator<Item = usize>, name: &'static FieldName) -> Result<usize, DecodeError> {
    let mut total = 0_usize;
    for length in lengths {
        total = total
            .checked_add(length)
            .filter(|total| *total <= MAX_CUSTOM_FIELD_BYTES)
            .ok_or_else(|| DecodeError::new(name, DecodeErrorKind::SourceLimitExceeded))?;
    }
    Ok(total)
}

pub(super) fn weight_len(quality: Option<QualityView<'_>>) -> usize {
    quality.map_or(0, |value| 3 + value.encoded_len())
}

pub(super) fn append_weight(bytes: &mut Vec<u8>, quality: Option<QualityView<'_>>) {
    if let Some(quality) = quality {
        bytes.extend_from_slice(b";q=");
        quality.append_to(bytes);
    }
}

pub(super) fn collect_members<T>(
    entries: impl IntoIterator<Item = T>,
    name: &'static FieldName,
    length: impl Fn(&T) -> Result<usize, DecodeError>,
    append: impl Fn(&T, &mut Vec<u8>),
) -> Result<ListValues, DecodeError> {
    let mut bytes = Vec::new();
    for (index, entry) in entries.into_iter().enumerate() {
        if index >= MAX_CUSTOM_LIST_ITEMS {
            return Err(DecodeError::new(name, DecodeErrorKind::SourceLimitExceeded));
        }
        let separator = usize::from(index != 0) * 2;
        let len = checked_len([bytes.len(), separator, length(&entry)?], name)?;
        bytes.reserve(len - bytes.len());
        if separator != 0 {
            bytes.extend_from_slice(b", ");
        }
        append(&entry, &mut bytes);
    }
    let value = FieldValue::try_from(bytes).map_err(|_invalid| invalid_syntax(name))?;
    Ok(ListValues::One(value))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::checked_len;
    use crate::{DecodeError, DecodeErrorKind, FieldName};

    #[test]
    fn member_length_bounds_and_arithmetic_overflow_are_admission_errors() {
        assert_eq!(checked_len([65_535, 1], &FieldName::Accept), Ok(65_536));
        for lengths in [[65_536, 1], [1, usize::MAX]] {
            assert_eq!(
                checked_len(lengths, &FieldName::Accept),
                Err(DecodeError::new(&FieldName::Accept, DecodeErrorKind::SourceLimitExceeded))
            );
        }
    }
}
