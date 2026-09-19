// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The trait a field container implements to store field lines.

use smallvec::SmallVec;

use crate::sink::{EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSensitivity, FieldValueWriter, InsertError, InsertErrorKind};
use crate::source::FieldSource;
use crate::{FieldName, FieldValue};

// Match FieldValue's inline representation to avoid temporary heap storage.
const FIELD_VALUE_INLINE_CAPACITY: usize = 64;
type CollectBytes = SmallVec<[u8; FIELD_VALUE_INLINE_CAPACITY]>;

struct CollectOutput {
    values: EncodedValues,
}

struct CollectWriter<'a> {
    values: &'a mut EncodedValues,
    bytes: WriterBytes,
    expected: usize,
    sensitive: bool,
}

enum WriterBytes {
    Inline(CollectBytes),
    Spilled(Vec<u8>),
}

impl FieldEncodeOutput for CollectOutput {
    type Writer<'a> = CollectWriter<'a>;

    #[expect(
        clippy::inline_always,
        reason = "keeps the inline-buffer dispatch within the measured instruction gate"
    )]
    #[inline(always)]
    fn begin_value(&mut self, length: usize, sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError> {
        if length > isize::MAX as usize {
            return Err(InsertError::new(InsertErrorKind::CapacityExceeded));
        }
        let bytes = if length <= FIELD_VALUE_INLINE_CAPACITY {
            WriterBytes::Inline(CollectBytes::new())
        } else {
            WriterBytes::Spilled(reserve_bytes(length)?)
        };
        Ok(CollectWriter {
            values: &mut self.values,
            bytes,
            expected: length,
            sensitive: sensitivity.is_sensitive(),
        })
    }

    fn push_value(&mut self, value: FieldValue) -> Result<(), InsertError> {
        self.values.push(value);
        Ok(())
    }
}

fn reserve_bytes(length: usize) -> Result<Vec<u8>, InsertError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_error| InsertError::new(InsertErrorKind::AllocationFailed))?;
    Ok(bytes)
}

impl FieldValueWriter for CollectWriter<'_> {
    #[expect(
        clippy::inline_always,
        reason = "keeps the inline-buffer dispatch within the measured instruction gate"
    )]
    #[inline(always)]
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), InsertError> {
        let written = match &self.bytes {
            WriterBytes::Inline(buffer) => buffer.len(),
            WriterBytes::Spilled(buffer) => buffer.len(),
        };
        if bytes.len() > self.expected.saturating_sub(written) {
            return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
        }
        match &mut self.bytes {
            WriterBytes::Inline(buffer) => buffer.extend_from_slice(bytes),
            WriterBytes::Spilled(buffer) => buffer.extend_from_slice(bytes),
        }
        Ok(())
    }

    #[expect(
        clippy::inline_always,
        reason = "keeps the inline-buffer dispatch within the measured instruction gate"
    )]
    #[inline(always)]
    fn finish(self) -> Result<(), InsertError> {
        let length = match &self.bytes {
            WriterBytes::Inline(bytes) => bytes.len(),
            WriterBytes::Spilled(bytes) => bytes.len(),
        };
        if length != self.expected {
            return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
        }
        let value = match self.bytes {
            WriterBytes::Inline(bytes) => FieldValue::from_bytes(bytes),
            WriterBytes::Spilled(bytes) => FieldValue::try_from(bytes),
        }
        .map_err(|_invalid| InsertError::new(InsertErrorKind::InvalidValue))?
        .with_sensitive(self.sensitive);
        self.values.push(value);
        Ok(())
    }
}

