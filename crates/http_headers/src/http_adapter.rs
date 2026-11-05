// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Optional integration with the `http` crate.
//!
//! This module adapts `http::HeaderMap` to [`FieldSource`] and [`FieldSink`]
//! and converts values at the integration boundary. The core types do not use
//! `http` types as their backing representation.

use std::mem;

use http::header::Entry;
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response};
use smallvec::SmallVec;

use crate::sink::{
    EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSensitivity, FieldSink, FieldValueWriter, InsertError, InsertErrorKind,
};
use crate::source::{FieldLines, FieldSource};
use crate::{FieldName, FieldValue};

impl FieldSource for HeaderMap {
    #[expect(clippy::inline_always, reason = "header decoding must inline the external map adapter")]
    #[inline(always)]
    fn contains(&self, name: &'static FieldName) -> bool {
        match name.http_name() {
            Some(http_name) => self.contains_key(http_name),
            None => self.contains_key(HeaderName::from_static(name.as_str())),
        }
    }

    #[expect(clippy::inline_always, reason = "every typed decode crosses this hot adapter boundary")]
    #[inline(always)]
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        // A well-known name carries the matching `http` constant, while a
        // custom typed name supplies validated static bytes without allocation.
        match name.http_name() {
            Some(http_name) => FieldLines::from_http(name, self.get_all(http_name)),
            None => FieldLines::from_http(name, self.get_all(HeaderName::from_static(name.as_str()))),
        }
    }
}

impl FieldSink for HeaderMap {
    fn set_encoded<E>(&mut self, name: &'static FieldName, encoder: E) -> Result<(), InsertError>
    where
        E: FieldEncoder,
    {
        let mut output = HttpOutput::default();
        encoder.encode(&mut output)?;
        insert_http_values(self, http_name(name), output.values)
    }

    fn append_encoded<E>(&mut self, name: &'static FieldName, encoder: E) -> Result<(), InsertError>
    where
        E: FieldEncoder,
    {
        let mut output = HttpOutput::default();
        encoder.encode(&mut output)?;
        // #[gamma::skip(cond.always_false, relational.gt_to_ge, literal.int_decrement, literal.int_increment, tag = "equivalent", reason = "Both append paths reserve the exact encoded count before appending every value with the same name; changing this threshold only selects identical inline or outlined implementations")]
        if output.values.len() > 1 {
            return append_http_values(self, http_name(name), output.values);
        }
        self.try_reserve(output.values.len())
            .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?;
        let name = http_name(name);
        for value in output.values {
            self.append(name.clone(), value);
        }
        Ok(())
    }

    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.set_encoded(name, values)
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.append_encoded(name, values)
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        match name.http_name() {
            Some(http_name) => {
                drop(self.remove(http_name));
            }
            None => {
                drop(self.remove(HeaderName::from_static(name.as_str())));
            }
        }
    }
}

macro_rules! impl_http_message {
    ($message:ident) => {
        impl<B> FieldSource for $message<B> {
            fn contains(&self, name: &'static FieldName) -> bool {
                FieldSource::contains(self.headers(), name)
            }

            fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
                FieldSource::lines(self.headers(), name)
            }
        }

        impl<B> FieldSink for $message<B> {
            fn set_encoded<E>(&mut self, name: &'static FieldName, encoder: E) -> Result<(), InsertError>
            where
                E: FieldEncoder,
            {
                self.headers_mut().set_encoded(name, encoder)
            }

            fn append_encoded<E>(&mut self, name: &'static FieldName, encoder: E) -> Result<(), InsertError>
            where
                E: FieldEncoder,
            {
                self.headers_mut().append_encoded(name, encoder)
            }

            fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
                self.headers_mut().set_values(name, values)
            }

            fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
                self.headers_mut().append_values(name, values)
            }

            fn remove_values(&mut self, name: &'static FieldName) {
                self.headers_mut().remove_values(name);
            }
        }
    };
}

impl_http_message!(Request);
impl_http_message!(Response);

fn http_name(name: &'static FieldName) -> HeaderName {
    match name.http_name() {
        Some(known) => known.clone(),
        None => HeaderName::from_static(name.as_str()),
    }
}

