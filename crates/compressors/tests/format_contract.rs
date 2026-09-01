// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One contract, applied to every format.
//!
//! These tests exist to keep the abstraction honest. Every format goes through the same scenarios,
//! so a format that behaves differently from its siblings -- or an abstraction that quietly only
//! fits the deflate family -- fails here rather than surprising a consumer.

#![cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use compressors::format::Format;
use compressors::{Compress, Compressing, Compression, Decompress, DecompressionLimits, Level, Output, Pool, TrailingData};

fn view(bytes: &[u8]) -> BytesView {
    BytesView::copied_from_slice(bytes, &GlobalPool::new())
}

/// Builds a view split into `segment` sized spans, exercising the multi-segment paths.
fn fragmented(bytes: &[u8], segment: usize) -> BytesView {
    let memory = GlobalPool::new();
    BytesView::from_views(bytes.chunks(segment).map(|chunk| BytesView::copied_from_slice(chunk, &memory)))
}

fn chunk(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("test chunk sizes are never zero")
}

/// Drives any compression operation to completion, feeding the input in `feed` sized pieces.
fn process<D>(compression: &mut dyn Compression<Mode = D>, input: &BytesView, feed: usize) -> compressors::Result<BytesView> {
    let mut offset = 0;
    let mut collected = BytesBuf::new();

    loop {
        match compression.pull()? {
            Output::Data(data) => collected.put_bytes(data),
            Output::Progress => {}
            Output::Done => break,
            Output::NeedInput => {
                if offset >= input.len() {
                    compression.end_input();
                    continue;
                }

                let end = (offset + feed).min(input.len());
                compression.push(input.range(offset..end))?;
                offset = end;
            }
        }
    }

    Ok(collected.consume_all())
}

fn compress(compressor: &mut dyn Compression<Mode = Compress>, input: &BytesView, feed: usize) -> compressors::Result<BytesView> {
    process(compressor, input, feed)
}

fn decompress(decompressor: &mut dyn Compression<Mode = Decompress>, input: &BytesView, feed: usize) -> compressors::Result<BytesView> {
    process(decompressor, input, feed)
}

