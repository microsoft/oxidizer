// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end behaviour of gzip, including the parts no other format exercises.
//!
//! Gzip-specific: interop fixtures produced by the system `gzip`, and the concatenated-stream
//! behaviour that only gzip enables by default. Drives the crate-private mechanics where a
//! transition has to be observed rather than inferred from the final bytes.

use std::num::NonZeroU64;

use bytesbuf::{BytesBuf, BytesView};

use crate::core::{CompressionInternal as _, Output};
use crate::testing::{chunk, fragmented, view};
use crate::{DecompressorLimits, Resources, gzip};

/// The payload behind `fixtures/system_gzip.gz`, compressed by the system `gzip -9 -n`.
const FIXTURE_PLAINTEXT: &[u8] = b"The quick brown fox jumps over the lazy dog.\nPack my box with five dozen liquor jugs.\n";

const SYSTEM_GZIP: &[u8] = include_bytes!("fixtures/system_gzip.gz");

/// Caps every drain loop in this file.
///
/// A conforming engine always terminates, so exceeding this means the code under test is
/// spinning. A hanging test reports nothing at all, so the cap turns a hang into a failure --
/// which also lets mutation testing reach a verdict instead of timing out.
///
/// The cap has to stay tight enough for that verdict to arrive inside the mutation harness's
/// per-mutant timeout. No test here needs more than a few hundred steps, so this leaves well over
/// an order of magnitude of headroom while still failing a spinning mutant in under a second.
const MAX_STEPS: usize = 10_000;

/// Fails a spinning test instead of letting it hang.
///
/// A conforming engine always terminates, so exceeding the cap means the code under test is
/// looping. A hanging test reports nothing at all, and mutation testing records a timeout rather
/// than a verdict, so every drain loop below counts its steps through this.
struct StepGuard(usize);

impl StepGuard {
    fn new() -> Self {
        Self(0)
    }

    fn step(&mut self) {
        self.0 += 1;
        assert!(self.0 < MAX_STEPS, "the operation did not finish within {MAX_STEPS} steps");
    }
}

/// Drives an engine to completion over an input delivered in `feed`-sized pieces.
fn drive_decompressor(mut decompressor: gzip::Decompressor, input: &BytesView, feed: usize) -> crate::Result<BytesView> {
    let mut offset = 0;
    let mut collected = BytesBuf::new();

    let mut guard = StepGuard::new();
    loop {
        guard.step();
        match decompressor.pull()? {
            Output::Data(data) => collected.put_bytes(data),
            Output::Progress => {}
            Output::Done => return Ok(collected.consume_all()),
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
}

#[test]
fn decompresses_a_stream_produced_by_the_system_gzip() {
    let plain = gzip::decompress(view(SYSTEM_GZIP), &Resources::default()).expect("the fixture decompresses");

    assert_eq!(plain.to_vec(), FIXTURE_PLAINTEXT);
}

#[test]
fn decompresses_concatenated_members_produced_by_the_system_gzip() {
    // Concatenation is what is under test, not a second producer: both members of the packaged
    // two-member file were byte-identical copies of this one, so assembling the input here keeps
    // one independently generated fixture as the source of truth.
    let two_members = [SYSTEM_GZIP, SYSTEM_GZIP].concat();

    let plain = gzip::decompress(view(&two_members), &Resources::default()).expect("the fixture decompresses");

    assert_eq!(plain.to_vec(), [FIXTURE_PLAINTEXT, FIXTURE_PLAINTEXT].concat());
}

#[test]
fn our_framing_matches_an_independent_gzip_reader() {
    // Cross-checks our container against flate2's own gzip framing, which parses the header,
    // checksum and length trailer in Rust rather than in the compression engine.
    use std::io::Read as _;

    let payload = b"cross checked against an independent reader ".repeat(200);
    let compressed = gzip::compress(fragmented(&payload, 71), &Resources::default()).expect("compression succeeds");

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

        let compressed = gzip::compress(fragmented(&payload, segment), &Resources::default()).expect("compression succeeds");
        let plain = gzip::decompress(compressed, &Resources::default()).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), payload, "round trip failed for {segment} byte segments");
    }
}