/// A container that can store the field lines of a field.
///
/// With a header-family feature enabled, most callers create headers with
/// `crate::sink::FieldSinkExt` or [`crate::Field::insert`]. Core-only builds
/// can use this trait directly. Implement it only when integrating a custom
/// field container.
///
/// All name-taking methods require static descriptors, just like
/// [`FieldSource`]. Custom descriptors can use a `static LazyLock<FieldName>`.
/// Use the container's native API to insert, append, or remove fields whose
/// names are constructed locally at runtime.
///
/// # Required and provided methods
///
/// [`set_values`], [`append_values`], and [`remove_values`] are required.
/// Separating replacement from append lets inherited encoded appends buffer
/// only their new field lines before one atomic storage operation.
///
/// The other two methods take a [`FieldEncoder`] rather than finished values,
/// which lets a typed field write its bytes straight into whatever storage the
/// container prefers. There is deliberately no `remove_encoded`, because
/// removal has nothing to encode.
///
/// [`set_values`]: FieldSink::set_values
/// [`append_values`]: FieldSink::append_values
/// [`remove_values`]: FieldSink::remove_values
/// [`append_encoded`]: FieldSink::append_encoded
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), http_headers::sink::InsertError> {
/// use http::HeaderMap;
/// use http_headers::sink::{EncodedValues, FieldSink};
/// use http_headers::source::FieldSource;
/// use http_headers::{FieldName, FieldValue};
///
/// let mut map = HeaderMap::new();
/// let values = EncodedValues::single(FieldValue::from_static("client/1"));
/// FieldSink::set_values(&mut map, &FieldName::UserAgent, values)?;
/// let stored = FieldSource::lines(&map, &FieldName::UserAgent).expect("value was stored");
/// assert_eq!(
///     stored.exactly_one().expect("exactly one value").as_bytes(),
///     b"client/1"
/// );
/// FieldSink::remove_values(&mut map, &FieldName::UserAgent);
/// assert!(!FieldSource::contains(&map, &FieldName::UserAgent));
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub trait FieldSink: FieldSource {
    /// Replaces every field line stored under `name` from an encoding plan.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when encoding or storage
    /// fails.
    fn set_encoded<E>(&mut self, name: &'static FieldName, encoder: E) -> Result<(), InsertError>
    where
        E: FieldEncoder,
        Self: Sized,
    {
        let mut output = CollectOutput {
            values: EncodedValues::new(),
        };
        encoder.encode(&mut output)?;
        self.set_values(name, output.values)
    }

    /// Appends field lines produced by an encoding plan.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when encoding or storage
    /// fails.
    fn append_encoded<E>(&mut self, name: &'static FieldName, encoder: E) -> Result<(), InsertError>
    where
        E: FieldEncoder,
        Self: Sized,
    {
        let mut output = CollectOutput {
            values: EncodedValues::new(),
        };
        encoder.encode(&mut output)?;
        self.append_values(name, output.values)
    }

    /// Replaces every field line stored under `name`.
    ///
    /// An empty `values` removes the field.
    ///
    /// # Errors
    ///
    /// Returns an error, leaving the container unchanged, when it cannot hold
    /// the values.
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError>;

    /// Appends finished field lines under `name`.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the sink when storage cannot retain
    /// the appended values.
    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError>;

    /// Removes every field line stored under `name`.
    fn remove_values(&mut self, name: &'static FieldName);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(feature = "http")]
    use http::{HeaderMap, HeaderValue, header};

    use super::{CollectOutput, FIELD_VALUE_INLINE_CAPACITY, FieldSink, reserve_bytes};
    use crate::sink::{
        EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSensitivity, FieldValueWriter, InsertError, InsertErrorKind, U64Encoder,
        ValueRefsEncoder,
    };
    use crate::source::{FieldLines, FieldSource};
    use crate::{FieldName, FieldValue, FieldValueRef};

    type BorrowedFieldValue<'a> = FieldValueRef<'a>;

    #[derive(Default)]
    struct Sink {
        values: Vec<FieldValue>,
        append_calls: usize,
    }

    impl FieldSource for Sink {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            FieldLines::from_slice(name, &self.values)
        }
    }

    impl FieldSink for Sink {
        fn set_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
            self.values = values.into_iter().collect();
            Ok(())
        }

        fn append_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
            self.append_calls += 1;
            self.values.extend(values);
            Ok(())
        }

        fn remove_values(&mut self, _name: &'static FieldName) {
            self.values.clear();
        }
    }

    struct BytesEncoder {
        expected: usize,
        bytes: &'static [u8],
        sensitive: bool,
    }

    impl FieldEncoder for BytesEncoder {
        fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
        where
            O: FieldEncodeOutput,
        {
            let mut writer = output.begin_value(self.expected, crate::sink::FieldSensitivity::from_sensitive(self.sensitive))?;
            writer.write_bytes(self.bytes)?;
            writer.finish()
        }
    }

    struct RejectBegin;

    struct RejectWrite;

    struct RejectFinish;

    struct RejectOwned;

    struct Writer {
        reject_write: bool,
        reject_finish: bool,
    }

    impl FieldValueWriter for Writer {
        fn write_bytes(&mut self, _bytes: &[u8]) -> Result<(), InsertError> {
            if self.reject_write {
                Err(InsertError::new(InsertErrorKind::InvalidEncoding))
            } else {
                Ok(())
            }
        }

        fn finish(self) -> Result<(), InsertError> {
            if self.reject_finish {
                Err(InsertError::new(InsertErrorKind::InvalidValue))
            } else {
                Ok(())
            }
        }
    }

    macro_rules! writer_output {
        ($output:ty, $write:literal, $finish:literal) => {
            impl FieldEncodeOutput for $output {
                type Writer<'a> = Writer;

                fn begin_value(&mut self, _length: usize, _sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError> {
                    Ok(Writer {
                        reject_write: $write,
                        reject_finish: $finish,
                    })
                }

                fn push_value(&mut self, _value: FieldValue) -> Result<(), InsertError> {
                    Ok(())
                }
            }
        };
    }

    impl FieldEncodeOutput for RejectBegin {
        type Writer<'a> = Writer;

        fn begin_value(&mut self, _length: usize, _sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError> {
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        }

        fn push_value(&mut self, _value: FieldValue) -> Result<(), InsertError> {
            Ok(())
        }
    }

    writer_output!(RejectWrite, true, false);
    writer_output!(RejectFinish, false, true);

    impl FieldEncodeOutput for RejectOwned {
        type Writer<'a> = Writer;

        fn begin_value(&mut self, _length: usize, _sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError> {
            Ok(Writer {
                reject_write: false,
                reject_finish: false,
            })
        }

        fn push_value(&mut self, _value: FieldValue) -> Result<(), InsertError> {
            Err(InsertError::new(InsertErrorKind::CapacityExceeded))
        }
    }

    #[test]
    fn default_sink_encoders_cover_owned_borrowed_numeric_and_append_paths() {
        let mut sink = Sink::default();
        assert!(!FieldSource::contains(&sink, &FieldName::ContentLength));
        assert!(!FieldSource::contains(&&sink, &FieldName::ContentLength));

        sink.set_encoded(&FieldName::ContentLength, U64Encoder::new(0))
            .expect("zero encodes");
        assert_eq!(sink.values[0].as_bytes(), b"0");

        sink.set_encoded(&FieldName::ContentLength, U64Encoder::new(u64::MAX))
            .expect("maximum encodes");
        assert_eq!(sink.values[0].as_bytes(), u64::MAX.to_string().as_bytes());

        sink.set_encoded(
            &FieldName::Authorization,
            BorrowedFieldValue::new(b"Bearer token").with_sensitive(true),
        )
        .expect("borrowed value encodes");
        assert!(sink.values[0].is_sensitive());

        let borrowed = [FieldValueRef::new(b"a=1"), FieldValueRef::new(b"b=2")];
        sink.set_encoded(&FieldName::SetCookie, ValueRefsEncoder::new(borrowed).with_sensitive(true))
            .expect("borrowed values encode");
        assert_eq!(sink.values.len(), 2);
        assert!(sink.values.iter().all(FieldValue::is_sensitive));

        let borrowed = [FieldValueRef::new(b"public"), FieldValueRef::new(b"private").with_sensitive(true)];
        sink.set_encoded(&FieldName::Vary, ValueRefsEncoder::new(borrowed))
            .expect("borrowed values preserve markers");
        assert!(!sink.values[0].is_sensitive());
        assert!(sink.values[1].is_sensitive());

        sink.set_encoded(&FieldName::UserAgent, FieldValue::from_static("client/1"))
            .expect("owned value transfers");
        sink.append_encoded(&FieldName::UserAgent, EncodedValues::single(FieldValue::from_static("client/2")))
            .expect("owned values append");
        assert_eq!(sink.values.len(), 2);
        assert_eq!(sink.append_calls, 1);

        sink.remove_values(&FieldName::UserAgent);
        assert!(sink.values.is_empty());
    }

    #[test]
    fn default_sink_rejects_bad_encoders_without_replacing_values() {
        let mut sink = Sink {
            values: vec![FieldValue::from_static("original")],
            append_calls: 0,
        };
        for (encoder, expected_kind) in [
            BytesEncoder {
                expected: 2,
                bytes: b"x",
                sensitive: false,
            },
            BytesEncoder {
                expected: 1,
                bytes: b"\n",
                sensitive: false,
            },
            BytesEncoder {
                expected: 0,
                bytes: b"x",
                sensitive: false,
            },
            BytesEncoder {
                expected: usize::MAX,
                bytes: b"x",
                sensitive: false,
            },
        ]
        .into_iter()
        .zip([
            InsertErrorKind::InvalidEncoding,
            InsertErrorKind::InvalidValue,
            InsertErrorKind::InvalidEncoding,
            InsertErrorKind::CapacityExceeded,
        ]) {
            assert_eq!(
                sink.set_encoded(&FieldName::UserAgent, encoder),
                Err(InsertError::new(expected_kind))
            );
            assert_eq!(sink.values[0].as_bytes(), b"original");
        }

        assert_eq!(
            sink.append_encoded(
                &FieldName::UserAgent,
                BytesEncoder {
                    expected: 1,
                    bytes: b"\n",
                    sensitive: false,
                },
            ),
            Err(InsertError::new(InsertErrorKind::InvalidValue))
        );
        assert_eq!(sink.values.len(), 1);
        assert_eq!(sink.append_calls, 0);
    }

    #[test]
    fn impossible_capacities_are_rejected_before_sink_mutation() {
        let mut sink = Sink {
            values: vec![FieldValue::from_static("original")],
            append_calls: 0,
        };
        for length in [isize::MAX as usize + 1, usize::MAX] {
            let encoder = || BytesEncoder {
                expected: length,
                bytes: b"x",
                sensitive: false,
            };
            let expected = Err(InsertError::new(InsertErrorKind::CapacityExceeded));
            assert_eq!(sink.set_encoded(&FieldName::UserAgent, encoder()), expected);
            assert_eq!(sink.append_encoded(&FieldName::UserAgent, encoder()), expected);
            assert_eq!(sink.values, [FieldValue::from_static("original")]);
            assert_eq!(sink.append_calls, 0);

            #[cfg(feature = "http")]
            {
                let mut map = HeaderMap::new();
                map.insert(header::USER_AGENT, HeaderValue::from_static("original"));
                let original = map.clone();
                assert_eq!(map.set_encoded(&FieldName::UserAgent, encoder()), expected);
                assert_eq!(map.append_encoded(&FieldName::UserAgent, encoder()), expected);
                assert_eq!(map, original);
            }
        }
    }

    #[test]
    fn buffer_reservation_reports_failure_without_a_large_allocation() {
        // Capacity overflow exercises reservation failure without requesting memory.
        assert_eq!(reserve_bytes(usize::MAX), Err(InsertError::new(InsertErrorKind::AllocationFailed)));
        let buffer = reserve_bytes(FIELD_VALUE_INLINE_CAPACITY + 1).unwrap();
        assert!(buffer.is_empty());
        assert!(buffer.capacity() > FIELD_VALUE_INLINE_CAPACITY);
    }

    #[test]
    fn encoders_propagate_each_output_failure_without_masking_it() {
        assert_eq!(
            U64Encoder::new(42).encode(&mut RejectBegin),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        assert_eq!(
            U64Encoder::new(42).encode(&mut RejectWrite),
            Err(InsertError::new(InsertErrorKind::InvalidEncoding))
        );
        assert_eq!(
            FieldValueRef::new(b"value").encode(&mut RejectBegin),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        assert_eq!(
            FieldValueRef::new(b"value").encode(&mut RejectWrite),
            Err(InsertError::new(InsertErrorKind::InvalidEncoding))
        );
        assert_eq!(
            FieldValueRef::new(b"value").encode(&mut RejectFinish),
            Err(InsertError::new(InsertErrorKind::InvalidValue))
        );

        let borrowed = [FieldValueRef::new(b"value")];
        assert_eq!(
            ValueRefsEncoder::new(borrowed).encode(&mut RejectWrite),
            Err(InsertError::new(InsertErrorKind::InvalidEncoding))
        );
        assert_eq!(
            EncodedValues::single(FieldValue::from_static("value")).encode(&mut RejectOwned),
            Err(InsertError::new(InsertErrorKind::CapacityExceeded))
        );
        assert_eq!(FieldValue::from_static("value").encode(&mut RejectWrite), Ok(()));
        assert_eq!(FieldValue::from_static("value").encode(&mut RejectBegin), Ok(()));
        assert_eq!(FieldValueRef::new(b"value").encode(&mut RejectOwned), Ok(()));

        let encoder = BytesEncoder {
            expected: 5,
            bytes: b"value",
            sensitive: false,
        };
        assert_eq!(
            encoder.encode(&mut RejectBegin),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        let encoder = BytesEncoder {
            expected: 5,
            bytes: b"value",
            sensitive: false,
        };
        assert_eq!(
            encoder.encode(&mut RejectWrite),
            Err(InsertError::new(InsertErrorKind::InvalidEncoding))
        );

        let sink = Sink {
            values: vec![FieldValue::from_static("value")],
            append_calls: 0,
        };
        assert_eq!(
            FieldSource::lines(&&sink, &FieldName::UserAgent)
                .expect("forwarded values")
                .exactly_one()
                .expect("one value"),
            "value"
        );
    }

    #[test]
    fn default_writer_handles_inline_boundary_spill_and_chunking() {
        for length in [FIELD_VALUE_INLINE_CAPACITY, FIELD_VALUE_INLINE_CAPACITY + 1] {
            let bytes = vec![b'a'; length];
            let mut output = CollectOutput {
                values: EncodedValues::new(),
            };
            let mut writer = output.begin_value(length, FieldSensitivity::Sensitive).expect("writer starts");
            for chunk in bytes.chunks(7) {
                writer.write_bytes(chunk).expect("chunk writes");
            }
            writer.finish().expect("complete value finishes");

            let value = output.values.iter().next().expect("one value");
            assert_eq!(value.as_bytes(), bytes);
            assert!(value.is_sensitive());
        }
    }
}
