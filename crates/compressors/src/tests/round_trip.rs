// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end behaviour of gzip, including the parts no other format exercises.
//!
//! Gzip-specific: interop fixtures produced by the system `gzip`, and the concatenated-stream
//! behaviour that only gzip enables by default. Drives the crate-private mechanics where a
//! transition has to be observed rather than inferred from the final bytes.

use std::num::NonZeroU64;

use bytesbuf::{BytesBuf, BytesView};

use crate::core::{CompressionInternal as _, Destination, Output};
use crate::format::Format;
use crate::limits::{DEFAULT_MAX_OUTPUT_LEN, DEFAULT_MAX_STREAMS};
use crate::testing::{chunk, fragmented, view};
use crate::{DecompressorLimits, Resources, gzip};

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
        match decompressor.pull(Destination::Stream)? {
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
fn round_trips_when_input_arrives_one_byte_at_a_time() {
    let payload = b"trickled in".repeat(50);
    let compressed = gzip::compress(view(&payload), &Resources::default()).unwrap();

    let decompressor = gzip::Decompressor::builder()
        .output_chunk_size(chunk(1))
        .build(&Resources::default());
    let plain = drive_decompressor(decompressor, &compressed, 1).unwrap();

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
    compressor.push(fragmented(&payload, 4096)).unwrap();
    compressor.end_input();

    let mut compressed = Vec::new();
    let mut guard = StepGuard::new();
    loop {
        guard.step();
        match compressor.pull(Destination::Stream).unwrap() {
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
    let plain = drive_decompressor(decompressor, &gz, 8192).unwrap();

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
    let bomb = gzip::compress(view(&vec![0_u8; 1024 * 1024]), &Resources::default()).unwrap();
    assert!(bomb.len() < 16 * 1024, "the bomb should be tiny: {} bytes", bomb.len());

    let mut decompressor = gzip::Decompressor::builder()
        .limits(DecompressorLimits::new().max_output_len(NonZeroU64::new(16 * 1024).unwrap()))
        .build(&Resources::default());
    decompressor.push(bomb).unwrap();
    decompressor.end_input();

    let mut guard = StepGuard::new();
    let error = loop {
        guard.step();
        match decompressor.pull(Destination::Stream) {
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
fn a_custom_memory_provider_is_used_for_output() {
    // Anything implementing `MemoryShared` works; the engine never reaches for a global allocator
    // of its own. Counting the reservations is what proves it, rather than merely building a
    // provider and hoping.
    let (memory, activity) = crate::testing::counting_memory();
    let resources = Resources::new(memory);

    let compressed = gzip::compress(view(b"provider supplied"), &resources).unwrap();
    let after_compress = activity.reservations();
    assert!(after_compress > 0, "compression must draw its output from the caller's provider");

    let plain = gzip::decompress(compressed, &resources).unwrap();

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
    let members = usize::try_from(crate::limits::DEFAULT_MAX_STREAMS).unwrap() + 1;

    let member = gzip::compress(BytesView::new(), &Resources::default()).unwrap().to_vec();
    let mut many = Vec::with_capacity(member.len() * members);
    for _ in 0..members {
        many.extend_from_slice(&member);
    }

    let error = gzip::decompress(view(&many), &Resources::default()).unwrap_err();

    assert!(error.is_limit_exceeded(), "expected a limit failure, got: {error}");
    assert!(
        error.to_string().contains("decoded stream count"),
        "the stream cap should be what fired: {error}"
    );
}

#[test]
fn the_crate_level_decompress_applies_the_default_ceiling_unless_the_caller_decided() {
    // `decompress` takes an already-built decompressor, so without a ceiling of its own it was
    // bounded only by whatever that decompressor carried -- and brotli declares no defaults at all,
    // so a hundred compressed bytes could expand without limit.
    let over_the_cap = vec![0_u8; usize::try_from(DEFAULT_MAX_OUTPUT_LEN).unwrap() + 1];
    let compressed = gzip::compress(&*over_the_cap, &Resources::default()).unwrap();

    let decompress_with = |limits: DecompressorLimits| {
        crate::decompress(
            compressed.clone(),
            gzip::Decompressor::builder().limits(limits).build(&Resources::default()),
        )
    };

    // Bound left unset: our 64 MiB stands in, so this is refused.
    let error = decompress_with(DecompressorLimits::new()).unwrap_err();
    assert!(error.is_limit_exceeded(), "got {error}");

    // Raised explicitly: the caller's number wins over ours.
    let raised = DecompressorLimits::new().max_output_len(NonZeroU64::new(DEFAULT_MAX_OUTPUT_LEN * 2).unwrap());
    assert_eq!(decompress_with(raised).unwrap().len(), over_the_cap.len());

    // Removed explicitly: also the caller's decision, so nothing is added on top.
    for removed in [DecompressorLimits::UNLIMITED, DecompressorLimits::new().unbounded_output_len()] {
        assert_eq!(decompress_with(removed).unwrap().len(), over_the_cap.len());
    }

    // Lowered explicitly: still the caller's decision, in the other direction.
    let lowered = DecompressorLimits::new().max_output_len(NonZeroU64::new(1024).unwrap());
    assert!(decompress_with(lowered).unwrap_err().is_limit_exceeded());
}

#[test]
fn the_crate_level_decompress_applies_the_default_stream_cap_unless_the_caller_decided() {
    // The companion to the output ceiling, and the case it cannot cover: many tiny members each
    // pay a full engine setup while producing almost no output, so no output bound ever trips.
    // gzip decompresses concatenated members by default, so a default-built decompressor handed to
    // `decompress` is exactly the exposed shape.
    let member = gzip::compress(b"".as_slice(), &Resources::default()).unwrap();
    let mut concatenated = BytesBuf::new();
    for _ in 0..=DEFAULT_MAX_STREAMS {
        concatenated.put_bytes(member.clone());
    }
    let over_the_cap = concatenated.consume_all();

    let decompress_with = |limits: DecompressorLimits| {
        crate::decompress(
            over_the_cap.clone(),
            gzip::Decompressor::builder().limits(limits).build(&Resources::default()),
        )
    };

    // Bound left unset: our 1024 stands in, so this is refused.
    let error = decompress_with(DecompressorLimits::new()).unwrap_err();
    assert!(error.is_limit_exceeded(), "got {error}");

    // Removed explicitly: the caller's decision, so nothing is added on top.
    for removed in [DecompressorLimits::UNLIMITED, DecompressorLimits::new().unbounded_streams()] {
        assert!(decompress_with(removed).unwrap().is_empty());
    }

    // Raised explicitly: also the caller's decision.
    let raised = DecompressorLimits::new().max_streams(NonZeroU64::new(DEFAULT_MAX_STREAMS * 2).unwrap());
    assert!(decompress_with(raised).unwrap().is_empty());

    // Lowered explicitly: still the caller's decision, in the other direction.
    let lowered = DecompressorLimits::new().max_streams(NonZeroU64::new(4).unwrap());
    assert!(decompress_with(lowered).unwrap_err().is_limit_exceeded());
}

#[test]
fn the_crate_level_decompress_applies_the_default_ceiling_to_runtime_formats_too() {
    // The runtime-format decompressor is a separate `CompressionInternal` implementation, so the
    // ceiling has to reach it the same way it reaches every other one. Brotli is the format that
    // makes this observable: it declares no bounds of its own, so nothing but the ceiling can
    // refuse this.
    let over_the_cap = vec![0_u8; usize::try_from(DEFAULT_MAX_OUTPUT_LEN).unwrap() + 1];
    let compressed = crate::format::compress(Format::Brotli, &*over_the_cap, &Resources::default()).unwrap();

    let decompress_with = |limits: DecompressorLimits| {
        crate::decompress(
            compressed.clone(),
            crate::format::Decompressor::builder()
                .limits(limits)
                .build_format(Format::Brotli, &Resources::default())
                .unwrap(),
        )
    };

    let error = decompress_with(DecompressorLimits::new()).unwrap_err();
    assert!(error.is_limit_exceeded(), "got {error}");

    for removed in [DecompressorLimits::UNLIMITED, DecompressorLimits::new().unbounded_output_len()] {
        assert_eq!(decompress_with(removed).unwrap().len(), over_the_cap.len());
    }
}
