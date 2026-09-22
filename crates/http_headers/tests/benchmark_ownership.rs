// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ownership regressions for the storage and name-recognition benchmark fixtures.

#![cfg(all(feature = "http", feature = "headers-content-length", feature = "headers-content-type"))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::{iter, ptr};

use bytes::Bytes;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, InvalidHeaderValue, SET_COOKIE};
use http::{HeaderMap, HeaderName, HeaderValue};
use http_headers::{FieldName, FieldValue};

use self::name_corpus::{crate_names, custom_header_names, custom_names_lowercase, custom_names_mixed_case, http_names, known_names};
use self::storage_operations::{
    MapOutput, content_length_deferred_http, content_length_materialized, http_append_values, http_writer_borrowed,
    http_writer_materialized, http_writer_streamed, http_writer_streamed_sized,
};

#[path = "common/http_headers_name_corpus.rs"]
mod name_corpus;
#[path = "common/http_headers_storage_operations.rs"]
mod storage_operations;

const SENTINEL_NAME: &str = "x-benchmark-owner";
const SENTINEL_VALUE: &[u8] = b"this allocation must survive until output cleanup";
const SHORT_VALUE: &str = "application/json; charset=utf-8";
const LONG_VALUE: &str = "multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW; charset=utf-8";
static LARGE_STREAM: [u8; 65_536] = [b'y'; 65_536];
static CUSTOM_APPEND_NAME: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-cookie-batch"));

#[derive(Debug)]
struct TrackedBytes {
    bytes: Vec<u8>,
    dropped: Arc<AtomicUsize>,
}