#[test]
fn round_trips_when_input_arrives_one_byte_at_a_time() {
    let payload = b"trickled in".repeat(50);
    let compressed = gzip::compress(view(&payload), &Resources::default()).expect("compression succeeds");

    let decompressor = gzip::Decompressor::builder()
        .output_chunk_size(chunk(1))
        .build(&Resources::default());
    let plain = drive_decompressor(decompressor, &compressed, 1).expect("decompression succeeds");

    assert_eq!(plain.to_vec(), payload);
}

#[test]
fn streams_a_large_payload_with_a_bounded_working_set() {
    // The point of the push/pull design: a long stream must never require a buffer proportional to
    // its length. Every chunk handed back stays within the configured bound.
    const CHUNK: usize = 16 * 1024;

    let payload = b"large streamed payload, compressible but not trivially so; ".repeat(20_000);
    assert!(payload.len() > 1024 * 1024, "the payload should be large enough to matter");

    let mut compressor = gzip::Compressor::builder()
        .output_chunk_size(chunk(CHUNK))
        .build(&Resources::default());
    compressor.push(fragmented(&payload, 4096)).expect("push succeeds");
    compressor.end_input();

    let mut compressed = Vec::new();
    let mut guard = StepGuard::new();
    loop {
        guard.step();
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
        .build(&Resources::default());
    let plain = drive_decompressor(decompressor, &gz, 8192).expect("decompression succeeds");

    assert_eq!(plain.len(), payload.len());
    assert_eq!(plain.to_vec(), payload);
}

#[test]
fn rejects_a_bomb_before_materialising_it() {
    // A megabyte of zeros compresses to a few hundred bytes. The guard must fire long before the
    // output is fully materialised, so the cap is set far below what the bomb would expand to.
    //
    // The cap is set explicitly rather than relying on the default: deflate cannot expand by more
    // than about `1032x`, so its default ratio never fires on data the format could have produced.
    // An absolute cap is what actually protects a caller that buffers the output.
    let bomb = gzip::compress(view(&vec![0_u8; 1024 * 1024]), &Resources::default()).expect("compression succeeds");
    assert!(bomb.len() < 16 * 1024, "the bomb should be tiny: {} bytes", bomb.len());

    let mut decompressor = gzip::Decompressor::builder()
        .limits(DecompressorLimits::new().max_output_len(NonZeroU64::new(16 * 1024).unwrap()))
        .build(&Resources::default());
    decompressor.push(bomb).expect("push succeeds");
    decompressor.end_input();

    let mut guard = StepGuard::new();
    let error = loop {
        guard.step();
        match decompressor.pull() {
            Ok(Output::Data(_) | Output::Progress) => {}
            Ok(_) => panic!("the bomb decompressed fully instead of being rejected"),
            Err(error) => break error,
        }
    };

    assert!(error.is_limit_exceeded(), "got {error}");
    assert!(
        decompressor.total_out() < 1024 * 1024,
        "the guard should fire before the full expansion, stopped at {}",
        decompressor.total_out()
    );
}

#[test]
fn the_default_limits_accept_maximally_compressible_deflate_data() {
    // Deflate's structural ceiling is about `1032x`, so the gzip default must sit above it: data the
    // format could legitimately have produced must never be rejected as a bomb. A megabyte of zeros
    // reaches that ceiling and clears the ratio guard's 32 KiB floor, so the guard is genuinely
    // active here rather than skipped as too small to judge.
    let payload = vec![0_u8; 1024 * 1024];
    let compressed = gzip::compress(view(&payload), &Resources::default()).expect("compression succeeds");

    let plain = gzip::decompress(compressed, &Resources::default()).expect("default limits must accept maximal deflate compression");

    assert_eq!(plain.len(), payload.len());
}

#[test]
fn known_good_data_can_opt_out_of_the_limits() {
    // The precondition is the data, not the caller: this payload is generated here, so its
    // expansion is known. A trusted caller relaying an attacker's bytes would not qualify.
    let payload = vec![0_u8; 1024 * 1024];
    let compressed = gzip::compress(view(&payload), &Resources::default()).expect("compression succeeds");

    let decompressor = gzip::Decompressor::builder()
        .limits(DecompressorLimits::UNLIMITED)
        .build(&Resources::default());
    let plain = drive_decompressor(decompressor, &compressed, usize::MAX).expect("decompression succeeds");

    assert_eq!(plain.len(), payload.len());
}

#[test]
fn detects_truncation_at_every_offset() {
    let compressed = gzip::compress(view(&b"truncate me ".repeat(500)), &Resources::default()).expect("compression succeeds");

    for cut in [
        1,
        compressed.len() / 4,
        compressed.len() / 2,
        compressed.len() - 8,
        compressed.len() - 1,
    ] {
        let error = gzip::decompress(compressed.range(0..cut), &Resources::default())
            .expect_err("a truncated stream must not decompress successfully");

        assert!(
            error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
            "truncating at {cut} gave an unexpected classification: {error}"
        );
    }
}

#[test]
fn a_corrupted_byte_anywhere_is_detected() {
    let payload = b"integrity checked payload ".repeat(100);
    let compressed = gzip::compress(view(&payload), &Resources::default()).expect("compression succeeds");
    let original = compressed.to_vec();

    for index in [0, 1, 2, original.len() / 2, original.len() - 5, original.len() - 1] {
        let mut corrupted = original.clone();
        corrupted[index] ^= 0xff;

        let result = gzip::decompress(view(&corrupted), &Resources::default());

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
    let compressed = gzip::compress(BytesView::new(), &Resources::default()).expect("compression succeeds");
    let plain = gzip::decompress(compressed, &Resources::default()).expect("decompression succeeds");

    assert!(plain.is_empty());
}

#[test]
fn a_custom_memory_provider_is_used_for_output() {
    // Anything implementing `MemoryShared` works; the engine never reaches for a global allocator
    // of its own. Counting the reservations is what proves it, rather than merely building a
    // provider and hoping.
    let (memory, activity) = crate::testing::counting_memory();
    let resources = Resources::new(memory);

    let compressed = gzip::compress(view(b"provider supplied"), &resources).expect("compression succeeds");
    let after_compress = activity.reservations();
    assert!(after_compress > 0, "compression must draw its output from the caller's provider");

    let plain = gzip::decompress(compressed, &resources).expect("decompression succeeds");

    assert_eq!(plain.to_vec(), b"provider supplied".to_vec());
    assert!(
        activity.reservations() > after_compress,
        "decompression must draw its output from the caller's provider too"
    );
}

#[test]
fn a_stream_of_many_tiny_members_is_rejected_without_the_caller_setting_any_limit() {
    // Each member costs engine setup its own payload never pays for, so a stream of empty members
    // amplifies work out of all proportion to its size. The default stream cap is what bounds it,
    // so the count is derived from that constant rather than restated: this is the smallest input
    // that crosses the boundary, whatever the boundary currently is.
    let members = usize::try_from(crate::limits::DEFAULT_MAX_STREAMS).expect("the cap fits a count") + 1;

    let member = gzip::compress(BytesView::new(), &Resources::default())
        .expect("compression succeeds")
        .to_vec();
    let mut many = Vec::with_capacity(member.len() * members);
    for _ in 0..members {
        many.extend_from_slice(&member);
    }

    let error = gzip::decompress(view(&many), &Resources::default()).expect_err("the default cap should reject this");

    assert!(error.is_limit_exceeded(), "expected a limit failure, got: {error}");
    assert!(
        error.to_string().contains("decoded stream count"),
        "the stream cap should be what fired: {error}"
    );
}