#[derive(Default)]
struct HttpOutput {
    values: SmallVec<[HeaderValue; 1]>,
}

struct HttpWriter<'a> {
    values: &'a mut SmallVec<[HeaderValue; 1]>,
    state: WriterState,
    expected: usize,
    sensitive: bool,
}

/// What a writer has taken so far.
///
/// A borrowed encoder hands over its whole field line in one `write_bytes` of
/// an already-contiguous slice, which becomes the header value directly: the
/// value is validated and copied once, and no intermediate buffer is ever
/// created. Anything that writes in pieces — or that writes a length other
/// than the one it announced — accumulates into a buffer instead, so the
/// length and validity checks still happen in `finish`.
enum WriterState {
    Empty,
    Single(HeaderValue),
    Buffered(SmallVec<[u8; 32]>),
}

fn buffered(expected: usize, bytes: &[u8]) -> Result<SmallVec<[u8; 32]>, InsertError> {
    #[cfg(test)]
    if tests::FAIL_NEXT_BUFFER_RESERVATION.replace(false) {
        return Err(InsertError::new(InsertErrorKind::AllocationFailed));
    }
    let mut buffer = SmallVec::new();
    buffer
        .try_reserve_exact(expected)
        .map_err(|_error| InsertError::new(InsertErrorKind::AllocationFailed))?;
    buffer.extend_from_slice(bytes);
    Ok(buffer)
}

fn try_push_http_value(values: &mut SmallVec<[HeaderValue; 1]>, value: HeaderValue) -> Result<(), InsertError> {
    reserve_http_values(values, 1)?;
    values.push(value);
    Ok(())
}

fn reserve_http_values(values: &mut SmallVec<[HeaderValue; 1]>, additional: usize) -> Result<(), InsertError> {
    #[cfg(test)]
    tests::LAST_VALUE_RESERVATION.set(Some(additional));
    #[cfg(test)]
    if tests::FAIL_NEXT_VALUE_RESERVATION.replace(false) {
        return Err(InsertError::new(InsertErrorKind::AllocationFailed));
    }
    values
        .try_reserve(additional)
        .map_err(|_error| InsertError::new(InsertErrorKind::AllocationFailed))?;
    Ok(())
}

impl FieldEncodeOutput for HttpOutput {
    type Writer<'a> = HttpWriter<'a>;

    fn begin_value(&mut self, length: usize, sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError> {
        if length > isize::MAX as usize {
            return Err(InsertError::new(InsertErrorKind::CapacityExceeded));
        }
        Ok(HttpWriter {
            values: &mut self.values,
            state: WriterState::Empty,
            expected: length,
            sensitive: sensitivity.is_sensitive(),
        })
    }

    fn push_value(&mut self, value: FieldValue) -> Result<(), InsertError> {
        // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "Every owned FieldValue already satisfies the HTTP field-value byte invariant required by HeaderValue")]
        let value = HeaderValue::try_from(value).map_err(|_invalid| InsertError::new(InsertErrorKind::InvalidValue))?;
        try_push_http_value(&mut self.values, value)
    }

    fn push_u64(&mut self, value: u64) -> Result<(), InsertError> {
        try_push_http_value(&mut self.values, HeaderValue::from(value))
    }
}

impl FieldValueWriter for HttpWriter<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), InsertError> {
        let written = match &self.state {
            WriterState::Empty => 0,
            WriterState::Single(value) => value.as_bytes().len(),
            WriterState::Buffered(buffer) => buffer.len(),
        };
        if bytes.len() > self.expected.saturating_sub(written) {
            return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
        }
        self.state = match mem::replace(&mut self.state, WriterState::Empty) {
            // The single-shot case: the announced length arrives whole, so it
            // becomes the header value here and `finish` only has to push it.
            // An invalid slice falls back to the buffer so that the error
            // still surfaces from `finish`, as it does for every other writer.
            WriterState::Empty if bytes.len() == self.expected => match HeaderValue::from_bytes(bytes) {
                Ok(value) => WriterState::Single(value),
                Err(_invalid) => WriterState::Buffered(buffered(self.expected, bytes)?),
            },
            WriterState::Empty => WriterState::Buffered(buffered(self.expected, bytes)?),
            WriterState::Single(value) => WriterState::Single(value),
            WriterState::Buffered(mut buffer) => {
                buffer.extend_from_slice(bytes);
                WriterState::Buffered(buffer)
            }
        };
        Ok(())
    }

    fn finish(self) -> Result<(), InsertError> {
        let mut value = match self.state {
            WriterState::Single(value) => value,
            WriterState::Empty => {
                if self.expected != 0 {
                    return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
                }
                HeaderValue::from_static("")
            }
            WriterState::Buffered(bytes) => {
                if bytes.len() != self.expected {
                    return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
                }
                if bytes.spilled() {
                    #[cfg(test)]
                    tests::LAST_BUFFER_CONVERSION.set(Some("owned"));
                    // `into_vec` retains the spilled allocation; doing this for
                    // inline bytes would allocate before constructing the value.
                    HeaderValue::try_from(bytes.into_vec()).map_err(|_invalid| InsertError::new(InsertErrorKind::InvalidValue))?
                } else {
                    #[cfg(test)]
                    tests::LAST_BUFFER_CONVERSION.set(Some("copied"));
                    HeaderValue::from_bytes(&bytes).map_err(|_invalid| InsertError::new(InsertErrorKind::InvalidValue))?
                }
            }
        };
        value.set_sensitive(self.sensitive);
        try_push_http_value(self.values, value)
    }
}

