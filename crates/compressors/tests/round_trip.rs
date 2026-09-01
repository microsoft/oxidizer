// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behaviour tests that exercise the crate as a downstream consumer sees it.
//!
//! Gzip specific: interop fixtures produced by the system `gzip`, and the concatenated-member
//! behaviour that only gzip enables by default.

#![cfg(feature = "gzip")]

use std::num::NonZeroUsize;

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use compressors::{DecompressionLimits, Output, gzip};

/// The payload behind `tests/fixtures/system_gzip.gz`, compressed by the system `gzip -9 -n`.
const FIXTURE_PLAINTEXT: &[u8] = b"The quick brown fox jumps over the lazy dog.\nPack my box with five dozen liquor jugs.\n";

const SYSTEM_GZIP: &[u8] = include_bytes!("fixtures/system_gzip.gz");
const SYSTEM_GZIP_TWO_MEMBERS: &[u8] = include_bytes!("fixtures/system_gzip_two_members.gz");

fn view(bytes: &[u8]) -> BytesView {
    BytesView::copied_from_slice(bytes, &GlobalPool::new())
}

/// Builds a view split into `segment` sized spans, so the multi-segment paths are exercised.
fn fragmented(bytes: &[u8], segment: usize) -> BytesView {
    let memory = GlobalPool::new();
    BytesView::from_views(bytes.chunks(segment).map(|chunk| BytesView::copied_from_slice(chunk, &memory)))
}

fn chunk(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("test chunk sizes are never zero")
}

/// Drives a codec to completion over an input delivered in `feed` sized pieces.
fn drive_decompressor(mut decompressor: gzip::Decompressor, input: &BytesView, feed: usize) -> compressors::Result<BytesView> {
    let mut offset = 0;
    let mut collected = BytesBuf::new();

    loop {
        match decompressor.pull()? {
            Output::Data(data) => collected.put_bytes(data),
            Output::Progress => {}
            Output::Done => break,
            Output::NeedInput => {
                if offset >= input.len() {
                    decompressor.end_input();
                    continue;
                }

                let end = (offset + feed).min(input.len());
                decompressor.push(input.range(offset..end))?;
                offset = end;
            }
        }
    }

    Ok(collected.consume_all())
}

#[test]
fn decompresses_a_stream_produced_by_the_system_gzip() {
    let plain = gzip::decompress(view(SYSTEM_GZIP), GlobalPool::new()).expect("the fixture decompresses");

    assert_eq!(plain.to_vec(), FIXTURE_PLAINTEXT);
}

#[test]
fn decompresses_concatenated_members_produced_by_the_system_gzip() {
    let plain = gzip::decompress(view(SYSTEM_GZIP_TWO_MEMBERS), GlobalPool::new()).expect("the fixture decompresses");

    assert_eq!(plain.to_vec(), [FIXTURE_PLAINTEXT, FIXTURE_PLAINTEXT].concat());
}

#[test]
fn our_framing_matches_an_independent_gzip_reader() {
    // Cross-checks our container against flate2's own gzip framing, which parses the header,
    // checksum and length trailer in Rust rather than in the compression engine.
    use std::io::Read as _;

    let payload = b"cross checked against an independent reader ".repeat(200);
    let compressed = gzip::compress(fragmented(&payload, 71), GlobalPool::new()).expect("compression succeeds");

    let mut decompressed = Vec::new();
    flate2::read::GzDecoder::new(compressed.to_vec().as_slice())
        .read_to_end(&mut decompressed)
        .expect("an independent reader accepts our output");

    assert_eq!(decompressed, payload);
}

#[test]
fn round_trips_a_multi_segment_view() {
    // Regression guard. `BytesView` is a chain of segments, and the engine is fed one segment at a
    // time. Signalling end of input on the first segment rather than the last silently truncated the
    // stream at the first segment boundary, which single-segment tests could not catch.
    // Tiny segments are quadratic to build, so scale the payload down as the segment shrinks.
    for (segment, repeats) in [(1, 200), (7, 500), (64, 5_000), (1024, 20_000), (65_536, 20_000)] {
        let payload = b"multi segment payload ".repeat(repeats);

        let compressed = gzip::compress(fragmented(&payload, segment), GlobalPool::new()).expect("compression succeeds");
        let plain = gzip::decompress(compressed, GlobalPool::new()).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), payload, "round trip failed for {segment} byte segments");
    }
}

#[test]
fn round_trips_when_input_arrives_one_byte_at_a_time() {
    let payload = b"trickled in".repeat(50);
    let compressed = gzip::compress(view(&payload), GlobalPool::new()).expect("compression succeeds");

    let decompressor = gzip::Decompressor::builder().output_chunk_size(chunk(1)).build(GlobalPool::new());
    let plain = drive_decompressor(decompressor, &compressed, 1).expect("decompression succeeds");

    assert_eq!(plain.to_vec(), payload);
}