impl AsRef<[u8]> for TrackedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for TrackedBytes {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn assert_batches_reclaim_owners(
    mut operation: impl FnMut(HeaderMap) -> MapOutput,
    inspect: impl Fn(&HeaderMap),
) -> Result<(), InvalidHeaderValue> {
    const BATCH_SIZE: usize = 8;
    const SAMPLES: usize = 4;

    let dropped = Arc::new(AtomicUsize::new(0));
    for sample in 0..SAMPLES {
        let mut outputs = Vec::with_capacity(BATCH_SIZE);
        for _ in 0..BATCH_SIZE {
            let sentinel = Bytes::from_owner(TrackedBytes {
                bytes: SENTINEL_VALUE.to_vec(),
                dropped: Arc::clone(&dropped),
            });
            let mut map = HeaderMap::with_capacity(128);
            map.insert(SENTINEL_NAME, HeaderValue::from_maybe_shared(sentinel)?);

            let (length, map) = operation(map);
            assert_eq!(length, map.len());
            assert_eq!(map[SENTINEL_NAME].as_bytes(), SENTINEL_VALUE);
            inspect(&map);
            outputs.push((length, map));
            assert_eq!(dropped.load(Ordering::Relaxed), sample * BATCH_SIZE);
        }
        drop(outputs);
        assert_eq!(dropped.load(Ordering::Relaxed), (sample + 1) * BATCH_SIZE);
    }
    Ok(())
}

#[test]
fn writer_outputs_retain_and_release_every_map() -> Result<(), InvalidHeaderValue> {
    for value in [SHORT_VALUE, LONG_VALUE] {
        assert_batches_reclaim_owners(
            |map| http_writer_borrowed((map, value)),
            |map| {
                assert_eq!(map.len(), 2);
                assert_eq!(map[CONTENT_TYPE].as_bytes(), value.as_bytes());
            },
        )?;
        assert_batches_reclaim_owners(
            |map| http_writer_streamed((map, value)),
            |map| {
                assert_eq!(map.len(), 2);
                assert_eq!(map[CONTENT_TYPE].as_bytes(), value.as_bytes());
            },
        )?;
    }

    let streamed: [&'static [u8]; 6] = [&[], &[b's'; 16], &[b'm'; 32], &[b'l'; 65], &[b'x'; 4096], &LARGE_STREAM];
    for value in streamed {
        assert_batches_reclaim_owners(
            |map| http_writer_streamed_sized((map, value)),
            |map| {
                assert_eq!(map.len(), 2);
                assert_eq!(map[CONTENT_TYPE].as_bytes(), value);
            },
        )?;
    }

    assert_batches_reclaim_owners(
        |map| http_writer_materialized((map, FieldValue::from_static(LONG_VALUE))),
        |map| {
            assert_eq!(map.len(), 2);
            assert_eq!(map[CONTENT_TYPE].as_bytes(), LONG_VALUE.as_bytes());
        },
    )?;
    Ok(())
}

#[test]
fn append_outputs_retain_existing_values_and_release_every_map() -> Result<(), InvalidHeaderValue> {
    let names: [&'static FieldName; 2] = [&FieldName::SetCookie, &CUSTOM_APPEND_NAME];
    for name in names {
        let http_name = if name == &FieldName::SetCookie {
            SET_COOKIE
        } else {
            HeaderName::from_static("x-cookie-batch")
        };
        for occupied in [false, true] {
            for count in [1, 4, 16, 64] {
                assert_batches_reclaim_owners(
                    |mut map| {
                        if occupied {
                            map.insert(http_name.clone(), HeaderValue::from_static("existing"));
                        }
                        let values = iter::repeat_n(FieldValue::from_static("value"), count).collect();
                        http_append_values((map, name, values))
                    },
                    |map| {
                        assert_eq!(map.len(), 1 + count + usize::from(occupied));
                        for (index, value) in map.get_all(&http_name).iter().enumerate() {
                            let expected = if occupied && index == 0 { b"existing".as_slice() } else { b"value" };
                            assert_eq!(value.as_bytes(), expected);
                        }
                    },
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn deferred_and_materialized_outputs_release_every_map() -> Result<(), InvalidHeaderValue> {
    for operation in [content_length_materialized, content_length_deferred_http] {
        assert_batches_reclaim_owners(operation, |map| {
            assert_eq!(map.len(), 2);
            assert_eq!(map[CONTENT_LENGTH].as_bytes(), b"1024");
        })?;
    }
    Ok(())
}

#[test]
fn name_setups_reuse_immutable_corpora_and_owned_names() {
    let known = known_names();
    let lowercase = custom_names_lowercase();
    let mixed_case = custom_names_mixed_case();
    let custom = custom_header_names();
    let http = http_names();
    let crate_names = crate_names();

    assert_eq!(known.len(), 15);
    assert_eq!(lowercase.len(), 8);
    assert_eq!(mixed_case.len(), 8);
    assert_eq!(custom.len(), 8);
    assert_eq!(http.len(), 23);
    assert_eq!(crate_names.len(), 23);
    for (name, expected) in custom.iter().zip(lowercase) {
        assert_eq!(name.as_str().as_bytes(), *expected);
    }
    for ((http_name, crate_name), expected) in http.iter().zip(crate_names).zip(known.iter().chain(mixed_case)) {
        assert!(http_name.as_str().as_bytes().eq_ignore_ascii_case(expected));
        assert_eq!(http_name.as_str(), crate_name.as_str());
    }

    for _ in 0..128 {
        let next_custom = custom_header_names();
        let next_http = http_names();
        let next_crate = name_corpus::crate_names();
        assert!(ptr::eq(custom, next_custom));
        assert!(ptr::eq(http, next_http));
        assert!(ptr::eq(crate_names, next_crate));
        for (original, next) in custom.iter().zip(next_custom) {
            assert!(ptr::eq(original.as_str(), next.as_str()));
        }
        for (original, next) in http.iter().zip(next_http) {
            assert!(ptr::eq(original.as_str(), next.as_str()));
        }
        for (original, next) in crate_names.iter().zip(next_crate) {
            assert!(ptr::eq(original.as_str(), next.as_str()));
        }
    }
}