fn append_http_values(map: &mut HeaderMap, name: HeaderName, encoded: SmallVec<[HeaderValue; 1]>) -> Result<(), InsertError> {
    map.try_reserve(encoded.len())
        .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?;
    let mut values = encoded.into_iter();
    let first = values
        .next()
        .expect("append_http_values is called only for multiple encoded values");
    // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "try_reserve above reserved room for every encoded value; encoded is nonempty here and no map mutation has consumed the reserved capacity")]
    match map
        .try_entry(name)
        .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?
    {
        Entry::Occupied(mut entry) => {
            entry.append(first);
            for value in values {
                entry.append(value);
            }
        }
        Entry::Vacant(entry) => {
            // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The successful reservation above covers this first insertion and no intervening insertion has consumed it")]
            let mut entry = entry
                .try_insert_entry(first)
                .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?;
            for value in values {
                entry.append(value);
            }
        }
    }
    Ok(())
}

/// Replaces every value stored under `name` with the encoded field lines.
fn insert_http_values(map: &mut HeaderMap, name: HeaderName, encoded: SmallVec<[HeaderValue; 1]>) -> Result<(), InsertError> {
    map.try_reserve(encoded.len())
        .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?;
    let mut values = encoded.into_iter();
    let Some(first) = values.next() else {
        map.remove(&name);
        return Ok(());
    };
    // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "try_reserve above reserved room for every encoded value; the empty case returned and no intervening insertion consumed that capacity")]
    match map
        .try_entry(name)
        .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?
    {
        Entry::Occupied(mut entry) => {
            entry.insert(first);
            for value in values {
                entry.append(value);
            }
        }
        Entry::Vacant(entry) => {
            // #[gamma::skip(try.propagate_to_unwrap, tag = "equivalent", reason = "The successful reservation above covers this first insertion and no intervening insertion has consumed it")]
            let mut entry = entry
                .try_insert_entry(first)
                .map_err(|_full| InsertError::new(InsertErrorKind::CapacityExceeded))?;
            for value in values {
                entry.append(value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::LazyLock;

    use http::{HeaderMap, HeaderValue};

    use super::{HttpOutput, append_http_values, buffered, insert_http_values, reserve_http_values, try_push_http_value};
    use crate::sink::{EncodedValues, FieldEncodeOutput, FieldSink, FieldValueWriter, InsertError, InsertErrorKind, U64Encoder};
    use crate::source::FieldSource;
    use crate::{FieldValue, FieldValueRef};

    static CUSTOM: LazyLock<crate::FieldName> = LazyLock::new(|| crate::FieldName::from_static("x-trace-id"));

    const ENCODING_ERROR: InsertError = InsertError::new(InsertErrorKind::InvalidEncoding);
    const VALUE_ERROR: InsertError = InsertError::new(InsertErrorKind::InvalidValue);
    const CAPACITY_ERROR: InsertError = InsertError::new(InsertErrorKind::CapacityExceeded);

    // One-shot, thread-local faults exercise propagation without exhausting memory.
    std::thread_local! {
        pub(super) static FAIL_NEXT_BUFFER_RESERVATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        pub(super) static FAIL_NEXT_VALUE_RESERVATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        pub(super) static LAST_VALUE_RESERVATION: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
        pub(super) static LAST_BUFFER_CONVERSION: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn http_adapter_gamma_afternoon_preserves_native_and_buffered_ownership_policies() {
        for length in [1, 64, 65] {
            let bytes = vec![b'a'; length];
            let mut output = HttpOutput::default();
            let mut writer = output.begin_value(length, crate::sink::FieldSensitivity::Sensitive).unwrap();
            writer.write_bytes(&bytes).unwrap();
            assert!(matches!(&writer.state, super::WriterState::Single(_)));
            writer.finish().unwrap();
            assert_eq!(output.values[0].as_bytes(), bytes);
            assert!(output.values[0].is_sensitive());
        }
        for length in [32, 33] {
            let bytes = vec![b'a'; length];
            let mut output = HttpOutput::default();
            let mut writer = output.begin_value(length, crate::sink::FieldSensitivity::Sensitive).unwrap();
            writer.write_bytes(&bytes[..1]).unwrap();
            writer.write_bytes(&bytes[1..]).unwrap();
            let pointer = match &writer.state {
                super::WriterState::Buffered(buffer) => buffer.as_ptr(),
                _ => panic!("a partial first write must use buffered storage"),
            };
            LAST_BUFFER_CONVERSION.set(None);
            writer.finish().unwrap();
            assert_eq!(LAST_BUFFER_CONVERSION.get(), Some(if length > 32 { "owned" } else { "copied" }));
            assert_eq!(output.values[0].as_bytes(), bytes);
            assert!(output.values[0].is_sensitive());
            if length > 32 {
                assert!(std::ptr::eq(output.values[0].as_bytes().as_ptr(), pointer));
            }
        }
    }

    #[test]
    fn http_adapter_gamma_afternoon_returns_reservation_failures_without_committing() {
        let mut map = HeaderMap::new();
        map.insert(http::header::USER_AGENT, HeaderValue::from_static("original"));
        let original = map.clone();
        FAIL_NEXT_VALUE_RESERVATION.set(true);
        assert_eq!(
            map.set_encoded(&crate::FieldName::UserAgent, FieldValue::from_static("new")),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        assert!(!FAIL_NEXT_VALUE_RESERVATION.get());
        assert_eq!(LAST_VALUE_RESERVATION.get(), Some(1));
        assert_eq!(map, original);

        FAIL_NEXT_VALUE_RESERVATION.set(true);
        assert_eq!(
            map.append_encoded(&crate::FieldName::UserAgent, U64Encoder::new(42)),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        assert!(!FAIL_NEXT_VALUE_RESERVATION.get());
        assert_eq!(LAST_VALUE_RESERVATION.get(), Some(1));
        assert_eq!(map, original);
    }

    #[test]
    fn http_adapter_gamma_afternoon_returns_both_buffer_reservation_failures() {
        let mut map = HeaderMap::new();
        map.insert(http::header::USER_AGENT, HeaderValue::from_static("original"));
        let original = map.clone();
        let invalid = [b'\n'; 64];
        FAIL_NEXT_BUFFER_RESERVATION.set(true);
        assert_eq!(
            map.set_encoded(&crate::FieldName::UserAgent, FieldValueRef::new(&invalid)),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        assert!(!FAIL_NEXT_BUFFER_RESERVATION.get());
        assert_eq!(map, original);

        let mut output = HttpOutput::default();
        let mut writer = output.begin_value(64, crate::sink::FieldSensitivity::NonSensitive).unwrap();
        FAIL_NEXT_BUFFER_RESERVATION.set(true);
        assert_eq!(writer.write_bytes(b"x"), Err(InsertError::new(InsertErrorKind::AllocationFailed)));
        assert!(!FAIL_NEXT_BUFFER_RESERVATION.get());
        assert_eq!(writer.finish(), Err(ENCODING_ERROR));
        assert!(output.values.is_empty());
    }

    struct RejectEncoder;

    impl crate::sink::FieldEncoder for RejectEncoder {
        fn encode<O>(self, _output: &mut O) -> Result<(), InsertError>
        where
            O: FieldEncodeOutput,
        {
            Err(ENCODING_ERROR)
        }
    }

    #[test]
    fn map_source_and_sink_cover_known_and_custom_names_and_entry_states() {
        let mut map = http::HeaderMap::new();
        assert!(!map.contains(&crate::FieldName::UserAgent));
        assert!(!map.contains(&CUSTOM));
        assert!(FieldSource::lines(&map, &crate::FieldName::UserAgent).is_none());
        assert!(FieldSource::lines(&map, &CUSTOM).is_none());

        map.set_values(
            &crate::FieldName::UserAgent,
            EncodedValues::from_vec(vec![FieldValue::from_static("client/1"), FieldValue::from_static("client/2")]),
        )
        .expect("known values insert");
        assert!(map.contains(&crate::FieldName::UserAgent));
        assert_eq!(
            FieldSource::lines(&map, &crate::FieldName::UserAgent)
                .expect("known values")
                .repeated()
                .map(crate::FieldValueRef::as_bytes)
                .collect::<Vec<_>>(),
            [b"client/1".as_slice(), b"client/2".as_slice()]
        );

        map.set_values(
            &crate::FieldName::UserAgent,
            EncodedValues::from_vec(vec![FieldValue::from_static("replacement"), FieldValue::from_static("additional")]),
        )
        .expect("occupied entry replaces");
        assert_eq!(map[http::header::USER_AGENT], "replacement");
        assert_eq!(map.get_all(http::header::USER_AGENT).iter().count(), 2);

        map.set_encoded(&CUSTOM, FieldValueRef::new(b"trace").with_sensitive(true))
            .expect("custom value inserts");
        assert!(map.contains(&CUSTOM));
        assert!(map.get("x-trace-id").expect("custom value").is_sensitive());

        map.append_encoded(&CUSTOM, U64Encoder::new(42)).expect("custom value appends");
        assert_eq!(map.get_all("x-trace-id").iter().count(), 2);

        map.remove_values(&crate::FieldName::UserAgent);
        map.remove_values(&CUSTOM);
        assert!(!map.contains(&crate::FieldName::UserAgent));
        assert!(!map.contains(&CUSTOM));

        assert_eq!(map.set_encoded(&crate::FieldName::UserAgent, RejectEncoder), Err(ENCODING_ERROR));
        assert_eq!(map.append_encoded(&crate::FieldName::UserAgent, RejectEncoder), Err(ENCODING_ERROR));
        assert!(!map.contains(&crate::FieldName::UserAgent));

        insert_http_values(&mut map, http::header::USER_AGENT, smallvec::SmallVec::new()).expect("empty values remove");
        assert!(!map.contains_key(http::header::USER_AGENT));

        append_http_values(
            &mut map,
            http::header::SET_COOKIE,
            smallvec::smallvec![http::HeaderValue::from_static("a=1"), http::HeaderValue::from_static("b=2"),],
        )
        .expect("vacant entry accepts repeated values");
        append_http_values(
            &mut map,
            http::header::SET_COOKIE,
            smallvec::smallvec![http::HeaderValue::from_static("c=3"), http::HeaderValue::from_static("d=4"),],
        )
        .expect("occupied entry accepts repeated values");
        assert_eq!(
            map.get_all(http::header::SET_COOKIE)
                .iter()
                .map(http::HeaderValue::as_bytes)
                .collect::<Vec<_>>(),
            [b"a=1".as_slice(), b"b=2".as_slice(), b"c=3".as_slice(), b"d=4".as_slice(),]
        );
    }

    #[test]
    fn http_output_validates_lengths_bytes_sensitivity_and_native_values() {
        let mut output = HttpOutput::default();
        let mut writer = output
            .begin_value(3, crate::sink::FieldSensitivity::Sensitive)
            .expect("writer starts");
        writer.write_bytes(b"abc").expect("bytes append");
        writer.finish().expect("exact valid value finishes");
        assert_eq!(output.values[0], "abc");
        assert!(output.values[0].is_sensitive());

        let mut output = HttpOutput::default();
        let mut short = output
            .begin_value(2, crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        short.write_bytes(b"x").expect("bytes append");
        assert_eq!(short.finish(), Err(ENCODING_ERROR));

        let mut output = HttpOutput::default();
        let mut invalid = output
            .begin_value(1, crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        invalid.write_bytes(b"\n").expect("bytes append");
        assert_eq!(invalid.finish(), Err(VALUE_ERROR));

        output.push_value(FieldValue::from_static("owned")).expect("owned value transfers");
        output.push_u64(u64::MAX).expect("integer value formats");
        assert_eq!(output.values[0], "owned");
        assert_eq!(output.values[1], u64::MAX.to_string());
    }

    #[test]
    fn http_writer_streams_overruns_and_empty_values_without_a_single_shot_buffer() {
        // A streaming encoder writes in pieces, so the buffered path has to
        // reassemble the announced length.
        let mut output = HttpOutput::default();
        let mut writer = output
            .begin_value(6, crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        writer.write_bytes(b"abc").expect("first piece appends");
        writer.write_bytes(b"def").expect("second piece appends");
        writer.finish().expect("streamed value finishes");
        assert_eq!(output.values[0], "abcdef");
        assert!(!output.values[0].is_sensitive());

        let bytes = [b'x'; 65];
        let mut output = HttpOutput::default();
        let mut writer = output
            .begin_value(bytes.len(), crate::sink::FieldSensitivity::Sensitive)
            .expect("writer starts");
        for chunk in bytes.chunks(7) {
            writer.write_bytes(chunk).expect("chunk appends");
        }
        writer.finish().expect("spilled streamed value finishes");
        assert_eq!(output.values[0].as_bytes(), bytes);
        assert!(output.values[0].is_sensitive());

        let mut invalid_bytes = [b'x'; 65];
        invalid_bytes[32] = b'\n';
        let mut output = HttpOutput::default();
        let mut writer = output
            .begin_value(invalid_bytes.len(), crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        for chunk in invalid_bytes.chunks(7) {
            writer.write_bytes(chunk).expect("chunk appends");
        }
        assert_eq!(writer.finish(), Err(VALUE_ERROR));
        assert!(output.values.is_empty());

        // A complete single-shot write rejects more bytes before buffering.
        let mut output = HttpOutput::default();
        let mut writer = output
            .begin_value(3, crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        writer.write_bytes(b"abc").expect("whole value appends");
        writer.write_bytes(b"").expect("an empty continuation is a no-op");
        assert_eq!(writer.write_bytes(b"d"), Err(ENCODING_ERROR));
        assert!(output.values.is_empty());

        // Nothing written at all: legal only for a zero-length value.
        let mut output = HttpOutput::default();
        let writer = output
            .begin_value(0, crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        writer.finish().expect("empty value finishes");
        assert_eq!(output.values[0], "");

        let mut output = HttpOutput::default();
        let writer = output
            .begin_value(1, crate::sink::FieldSensitivity::NonSensitive)
            .expect("writer starts");
        assert_eq!(writer.finish(), Err(ENCODING_ERROR));
        assert!(output.values.is_empty());
    }

    #[test]
    fn http_output_distinguishes_size_limits_from_reservation_failures() {
        let mut output = HttpOutput::default();
        assert_eq!(
            output.begin_value(usize::MAX, crate::sink::FieldSensitivity::NonSensitive).err(),
            Some(CAPACITY_ERROR)
        );
        assert!(output.values.is_empty());
        assert_eq!(buffered(usize::MAX, b"x"), Err(InsertError::new(InsertErrorKind::AllocationFailed)));

        let mut values = smallvec::smallvec![http::HeaderValue::from_static("original")];
        assert_eq!(
            reserve_http_values(&mut values, usize::MAX),
            Err(InsertError::new(InsertErrorKind::AllocationFailed))
        );
        assert_eq!(values.as_slice(), &[http::HeaderValue::from_static("original")]);
        try_push_http_value(&mut values, http::HeaderValue::from_static("second")).unwrap();
        assert_eq!(values.len(), 2);
    }

    /// The number of entries an `http::HeaderMap` holds once it can no longer
    /// grow: `http` caps its index table at `1 << 15` slots and keeps three
    /// quarters of them usable.
    const MAXIMUM_ENTRIES: usize = (1 << 15) - (1 << 13);

    /// Builds a map that has reached `http`'s maximum entry count, so every
    /// `try_reserve` for an additional entry fails.
    fn saturated_map() -> http::HeaderMap {
        let mut map = http::HeaderMap::with_capacity(MAXIMUM_ENTRIES);
        map.insert(http::header::USER_AGENT, http::HeaderValue::from_static("original"));
        let mut filler = String::new();
        for index in 0..MAXIMUM_ENTRIES - 1 {
            filler.clear();
            filler.push_str("x-fill-");
            filler.push_str(&index.to_string());
            let name = http::HeaderName::from_bytes(filler.as_bytes()).expect("generated name is legal");
            drop(map.insert(name, http::HeaderValue::from_static("v")));
        }
        assert_eq!(map.len(), MAXIMUM_ENTRIES);
        assert!(map.try_reserve(1).is_err(), "the map must be unable to grow");
        map
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "safe error mapping at http's 24,576-entry limit; native tests and cargo careful retain it"
    )]
    fn capacity_exhaustion_reports_an_error_without_changing_the_map() {
        let mut map = saturated_map();

        // A new name needs a new entry the map cannot make room for.
        assert_eq!(map.set_encoded(&CUSTOM, FieldValueRef::new(b"trace")), Err(CAPACITY_ERROR));
        assert!(!map.contains(&CUSTOM));
        assert_eq!(
            map.set_values(&CUSTOM, EncodedValues::single(FieldValue::from_static("trace"))),
            Err(CAPACITY_ERROR)
        );
        assert!(!map.contains(&CUSTOM));
        assert_eq!(map.append_encoded(&CUSTOM, FieldValueRef::new(b"trace")), Err(CAPACITY_ERROR));
        assert!(!map.contains(&CUSTOM));

        // An existing name is rejected too: `try_reserve` guards the whole
        // insertion, and the values already stored stay exactly as they were.
        assert_eq!(
            map.set_values(
                &crate::FieldName::UserAgent,
                EncodedValues::from_vec(vec![FieldValue::from_static("replacement"), FieldValue::from_static("additional"),]),
            ),
            Err(CAPACITY_ERROR)
        );
        assert_eq!(
            map.append_encoded(&crate::FieldName::UserAgent, FieldValueRef::new(b"appended")),
            Err(CAPACITY_ERROR)
        );
        assert_eq!(map[http::header::USER_AGENT], "original");
        assert_eq!(map.get_all(http::header::USER_AGENT).iter().count(), 1);
        assert_eq!(map.len(), MAXIMUM_ENTRIES);

        let mut values = smallvec::SmallVec::<[http::HeaderValue; 1]>::new();
        values.push(http::HeaderValue::from_static("appended"));
        assert_eq!(append_http_values(&mut map, http::header::USER_AGENT, values), Err(CAPACITY_ERROR));
        assert_eq!(map[http::header::USER_AGENT], "original");

        // The private helper is reached the same way through both sinks; call
        // it directly so the failure is attributed to it and not to a caller.
        // Its `try_entry`/`try_insert_entry` error arms stay defensive: the
        // `try_reserve` above already fails for every additional entry the
        // map cannot make room for, so nothing reaches them first.
        let mut values = smallvec::SmallVec::<[http::HeaderValue; 1]>::new();
        values.push(http::HeaderValue::from_static("direct"));
        assert_eq!(
            insert_http_values(&mut map, http::HeaderName::from_static("x-direct"), values),
            Err(CAPACITY_ERROR)
        );
        assert!(!map.contains_key("x-direct"));
        assert_eq!(map.len(), MAXIMUM_ENTRIES);
    }
}