/// Generates the shared contract for one format, using its concrete module so the builders are
/// exercised too, not just the runtime `Format` factories.
macro_rules! format_contract {
    ($module:ident, $format:expr) => {
        mod $module {
            use compressors::$module;

            use super::*;

            const FORMAT: Format = $format;

            fn payload() -> Vec<u8> {
                b"the quick brown fox jumps over the lazy dog; pack my box with five dozen liquor jugs. ".repeat(300)
            }

            #[test]
            fn round_trips_a_payload() {
                let memory = GlobalPool::new();
                let data = payload();

                let compressed = $module::compress(view(&data), memory.clone()).expect("compression succeeds");
                assert!(compressed.len() < data.len(), "the payload should compress");

                let plain = $module::decompress(compressed, memory).expect("decompression succeeds");
                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn compress_matches_driving_the_operation_by_hand() {
                // The convenience must be exactly the manual loop, not an approximation of it.
                let memory = GlobalPool::new();
                let data = payload();

                let convenient = $module::Compressor::new(memory.clone())
                    .compress(view(&data))
                    .expect("compression succeeds");

                let mut by_hand = $module::Compressor::new(memory.clone());
                by_hand.push(view(&data)).expect("push succeeds");
                Compression::end_input(&mut by_hand);
                let mut collected = BytesBuf::new();
                loop {
                    match Compression::pull(&mut by_hand).expect("pull succeeds") {
                        Output::Data(chunk) => collected.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("compressor requested input after end"),
                        Output::Done => break,
                    }
                }

                assert_eq!(convenient.to_vec(), collected.consume_all().to_vec());

                let plain = $module::Decompressor::new(memory)
                    .decompress(convenient)
                    .expect("decompression succeeds");

                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn compress_and_decompress_work_through_a_trait_object() {
                // Provided methods are easy to break for `dyn`, so reach them that way too.
                let memory = GlobalPool::new();
                let data = payload();

                let compressor: Box<dyn Compression<Mode = Compress>> = Box::new($module::Compressor::new(memory.clone()));
                let compressed = compressor.compress(view(&data)).expect("compression succeeds");

                let decompressor: Box<dyn Compression<Mode = Decompress>> = Box::new($module::Decompressor::new(memory));

                assert_eq!(
                    decompressor.decompress(compressed).expect("decompression succeeds").to_vec(),
                    data
                );
            }

            #[test]
            fn round_trips_empty_input() {
                let memory = GlobalPool::new();

                let compressed = $module::compress(BytesView::new(), memory.clone()).expect("compression succeeds");
                let plain = $module::decompress(compressed, memory).expect("decompression succeeds");

                assert!(plain.is_empty());
            }

            #[test]
            fn round_trips_a_multi_segment_view() {
                // The reason this crate exists: input arrives as a chain of spans, never as one
                // contiguous slice.
                for (segment, repeats) in [(1_usize, 40_usize), (7, 200), (1024, 2_000)] {
                    let data = b"multi segment ".repeat(repeats);
                    let memory = GlobalPool::new();

                    let compressed = $module::compress(fragmented(&data, segment), memory.clone()).expect("compression succeeds");
                    let plain = $module::decompress(compressed, memory).expect("decompression succeeds");

                    assert_eq!(plain.to_vec(), data, "failed at {segment} byte segments");
                }
            }

            #[test]
            fn round_trips_when_driven_one_byte_at_a_time() {
                // Worst case for a push/pull codec: minimal input pieces and minimal output chunks.
                let memory = GlobalPool::new();
                let data = b"drip fed".repeat(20);

                let mut compressor = $module::Compressor::builder()
                    .output_chunk_size(chunk(1))
                    .build(memory.clone());
                let compressed = compress(&mut compressor, &view(&data), 1).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder().output_chunk_size(chunk(1)).build(memory);
                let plain = decompress(&mut decompressor, &compressed, 1).expect("decompression succeeds");

                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn honours_the_output_chunk_size() {
                let memory = GlobalPool::new();
                let data = payload();

                let mut compressor = $module::Compressor::builder()
                    .output_chunk_size(chunk(256))
                    .build(memory.clone());
                compressor.push(view(&data)).expect("push succeeds");
                Compression::end_input(&mut compressor);

                let mut compressed = BytesBuf::new();
                loop {
                    match Compression::pull(&mut compressor).expect("pull succeeds") {
                        Output::Data(piece) => {
                            assert!(piece.len() <= 256, "chunk of {} bytes exceeded the bound", piece.len());
                            compressed.put_bytes(piece);
                        }
                        Output::Progress => {}
                        Output::NeedInput => panic!("compressor requested input after end"),
                        Output::Done => break,
                    }
                }

                let mut decompressor = $module::Decompressor::builder().output_chunk_size(chunk(256)).build(memory);
                decompressor.push(compressed.consume_all()).expect("push succeeds");
                Compression::end_input(&mut decompressor);

                let mut plain = BytesBuf::new();
                loop {
                    match Compression::pull(&mut decompressor).expect("pull succeeds") {
                        Output::Data(piece) => {
                            assert!(piece.len() <= 256, "chunk of {} bytes exceeded the bound", piece.len());
                            plain.put_bytes(piece);
                        }
                        Output::Progress => {}
                        Output::NeedInput => panic!("decompressor requested input after end"),
                        Output::Done => break,
                    }
                }

                assert_eq!(plain.consume_all().to_vec(), data);
            }

            #[test]
            fn every_level_produces_a_decodable_stream() {
                let data = payload();

                for raw in 0..=Level::MAX.get() {
                    let level = Level::new(raw).expect("level is in range");
                    let memory = GlobalPool::new();

                    let mut compressor = $module::Compressor::builder().level(level).build(memory.clone());
                    let compressed = compress(&mut compressor, &view(&data), usize::MAX).expect("compression succeeds");

                    let plain = $module::decompress(compressed, memory).expect("decompression succeeds");
                    assert_eq!(plain.to_vec(), data, "level {raw} did not round trip");
                }
            }

            #[test]
            fn tracks_byte_counts() {
                let memory = GlobalPool::new();
                let data = payload();

                let mut compressor = $module::Compressor::new(memory.clone());
                let compressed = compress(&mut compressor, &view(&data), usize::MAX).expect("compression succeeds");

                assert_eq!(compressor.total_in(), data.len() as u64);
                assert_eq!(compressor.total_out(), compressed.len() as u64);

                let mut decompressor = $module::Decompressor::new(memory);
                let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                assert_eq!(decompressor.total_in(), compressed.len() as u64);
                assert_eq!(decompressor.total_out(), plain.len() as u64);
            }

            #[test]
            fn rejects_a_truncated_stream() {
                let memory = GlobalPool::new();
                let compressed = $module::compress(view(&payload()), memory).expect("compression succeeds");

                for cut in [1, compressed.len() / 3, compressed.len() - 1] {
                    let error = $module::decompress(compressed.range(0..cut), GlobalPool::new())
                        .expect_err("a truncated stream must not decompress successfully");

                    assert!(
                        error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
                        "truncating at {cut} gave an unexpected classification: {error}"
                    );
                }
            }

            #[test]
            fn rejects_input_after_end_input() {
                let mut compressor = $module::Compressor::new(GlobalPool::new());
                Compression::end_input(&mut compressor);

                let error = compressor.push(view(b"late")).expect_err("push after end_input is rejected");
                assert!(error.is_invalid_state());

                let mut decompressor = $module::Decompressor::new(GlobalPool::new());
                Compression::end_input(&mut decompressor);

                let error = decompressor
                    .push(view(b"late"))
                    .expect_err("push after end_input is rejected");
                assert!(error.is_invalid_state());
            }

            #[test]
            fn asks_for_more_input_before_end_input() {
                let mut compressor = $module::Compressor::new(GlobalPool::new());
                compressor.push(view(b"partial")).expect("push succeeds");

                let output = loop {
                    match Compression::pull(&mut compressor).expect("pull succeeds") {
                        Output::Data(_) | Output::Progress => {}
                        other => break other,
                    }
                };

                assert!(output.is_need_input(), "an unfinished compressor must ask for more input");
            }

            #[test]
            fn enforces_a_configured_expansion_limit() {
                // A ratio the data is guaranteed to exceed, so the mechanism itself is tested
                // rather than whichever default the format happens to carry.
                let memory = GlobalPool::new();
                let bomb = $module::compress(view(&vec![0_u8; 16 * 1024 * 1024]), memory.clone()).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .limits(DecompressionLimits::new().with_max_ratio(NonZeroU32::new(4).expect("4 is not zero")))
                    .build(memory);
                decompressor.push(bomb).expect("push succeeds");
                Compression::end_input(&mut decompressor);

                let error = loop {
                    match Compression::pull(&mut decompressor) {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("the bomb decompressed fully instead of being rejected"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_limit_exceeded(), "got {error}");
                assert!(
                    decompressor.total_out() < 16 * 1024 * 1024,
                    "the guard should fire before the full expansion"
                );
            }

            #[test]
            fn default_limits_accept_ordinary_highly_compressible_data() {
                // Regression guard. A single portable ratio limit was calibrated on deflate, whose
                // structural ceiling is about `1032x`. Brotli legitimately reaches tens of thousands of
                // times expansion, so that limit rejected ordinary repetitive input -- a repeated
                // sentence, and JSON. Each format now carries its own default.
                let memory = GlobalPool::new();

                let cases: [(&str, Vec<u8>); 3] = [
                    ("repeated short string", b"windowed ".repeat(20_000)),
                    (
                        "repeated sentence",
                        b"the quick brown fox jumps over the lazy dog. ".repeat(20_000),
                    ),
                    (
                        "repetitive json",
                        br#"{"id":1,"name":"widget","tags":["a","b"]},"#.repeat(12_000),
                    ),
                ];

                for (label, data) in cases {
                    let compressed = $module::compress(view(&data), memory.clone()).expect("compression succeeds");
                    let ratio = data.len() / compressed.len().max(1);

                    let plain = $module::decompress(compressed, memory.clone())
                        .unwrap_or_else(|error| panic!("default limits rejected {label} at {ratio}x expansion: {error}"));

                    assert_eq!(plain.to_vec(), data, "{label} did not round trip");
                }
            }

            #[test]
            fn an_absolute_cap_is_enforced() {
                let memory = GlobalPool::new();
                let compressed = $module::compress(view(&vec![0_u8; 4 * 1024 * 1024]), memory.clone()).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .limits(DecompressionLimits::new().without_max_ratio().with_max_output_len(1024))
                    .build(memory);
                decompressor.push(compressed).expect("push succeeds");
                Compression::end_input(&mut decompressor);

                let error = loop {
                    match Compression::pull(&mut decompressor) {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("the cap should have fired"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_limit_exceeded(), "got {error}");
            }

            #[test]
            fn trusted_callers_can_opt_out_of_the_limits() {
                let memory = GlobalPool::new();
                let data = vec![0_u8; 4 * 1024 * 1024];
                let compressed = $module::compress(view(&data), memory.clone()).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .limits(DecompressionLimits::UNLIMITED)
                    .build(memory);
                let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                assert_eq!(plain.len(), data.len());
            }

            #[test]
            fn corruption_is_detected_or_changes_the_output() {
                // Formats with a checksum report corruption; raw deflate has none, so the honest
                // universal guarantee is only that corrupt input does not silently reproduce the
                // original bytes.
                let memory = GlobalPool::new();
                let data = payload();
                let compressed = $module::compress(view(&data), memory).expect("compression succeeds");
                let original = compressed.to_vec();

                for index in [0, original.len() / 2, original.len() - 1] {
                    let mut corrupted = original.clone();
                    corrupted[index] ^= 0xff;

                    match $module::decompress(view(&corrupted), GlobalPool::new()) {
                        Ok(plain) => assert_ne!(plain.to_vec(), data, "corruption at {index} went unnoticed"),
                        Err(error) => assert!(
                            error.is_corrupt_data() || error.is_unexpected_end_of_stream() || error.is_limit_exceeded(),
                            "corruption at {index} gave an unexpected classification: {error}"
                        ),
                    }
                }
            }

            #[test]
            fn the_runtime_factory_matches_the_module() {
                // `Format` must produce codecs equivalent to the concrete modules, or runtime
                // selection would silently behave differently from compile-time selection.
                let memory = GlobalPool::new();
                let data = payload();

                let via_module = $module::compress(view(&data), memory.clone()).expect("compression succeeds");
                let via_format = FORMAT.compress(view(&data), memory.clone()).expect("compression succeeds");

                assert_eq!(
                    via_module.to_vec(),
                    via_format.to_vec(),
                    "runtime and compile-time selection diverged"
                );

                // Either output must decompress through either path.
                assert_eq!(
                    FORMAT
                        .decompress(via_module, memory.clone())
                        .expect("decompression succeeds")
                        .to_vec(),
                    data
                );
                assert_eq!(
                    $module::decompress(via_format, memory)
                        .expect("decompression succeeds")
                        .to_vec(),
                    data
                );
            }

            #[test]
            fn works_through_boxed_trait_objects() {
                let memory = GlobalPool::new();
                let data = payload();

                let mut compressor = FORMAT.compressor().build(memory.clone());
                let compressed = compress(&mut *compressor, &view(&data), usize::MAX).expect("compression succeeds");

                let mut decompressor = FORMAT.decompressor().build(memory);
                let plain = decompress(&mut *decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn pooling_does_not_change_the_output() {
                // Reuse is an optimisation, so it must change nothing a caller can observe. The
                // baseline and the pooled runs share one input view on purpose: some engines
                // legitimately vary with input segmentation (zstd records the content size in its
                // frame header only when the whole input arrives in one call), so a fresh view per
                // run would compare allocator behaviour rather than pooling.
                let pool = Pool::new();
                let input = view(&payload());
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(GlobalPool::new());
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                // Several rounds: the first compressor always misses the pool, so only later rounds
                // exercise a recycled engine.
                for round in 0..5 {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .pool(pool.clone())
                        .build(GlobalPool::new());
                    let pooled = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");
                    drop(compressor);

                    assert_eq!(pooled.to_vec(), baseline.to_vec(), "round {round}: pooled output diverged");

                    let mut decompressor = $module::Decompressor::builder().pool(pool.clone()).build(GlobalPool::new());
                    let plain = decompress(&mut decompressor, &pooled, usize::MAX).expect("decompression succeeds");

                    assert_eq!(plain.to_vec(), payload(), "round {round}: pooled decompressor lost data");
                }
            }

            #[test]
            fn an_engine_abandoned_mid_stream_is_cleaned_before_reuse() {
                // A request cancelled part-way through returns a half-used engine.
                let pool = Pool::new();
                let input = view(&payload());
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(GlobalPool::new());
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                for round in 0..4 {
                    {
                        let mut abandoned = $module::Compressor::builder()
                            .output_chunk_size(chunk(4096))
                            .pool(pool.clone())
                            .build(GlobalPool::new());
                        abandoned.push(input.clone()).expect("push succeeds");
                        let _ = Compression::pull(&mut abandoned).expect("pull succeeds");
                        // Dropped without finishing, so its engine is mid-frame.
                    }

                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .pool(pool.clone())
                        .build(GlobalPool::new());
                    let recovered = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");

                    assert_eq!(recovered.to_vec(), baseline.to_vec(), "round {round}: a dirty engine leaked");
                }
            }

            #[test]
            fn an_engine_left_dirty_by_a_failed_decompression_is_cleaned_before_reuse() {
                let pool = Pool::new();
                let compressed = $module::compress(view(&payload()), GlobalPool::new()).expect("compression succeeds");
                let garbage = view(&b"definitely not a valid stream".repeat(20));

                for round in 0..4 {
                    {
                        let mut failing = $module::Decompressor::builder().pool(pool.clone()).build(GlobalPool::new());
                        let _ = decompress(&mut failing, &garbage, usize::MAX);
                    }

                    let mut decompressor = $module::Decompressor::builder().pool(pool.clone()).build(GlobalPool::new());
                    let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("a clean stream still decompresses");

                    assert_eq!(
                        plain.to_vec(),
                        payload(),
                        "round {round}: a failed decompress poisoned the pool"
                    );
                }
            }

            #[test]
            fn levels_never_share_engines() {
                // Resetting a compressor preserves its level, so engines must be keyed by it.
                let pool = Pool::new();
                let input = view(&payload());
                let levels = [Level::MIN, Level::FAST, Level::DEFAULT, Level::HIGH];

                let baselines: Vec<_> = levels
                    .iter()
                    .map(|&level| {
                        let mut compressor = $module::Compressor::builder()
                            .level(level)
                            .output_chunk_size(chunk(4096))
                            .build(GlobalPool::new());
                        compress(&mut compressor, &input, usize::MAX)
                            .expect("compression succeeds")
                            .to_vec()
                    })
                    .collect();

                for round in 0..4 {
                    for (index, &level) in levels.iter().enumerate() {
                        let mut compressor = $module::Compressor::builder()
                            .level(level)
                            .output_chunk_size(chunk(4096))
                            .pool(pool.clone())
                            .build(GlobalPool::new());
                        let pooled = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");

                        assert_eq!(
                            pooled.to_vec(),
                            baselines[index],
                            "round {round}: a pooled engine came back at the wrong level"
                        );
                    }
                }
            }

            #[test]
            fn two_live_codecs_get_distinct_engines() {
                // All three compressors are driven by exactly the same sequence, so any difference in
                // their output is the engine and nothing else.
                fn run(compressor: &mut $module::Compressor, input: &BytesView) -> Vec<u8> {
                    compressor.push(input.clone()).expect("push succeeds");
                    Compression::end_input(compressor);

                    let mut collected = BytesBuf::new();
                    loop {
                        match Compression::pull(compressor).expect("pull succeeds") {
                            Output::Data(chunk) => collected.put_bytes(chunk),
                            Output::Progress => {}
                            Output::NeedInput => panic!("compressor requested input after end"),
                            Output::Done => break,
                        }
                    }

                    collected.consume_all().to_vec()
                }

                fn build(pool: Option<&Pool>) -> $module::Compressor {
                    let builder = $module::Compressor::builder().output_chunk_size(chunk(4096));
                    match pool {
                        Some(pool) => builder.pool(pool.clone()).build(GlobalPool::new()),
                        None => builder.build(GlobalPool::new()),
                    }
                }

                let pool = Pool::new();
                let input = view(&payload());
                let baseline = run(&mut build(None), &input);

                // Prime the pool so there is exactly one idle engine for two codecs to want.
                drop(run(&mut build(Some(&pool)), &input));

                let mut first = build(Some(&pool));
                let mut second = build(Some(&pool));

                // Interleave: both are live before either finishes, so they cannot be sharing.
                first.push(input.clone()).expect("push succeeds");
                second.push(input.clone()).expect("push succeeds");
                Compression::end_input(&mut first);
                Compression::end_input(&mut second);

                for (label, compressor) in [("first", &mut first), ("second", &mut second)] {
                    let mut collected = BytesBuf::new();
                    loop {
                        match Compression::pull(compressor).expect("pull succeeds") {
                            Output::Data(chunk) => collected.put_bytes(chunk),
                            Output::Progress => {}
                            Output::NeedInput => panic!("compressor requested input after end"),
                            Output::Done => break,
                        }
                    }

                    assert_eq!(
                        collected.consume_all().to_vec(),
                        baseline,
                        "{label} compressor was corrupted by sharing"
                    );
                }
            }

            #[test]
            fn a_codec_outliving_its_pool_handle_still_works() {
                let input = view(&payload());
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(GlobalPool::new());
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                let mut compressor = {
                    let pool = Pool::new();
                    let compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .pool(pool.clone())
                        .build(GlobalPool::new());
                    drop(pool);
                    compressor
                };

                let pooled = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");

                assert_eq!(
                    pooled.to_vec(),
                    baseline.to_vec(),
                    "dropping the pool handle changed the output"
                );
            }

            #[test]
            fn pool_capacity_bounds_retention_without_changing_output() {
                let input = view(&payload());
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(GlobalPool::new());
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                for capacity in [0_usize, 1, 4] {
                    let pool = Pool::with_capacity(capacity);
                    assert_eq!(pool.capacity(), capacity);

                    for round in 0..12 {
                        let mut compressor = $module::Compressor::builder()
                            .output_chunk_size(chunk(4096))
                            .pool(pool.clone())
                            .build(GlobalPool::new());
                        let pooled = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");

                        assert_eq!(
                            pooled.to_vec(),
                            baseline.to_vec(),
                            "capacity {capacity} round {round}: output changed"
                        );
                    }
                }
            }

            #[test]
            fn empty_input_round_trips_through_a_pool() {
                let pool = Pool::new();
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(GlobalPool::new());
                    compress(&mut compressor, &BytesView::new(), usize::MAX).expect("compression succeeds")
                };

                for round in 0..4 {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .pool(pool.clone())
                        .build(GlobalPool::new());
                    let pooled = compress(&mut compressor, &BytesView::new(), usize::MAX).expect("compression succeeds");
                    drop(compressor);

                    assert_eq!(pooled.to_vec(), baseline.to_vec(), "round {round}: empty framing changed");

                    let mut decompressor = $module::Decompressor::builder().pool(pool.clone()).build(GlobalPool::new());
                    let plain = decompress(&mut decompressor, &pooled, usize::MAX).expect("decompression succeeds");

                    assert!(plain.is_empty(), "round {round}: empty input produced bytes");
                }
            }

            #[test]
            fn truncation_is_still_detected_when_pooled() {
                let pool = Pool::new();
                let compressed = $module::compress(view(&payload()), GlobalPool::new()).expect("compression succeeds");

                for round in 0..3 {
                    // A healthy decompress first, so the next decompressor is guaranteed to be recycled.
                    let mut healthy = $module::Decompressor::builder().pool(pool.clone()).build(GlobalPool::new());
                    decompress(&mut healthy, &compressed, usize::MAX).expect("the full stream decompresses");
                    drop(healthy);

                    let mut decompressor = $module::Decompressor::builder().pool(pool.clone()).build(GlobalPool::new());
                    let error = decompress(&mut decompressor, &compressed.range(0..compressed.len() - 1), usize::MAX)
                        .expect_err("a truncated stream must not decompress successfully");

                    assert!(
                        error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
                        "round {round}: unexpected classification {error}"
                    );
                }
            }

            #[test]
            fn a_flush_makes_supplied_input_decompressible_without_ending_the_stream() {
                let memory = GlobalPool::new();
                let data = b"flush this data now ".repeat(20_000);
                let mut compressor = $module::Compressor::new(memory.clone());
                compressor.push(view(&data)).expect("push succeeds");
                compressor.flush().expect("flush request succeeds");

                let mut compressed = BytesBuf::new();
                loop {
                    match compressor.pull().expect("pull succeeds") {
                        Output::Data(chunk) => compressed.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("a flush must not end the stream"),
                    }
                }

                let mut decompressor = $module::Decompressor::new(memory);
                decompressor.push(compressed.consume_all()).expect("push succeeds");

                let mut plain = BytesBuf::new();
                loop {
                    match decompressor.pull().expect("pull succeeds") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("the compressor has not ended the stream"),
                    }
                }

                assert_eq!(plain.consume_all().to_vec(), data);
            }

            #[test]
            fn end_input_can_be_queued_behind_a_flush() {
                let memory = GlobalPool::new();
                let data = b"flush and finish ".repeat(200);
                let mut compressor = $module::Compressor::new(memory.clone());
                compressor.push(view(&data)).expect("push succeeds");
                compressor.flush().expect("flush request succeeds");
                compressor.end_input();
                let error = compressor
                    .flush()
                    .expect_err("a flush queued behind end_input cannot be requested again");
                assert!(error.is_invalid_state(), "got {error}");

                let mut compressed = BytesBuf::new();
                loop {
                    match compressor.pull().expect("pull succeeds") {
                        Output::Data(chunk) => compressed.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("end of input is already queued"),
                        Output::Done => break,
                    }
                }

                let plain = $module::decompress(compressed.consume_all(), memory).expect("decompression succeeds");
                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn flush_terminates_with_tiny_output_chunks() {
                let data = b"tiny flush chunks ".repeat(100);

                for size in 1..=7 {
                    let memory = GlobalPool::new();
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(size))
                        .build(memory.clone());
                    compressor.push(view(&data)).expect("push succeeds");
                    compressor.flush().expect("flush request succeeds");

                    let mut compressed = BytesBuf::new();
                    let mut pulls = 0;
                    loop {
                        pulls += 1;
                        assert!(pulls < 20_000, "flush did not terminate at chunk size {size}");

                        match compressor.pull().expect("pull succeeds") {
                            Output::Data(piece) => {
                                assert!(piece.len() <= size);
                                compressed.put_bytes(piece);
                            }
                            Output::Progress => {}
                            Output::NeedInput => break,
                            Output::Done => panic!("flush ended the stream"),
                        }
                    }

                    compressor.end_input();
                    loop {
                        match compressor.pull().expect("finish succeeds") {
                            Output::Data(piece) => {
                                assert!(piece.len() <= size);
                                compressed.put_bytes(piece);
                            }
                            Output::Progress => {}
                            Output::NeedInput => panic!("compressor requested input after end"),
                            Output::Done => break,
                        }
                    }

                    let plain = $module::decompress(compressed.consume_all(), memory)
                        .unwrap_or_else(|error| panic!("chunk size {size} did not round trip: {error}"));
                    assert_eq!(plain.to_vec(), data);
                }
            }

            #[test]
            fn multi_stream_decompression_crosses_push_boundaries() {
                let memory = GlobalPool::new();
                let first_plain = b"first stream ".repeat(40);
                let second_plain = b"second stream ".repeat(40);
                let first = $module::compress(view(&first_plain), memory.clone()).expect("compress");
                let second = $module::compress(view(&second_plain), memory.clone()).expect("compress");
                let mut decompressor = $module::Decompressor::builder().multi_stream(true).build(memory);
                let mut plain = BytesBuf::new();

                decompressor.push(first).expect("first push succeeds");
                loop {
                    match decompressor.pull().expect("first stream decompresses") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("decompressor ended before the next pushed stream"),
                    }
                }

                decompressor.push(second).expect("second push succeeds");
                decompressor.end_input();
                loop {
                    match decompressor.pull().expect("second stream decompresses") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("decompressor requested input after end"),
                        Output::Done => break,
                    }
                }

                assert_eq!(plain.consume_all().to_vec(), [first_plain, second_plain].concat());
            }

            #[test]
            fn single_stream_decompression_preserves_buffered_trailing_data() {
                let memory = GlobalPool::new();
                let data = payload();
                let compressed = $module::compress(view(&data), memory.clone()).expect("compress");
                let trailing = view(b"next protocol message");
                let joined = BytesView::from_views([compressed, trailing.clone()]);
                let mut decompressor = $module::Decompressor::builder().multi_stream(false).build(memory);
                decompressor.push(joined).expect("push succeeds");

                let mut plain = BytesBuf::new();
                loop {
                    match decompressor.pull().expect("decompression succeeds") {
                        Output::Data(chunk) => {
                            plain.put_bytes(chunk);
                            let error = decompressor
                                .take_remainder()
                                .expect_err("the remainder is unavailable before Done");
                            assert!(error.is_invalid_state(), "got {error}");
                        }
                        Output::Progress => {}
                        Output::NeedInput => panic!("single stream was complete"),
                        Output::Done => break,
                    }
                }

                assert_eq!(plain.consume_all().to_vec(), data);
                assert_eq!(
                    decompressor.take_remainder().expect("done exposes remainder").to_vec(),
                    trailing.to_vec()
                );
            }

            #[test]
            fn an_empty_push_does_not_create_a_phantom_stream() {
                let memory = GlobalPool::new();
                let data = b"one member only".repeat(20);
                let compressed = $module::compress(view(&data), memory.clone()).expect("compress");
                let mut decompressor = $module::Decompressor::builder().multi_stream(true).build(memory);
                decompressor.push(compressed).expect("first push succeeds");

                let mut plain = BytesBuf::new();
                loop {
                    match decompressor.pull().expect("stream decompresses") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("multi-stream decompressor must wait for EOF"),
                    }
                }

                decompressor.push(BytesView::new()).expect("empty chunks are ignored");
                assert!(decompressor.pull().expect("pull succeeds").is_need_input());
                decompressor.end_input();
                assert!(decompressor.pull().expect("EOF completes").is_done());
                assert_eq!(plain.consume_all().to_vec(), data);
            }

            #[test]
            fn multi_stream_end_input_handles_an_internal_member_boundary() {
                let memory = GlobalPool::new();
                let first_plain = b"AAAAAAAAAA";
                let second_plain = b"BBBBBBBBBB";
                let first = $module::compress(view(first_plain), memory.clone()).expect("compress");
                let second = $module::compress(view(second_plain), memory.clone()).expect("compress");
                let split = first.len().saturating_sub(1);
                let joined = BytesView::from_views([first.range(0..split), first.range(split..), second]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(true)
                    .output_chunk_size(chunk(first_plain.len()))
                    .build(memory);
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let mut plain = BytesBuf::new();
                loop {
                    match decompressor.pull().expect("both streams decompress") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("all input was already supplied"),
                        Output::Done => break,
                    }
                }

                assert_eq!(
                    plain.consume_all().to_vec(),
                    [first_plain.as_slice(), second_plain.as_slice()].concat()
                );
            }

            #[test]
            fn strict_trailing_data_is_rejected_across_push_boundaries() {
                let memory = GlobalPool::new();
                let compressed = $module::compress(view(&payload()), memory.clone()).expect("compress");
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(false)
                    .trailing_data(TrailingData::Reject)
                    .build(memory);
                decompressor.push(compressed).expect("push succeeds");

                loop {
                    match decompressor.pull().expect("stream itself is valid") {
                        Output::Data(_) | Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("strict trailing validation must wait for EOF"),
                    }
                }

                let error = decompressor
                    .push(view(b"trailing"))
                    .expect_err("later trailing input is rejected");
                assert!(error.is_corrupt_data(), "got {error}");
            }

            #[test]
            fn strict_trailing_data_is_rejected_in_the_same_push() {
                let memory = GlobalPool::new();
                let compressed = $module::compress(view(&payload()), memory.clone()).expect("compress");
                let joined = BytesView::from_views([compressed, view(b"trailing")]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(false)
                    .trailing_data(TrailingData::Reject)
                    .build(memory);
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let error = loop {
                    match decompressor.pull() {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("trailing input unexpectedly completed"),
                        Err(error) => break error,
                    }
                };
                assert!(error.is_corrupt_data(), "got {error}");
            }

            #[test]
            fn incomplete_trailing_stream_is_corrupt_data() {
                let memory = GlobalPool::new();
                let compressed = $module::compress(view(&payload()), memory.clone()).expect("compress");
                let joined = BytesView::from_views([compressed, view(&[0])]);
                let mut decompressor = $module::Decompressor::builder().multi_stream(true).build(memory);
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let error = loop {
                    match decompressor.pull() {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("incomplete trailing stream unexpectedly completed"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_corrupt_data(), "got {error}");
            }

            #[test]
            fn stream_count_limit_rejects_before_decompressing_the_next_stream() {
                let memory = GlobalPool::new();
                let data = payload();
                let compressed = $module::compress(view(&data), memory.clone()).expect("compress");
                let joined = BytesView::from_views([compressed.clone(), compressed]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(true)
                    .limits(DecompressionLimits::new().with_max_streams(NonZeroU64::new(1).expect("one is non-zero")))
                    .build(memory);
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let error = loop {
                    match decompressor.pull() {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("the second stream should exceed the limit"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_limit_exceeded(), "got {error}");
                assert!(error.to_string().contains("reached 2"), "got {error}");
                assert!(error.to_string().contains("limit of 1"), "got {error}");
                assert_eq!(decompressor.total_out(), data.len() as u64);
            }

            #[test]
            fn stream_count_limit_rejects_a_later_push() {
                let memory = GlobalPool::new();
                let compressed = $module::compress(view(&payload()), memory.clone()).expect("compress");
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(true)
                    .limits(DecompressionLimits::new().with_max_streams(NonZeroU64::new(1).expect("one is non-zero")))
                    .build(memory);
                decompressor.push(compressed.clone()).expect("first push succeeds");

                loop {
                    match decompressor.pull().expect("first stream decompresses") {
                        Output::Data(_) | Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("multi-stream decompressor must wait for EOF"),
                    }
                }

                let error = decompressor.push(compressed).expect_err("a second stream exceeds the limit");
                assert!(error.is_limit_exceeded(), "got {error}");
            }

            #[test]
            fn absolute_output_limit_is_exact() {
                let memory = GlobalPool::new();
                let data = payload();
                let compressed = $module::compress(view(&data), memory.clone()).expect("compress");

                let exact = $module::decompress_with_limits(
                    compressed.clone(),
                    memory.clone(),
                    DecompressionLimits::new()
                        .without_max_ratio()
                        .with_max_output_len(data.len() as u64),
                )
                .expect("an exact limit succeeds");
                assert_eq!(exact.to_vec(), data);

                let maximum = data.len() as u64 - 1;
                let error = $module::decompress_with_limits(
                    compressed,
                    memory,
                    DecompressionLimits::new().without_max_ratio().with_max_output_len(maximum),
                )
                .expect_err("one byte beyond the cap is rejected");

                assert!(error.is_limit_exceeded(), "got {error}");
                assert!(error.to_string().contains(&(maximum + 1).to_string()), "got {error}");
            }

            #[test]
            fn a_fatal_error_makes_the_decompressor_terminal() {
                let mut decompressor = $module::Decompressor::new(GlobalPool::new());
                decompressor.push(view(b"not a valid stream")).expect("push succeeds");
                decompressor.end_input();

                let first = loop {
                    match decompressor.pull() {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("invalid input unexpectedly completed"),
                        Err(error) => break error,
                    }
                };
                assert!(first.is_corrupt_data() || first.is_unexpected_end_of_stream(), "got {first}");

                let second = decompressor.pull().expect_err("failed operations are terminal");
                assert!(second.is_invalid_state(), "got {second}");
            }

            #[test]
            fn works_through_generic_format_agnostic_code() {
                /// Code written once, against the traits, with no knowledge of the format.
                fn transcode(
                    mut compressor: impl Compression<Mode = Compress>,
                    mut decompressor: impl Compression<Mode = Decompress>,
                    data: &[u8],
                ) -> Vec<u8> {
                    let compressed = compress(&mut compressor, &view(data), 64).expect("compression succeeds");
                    decompress(&mut decompressor, &compressed, 64)
                        .expect("decompression succeeds")
                        .to_vec()
                }

                let memory = GlobalPool::new();
                let data = payload();

                assert_eq!(
                    transcode(
                        $module::Compressor::new(memory.clone()),
                        $module::Decompressor::new(memory),
                        &data
                    ),
                    data
                );
            }
        }
    };
}

#[cfg(feature = "deflate")]
format_contract!(deflate, Format::Deflate);
#[cfg(feature = "zlib")]
format_contract!(zlib, Format::Zlib);
#[cfg(feature = "gzip")]
format_contract!(gzip, Format::Gzip);
#[cfg(feature = "brotli")]
format_contract!(brotli, Format::Brotli);
#[cfg(feature = "zstd")]
format_contract!(zstd, Format::Zstd);

#[test]
fn every_compiled_format_satisfies_the_contract() {
    // Guards against a format being added to `Format::ALL` without being added to the suite above.
    let covered = usize::from(cfg!(feature = "deflate"))
        + usize::from(cfg!(feature = "zlib"))
        + usize::from(cfg!(feature = "gzip"))
        + usize::from(cfg!(feature = "brotli"))
        + usize::from(cfg!(feature = "zstd"));

    assert_eq!(
        Format::ALL.len(),
        covered,
        "a format was added without extending the contract suite"
    );
}

#[test]
fn formats_produce_mutually_incompatible_streams() {
    // Each format must be genuinely distinct: decoding one format's output with another's decompressor
    // must fail rather than silently produce garbage.
    let memory = GlobalPool::new();
    let data = b"cross format check ".repeat(200);

    for &produced_by in Format::ALL {
        let compressed = produced_by.compress(view(&data), memory.clone()).expect("compression succeeds");

        for &decompressed_by in Format::ALL {
            if produced_by == decompressed_by {
                continue;
            }

            if let Ok(plain) = decompressed_by.decompress(compressed.clone(), memory.clone()) {
                assert_ne!(
                    plain.to_vec(),
                    data,
                    "{decompressed_by:?} decompressed a {produced_by:?} stream as if it were its own"
                );
            }
        }
    }
}

#[test]
fn a_decompressor_can_be_chosen_from_a_declared_encoding() {
    // The end-to-end runtime scenario: a peer declares its encoding in a header, and the decompressor is
    // chosen from that string.
    let memory = GlobalPool::new();
    let data = b"declared encoding ".repeat(100);

    for &format in Format::ALL {
        let Some(token) = format.content_encoding() else {
            continue;
        };

        let compressed = format.compress(view(&data), memory.clone()).expect("compression succeeds");

        let declared = Format::from_content_encoding(token).expect("the token is supported");
        let plain = declared.decompress(compressed, memory.clone()).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), data, "{format:?} did not decompress via its declared token");
    }
}

/// Format-specific settings: how a format extends the shared builder without breaking the contract.
#[cfg(feature = "brotli")]
mod format_specific_settings {
    use compressors::brotli;
    use compressors::brotli::{Mode, Quality, WindowSize};

    use super::*;

    #[test]
    fn default_limits_accept_the_compressors_own_high_ratio_output() {
        let memory = GlobalPool::new();
        let data = vec![0_u8; 4 * 1024 * 1024];
        let compressed = brotli::compress(view(&data), memory.clone()).expect("compression succeeds");

        let plain = brotli::decompress(compressed, memory).expect("default limits accept valid brotli");

        assert_eq!(plain.to_vec(), data);
    }

    #[test]
    fn a_format_specific_setting_still_produces_a_conforming_stream() {
        // Whatever brotli-only knobs are set, the result must still satisfy the shared contract.
        let memory = GlobalPool::new();
        let data = b"format specific settings ".repeat(400);

        let mut tuned = brotli::Compressor::builder()
            .level(Level::HIGH)
            .quality(Quality::new(3).expect("quality is in range"))
            .mode(Mode::Text)
            .window_size(WindowSize::new(20).expect("20 is in range"))
            .build(memory.clone());

        let compressed = compress(&mut tuned, &view(&data), usize::MAX).expect("compression succeeds");
        let plain = brotli::decompress(compressed, memory).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), data);
    }

    #[test]
    fn window_size_rejects_values_outside_brotlis_range() {
        // Configuration input must report a mistake, not panic.
        assert_eq!(WindowSize::new(9), None);
        assert_eq!(WindowSize::new(25), None);
        assert_eq!(WindowSize::new(10), Some(WindowSize::MIN));
        assert_eq!(WindowSize::new(24), Some(WindowSize::MAX));
        assert_eq!(WindowSize::default(), WindowSize::DEFAULT);
    }

    #[test]
    fn a_smaller_window_still_round_trips() {
        let memory = GlobalPool::new();
        let data = b"windowed ".repeat(20_000);

        for exponent in [10, 16, 24] {
            let window = WindowSize::new(exponent).expect("exponent is in range");
            let mut tuned = brotli::Compressor::builder().window_size(window).build(memory.clone());

            let compressed = compress(&mut tuned, &view(&data), usize::MAX).expect("compression succeeds");
            let plain = brotli::decompress(compressed, memory.clone()).expect("decompression succeeds");

            assert_eq!(plain.to_vec(), data, "window 2^{exponent} did not round trip");
        }
    }

    #[test]
    fn a_runtime_chosen_format_can_still_reach_format_specific_settings() {
        // The documented escape hatch: a runtime `Format` builder cannot carry a brotli-only
        // setting, so branch on the format, use the concrete builder, and box the result. That
        // works because a boxed compression operation is itself a `Compression`.
        fn compressor_for(format: Format, memory: GlobalPool) -> Box<dyn Compressing> {
            match format {
                Format::Brotli => Box::new(brotli::Compressor::builder().mode(Mode::Text).build(memory)),
                other => other.compressor().build(memory),
            }
        }

        let memory = GlobalPool::new();
        let data = b"escape hatch ".repeat(200);

        for &format in Format::ALL {
            let mut tuned = compressor_for(format, memory.clone());
            let compressed = compress(&mut *tuned, &view(&data), usize::MAX).expect("compression succeeds");

            let plain = format.decompress(compressed, memory.clone()).expect("decompression succeeds");
            assert_eq!(plain.to_vec(), data, "{format:?} failed through the escape hatch");
        }
    }

    #[test]
    fn text_mode_does_not_change_the_decompressed_bytes() {
        // The mode is a compressor-side hint only: it must never alter what comes back out.
        let memory = GlobalPool::new();
        let data = b"the quick brown fox jumps over the lazy dog ".repeat(300);

        for mode in [Mode::Generic, Mode::Text, Mode::Font] {
            let mut tuned = brotli::Compressor::builder().mode(mode).build(memory.clone());
            let compressed = compress(&mut tuned, &view(&data), usize::MAX).expect("compression succeeds");

            let plain = brotli::decompress(compressed, memory.clone()).expect("decompression succeeds");
            assert_eq!(plain.to_vec(), data, "{mode:?} changed the decompressed bytes");
        }
    }
}

#[cfg(feature = "zstd")]
mod zstd_specific_settings {
    use compressors::zstd;
    use compressors::zstd::{CompressionLevel, WindowLog};

    use super::*;

    #[test]
    fn native_level_and_decompressor_window_limit_are_wired() {
        let memory = GlobalPool::new();
        let data = b"zstd format-specific settings ".repeat(400);
        let compressor = zstd::Compressor::builder()
            .compression_level(CompressionLevel::min())
            .build(memory.clone());
        let compressed = compressor.compress(view(&data)).expect("compression succeeds");

        let decompressor = zstd::Decompressor::builder().max_window_log(WindowLog::DEFAULT).build(memory);
        let plain = decompressor.decompress(compressed).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), data);
    }
}

/// Engine reuse must be invisible: a recycled compressor has to behave exactly like a fresh one.
#[cfg(feature = "gzip")]
mod pooling {
    use compressors::gzip;

    use super::*;

    fn compress_with(pool: Option<Pool>, level: Level, data: &[u8]) -> BytesView {
        let memory = GlobalPool::new();
        let builder = gzip::Compressor::builder().level(level);
        let builder = match pool {
            Some(pool) => builder.pool(pool),
            None => builder,
        };

        let mut compressor = builder.build(memory);
        compress(&mut compressor, &view(data), usize::MAX).expect("compression succeeds")
    }

    #[test]
    fn a_recycled_engine_produces_byte_identical_output() {
        // The whole safety argument for pooling: reset state must leave no trace of the previous
        // stream. Compare many pooled rounds against a fresh-engine baseline.
        let pool = Pool::new();
        let payloads = [
            b"first request body".repeat(50),
            b"a completely different second body, longer".repeat(80),
            b"third".repeat(500),
        ];

        for round in 0..4 {
            for payload in &payloads {
                let pooled = compress_with(Some(pool.clone()), Level::DEFAULT, payload);
                let fresh = compress_with(None, Level::DEFAULT, payload);

                assert_eq!(
                    pooled.to_vec(),
                    fresh.to_vec(),
                    "round {round}: pooled output diverged from a fresh engine"
                );
                assert_eq!(gzip::decompress(pooled, GlobalPool::new()).expect("decompress").to_vec(), *payload);
            }
        }
    }

    #[test]
    fn a_compressor_abandoned_mid_stream_does_not_poison_the_pool() {
        // A request cancelled part-way through returns a dirty engine. The next user must still
        // get a clean stream.
        let pool = Pool::new();

        {
            let mut abandoned = gzip::Compressor::builder().pool(pool.clone()).build(GlobalPool::new());
            abandoned.push(view(&b"half a stream ".repeat(100))).expect("push succeeds");
            let _ = Compression::pull(&mut abandoned).expect("pull succeeds");
            // Dropped without `end_input`, so its engine is mid-stream.
        }

        let recovered = compress_with(Some(pool), Level::DEFAULT, b"a fresh stream");
        let fresh = compress_with(None, Level::DEFAULT, b"a fresh stream");

        assert_eq!(recovered.to_vec(), fresh.to_vec(), "a recycled dirty engine must be reset");
        assert_eq!(
            gzip::decompress(recovered, GlobalPool::new()).expect("decompress").to_vec(),
            b"a fresh stream".to_vec()
        );
    }

    #[test]
    fn levels_do_not_share_engines() {
        // Reset preserves the level, so a level-9 request must never receive a level-1 engine.
        let pool = Pool::new();
        let payload = b"the quick brown fox jumps over the lazy dog ".repeat(200);

        let fast = compress_with(Some(pool.clone()), Level::FAST, &payload);
        let best = compress_with(Some(pool), Level::HIGH, &payload);

        assert_eq!(fast.to_vec(), compress_with(None, Level::FAST, &payload).to_vec());
        assert_eq!(best.to_vec(), compress_with(None, Level::HIGH, &payload).to_vec());
        assert!(best.len() <= fast.len(), "level 9 must still out-compress level 1");
    }

    #[test]
    fn a_pool_is_shared_across_threads() {
        // The point of the design: one handle lives in a client and is cloned per request.
        let pool = Pool::new();
        let payload = b"concurrent body ".repeat(200);

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let pool = pool.clone();
                let payload = payload.clone();
                scope.spawn(move || {
                    for _ in 0..10 {
                        let compressed = compress_with(Some(pool.clone()), Level::DEFAULT, &payload);
                        assert_eq!(
                            gzip::decompress(compressed, GlobalPool::new()).expect("decompress").to_vec(),
                            payload
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn a_pooled_decompressor_round_trips_every_format() {
        // Whether or not a format's engine is actually recycled is an implementation detail; the
        // decompressed bytes must be identical either way.
        let payloads = [b"first response body".repeat(60), b"a different second body".repeat(90)];

        for &format in Format::ALL {
            let pool = Pool::new();
            let memory = GlobalPool::new();

            for round in 0..4 {
                for payload in &payloads {
                    let compressed = format.compress(view(payload), memory.clone()).expect("compression succeeds");

                    let mut decompressor = format.decompressor().pool(pool.clone()).build(memory.clone());
                    let plain = decompress(&mut *decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                    assert_eq!(plain.to_vec(), *payload, "{format:?} round {round} diverged when pooled");
                }
            }
        }
    }

    #[cfg(feature = "zlib")]
    #[test]
    fn a_decompressor_abandoned_mid_stream_does_not_poison_the_pool() {
        use compressors::zlib;

        let pool = Pool::new();
        let memory = GlobalPool::new();
        let payload = b"a stream that gets cut short ".repeat(200);
        let compressed = zlib::compress(view(&payload), memory.clone()).expect("compression succeeds");

        {
            let mut abandoned = zlib::Decompressor::builder().pool(pool.clone()).build(memory.clone());
            abandoned.push(compressed.range(0..compressed.len() / 2)).expect("push succeeds");
            let _ = Compression::pull(&mut abandoned).expect("pull succeeds");
            // Dropped mid-stream, so its engine is dirty.
        }

        let mut recovered = zlib::Decompressor::builder().pool(pool).build(memory);
        let plain = decompress(&mut recovered, &compressed, usize::MAX).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), payload, "a recycled dirty decompressor must be reset");
    }

    #[test]
    fn gzip_decompressors_are_not_recycled() {
        // `Decompress::reset` takes a boolean that cannot express gzip framing, so a recycled gzip
        // decompressor would silently decompress as raw deflate. It must therefore never be pooled --
        // and the caller must not be able to tell the difference.
        let pool = Pool::new();
        let memory = GlobalPool::new();
        let payload = b"gzip stays correct ".repeat(200);
        let compressed = gzip::compress(view(&payload), memory.clone()).expect("compression succeeds");

        for round in 0..5 {
            let mut decompressor = gzip::Decompressor::builder().pool(pool.clone()).build(memory.clone());
            let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

            assert_eq!(plain.to_vec(), payload, "gzip round {round} decompressed incorrectly");
        }
    }

    #[test]
    fn a_zero_capacity_pool_still_works() {
        let pool = Pool::with_capacity(0);
        let payload = b"no recycling here".repeat(20);

        let compressed = compress_with(Some(pool), Level::DEFAULT, &payload);

        assert_eq!(
            gzip::decompress(compressed, GlobalPool::new()).expect("decompress").to_vec(),
            payload
        );
    }
}

/// The riskiest pooling bug: deflate, zlib and gzip share one engine type, so a mis-keyed pool
/// would hand a zlib compressor to a gzip request and emit a well-formed stream in the wrong
/// format. Nothing else in the suite would catch that.
#[test]
fn formats_never_share_pooled_engines() {
    let pool = Pool::new();
    let data = b"interleaved through one pool ".repeat(200);
    let input = view(&data);

    let baselines: Vec<_> = Format::ALL
        .iter()
        .map(|&format| {
            let mut compressor = format.compressor().output_chunk_size(chunk(4096)).build(GlobalPool::new());
            let bytes = compress(&mut *compressor, &input, usize::MAX)
                .expect("compression succeeds")
                .to_vec();
            (format, bytes)
        })
        .collect();

    // Interleave, so every format has had a turn before any is asked again.
    for round in 0..6 {
        for (format, baseline) in &baselines {
            let mut compressor = format
                .compressor()
                .output_chunk_size(chunk(4096))
                .pool(pool.clone())
                .build(GlobalPool::new());
            let pooled = compress(&mut *compressor, &input, usize::MAX).expect("compression succeeds");
            drop(compressor);

            assert_eq!(
                &pooled.to_vec(),
                baseline,
                "{format:?} round {round}: interleaving formats through one pool changed the output"
            );

            // And the bytes really are this format's, not a sibling's that happens to decompress.
            for (other, _) in &baselines {
                let mut reader = other.decompressor().pool(pool.clone()).build(GlobalPool::new());
                let decompressed = decompress(&mut *reader, &pooled, usize::MAX);

                if other == format {
                    assert_eq!(
                        decompressed.expect("its own decompressor must accept it").to_vec(),
                        data,
                        "{format:?} round {round}: own decompressor failed"
                    );
                } else if let Ok(plain) = decompressed {
                    assert_ne!(plain.to_vec(), data, "{other:?} decompressed a {format:?} stream as its own");
                }
            }
        }
    }
}

/// One pool shared by many threads, the way a client would actually use it.
#[test]
fn a_shared_pool_is_correct_under_concurrency() {
    let pool = Pool::new();
    let data = b"concurrent request body ".repeat(150);

    let baselines: Vec<_> = Format::ALL
        .iter()
        .map(|&format| {
            let input = view(&data);
            let mut compressor = format.compressor().output_chunk_size(chunk(4096)).build(GlobalPool::new());
            let bytes = compress(&mut *compressor, &input, usize::MAX)
                .expect("compression succeeds")
                .to_vec();
            (format, bytes)
        })
        .collect();

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let pool = pool.clone();
            let data = data.clone();
            let baselines = baselines.clone();

            scope.spawn(move || {
                // Each thread builds its own view, so segmentation is stable within the thread.
                let input = view(&data);

                for round in 0..10 {
                    for (format, baseline) in &baselines {
                        let mut compressor = format
                            .compressor()
                            .output_chunk_size(chunk(4096))
                            .pool(pool.clone())
                            .build(GlobalPool::new());
                        let pooled = compress(&mut *compressor, &input, usize::MAX).expect("compression succeeds");
                        drop(compressor);

                        assert_eq!(&pooled.to_vec(), baseline, "{format:?} round {round}: concurrent pooling diverged");

                        let mut decompressor = format.decompressor().pool(pool.clone()).build(GlobalPool::new());
                        let plain = decompress(&mut *decompressor, &pooled, usize::MAX).expect("decompression succeeds");

                        assert_eq!(plain.to_vec(), data, "{format:?} round {round}: concurrent decompress lost data");
                    }
                }
            });
        }
    });
}

/// A long run must not drift: the hundredth message has to match the first.
#[test]
fn pooled_output_does_not_drift_over_many_reuses() {
    let pool = Pool::new();
    let data = b"steady state ".repeat(120);

    for &format in Format::ALL {
        let input = view(&data);
        let mut first: Option<Vec<u8>> = None;

        for round in 0..60 {
            let mut compressor = format
                .compressor()
                .output_chunk_size(chunk(4096))
                .pool(pool.clone())
                .build(GlobalPool::new());
            let pooled = compress(&mut *compressor, &input, usize::MAX)
                .expect("compression succeeds")
                .to_vec();
            drop(compressor);

            match first {
                None => first = Some(pooled),
                Some(ref expected) => assert_eq!(&pooled, expected, "{format:?} round {round}: output drifted"),
            }
        }
    }
}