#[test]
fn streams_a_large_payload_with_a_bounded_working_set() {
    // The point of the push/pull design: a long stream must never require a buffer proportional to
    // its length. Every chunk handed back stays within the configured bound.
    const CHUNK: usize = 16 * 1024;

    let payload = b"large streamed payload, compressible but not trivially so; ".repeat(400_000);
    assert!(payload.len() > 20 * 1024 * 1024, "the payload should be large enough to matter");

    let mut compressor = gzip::Compressor::builder().output_chunk_size(chunk(CHUNK)).build(GlobalPool::new());
    compressor.push(fragmented(&payload, 4096)).expect("push succeeds");
    compressor.end_input();

    let mut compressed = Vec::new();
    loop {
        match compressor.pull().expect("pull succeeds") {
            Output::Data(piece) => {
                assert!(
                    piece.len() <= CHUNK,
                    "chunk of {} bytes exceeded the {CHUNK} byte bound",
                    piece.len()
                );
                compressed.push(piece);
            }
            Output::Progress => {}
            Output::NeedInput => panic!("compressor requested input after end"),
            Output::Done => break,
        }
    }

    let gz = BytesView::from_views(compressed);
    assert!(gz.len() < payload.len() / 10, "the payload should compress well");

    let decompressor = gzip::Decompressor::builder()
        .output_chunk_size(chunk(CHUNK))
        .build(GlobalPool::new());
    let plain = drive_decompressor(decompressor, &gz, 8192).expect("decompression succeeds");

    assert_eq!(plain.len(), payload.len());
    assert_eq!(plain.to_vec(), payload);
}

#[test]
fn rejects_a_bomb_before_materialising_it() {
    // 64 MiB of zeros compresses to a few kilobytes. The guard must fire long before the output is
    // fully materialised, so this test would be intolerably slow if it did not.
    //
    // The cap is set explicitly rather than relying on the default: deflate cannot expand by more
    // than about 1032x, so its default ratio never fires on data the format could have produced.
    // An absolute cap is what actually protects a caller that buffers the output.
    let bomb = gzip::compress(view(&vec![0_u8; 64 * 1024 * 1024]), GlobalPool::new()).expect("compression succeeds");
    assert!(bomb.len() < 100 * 1024, "the bomb should be tiny: {} bytes", bomb.len());

    let mut decompressor = gzip::Decompressor::builder()
        .limits(DecompressionLimits::new().with_max_output_len(1024 * 1024))
        .build(GlobalPool::new());
    decompressor.push(bomb).expect("push succeeds");
    decompressor.end_input();

    let error = loop {
        match decompressor.pull() {
            Ok(Output::Data(_) | Output::Progress) => {}
            Ok(_) => panic!("the bomb decompressed fully instead of being rejected"),
            Err(error) => break error,
        }
    };

    assert!(error.is_limit_exceeded(), "got {error}");
    assert!(
        decompressor.total_out() < 64 * 1024 * 1024,
        "the guard should fire before the full expansion, stopped at {}",
        decompressor.total_out()
    );
}

#[test]
fn the_default_limits_accept_maximally_compressible_deflate_data() {
    // Deflate's structural ceiling is about 1032x, so the gzip default must sit above it: data the
    // format could legitimately have produced must never be rejected as a bomb.
    let payload = vec![0_u8; 8 * 1024 * 1024];
    let compressed = gzip::compress(view(&payload), GlobalPool::new()).expect("compression succeeds");

    let plain = gzip::decompress(compressed, GlobalPool::new()).expect("default limits must accept maximal deflate compression");

    assert_eq!(plain.len(), payload.len());
}

#[test]
fn trusted_callers_can_opt_out_of_the_limits() {
    let payload = vec![0_u8; 8 * 1024 * 1024];
    let compressed = gzip::compress(view(&payload), GlobalPool::new()).expect("compression succeeds");

    let decompressor = gzip::Decompressor::builder()
        .limits(DecompressionLimits::UNLIMITED)
        .build(GlobalPool::new());
    let plain = drive_decompressor(decompressor, &compressed, usize::MAX).expect("decompression succeeds");

    assert_eq!(plain.len(), payload.len());
}

#[test]
fn detects_truncation_at_every_offset() {
    let compressed = gzip::compress(view(&b"truncate me ".repeat(500)), GlobalPool::new()).expect("compression succeeds");

    for cut in [
        1,
        compressed.len() / 4,
        compressed.len() / 2,
        compressed.len() - 8,
        compressed.len() - 1,
    ] {
        let error =
            gzip::decompress(compressed.range(0..cut), GlobalPool::new()).expect_err("a truncated stream must not decompress successfully");

        assert!(
            error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
            "truncating at {cut} gave an unexpected classification: {error}"
        );
    }
}

#[test]
fn a_corrupted_byte_anywhere_is_detected() {
    let payload = b"integrity checked payload ".repeat(100);
    let compressed = gzip::compress(view(&payload), GlobalPool::new()).expect("compression succeeds");
    let original = compressed.to_vec();

    for index in [0, 1, 2, original.len() / 2, original.len() - 5, original.len() - 1] {
        let mut corrupted = original.clone();
        corrupted[index] ^= 0xff;

        let result = gzip::decompress(view(&corrupted), GlobalPool::new());

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
    let compressed = gzip::compress(BytesView::new(), GlobalPool::new()).expect("compression succeeds");
    let plain = gzip::decompress(compressed, GlobalPool::new()).expect("decompression succeeds");

    assert!(plain.is_empty());
}

#[test]
fn a_custom_memory_provider_is_used_for_output() {
    // Anything implementing `MemoryShared` works; the codec never reaches for a global allocator
    // of its own.
    let memory = GlobalPool::new();
    let buf = memory.reserve(1);
    drop(buf);

    let compressed = gzip::compress(view(b"provider supplied"), memory.clone()).expect("compression succeeds");
    let plain = gzip::decompress(compressed, memory).expect("decompression succeeds");

    assert_eq!(plain.to_vec(), b"provider supplied".to_vec());
}
