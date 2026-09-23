// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unified `http_headers` benchmarks for storage and custom-sink encoding decisions.

use std::hint::black_box;
use std::iter;
use std::sync::{LazyLock, OnceLock};

use compact_str::CompactString;
use criterion::{BatchSize, BenchmarkId, Criterion};
use http::HeaderMap;
use http_headers::sink::{EncodedValues, FieldSink, InsertError, ValueRefsEncoder};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{FieldName, FieldValue, FieldValueRef};
use smallvec::SmallVec;

use self::storage_operations::{AppendInput, MapOutput};

#[path = "../tests/common/http_headers_storage_operations.rs"]
mod storage_operations;

const STORAGE_TEXT: &str = "http_headers_storage/storage_text";
const TEXT: &str = "http_headers_storage/text";
const MAP_WRITER: &str = "http_headers_storage/map_writer";
const MAP_APPEND: &str = "http_headers_storage/map_append";
const DEFERRED_INSERTION: &str = "http_headers_storage/deferred_insertion";
const STORAGE_REPEATED_VALUES: &str = "http_headers_storage/storage_repeated_values";
const REPEATED_VALUES: &str = "http_headers_storage/repeated_values";
const STORAGE_INSERTION: &str = "http_headers_storage/storage_insertion";
const INSERTION: &str = "http_headers_storage/insertion";
const CUSTOM_SINK_ENCODING: &str = "http_headers_storage/custom_sink_encoding";

fn short_text() -> &'static str {
    "stale-if-error=30"
}

fn long_text() -> &'static str {
    "extension-directive-with-a-value-that-exceeds-inline-storage=enabled"
}

fn one_value() -> FieldValue {
    FieldValue::from_static("value")
}

fn four_values() -> FieldValue {
    FieldValue::from_static("value")
}

#[metabench::benchmark(SHORT_STRING, TEXT, "short_string")]
#[bench::short_text(setup = short_text)]
fn short_string(value: &str) -> usize {
    let value = String::from(value);
    black_box(value).len()
}

#[metabench::benchmark(SHORT_COMPACT_STRING, TEXT, "short_compact_string")]
#[bench::short_text(setup = short_text)]
fn short_compact_string(value: &str) -> usize {
    let value = CompactString::from(value);
    black_box(value).len()
}

#[metabench::benchmark(LONG_STRING, TEXT, "long_string")]
#[bench::long_text(setup = long_text)]
fn long_string(value: &str) -> usize {
    let value = String::from(value);
    black_box(value).len()
}

#[metabench::benchmark(LONG_COMPACT_STRING, TEXT, "long_compact_string")]
#[bench::long_text(setup = long_text)]
fn long_compact_string(value: &str) -> usize {
    let value = CompactString::from(value);
    black_box(value).len()
}

fn empty_http_map() -> HeaderMap {
    HeaderMap::with_capacity(4)
}

const SHORT_VALUE: &str = "application/json; charset=utf-8";
const LONG_VALUE: &str = "multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW; charset=utf-8";
const DEFAULT_SINK_SHORT_VALUE: &[u8] = &[b's'; 16];
const DEFAULT_SINK_SPILLED_VALUE: &[u8] = &[b'l'; 65];
const STREAMED_0: &[u8] = &[];
const STREAMED_16: &[u8] = &[b's'; 16];
const STREAMED_32: &[u8] = &[b'm'; 32];
const STREAMED_65: &[u8] = &[b'l'; 65];
const STREAMED_4096: &[u8] = &[b'x'; 4096];
const STREAMED_65536: &[u8] = &[b'y'; 65_536];
static CUSTOM_APPEND_NAME: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-cookie-batch"));

#[derive(Default)]
struct MinimalSink {
    values: Vec<FieldValue>,
}

impl FieldSource for MinimalSink {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_slice(name, &self.values)
    }
}

impl FieldSink for MinimalSink {
    fn set_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.values = values.into_iter().collect();
        Ok(())
    }

    fn append_values(&mut self, _name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.values.extend(values);
        Ok(())
    }

    fn remove_values(&mut self, _name: &'static FieldName) {
        self.values.clear();
    }
}

