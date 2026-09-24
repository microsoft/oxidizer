// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public API tests for decode/insert errors and encoded-value collections.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod decode_error {
    use std::collections::HashSet;
    use std::error::Error;
    use std::fmt::{self, Write};

    use http_headers::sink::{InsertError, InsertErrorKind};
    use http_headers::{DecodeError, DecodeErrorKind, FieldName};

    struct FailAfter {
        writes_left: usize,
    }

    impl Write for FailAfter {
        fn write_str(&mut self, _value: &str) -> fmt::Result {
            if self.writes_left == 0 {
                Err(fmt::Error)
            } else {
                self.writes_left -= 1;
                Ok(())
            }
        }
    }

    #[test]
    fn error_accessors_display_and_index_saturation_are_structured() {
        let unindexed = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
        assert_eq!(unindexed.header(), &FieldName::ContentType);
        assert_eq!(unindexed.value_index(), None);
        assert_eq!(unindexed.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(unindexed.to_string(), "invalid content-type header: invalid syntax");

        let indexed = unindexed.at_value(7);
        assert_eq!(indexed.value_index(), Some(7));
        assert_eq!(indexed.to_string(), "invalid content-type header: invalid syntax at value 7");

        let saturated = unindexed.at_value(usize::MAX);
        assert_eq!(saturated.value_index(), Some(u32::MAX as usize - 1));
        let error: &dyn Error = &saturated;
        assert!(error.source().is_none());
    }

    #[test]
    fn every_decode_kind_has_stable_nonsensitive_text() {
        let cases = [
            (DecodeErrorKind::MissingValue, "missing value"),
            (DecodeErrorKind::UnexpectedMultipleValues, "unexpected multiple values"),
            (DecodeErrorKind::InvalidSyntax, "invalid syntax"),
            (DecodeErrorKind::InvalidUtf8, "invalid UTF-8"),
            (DecodeErrorKind::InvalidToken, "invalid token"),
            (DecodeErrorKind::InvalidNumber, "invalid number"),
            (DecodeErrorKind::UnterminatedQuote, "unterminated quoted string"),
            (DecodeErrorKind::SourceLimitExceeded, "source limit exceeded"),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.to_string(), expected);
        }
    }

    #[test]
    fn display_propagates_formatter_failures() {
        let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax).at_value(3);
        let mut immediate = FailAfter { writes_left: 0 };
        assert!(immediate.write_fmt(format_args!("{error}")).is_err());

        let mut after_message = FailAfter { writes_left: 4 };
        assert!(after_message.write_fmt(format_args!("{error}")).is_err());

        let kind = InsertErrorKind::InvalidValue;
        let error = InsertError::new(kind);
        assert!(immediate.write_fmt(format_args!("{kind}")).is_err());
        assert!(immediate.write_fmt(format_args!("{error}")).is_err());
    }

    #[test]
    fn insertion_errors_preserve_compact_distinct_failure_categories() {
        const ERROR: InsertError = InsertError::new(InsertErrorKind::CapacityExceeded);
        const KIND: InsertErrorKind = ERROR.kind();
        assert_eq!(KIND, InsertErrorKind::CapacityExceeded);
        assert!(size_of::<InsertError>() <= size_of::<usize>());

        let cases = [
            (InsertErrorKind::InvalidValue, "invalid field value"),
            (
                InsertErrorKind::InvalidEncoding,
                "encoded field length does not match the announced length",
            ),
            (InsertErrorKind::AllocationFailed, "field buffer capacity could not be reserved"),
            (InsertErrorKind::CapacityExceeded, "field value or container capacity exceeded"),
        ];
        let mut distinct = HashSet::new();
        for (kind, message) in cases {
            let error = InsertError::new(kind);
            let copies = [error; 2];
            assert_eq!(copies[0], copies[1]);
            assert_eq!(error.kind(), kind);
            assert_eq!(kind.to_string(), message);
            assert_eq!(error.to_string(), message);
            assert!(error.source().is_none());
            assert!(format!("{error:?}").contains(&format!("{kind:?}")));
            assert!(distinct.insert(error));
            assert!(!distinct.insert(error));
        }
        assert_eq!(distinct.len(), 4);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod encoded_values {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use http_headers::FieldValue;
    use http_headers::sink::EncodedValues;

    #[test]
    fn iterators_support_mixed_direction_iteration() {
        let values = ["first", "second", "third"]
            .into_iter()
            .map(FieldValue::from_static)
            .collect::<EncodedValues>();

        let mut borrowed = values.iter();
        assert_eq!(borrowed.next_back().expect("last value"), "third");
        assert_eq!(borrowed.next().expect("first value"), "first");
        assert_eq!(borrowed.next_back().expect("middle value"), "second");
        assert!(borrowed.next().is_none());

        let mut owned = values.into_iter();
        assert_eq!(owned.next_back().expect("last owned value"), "third");
        assert_eq!(owned.next().expect("first owned value"), "first");
        assert_eq!(owned.next_back().expect("middle owned value"), "second");
        assert!(owned.next().is_none());
    }

    #[test]
    fn comparison_and_hashing_ignore_internal_representation() {
        let single = EncodedValues::single(FieldValue::from_static("gzip"));
        let vector = EncodedValues::from_vec(vec![FieldValue::from_static("gzip")]);
        let greater = EncodedValues::from_vec(vec![FieldValue::from_static("gzip, br")]);

        assert_eq!(single, vector);
        assert!(single < greater);

        let mut single_hash = DefaultHasher::new();
        single.hash(&mut single_hash);
        let mut vector_hash = DefaultHasher::new();
        vector.hash(&mut vector_hash);
        assert_eq!(single_hash.finish(), vector_hash.finish());
    }
}
