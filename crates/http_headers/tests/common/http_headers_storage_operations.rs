// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Storage benchmark operations shared with ownership regression tests.

use std::hint::black_box;

use http::HeaderMap;
use http_headers::headers::{ContentLength, ContentType};
use http_headers::sink::{EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSink, FieldSinkExt, FieldValueWriter, InsertError};
use http_headers::{Field, FieldName, FieldValue, FieldValueRef};

pub(super) type AppendInput = (HeaderMap, &'static FieldName, EncodedValues);

/// Keep the map alive until the benchmark harness ends measurement and drops its output.
pub(super) type MapOutput = (usize, HeaderMap);

struct ChunkedEncoder {
    bytes: &'static [u8],
    chunks: usize,
}

impl FieldEncoder for ChunkedEncoder {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        let mut writer = output.begin_value(self.bytes.len(), http_headers::FieldSensitivity::NonSensitive)?;
        if !self.bytes.is_empty() {
            for piece in self.bytes.chunks(self.bytes.len().div_ceil(self.chunks)) {
                writer.write_bytes(piece)?;
            }
        }
        writer.finish()
    }
}

pub(super) fn http_writer_borrowed(state: (HeaderMap, &'static str)) -> MapOutput {
    let (mut map, value) = state;
    map.set_encoded(<ContentType as Field>::name(), FieldValueRef::new(value.as_bytes()))
        .expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}

pub(super) fn http_writer_streamed(state: (HeaderMap, &'static str)) -> MapOutput {
    let (mut map, value) = state;
    map.set_encoded(
        <ContentType as Field>::name(),
        ChunkedEncoder {
            bytes: value.as_bytes(),
            chunks: 4,
        },
    )
    .expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}

pub(super) fn http_writer_streamed_sized(state: (HeaderMap, &'static [u8])) -> MapOutput {
    let (mut map, value) = state;
    map.set_encoded(<ContentType as Field>::name(), ChunkedEncoder { bytes: value, chunks: 4 })
        .expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}

pub(super) fn http_writer_materialized(state: (HeaderMap, FieldValue)) -> MapOutput {
    let (mut map, value) = state;
    map.set_values(<ContentType as Field>::name(), EncodedValues::single(value))
        .expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}

pub(super) fn http_append_values(input: AppendInput) -> MapOutput {
    let (mut map, name, values) = input;
    map.append_values(name, values).expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}

pub(super) fn content_length_materialized(mut map: HeaderMap) -> MapOutput {
    map.set_values(<ContentLength as Field>::name(), EncodedValues::single(FieldValue::from(1_024_u64)))
        .expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}

pub(super) fn content_length_deferred_http(mut map: HeaderMap) -> MapOutput {
    map.set_content_length(1_024).expect("preallocated map has capacity");
    let length = black_box(map.len());
    (length, map)
}