type CustomSinkInput = (FieldValueRef<'static>, usize);
type CustomSinkCase = (&'static str, fn() -> CustomSinkInput);

fn custom_sink_input(value: &'static [u8], line_count: usize) -> CustomSinkInput {
    (FieldValueRef::new(value), line_count)
}

macro_rules! custom_sink_case {
    ($name:ident, $value:ident, $line_count:literal) => {
        fn $name() -> CustomSinkInput {
            custom_sink_input($value, $line_count)
        }
    };
}

custom_sink_case!(short_1, DEFAULT_SINK_SHORT_VALUE, 1);
custom_sink_case!(short_4, DEFAULT_SINK_SHORT_VALUE, 4);
custom_sink_case!(short_16, DEFAULT_SINK_SHORT_VALUE, 16);
custom_sink_case!(short_64, DEFAULT_SINK_SHORT_VALUE, 64);
custom_sink_case!(spilled_1, DEFAULT_SINK_SPILLED_VALUE, 1);
custom_sink_case!(spilled_4, DEFAULT_SINK_SPILLED_VALUE, 4);
custom_sink_case!(spilled_16, DEFAULT_SINK_SPILLED_VALUE, 16);
custom_sink_case!(spilled_64, DEFAULT_SINK_SPILLED_VALUE, 64);

#[metabench::benchmark(MINIMAL_SINK_SET_ENCODED, CUSTOM_SINK_ENCODING, "minimal_sink_set_encoded")]
#[bench::short_1(setup = short_1)]
#[bench::short_4(setup = short_4)]
#[bench::short_16(setup = short_16)]
#[bench::short_64(setup = short_64)]
#[bench::spilled_1(setup = spilled_1)]
#[bench::spilled_4(setup = spilled_4)]
#[bench::spilled_16(setup = spilled_16)]
#[bench::spilled_64(setup = spilled_64)]
fn minimal_sink_set_encoded(input: CustomSinkInput) -> usize {
    let (value, line_count) = input;
    let mut sink = MinimalSink::default();
    sink.set_encoded(
        &FieldName::SetCookie,
        ValueRefsEncoder::new(iter::repeat_n(black_box(value), line_count)),
    )
    .expect("minimal sink accepts encoded values");
    black_box(sink.values.len())
}

#[metabench::benchmark(MINIMAL_SINK_APPEND_ENCODED, CUSTOM_SINK_ENCODING, "minimal_sink_append_encoded")]
#[bench::short_1(setup = short_1)]
#[bench::short_4(setup = short_4)]
#[bench::short_16(setup = short_16)]
#[bench::short_64(setup = short_64)]
#[bench::spilled_1(setup = spilled_1)]
#[bench::spilled_4(setup = spilled_4)]
#[bench::spilled_16(setup = spilled_16)]
#[bench::spilled_64(setup = spilled_64)]
fn minimal_sink_append_encoded(input: CustomSinkInput) -> usize {
    let (value, line_count) = input;
    let mut sink = MinimalSink::default();
    for _ in 0..line_count {
        sink.append_encoded(&FieldName::SetCookie, black_box(value))
            .expect("minimal sink accepts encoded values");
    }
    black_box(sink.values.len())
}

fn short_value_map() -> (HeaderMap, &'static str) {
    (HeaderMap::with_capacity(4), SHORT_VALUE)
}

fn long_value_map() -> (HeaderMap, &'static str) {
    (HeaderMap::with_capacity(4), LONG_VALUE)
}

fn long_owned_map() -> (HeaderMap, FieldValue) {
    (HeaderMap::with_capacity(4), FieldValue::from_static(LONG_VALUE))
}

fn streamed_map(bytes: &'static [u8]) -> (HeaderMap, &'static [u8]) {
    (HeaderMap::with_capacity(4), bytes)
}

macro_rules! streamed_setup {
    ($name:ident, $bytes:ident) => {
        fn $name() -> (HeaderMap, &'static [u8]) {
            streamed_map($bytes)
        }
    };
}

streamed_setup!(streamed_0_map, STREAMED_0);
streamed_setup!(streamed_16_map, STREAMED_16);
streamed_setup!(streamed_32_map, STREAMED_32);
streamed_setup!(streamed_65_map, STREAMED_65);
streamed_setup!(streamed_4096_map, STREAMED_4096);
streamed_setup!(streamed_65536_map, STREAMED_65536);

#[metabench::benchmark(HTTP_WRITER_BORROWED, MAP_WRITER, "http_writer_borrowed")]
#[bench::short(setup = short_value_map)]
#[bench::long(setup = long_value_map)]
fn http_writer_borrowed(state: (HeaderMap, &'static str)) -> MapOutput {
    storage_operations::http_writer_borrowed(state)
}

#[metabench::benchmark(HTTP_WRITER_STREAMED, MAP_WRITER, "http_writer_streamed")]
#[bench::short(setup = short_value_map)]
#[bench::long(setup = long_value_map)]
fn http_writer_streamed(state: (HeaderMap, &'static str)) -> MapOutput {
    storage_operations::http_writer_streamed(state)
}

#[metabench::benchmark(HTTP_WRITER_STREAMED_SIZED, MAP_WRITER, "http_writer_streamed_sized")]
#[bench::bytes_0(setup = streamed_0_map)]
#[bench::bytes_16(setup = streamed_16_map)]
#[bench::bytes_32(setup = streamed_32_map)]
#[bench::bytes_65(setup = streamed_65_map)]
#[bench::bytes_4096(setup = streamed_4096_map)]
#[bench::bytes_65536(setup = streamed_65536_map)]
fn http_writer_streamed_sized(state: (HeaderMap, &'static [u8])) -> MapOutput {
    storage_operations::http_writer_streamed_sized(state)
}

#[metabench::benchmark(HTTP_WRITER_MATERIALIZED, MAP_WRITER, "http_writer_materialized")]
#[bench::long(setup = long_owned_map)]
fn http_writer_materialized(state: (HeaderMap, FieldValue)) -> MapOutput {
    storage_operations::http_writer_materialized(state)
}

type AppendCase = (&'static str, fn() -> AppendInput);

fn append_input(name: &'static FieldName, occupied: bool, count: usize) -> AppendInput {
    let mut map = HeaderMap::with_capacity(128);
    if occupied {
        let http_name = if name == &FieldName::SetCookie {
            http::header::SET_COOKIE
        } else {
            http::HeaderName::from_static("x-cookie-batch")
        };
        map.insert(http_name, http::HeaderValue::from_static("existing"));
    }
    let values = iter::repeat_n(FieldValue::from_static("value"), count).collect();
    (map, name, values)
}

macro_rules! append_setup {
    ($name:ident, $field_name:expr, $occupied:literal, $count:literal) => {
        fn $name() -> AppendInput {
            append_input($field_name, $occupied, $count)
        }
    };
}

append_setup!(known_vacant_1, &FieldName::SetCookie, false, 1);
append_setup!(known_vacant_4, &FieldName::SetCookie, false, 4);
append_setup!(known_vacant_16, &FieldName::SetCookie, false, 16);
append_setup!(known_vacant_64, &FieldName::SetCookie, false, 64);
append_setup!(known_occupied_1, &FieldName::SetCookie, true, 1);
append_setup!(known_occupied_4, &FieldName::SetCookie, true, 4);
append_setup!(known_occupied_16, &FieldName::SetCookie, true, 16);
append_setup!(known_occupied_64, &FieldName::SetCookie, true, 64);
append_setup!(custom_vacant_1, &CUSTOM_APPEND_NAME, false, 1);
append_setup!(custom_vacant_4, &CUSTOM_APPEND_NAME, false, 4);
append_setup!(custom_vacant_16, &CUSTOM_APPEND_NAME, false, 16);
append_setup!(custom_vacant_64, &CUSTOM_APPEND_NAME, false, 64);
append_setup!(custom_occupied_1, &CUSTOM_APPEND_NAME, true, 1);
append_setup!(custom_occupied_4, &CUSTOM_APPEND_NAME, true, 4);
append_setup!(custom_occupied_16, &CUSTOM_APPEND_NAME, true, 16);
append_setup!(custom_occupied_64, &CUSTOM_APPEND_NAME, true, 64);

#[metabench::benchmark(HTTP_APPEND_VALUES, MAP_APPEND, "http_append_values")]
#[bench::known_vacant_1(setup = known_vacant_1)]
#[bench::known_vacant_4(setup = known_vacant_4)]
#[bench::known_vacant_16(setup = known_vacant_16)]
#[bench::known_vacant_64(setup = known_vacant_64)]
#[bench::known_occupied_1(setup = known_occupied_1)]
#[bench::known_occupied_4(setup = known_occupied_4)]
#[bench::known_occupied_16(setup = known_occupied_16)]
#[bench::known_occupied_64(setup = known_occupied_64)]
#[bench::custom_vacant_1(setup = custom_vacant_1)]
#[bench::custom_vacant_4(setup = custom_vacant_4)]
#[bench::custom_vacant_16(setup = custom_vacant_16)]
#[bench::custom_vacant_64(setup = custom_vacant_64)]
#[bench::custom_occupied_1(setup = custom_occupied_1)]
#[bench::custom_occupied_4(setup = custom_occupied_4)]
#[bench::custom_occupied_16(setup = custom_occupied_16)]
#[bench::custom_occupied_64(setup = custom_occupied_64)]
fn http_append_values(input: AppendInput) -> MapOutput {
    storage_operations::http_append_values(input)
}

#[metabench::benchmark(CONTENT_LENGTH_MATERIALIZED, DEFERRED_INSERTION, "content_length_materialized")]
#[bench::materialized(setup = empty_http_map)]
fn content_length_materialized(map: HeaderMap) -> MapOutput {
    storage_operations::content_length_materialized(map)
}

#[metabench::benchmark(CONTENT_LENGTH_DEFERRED_HTTP, DEFERRED_INSERTION, "content_length_deferred_http")]
#[bench::deferred_http(setup = empty_http_map)]
fn content_length_deferred_http(map: HeaderMap) -> MapOutput {
    storage_operations::content_length_deferred_http(map)
}

#[metabench::benchmark(ONE_VEC, REPEATED_VALUES, "one_vec")]
#[bench::one_value(setup = one_value)]
fn one_vec(value: FieldValue) -> usize {
    let values = vec![value];
    black_box(values).len()
}

#[metabench::benchmark(ONE_SMALLVEC, REPEATED_VALUES, "one_smallvec")]
#[bench::one_value(setup = one_value)]
fn one_smallvec(value: FieldValue) -> usize {
    let values = SmallVec::<[FieldValue; 1]>::from_buf([value]);
    black_box(values).len()
}

#[metabench::benchmark(FOUR_VEC, REPEATED_VALUES, "four_vec")]
#[bench::four_values(setup = four_values)]
fn four_vec(value: FieldValue) -> usize {
    let values = vec![value.clone(), value.clone(), value.clone(), value];
    black_box(values).len()
}

#[metabench::benchmark(FOUR_SMALLVEC, REPEATED_VALUES, "four_smallvec")]
#[bench::four_values(setup = four_values)]
fn four_smallvec(value: FieldValue) -> usize {
    let values = SmallVec::<[FieldValue; 1]>::from_vec(vec![value.clone(), value.clone(), value.clone(), value]);
    black_box(values).len()
}

#[metabench::benchmark(ONE_VEC_ENCODE, INSERTION, "one_vec_encode")]
#[bench::one_encode(setup = one_value)]
fn one_vec_encode(value: FieldValue) -> usize {
    let values = EncodedValues::from_vec(vec![value]);
    black_box(values).len()
}

#[metabench::benchmark(ONE_SMALLVEC_ENCODE, INSERTION, "one_smallvec_encode")]
#[bench::one_encode(setup = one_value)]
fn one_smallvec_encode(value: FieldValue) -> usize {
    let values = SmallVec::<[FieldValue; 1]>::from_buf([value])
        .into_iter()
        .collect::<EncodedValues>();
    black_box(values).len()
}

#[metabench::benchmark(SHORT_STRING_TIME, STORAGE_TEXT, "short_string")]
fn short_string_time() -> String {
    black_box(String::from(black_box("stale-if-error=30")))
}

#[metabench::benchmark(SHORT_COMPACT_STRING_TIME, STORAGE_TEXT, "short_compact_string")]
fn short_compact_string_time() -> CompactString {
    black_box(CompactString::from(black_box("stale-if-error=30")))
}

#[metabench::benchmark(LONG_STRING_TIME, STORAGE_TEXT, "long_string")]
fn long_string_time() -> String {
    black_box(String::from(black_box(
        "extension-directive-with-a-value-that-exceeds-inline-storage=enabled",
    )))
}

#[metabench::benchmark(LONG_COMPACT_STRING_TIME, STORAGE_TEXT, "long_compact_string")]
fn long_compact_string_time() -> CompactString {
    black_box(CompactString::from(black_box(
        "extension-directive-with-a-value-that-exceeds-inline-storage=enabled",
    )))
}

fn reused_value() -> &'static FieldValue {
    static VALUE: OnceLock<FieldValue> = OnceLock::new();
    VALUE.get_or_init(|| FieldValue::from_static("value"))
}

#[metabench::benchmark(ONE_VEC_TIME, STORAGE_REPEATED_VALUES, "one_vec")]
#[bench::one_value(setup = reused_value)]
fn one_vec_time(value: &FieldValue) -> Vec<FieldValue> {
    black_box(vec![black_box(value.clone())])
}

#[metabench::benchmark(ONE_SMALLVEC_TIME, STORAGE_REPEATED_VALUES, "one_smallvec")]
#[bench::one_value(setup = reused_value)]
fn one_smallvec_time(value: &FieldValue) -> SmallVec<[FieldValue; 1]> {
    black_box(SmallVec::<[FieldValue; 1]>::from_buf([black_box(value.clone())]))
}

#[metabench::benchmark(FOUR_VEC_TIME, STORAGE_REPEATED_VALUES, "four_vec")]
#[bench::four_values(setup = reused_value)]
fn four_vec_time(value: &FieldValue) -> Vec<FieldValue> {
    black_box(vec![value.clone(), value.clone(), value.clone(), black_box(value.clone())])
}

#[metabench::benchmark(FOUR_SMALLVEC_TIME, STORAGE_REPEATED_VALUES, "four_smallvec")]
#[bench::four_values(setup = reused_value)]
fn four_smallvec_time(value: &FieldValue) -> SmallVec<[FieldValue; 1]> {
    black_box(SmallVec::<[FieldValue; 1]>::from_vec(vec![
        value.clone(),
        value.clone(),
        value.clone(),
        black_box(value.clone()),
    ]))
}

#[metabench::benchmark(ONE_VEC_ENCODE_TIME, STORAGE_INSERTION, "one_vec_encode")]
#[bench::one_encode(setup = reused_value)]
fn one_vec_encode_time(value: &FieldValue) -> EncodedValues {
    black_box(EncodedValues::from_vec(vec![black_box(value.clone())]))
}

#[metabench::benchmark(ONE_SMALLVEC_ENCODE_TIME, STORAGE_INSERTION, "one_smallvec_encode")]
#[bench::one_encode(setup = reused_value)]
fn one_smallvec_encode_time(value: &FieldValue) -> EncodedValues {
    black_box(
        SmallVec::<[FieldValue; 1]>::from_buf([black_box(value.clone())])
            .into_iter()
            .collect::<EncodedValues>(),
    )
}

fn register_storage_text(criterion: &mut Criterion) {
    let mut storage_text = criterion.benchmark_group(STORAGE_TEXT);
    storage_text.bench_function(SHORT_STRING_TIME.benchmark_name(), |bencher| {
        bencher.iter(short_string_time);
    });
    storage_text.bench_function(SHORT_COMPACT_STRING_TIME.benchmark_name(), |bencher| {
        bencher.iter(short_compact_string_time);
    });
    storage_text.bench_function(LONG_STRING_TIME.benchmark_name(), |bencher| {
        bencher.iter(long_string_time);
    });
    storage_text.bench_function(LONG_COMPACT_STRING_TIME.benchmark_name(), |bencher| {
        bencher.iter(long_compact_string_time);
    });
    storage_text.finish();
}

fn register_text(criterion: &mut Criterion) {
    let mut text = criterion.benchmark_group(TEXT);
    text.bench_function(BenchmarkId::new(SHORT_STRING.benchmark_name(), "short_text"), |bencher| {
        bencher.iter_batched(short_text, short_string, BatchSize::SmallInput);
    });
    text.bench_function(BenchmarkId::new(SHORT_COMPACT_STRING.benchmark_name(), "short_text"), |bencher| {
        bencher.iter_batched(short_text, short_compact_string, BatchSize::SmallInput);
    });
    text.bench_function(BenchmarkId::new(LONG_STRING.benchmark_name(), "long_text"), |bencher| {
        bencher.iter_batched(long_text, long_string, BatchSize::SmallInput);
    });
    text.bench_function(BenchmarkId::new(LONG_COMPACT_STRING.benchmark_name(), "long_text"), |bencher| {
        bencher.iter_batched(long_text, long_compact_string, BatchSize::SmallInput);
    });
    text.finish();
}

fn register_map_writer(criterion: &mut Criterion) {
    let mut map_writer = criterion.benchmark_group(MAP_WRITER);
    map_writer.bench_function(BenchmarkId::new(HTTP_WRITER_BORROWED.benchmark_name(), "short"), |bencher| {
        bencher.iter_batched(short_value_map, http_writer_borrowed, BatchSize::SmallInput);
    });
    map_writer.bench_function(BenchmarkId::new(HTTP_WRITER_BORROWED.benchmark_name(), "long"), |bencher| {
        bencher.iter_batched(long_value_map, http_writer_borrowed, BatchSize::SmallInput);
    });
    map_writer.bench_function(BenchmarkId::new(HTTP_WRITER_STREAMED.benchmark_name(), "short"), |bencher| {
        bencher.iter_batched(short_value_map, http_writer_streamed, BatchSize::SmallInput);
    });
    map_writer.bench_function(BenchmarkId::new(HTTP_WRITER_STREAMED.benchmark_name(), "long"), |bencher| {
        bencher.iter_batched(long_value_map, http_writer_streamed, BatchSize::SmallInput);
    });
    let streamed_cases = [
        ("bytes_0", streamed_0_map as fn() -> (HeaderMap, &'static [u8])),
        ("bytes_16", streamed_16_map),
        ("bytes_32", streamed_32_map),
        ("bytes_65", streamed_65_map),
        ("bytes_4096", streamed_4096_map),
        ("bytes_65536", streamed_65536_map),
    ];
    for &(case, setup) in &streamed_cases {
        map_writer.bench_function(BenchmarkId::new(HTTP_WRITER_STREAMED_SIZED.benchmark_name(), case), |bencher| {
            bencher.iter_batched(setup, http_writer_streamed_sized, BatchSize::SmallInput);
        });
    }
    map_writer.bench_function(BenchmarkId::new(HTTP_WRITER_MATERIALIZED.benchmark_name(), "long"), |bencher| {
        bencher.iter_batched(long_owned_map, http_writer_materialized, BatchSize::SmallInput);
    });
    map_writer.finish();
}

fn register_map_append(criterion: &mut Criterion) {
    let cases: [AppendCase; 16] = [
        ("known_vacant_1", known_vacant_1),
        ("known_vacant_4", known_vacant_4),
        ("known_vacant_16", known_vacant_16),
        ("known_vacant_64", known_vacant_64),
        ("known_occupied_1", known_occupied_1),
        ("known_occupied_4", known_occupied_4),
        ("known_occupied_16", known_occupied_16),
        ("known_occupied_64", known_occupied_64),
        ("custom_vacant_1", custom_vacant_1),
        ("custom_vacant_4", custom_vacant_4),
        ("custom_vacant_16", custom_vacant_16),
        ("custom_vacant_64", custom_vacant_64),
        ("custom_occupied_1", custom_occupied_1),
        ("custom_occupied_4", custom_occupied_4),
        ("custom_occupied_16", custom_occupied_16),
        ("custom_occupied_64", custom_occupied_64),
    ];
    let mut group = criterion.benchmark_group(MAP_APPEND);
    for &(case, setup) in &cases {
        group.bench_function(BenchmarkId::new(HTTP_APPEND_VALUES.benchmark_name(), case), |bencher| {
            bencher.iter_batched(setup, http_append_values, BatchSize::SmallInput);
        });
    }
    group.finish();
}

fn register_deferred_insertion(criterion: &mut Criterion) {
    let mut deferred = criterion.benchmark_group(DEFERRED_INSERTION);
    deferred.bench_function(
        BenchmarkId::new(CONTENT_LENGTH_MATERIALIZED.benchmark_name(), "materialized"),
        |bencher| {
            bencher.iter_batched(empty_http_map, content_length_materialized, BatchSize::SmallInput);
        },
    );
    deferred.bench_function(
        BenchmarkId::new(CONTENT_LENGTH_DEFERRED_HTTP.benchmark_name(), "deferred_http"),
        |bencher| {
            bencher.iter_batched(empty_http_map, content_length_deferred_http, BatchSize::SmallInput);
        },
    );
    deferred.finish();
}

fn register_storage_repeated_values(criterion: &mut Criterion, value: &FieldValue) {
    let mut storage_repeated = criterion.benchmark_group(STORAGE_REPEATED_VALUES);
    storage_repeated.bench_function(BenchmarkId::new(ONE_VEC_TIME.benchmark_name(), "one_value"), |bencher| {
        bencher.iter(|| one_vec_time(value));
    });
    storage_repeated.bench_function(BenchmarkId::new(ONE_SMALLVEC_TIME.benchmark_name(), "one_value"), |bencher| {
        bencher.iter(|| one_smallvec_time(value));
    });
    storage_repeated.bench_function(BenchmarkId::new(FOUR_VEC_TIME.benchmark_name(), "four_values"), |bencher| {
        bencher.iter(|| four_vec_time(value));
    });
    storage_repeated.bench_function(BenchmarkId::new(FOUR_SMALLVEC_TIME.benchmark_name(), "four_values"), |bencher| {
        bencher.iter(|| four_smallvec_time(value));
    });
    storage_repeated.finish();
}

fn register_repeated_values(criterion: &mut Criterion) {
    let mut repeated = criterion.benchmark_group(REPEATED_VALUES);
    repeated.bench_function(BenchmarkId::new(ONE_VEC.benchmark_name(), "one_value"), |bencher| {
        bencher.iter_batched(one_value, one_vec, BatchSize::SmallInput);
    });
    repeated.bench_function(BenchmarkId::new(ONE_SMALLVEC.benchmark_name(), "one_value"), |bencher| {
        bencher.iter_batched(one_value, one_smallvec, BatchSize::SmallInput);
    });
    repeated.bench_function(BenchmarkId::new(FOUR_VEC.benchmark_name(), "four_values"), |bencher| {
        bencher.iter_batched(four_values, four_vec, BatchSize::SmallInput);
    });
    repeated.bench_function(BenchmarkId::new(FOUR_SMALLVEC.benchmark_name(), "four_values"), |bencher| {
        bencher.iter_batched(four_values, four_smallvec, BatchSize::SmallInput);
    });
    repeated.finish();
}

fn register_storage_insertion(criterion: &mut Criterion, value: &FieldValue) {
    let mut storage_insertion = criterion.benchmark_group(STORAGE_INSERTION);
    storage_insertion.bench_function(BenchmarkId::new(ONE_VEC_ENCODE_TIME.benchmark_name(), "one_encode"), |bencher| {
        bencher.iter(|| one_vec_encode_time(value));
    });
    storage_insertion.bench_function(
        BenchmarkId::new(ONE_SMALLVEC_ENCODE_TIME.benchmark_name(), "one_encode"),
        |bencher| bencher.iter(|| one_smallvec_encode_time(value)),
    );
    storage_insertion.finish();
}

fn register_insertion(criterion: &mut Criterion) {
    let mut insertion = criterion.benchmark_group(INSERTION);
    insertion.bench_function(BenchmarkId::new(ONE_VEC_ENCODE.benchmark_name(), "one_encode"), |bencher| {
        bencher.iter_batched(one_value, one_vec_encode, BatchSize::SmallInput);
    });
    insertion.bench_function(BenchmarkId::new(ONE_SMALLVEC_ENCODE.benchmark_name(), "one_encode"), |bencher| {
        bencher.iter_batched(one_value, one_smallvec_encode, BatchSize::SmallInput);
    });
    insertion.finish();
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    register_storage_text(criterion);
    register_text(criterion);
    register_map_writer(criterion);
    register_map_append(criterion);
    register_deferred_insertion(criterion);

    let value = FieldValue::from_static("value");
    register_storage_repeated_values(criterion, &value);
    register_repeated_values(criterion);
    register_storage_insertion(criterion, &value);
    register_insertion(criterion);
    register_custom_sink_encoding(criterion);
}

fn register_custom_sink_encoding(criterion: &mut Criterion) {
    let cases: [CustomSinkCase; 8] = [
        ("short_1", short_1),
        ("short_4", short_4),
        ("short_16", short_16),
        ("short_64", short_64),
        ("spilled_1", spilled_1),
        ("spilled_4", spilled_4),
        ("spilled_16", spilled_16),
        ("spilled_64", spilled_64),
    ];
    let mut group = criterion.benchmark_group(CUSTOM_SINK_ENCODING);
    for &(case, setup) in &cases {
        group.bench_function(BenchmarkId::new(MINIMAL_SINK_SET_ENCODED.benchmark_name(), case), |bencher| {
            bencher.iter_batched(setup, minimal_sink_set_encoded, BatchSize::SmallInput);
        });
    }
    for &(case, setup) in &cases {
        group.bench_function(BenchmarkId::new(MINIMAL_SINK_APPEND_ENCODED.benchmark_name(), case), |bencher| {
            bencher.iter_batched(setup, minimal_sink_append_encoded, BatchSize::SmallInput);
        });
    }
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        SHORT_STRING,
        SHORT_COMPACT_STRING,
        LONG_STRING,
        LONG_COMPACT_STRING,
        HTTP_WRITER_BORROWED,
        HTTP_WRITER_STREAMED,
        HTTP_WRITER_STREAMED_SIZED,
        HTTP_WRITER_MATERIALIZED,
        HTTP_APPEND_VALUES,
        CONTENT_LENGTH_MATERIALIZED,
        CONTENT_LENGTH_DEFERRED_HTTP,
        ONE_VEC,
        ONE_SMALLVEC,
        FOUR_VEC,
        FOUR_SMALLVEC,
        ONE_VEC_ENCODE,
        ONE_SMALLVEC_ENCODE,
        SHORT_STRING_TIME,
        SHORT_COMPACT_STRING_TIME,
        LONG_STRING_TIME,
        LONG_COMPACT_STRING_TIME,
        ONE_VEC_TIME,
        ONE_SMALLVEC_TIME,
        FOUR_VEC_TIME,
        FOUR_SMALLVEC_TIME,
        ONE_VEC_ENCODE_TIME,
        ONE_SMALLVEC_ENCODE_TIME,
        MINIMAL_SINK_SET_ENCODED,
        MINIMAL_SINK_APPEND_ENCODED,
    ],
);
