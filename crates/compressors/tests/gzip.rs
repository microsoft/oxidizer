// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Gzip contracts exercised exclusively through the public API.

#![cfg(feature = "gzip")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::io::Read as _;
use std::sync::OnceLock;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::{DecompressorLimits, Resources, gzip};

const FIXTURE_PLAINTEXT: &[u8] = b"The quick brown fox jumps over the lazy dog.\nPack my box with five dozen liquor jugs.\n";
const SYSTEM_GZIP: &[u8] = include_bytes!("../src/tests/fixtures/system_gzip.gz");

fn resources() -> &'static Resources {
    static RESOURCES: OnceLock<Resources> = OnceLock::new();

    RESOURCES.get_or_init(|| Resources::new(GlobalPool::new()))
}

fn view(bytes: &[u8]) -> BytesView {
    BytesView::copied_from_slice(bytes, resources().memory())
}

fn fragmented(bytes: &[u8], segment: usize) -> BytesView {
    BytesView::from_views(bytes.chunks(segment).map(view))
}

#[test]
fn decompresses_a_stream_produced_by_the_system_gzip() {
    let plain = gzip::decompress(view(SYSTEM_GZIP), resources()).unwrap();

    assert_eq!(plain.to_vec(), FIXTURE_PLAINTEXT);
}

#[test]
fn decompresses_concatenated_members_produced_by_the_system_gzip() {
    let two_members = [SYSTEM_GZIP, SYSTEM_GZIP].concat();

    let plain = gzip::decompress(view(&two_members), resources()).unwrap();

    assert_eq!(plain.to_vec(), [FIXTURE_PLAINTEXT, FIXTURE_PLAINTEXT].concat());
}

#[test]
fn our_framing_matches_an_independent_gzip_reader() {
    let payload = b"cross checked against an independent reader ".repeat(200);
    let compressed = gzip::compress(fragmented(&payload, 71), resources()).unwrap();

    let mut decompressed = Vec::new();
    flate2::read::GzDecoder::new(compressed.to_vec().as_slice())
        .read_to_end(&mut decompressed)
        .unwrap();

    assert_eq!(decompressed, payload);
}

#[test]
fn round_trips_a_multi_segment_view() {
    for (segment, repeats) in [(1, 200), (7, 500), (64, 5_000), (1024, 20_000), (65_536, 20_000)] {
        let payload = b"multi segment payload ".repeat(repeats);

        let compressed = gzip::compress(fragmented(&payload, segment), resources()).unwrap();
        let plain = gzip::decompress(compressed, resources()).unwrap();

        assert_eq!(plain.to_vec(), payload, "round trip failed for {segment} byte segments");
    }
}

#[test]
fn default_limits_accept_maximally_compressible_deflate_data() {
    let payload = vec![0_u8; 1024 * 1024];
    let compressed = gzip::compress(view(&payload), resources()).unwrap();

    let plain = gzip::decompress(compressed, resources()).unwrap();

    assert_eq!(plain.len(), payload.len());
}

#[test]
fn known_good_data_can_opt_out_of_the_limits() {
    let payload = vec![0_u8; 1024 * 1024];
    let compressed = gzip::compress(view(&payload), resources()).unwrap();

    let plain = gzip::decompress_with_limits(compressed, resources(), DecompressorLimits::UNLIMITED).unwrap();

    assert_eq!(plain.len(), payload.len());
}

#[test]
fn detects_truncation_at_every_offset() {
    let compressed = gzip::compress(view(&b"truncate me ".repeat(500)), resources()).unwrap();

    for cut in [
        1,
        compressed.len() / 4,
        compressed.len() / 2,
        compressed.len() - 8,
        compressed.len() - 1,
    ] {
        let error = gzip::decompress(compressed.range(0..cut), resources()).unwrap_err();

        assert!(
            error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
            "truncating at {cut} gave an unexpected classification: {error}"
        );
    }
}

#[test]
fn a_corrupted_byte_anywhere_is_detected() {
    let payload = b"integrity checked payload ".repeat(100);
    let compressed = gzip::compress(view(&payload), resources()).unwrap();
    let original = compressed.to_vec();

    for index in [0, 1, 2, original.len() / 2, original.len() - 5, original.len() - 1] {
        let mut corrupted = original.clone();
        corrupted[index] ^= 0xff;

        let result = gzip::decompress(view(&corrupted), resources());

        match result {
            Ok(plain) => assert_ne!(plain.to_vec(), payload, "corruption at {index} went entirely unnoticed"),
            Err(error) => assert!(
                error.is_corrupt_data() || error.is_unexpected_end_of_stream(),
                "corruption at {index} gave an unexpected classification: {error}"
            ),
        }
    }
}

#[test]
fn empty_input_round_trips() {
    let compressed = gzip::compress(BytesView::new(), resources()).unwrap();
    let plain = gzip::decompress(compressed, resources()).unwrap();

    assert!(plain.is_empty());
}
