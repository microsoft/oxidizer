// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One contract, applied to every format.
//!
//! These tests exist to keep the abstraction honest. Every format goes through the same scenarios,
//! so a format that behaves differently from its siblings -- or an abstraction that quietly only
//! fits the deflate family -- fails here rather than surprising a consumer.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::OnceLock;

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};

use crate::core::{Compress, Compression, CompressionInternal, Decompress, Output};
use crate::{CompressorBuilder, DecompressorBuilder, DecompressorLimits, Format, Level, Resources, TrailingData};

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

/// The resources every test here draws on: one memory provider, one set of recycled engines.
///
/// Shared for the whole file on purpose. Recycling must be invisible, so tests that would fail if
/// an engine came back dirty are exactly the tests that should be sharing one.
fn resources() -> &'static Resources {
    static RESOURCES: OnceLock<Resources> = OnceLock::new();

    RESOURCES.get_or_init(|| Resources::new(GlobalPool::new()))
}

/// Erases the difference between a build that can fail and one that cannot.
///
/// Brotli and zstd validate their configuration as they apply it, so their builders return a
/// [`Result`]; the deflate family's cannot fail and return the codec directly. The contract below
/// is the same either way, so it goes through this to stay one test.
trait Built {
    type Codec;

    fn built(self) -> Self::Codec;
}

impl<T> Built for Result<T, crate::BuildError> {
    type Codec = T;

    fn built(self) -> T {
        self.expect("the engine accepts the configuration under test")
    }
}

/// Drives any compression operation to completion, feeding the input in `feed` sized pieces.
/// Caps every drain loop in this file.
///
/// A conforming operation always terminates, so exceeding this means the code under test is
/// spinning. A hanging test reports nothing at all, so the cap turns a hang into a failure --
/// which also lets mutation testing reach a verdict instead of timing out.
const MAX_STEPS: usize = 1_000_000;

/// Fails a spinning test instead of letting it hang.
///
/// A conforming operation always terminates, so exceeding the cap means the code under test is
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

fn process<D>(compression: &mut dyn Compression<Mode = D>, input: &BytesView, feed: usize) -> crate::Result<BytesView> {
    let mut offset = 0;
    let mut collected = BytesBuf::new();

    let mut guard = StepGuard::new();
    loop {
        guard.step();
        match compression.pull()? {
            Output::Data(data) => collected.put_bytes(data),
            Output::Progress => {}
            Output::Done => return Ok(collected.consume_all()),
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
}

fn compress(compressor: &mut dyn Compression<Mode = Compress>, input: &BytesView, feed: usize) -> crate::Result<BytesView> {
    process(compressor, input, feed)
}

fn decompress(decompressor: &mut dyn Compression<Mode = Decompress>, input: &BytesView, feed: usize) -> crate::Result<BytesView> {
    process(decompressor, input, feed)
}

/// Generates the shared contract for one format, using its concrete module so the builders are
/// exercised too, not just the runtime `Format` factories.
macro_rules! format_contract {
    ($module:ident, $format:expr) => {
        mod $module {
            use super::*;
            use crate::$module;

            const FORMAT: Format = $format;

            impl Built for $module::Compressor {
                type Codec = Self;

                fn built(self) -> Self {
                    self
                }
            }

            impl Built for $module::Decompressor {
                type Codec = Self;

                fn built(self) -> Self {
                    self
                }
            }

            fn payload() -> Vec<u8> {
                b"the quick brown fox jumps over the lazy dog; pack my box with five dozen liquor jugs. ".repeat(300)
            }

            #[test]
            fn round_trips_a_payload() {
                let data = payload();

                let compressed = $module::compress(view(&data), resources()).expect("compression succeeds");
                assert!(compressed.len() < data.len(), "the payload should compress");

                let plain = $module::decompress(compressed, resources()).expect("decompression succeeds");
                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn compress_matches_driving_the_operation_by_hand() {
                // The convenience must be exactly the manual loop, not an approximation of it.
                let data = payload();

                let convenient = crate::compress(view(&data), $module::Compressor::new(resources())).expect("compression succeeds");

                let mut by_hand = $module::Compressor::new(resources());
                by_hand.push(view(&data)).expect("push succeeds");
                CompressionInternal::end_input(&mut by_hand);
                let mut collected = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match CompressionInternal::pull(&mut by_hand).expect("pull succeeds") {
                        Output::Data(chunk) => collected.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("compressor requested input after end"),
                        Output::Done => break,
                    }
                }

                assert_eq!(convenient.to_vec(), collected.consume_all().to_vec());

                let plain = crate::decompress(convenient, $module::Decompressor::new(resources())).expect("decompression succeeds");

                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn compress_and_decompress_work_through_a_trait_object() {
                // Provided methods are easy to break for `dyn`, so reach them that way too.
                let data = payload();

                let compressor: Box<dyn Compression<Mode = Compress>> = Box::new($module::Compressor::new(resources()));
                let compressed = crate::compress(view(&data), compressor).expect("compression succeeds");

                let decompressor: Box<dyn Compression<Mode = Decompress>> = Box::new($module::Decompressor::new(resources()));

                assert_eq!(
                    crate::decompress(compressed, decompressor)
                        .expect("decompression succeeds")
                        .to_vec(),
                    data
                );
            }

            #[test]
            fn round_trips_empty_input() {
                let compressed = $module::compress(BytesView::new(), resources()).expect("compression succeeds");
                let plain = $module::decompress(compressed, resources()).expect("decompression succeeds");

                assert!(plain.is_empty());
            }

            #[test]
            fn round_trips_a_multi_segment_view() {
                // The reason this crate exists: input arrives as a chain of spans, never as one
                // contiguous slice.
                for (segment, repeats) in [(1_usize, 40_usize), (7, 200), (1024, 2_000)] {
                    let data = b"multi segment ".repeat(repeats);

                    let compressed = $module::compress(fragmented(&data, segment), resources()).expect("compression succeeds");
                    let plain = $module::decompress(compressed, resources()).expect("decompression succeeds");

                    assert_eq!(plain.to_vec(), data, "failed at {segment} byte segments");
                }
            }

            #[test]
            fn round_trips_when_driven_one_byte_at_a_time() {
                // Worst case for a push/pull codec: minimal input pieces and minimal output chunks.
                let data = b"drip fed".repeat(20);

                let mut compressor = $module::Compressor::builder()
                    .output_chunk_size(chunk(1))
                    .build(resources())
                    .built();
                let compressed = compress(&mut compressor, &view(&data), 1).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .output_chunk_size(chunk(1))
                    .build(resources())
                    .built();
                let plain = decompress(&mut decompressor, &compressed, 1).expect("decompression succeeds");

                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn honours_the_output_chunk_size() {
                let data = payload();

                let mut compressor = $module::Compressor::builder()
                    .output_chunk_size(chunk(256))
                    .build(resources())
                    .built();
                compressor.push(view(&data)).expect("push succeeds");
                CompressionInternal::end_input(&mut compressor);

                let mut compressed = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match CompressionInternal::pull(&mut compressor).expect("pull succeeds") {
                        Output::Data(piece) => {
                            assert!(piece.len() <= 256, "chunk of {} bytes exceeded the bound", piece.len());
                            compressed.put_bytes(piece);
                        }
                        Output::Progress => {}
                        Output::NeedInput => panic!("compressor requested input after end"),
                        Output::Done => break,
                    }
                }

                let mut decompressor = $module::Decompressor::builder()
                    .output_chunk_size(chunk(256))
                    .build(resources())
                    .built();
                decompressor.push(compressed.consume_all()).expect("push succeeds");
                CompressionInternal::end_input(&mut decompressor);

                let mut plain = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match CompressionInternal::pull(&mut decompressor).expect("pull succeeds") {
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

                    let mut compressor = $module::Compressor::builder().level(level).build(resources()).built();
                    let compressed = compress(&mut compressor, &view(&data), usize::MAX).expect("compression succeeds");

                    let plain = $module::decompress(compressed, resources()).expect("decompression succeeds");
                    assert_eq!(plain.to_vec(), data, "level {raw} did not round trip");
                }
            }

            #[test]
            fn tracks_byte_counts() {
                let data = payload();

                let mut compressor = $module::Compressor::new(resources());
                let compressed = compress(&mut compressor, &view(&data), usize::MAX).expect("compression succeeds");

                assert_eq!(compressor.total_in(), data.len() as u64);
                assert_eq!(compressor.total_out(), compressed.len() as u64);

                let mut decompressor = $module::Decompressor::new(resources());
                let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                assert_eq!(decompressor.total_in(), compressed.len() as u64);
                assert_eq!(decompressor.total_out(), plain.len() as u64);
            }

            #[test]
            fn rejects_a_truncated_stream() {
                let compressed = $module::compress(view(&payload()), resources()).expect("compression succeeds");

                for cut in [1, compressed.len() / 3, compressed.len() - 1] {
                    let error = $module::decompress(compressed.range(0..cut), resources())
                        .expect_err("a truncated stream must not decompress successfully");

                    assert!(
                        error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
                        "truncating at {cut} gave an unexpected classification: {error}"
                    );
                }
            }

            #[test]
            fn rejects_input_after_end_input() {
                let mut compressor = $module::Compressor::new(resources());
                CompressionInternal::end_input(&mut compressor);

                let error = compressor.push(view(b"late")).expect_err("push after end_input is rejected");
                assert!(error.is_invalid_state());

                let mut decompressor = $module::Decompressor::new(resources());
                CompressionInternal::end_input(&mut decompressor);

                let error = decompressor
                    .push(view(b"late"))
                    .expect_err("push after end_input is rejected");
                assert!(error.is_invalid_state());
            }

            #[test]
            fn asks_for_more_input_before_end_input() {
                let mut compressor = $module::Compressor::new(resources());
                compressor.push(view(b"partial")).expect("push succeeds");

                let mut guard = StepGuard::new();
                let output = loop {
                    guard.step();
                    match CompressionInternal::pull(&mut compressor).expect("pull succeeds") {
                        Output::Data(_) | Output::Progress => {}
                        other => break other,
                    }
                };

                assert!(output.is_need_input(), "an unfinished compressor must ask for more input");
            }

            #[test]
            fn enforces_a_configured_expansion_limit() {
                // A ratio the data is guaranteed to exceed, so the mechanism itself is tested
                // rather than whichever default the format happens to carry. A quarter megabyte of
                // zeros clears the guard's 32 KiB floor several times over while staying cheap.
                let bomb = $module::compress(view(&vec![0_u8; 256 * 1024]), resources()).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .limits(DecompressorLimits::new().with_max_ratio(NonZeroU32::new(4).expect("4 is not zero")))
                    .build(resources())
                    .built();
                decompressor.push(bomb).expect("push succeeds");
                CompressionInternal::end_input(&mut decompressor);

                let mut guard = StepGuard::new();
                let error = loop {
                    guard.step();
                    match CompressionInternal::pull(&mut decompressor) {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("the bomb decompressed fully instead of being rejected"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_limit_exceeded(), "got {error}");
                assert!(
                    decompressor.total_out() < 256 * 1024,
                    "the guard should fire before the full expansion"
                );
            }

            #[test]
            fn default_limits_accept_ordinary_highly_compressible_data() {
                // Regression guard. A single portable ratio limit was calibrated on deflate, whose
                // structural ceiling is about `1032x`. Brotli legitimately reaches tens of thousands of
                // times expansion, so that limit rejected ordinary repetitive input -- a repeated
                // sentence, and JSON. Each format now carries its own default.

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
                    let compressed = $module::compress(view(&data), resources()).expect("compression succeeds");
                    let ratio = data.len() / compressed.len().max(1);

                    let plain = $module::decompress(compressed, resources())
                        .unwrap_or_else(|error| panic!("default limits rejected {label} at {ratio}x expansion: {error}"));

                    assert_eq!(plain.to_vec(), data, "{label} did not round trip");
                }
            }

            #[test]
            fn an_absolute_cap_is_enforced() {
                let compressed = $module::compress(view(&vec![0_u8; 256 * 1024]), resources()).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .limits(DecompressorLimits::new().without_max_ratio().with_max_output_len(1024))
                    .build(resources())
                    .built();
                decompressor.push(compressed).expect("push succeeds");
                CompressionInternal::end_input(&mut decompressor);

                let mut guard = StepGuard::new();
                let error = loop {
                    guard.step();
                    match CompressionInternal::pull(&mut decompressor) {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("the cap should have fired"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_limit_exceeded(), "got {error}");
            }

            #[test]
            fn trusted_callers_can_opt_out_of_the_limits() {
                let data = vec![0_u8; 256 * 1024];
                let compressed = $module::compress(view(&data), resources()).expect("compression succeeds");

                let mut decompressor = $module::Decompressor::builder()
                    .limits(DecompressorLimits::UNLIMITED)
                    .build(resources())
                    .built();
                let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                assert_eq!(plain.len(), data.len());
            }

            #[test]
            fn corruption_is_detected_or_changes_the_output() {
                // Formats with a checksum report corruption; raw deflate has none, so the honest
                // universal guarantee is only that corrupt input does not silently reproduce the
                // original bytes.
                let data = payload();
                let compressed = $module::compress(view(&data), resources()).expect("compression succeeds");
                let original = compressed.to_vec();

                for index in [0, original.len() / 2, original.len() - 1] {
                    let mut corrupted = original.clone();
                    corrupted[index] ^= 0xff;

                    match $module::decompress(view(&corrupted), resources()) {
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
                let data = payload();

                let via_module = $module::compress(view(&data), resources()).expect("compression succeeds");
                let via_format = FORMAT.compress(view(&data), resources()).expect("compression succeeds");

                assert_eq!(
                    via_module.to_vec(),
                    via_format.to_vec(),
                    "runtime and compile-time selection diverged"
                );

                // Either output must decompress through either path.
                assert_eq!(
                    FORMAT
                        .decompress(via_module, resources())
                        .expect("decompression succeeds")
                        .to_vec(),
                    data
                );
                assert_eq!(
                    $module::decompress(via_format, resources())
                        .expect("decompression succeeds")
                        .to_vec(),
                    data
                );
            }

            #[test]
            fn works_through_boxed_trait_objects() {
                let data = payload();

                let mut compressor = CompressorBuilder::new().build_format(FORMAT, resources()).built();
                let compressed = compress(&mut *compressor, &view(&data), usize::MAX).expect("compression succeeds");

                let mut decompressor = DecompressorBuilder::new().build_format(FORMAT, resources()).built();
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
                let input = view(&payload());
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources())
                        .built();
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                // Several rounds: the first compressor always misses the pool, so only later rounds
                // exercise a recycled engine.
                for round in 0..5 {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources())
                        .built();
                    let pooled = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");
                    drop(compressor);

                    assert_eq!(pooled.to_vec(), baseline.to_vec(), "round {round}: pooled output diverged");

                    let mut decompressor = $module::Decompressor::builder().build(resources()).built();
                    let plain = decompress(&mut decompressor, &pooled, usize::MAX).expect("decompression succeeds");

                    assert_eq!(plain.to_vec(), payload(), "round {round}: pooled decompressor lost data");
                }
            }

            #[test]
            fn an_engine_abandoned_mid_stream_is_cleaned_before_reuse() {
                // A request cancelled part-way through returns a half-used engine.
                let input = view(&payload());
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources())
                        .built();
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                for round in 0..4 {
                    {
                        let mut abandoned = $module::Compressor::builder()
                            .output_chunk_size(chunk(4096))
                            .build(resources())
                            .built();
                        abandoned.push(input.clone()).expect("push succeeds");
                        let _ = CompressionInternal::pull(&mut abandoned).expect("pull succeeds");
                        // Dropped without finishing, so its engine is mid-frame.
                    }

                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources())
                        .built();
                    let recovered = compress(&mut compressor, &input, usize::MAX).expect("compression succeeds");

                    assert_eq!(recovered.to_vec(), baseline.to_vec(), "round {round}: a dirty engine leaked");
                }
            }

            #[test]
            fn an_engine_left_dirty_by_a_failed_decompression_is_cleaned_before_reuse() {
                let compressed = $module::compress(view(&payload()), resources()).expect("compression succeeds");
                let garbage = view(&b"definitely not a valid stream".repeat(20));

                for round in 0..4 {
                    {
                        let mut failing = $module::Decompressor::builder().build(resources()).built();
                        let _ = decompress(&mut failing, &garbage, usize::MAX);
                    }

                    let mut decompressor = $module::Decompressor::builder().build(resources()).built();
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
                let input = view(&payload());
                let levels = [Level::MIN, Level::FAST, Level::DEFAULT, Level::HIGH];

                let baselines: Vec<_> = levels
                    .iter()
                    .map(|&level| {
                        let mut compressor = $module::Compressor::builder()
                            .level(level)
                            .output_chunk_size(chunk(4096))
                            .build(resources())
                            .built();
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
                            .build(resources())
                            .built();
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
                    CompressionInternal::end_input(compressor);

                    let mut collected = BytesBuf::new();
                    let mut guard = StepGuard::new();
                    loop {
                        guard.step();
                        match CompressionInternal::pull(compressor).expect("pull succeeds") {
                            Output::Data(chunk) => collected.put_bytes(chunk),
                            Output::Progress => {}
                            Output::NeedInput => panic!("compressor requested input after end"),
                            Output::Done => break,
                        }
                    }

                    collected.consume_all().to_vec()
                }

                fn build(resources: &Resources) -> $module::Compressor {
                    $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources)
                        .built()
                }

                let shared = Resources::new(GlobalPool::new());
                let input = view(&payload());
                let baseline = run(&mut build(&Resources::new(GlobalPool::new()).enable_pooling(0)), &input);

                // Prime the pool so there is exactly one idle engine for two codecs to want.
                drop(run(&mut build(&shared), &input));

                let mut first = build(&shared);
                let mut second = build(&shared);

                // Interleave: both are live before either finishes, so they cannot be sharing.
                first.push(input.clone()).expect("push succeeds");
                second.push(input.clone()).expect("push succeeds");
                CompressionInternal::end_input(&mut first);
                CompressionInternal::end_input(&mut second);

                for (label, compressor) in [("first", &mut first), ("second", &mut second)] {
                    let mut collected = BytesBuf::new();
                    let mut guard = StepGuard::new();
                    loop {
                        guard.step();
                        match CompressionInternal::pull(compressor).expect("pull succeeds") {
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
                        .build(resources())
                        .built();
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                let mut compressor = {
                    let owned = Resources::new(GlobalPool::new());
                    let compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(&owned)
                        .built();
                    drop(owned);
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
                        .build(resources())
                        .built();
                    compress(&mut compressor, &input, usize::MAX).expect("compression succeeds")
                };

                for capacity in [0_usize, 1, 4] {
                    let bounded = Resources::new(GlobalPool::new()).enable_pooling(capacity);

                    for round in 0..12 {
                        let mut compressor = $module::Compressor::builder()
                            .output_chunk_size(chunk(4096))
                            .build(&bounded)
                            .built();
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
                let baseline = {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources())
                        .built();
                    compress(&mut compressor, &BytesView::new(), usize::MAX).expect("compression succeeds")
                };

                for round in 0..4 {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(4096))
                        .build(resources())
                        .built();
                    let pooled = compress(&mut compressor, &BytesView::new(), usize::MAX).expect("compression succeeds");
                    drop(compressor);

                    assert_eq!(pooled.to_vec(), baseline.to_vec(), "round {round}: empty framing changed");

                    let mut decompressor = $module::Decompressor::builder().build(resources()).built();
                    let plain = decompress(&mut decompressor, &pooled, usize::MAX).expect("decompression succeeds");

                    assert!(plain.is_empty(), "round {round}: empty input produced bytes");
                }
            }

            #[test]
            fn truncation_is_still_detected_when_pooled() {
                let compressed = $module::compress(view(&payload()), resources()).expect("compression succeeds");

                for round in 0..3 {
                    // A healthy decompress first, so the next decompressor is guaranteed to be recycled.
                    let mut healthy = $module::Decompressor::builder().build(resources()).built();
                    decompress(&mut healthy, &compressed, usize::MAX).expect("the full stream decompresses");
                    drop(healthy);

                    let mut decompressor = $module::Decompressor::builder().build(resources()).built();
                    let error = decompress(&mut decompressor, &compressed.range(0..compressed.len() - 1), usize::MAX)
                        .expect_err("a truncated stream must not decompress successfully");

                    assert!(
                        error.is_unexpected_end_of_stream() || error.is_corrupt_data(),
                        "round {round}: unexpected classification {error}"
                    );
                }
            }

            #[test]
            fn flushing_a_decompressor_does_nothing() {
                // Decompression produces output as soon as the input allows, so there is nothing
                // buffered to release early. The default must be a no-op, not an error and not an
                // end of stream.
                let data = payload();
                let compressed = $module::compress(view(&data), resources()).expect("compress");
                let mut decompressor = $module::Decompressor::new(resources());

                decompressor.flush().expect("flushing a decompressor is a no-op");

                let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                assert_eq!(plain.to_vec(), data, "the flush must leave the stream untouched");
            }

            #[test]
            fn a_flush_makes_supplied_input_decompressible_without_ending_the_stream() {
                let data = b"flush this data now ".repeat(20_000);
                let mut compressor = $module::Compressor::new(resources());
                compressor.push(view(&data)).expect("push succeeds");
                compressor.flush().expect("flush request succeeds");

                let mut compressed = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match compressor.pull().expect("pull succeeds") {
                        Output::Data(chunk) => compressed.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("a flush must not end the stream"),
                    }
                }

                let mut decompressor = $module::Decompressor::new(resources());
                decompressor.push(compressed.consume_all()).expect("push succeeds");

                let mut plain = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
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
                let data = b"flush and finish ".repeat(200);
                let mut compressor = $module::Compressor::new(resources());
                compressor.push(view(&data)).expect("push succeeds");
                compressor.flush().expect("flush request succeeds");
                compressor.end_input();
                let error = compressor
                    .flush()
                    .expect_err("a flush queued behind end_input cannot be requested again");
                assert!(error.is_invalid_state(), "got {error}");

                let mut compressed = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match compressor.pull().expect("pull succeeds") {
                        Output::Data(chunk) => compressed.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("end of input is already queued"),
                        Output::Done => break,
                    }
                }

                let plain = $module::decompress(compressed.consume_all(), resources()).expect("decompression succeeds");
                assert_eq!(plain.to_vec(), data);
            }

            #[test]
            fn flush_terminates_with_tiny_output_chunks() {
                let data = b"tiny flush chunks ".repeat(100);

                for size in 1..=7 {
                    let mut compressor = $module::Compressor::builder()
                        .output_chunk_size(chunk(size))
                        .build(resources())
                        .built();
                    compressor.push(view(&data)).expect("push succeeds");
                    compressor.flush().expect("flush request succeeds");

                    let mut compressed = BytesBuf::new();
                    let mut pulls = 0;
                    let mut guard = StepGuard::new();
                    loop {
                        guard.step();
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
                    let mut guard = StepGuard::new();
                    loop {
                        guard.step();
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

                    let plain = $module::decompress(compressed.consume_all(), resources())
                        .unwrap_or_else(|error| panic!("chunk size {size} did not round trip: {error}"));
                    assert_eq!(plain.to_vec(), data);
                }
            }

            #[test]
            fn multi_stream_decompression_crosses_push_boundaries() {
                let first_plain = b"first stream ".repeat(40);
                let second_plain = b"second stream ".repeat(40);
                let first = $module::compress(view(&first_plain), resources()).expect("compress");
                let second = $module::compress(view(&second_plain), resources()).expect("compress");
                let mut decompressor = $module::Decompressor::builder().multi_stream(true).build(resources()).built();
                let mut plain = BytesBuf::new();

                decompressor.push(first).expect("first push succeeds");
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match decompressor.pull().expect("first stream decompresses") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => break,
                        Output::Done => panic!("decompressor ended before the next pushed stream"),
                    }
                }

                decompressor.push(second).expect("second push succeeds");
                decompressor.end_input();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
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
            fn single_stream_decompression_stops_at_the_end_of_its_stream() {
                let data = payload();
                let compressed = $module::compress(view(&data), resources()).expect("compress");
                let trailing = view(b"next protocol message");
                let joined = BytesView::from_views([compressed, trailing]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(false)
                    .build(resources())
                    .built();
                decompressor.push(joined).expect("push succeeds");

                let mut plain = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
                    match decompressor.pull().expect("decompression succeeds") {
                        Output::Data(chunk) => plain.put_bytes(chunk),
                        Output::Progress => {}
                        Output::NeedInput => panic!("single stream was complete"),
                        Output::Done => break,
                    }
                }

                // The bytes after the stream are not decompressed, and not mistaken for more of it.
                assert_eq!(plain.consume_all().to_vec(), data);
            }

            #[test]
            fn an_empty_push_does_not_create_a_phantom_stream() {
                let data = b"one member only".repeat(20);
                let compressed = $module::compress(view(&data), resources()).expect("compress");
                let mut decompressor = $module::Decompressor::builder().multi_stream(true).build(resources()).built();
                decompressor.push(compressed).expect("first push succeeds");

                let mut plain = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
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
                let first_plain = b"AAAAAAAAAA";
                let second_plain = b"BBBBBBBBBB";
                let first = $module::compress(view(first_plain), resources()).expect("compress");
                let second = $module::compress(view(second_plain), resources()).expect("compress");
                let split = first.len().saturating_sub(1);
                let joined = BytesView::from_views([first.range(0..split), first.range(split..), second]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(true)
                    .output_chunk_size(chunk(first_plain.len()))
                    .build(resources())
                    .built();
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let mut plain = BytesBuf::new();
                let mut guard = StepGuard::new();
                loop {
                    guard.step();
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
                let compressed = $module::compress(view(&payload()), resources()).expect("compress");
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(false)
                    .trailing_data(TrailingData::Reject)
                    .build(resources())
                    .built();
                decompressor.push(compressed).expect("push succeeds");

                let mut guard = StepGuard::new();
                loop {
                    guard.step();
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
                let compressed = $module::compress(view(&payload()), resources()).expect("compress");
                let joined = BytesView::from_views([compressed, view(b"trailing")]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(false)
                    .trailing_data(TrailingData::Reject)
                    .build(resources())
                    .built();
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let mut guard = StepGuard::new();
                let error = loop {
                    guard.step();
                    match decompressor.pull() {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("trailing input unexpectedly completed"),
                        Err(error) => break error,
                    }
                };
                assert!(error.is_corrupt_data(), "got {error}");
            }

            #[test]
            fn a_truncated_later_member_reads_as_a_short_stream() {
                // A caller retrying a partial transfer needs to tell "it stopped early" from "these
                // bytes are wrong". A member that starts and then runs out is the former, however
                // many members decoded cleanly before it.
                let compressed = $module::compress(view(&payload()), resources()).expect("compress");
                let whole = compressed.to_vec();
                let truncated = &whole[..whole.len() - 1];
                let joined = BytesView::from_views([compressed, view(truncated)]);

                let mut decompressor = $module::Decompressor::builder().multi_stream(true).build(resources()).built();
                decompressor.push(joined).expect("push succeeds");
                CompressionInternal::end_input(&mut decompressor);

                let mut guard = StepGuard::new();
                let error = loop {
                    guard.step();
                    match CompressionInternal::pull(&mut decompressor) {
                        Ok(Output::Data(_) | Output::Progress) => {}
                        Ok(_) => panic!("a truncated member unexpectedly completed"),
                        Err(error) => break error,
                    }
                };

                assert!(error.is_unexpected_end_of_stream(), "got {error}");
            }

            #[test]
            fn stream_count_limit_rejects_before_decompressing_the_next_stream() {
                let data = payload();
                let compressed = $module::compress(view(&data), resources()).expect("compress");
                let joined = BytesView::from_views([compressed.clone(), compressed]);
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(true)
                    .limits(DecompressorLimits::new().with_max_streams(NonZeroU64::new(1).expect("one is non-zero")))
                    .build(resources())
                    .built();
                decompressor.push(joined).expect("push succeeds");
                decompressor.end_input();

                let mut guard = StepGuard::new();
                let error = loop {
                    guard.step();
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
                let compressed = $module::compress(view(&payload()), resources()).expect("compress");
                let mut decompressor = $module::Decompressor::builder()
                    .multi_stream(true)
                    .limits(DecompressorLimits::new().with_max_streams(NonZeroU64::new(1).expect("one is non-zero")))
                    .build(resources())
                    .built();
                decompressor.push(compressed.clone()).expect("first push succeeds");

                let mut guard = StepGuard::new();
                loop {
                    guard.step();
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
                let data = payload();
                let compressed = $module::compress(view(&data), resources()).expect("compress");

                let exact = $module::decompress_with_limits(
                    compressed.clone(),
                    resources(),
                    DecompressorLimits::new()
                        .without_max_ratio()
                        .with_max_output_len(data.len() as u64),
                )
                .expect("an exact limit succeeds");
                assert_eq!(exact.to_vec(), data);

                let maximum = data.len() as u64 - 1;
                let error = $module::decompress_with_limits(
                    compressed,
                    resources(),
                    DecompressorLimits::new().without_max_ratio().with_max_output_len(maximum),
                )
                .expect_err("one byte beyond the cap is rejected");

                assert!(error.is_limit_exceeded(), "got {error}");
                assert!(error.to_string().contains(&(maximum + 1).to_string()), "got {error}");
            }

            #[test]
            fn a_fatal_error_makes_the_decompressor_terminal() {
                let mut decompressor = $module::Decompressor::new(resources());
                decompressor.push(view(b"not a valid stream")).expect("push succeeds");
                decompressor.end_input();

                let mut guard = StepGuard::new();
                let first = loop {
                    guard.step();
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
                let data = payload();

                assert_eq!(
                    transcode(
                        $module::Compressor::new(resources()),
                        $module::Decompressor::new(resources()),
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
    let data = b"cross format check ".repeat(200);

    for &produced_by in Format::ALL {
        let compressed = produced_by.compress(view(&data), resources()).expect("compression succeeds");

        for &decompressed_by in Format::ALL {
            if produced_by == decompressed_by {
                continue;
            }

            if let Ok(plain) = decompressed_by.decompress(compressed.clone(), resources()) {
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
    let data = b"declared encoding ".repeat(100);

    for &format in Format::ALL {
        let Some(token) = format.content_encoding() else {
            continue;
        };

        let compressed = format.compress(view(&data), resources()).expect("compression succeeds");

        let declared = Format::from_content_encoding(token).expect("the token is supported");
        let plain = declared.decompress(compressed, resources()).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), data, "{format:?} did not decompress via its declared token");
    }
}

/// Format-specific settings: how a format extends the shared builder without breaking the contract.
#[cfg(feature = "brotli")]
mod format_specific_settings {
    use super::*;
    use crate::brotli;
    use crate::brotli::{Mode, Quality, WindowSize};

    #[test]
    fn default_limits_accept_the_compressors_own_high_ratio_output() {
        // Half a megabyte of zeros clears the ratio guard's 32 KiB floor and still reaches an
        // expansion far past deflate's structural `1032x`, which is the ratio a portable default
        // would have been calibrated on. Brotli's own default must accept it.
        let data = vec![0_u8; 512 * 1024];
        let compressed = brotli::compress(view(&data), resources()).expect("compression succeeds");

        let plain = brotli::decompress(compressed, resources()).expect("default limits accept valid brotli");

        assert_eq!(plain.to_vec(), data);
    }

    #[test]
    fn a_format_specific_setting_still_produces_a_conforming_stream() {
        // Whatever brotli-only knobs are set, the result must still satisfy the shared contract.
        let data = b"format specific settings ".repeat(400);

        let mut tuned = brotli::Compressor::builder()
            .level(Level::HIGH)
            .quality(Quality::new(3).expect("quality is in range"))
            .mode(Mode::Text)
            .window_size(WindowSize::new(20).expect("20 is in range"))
            .build(resources())
            .built();

        let compressed = compress(&mut tuned, &view(&data), usize::MAX).expect("compression succeeds");
        let plain = brotli::decompress(compressed, resources()).expect("decompression succeeds");

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
        let data = b"windowed ".repeat(20_000);

        for exponent in [10, 16, 24] {
            let window = WindowSize::new(exponent).expect("exponent is in range");
            let mut tuned = brotli::Compressor::builder().window_size(window).build(resources()).built();

            let compressed = compress(&mut tuned, &view(&data), usize::MAX).expect("compression succeeds");
            let plain = brotli::decompress(compressed, resources()).expect("decompression succeeds");

            assert_eq!(plain.to_vec(), data, "window 2^{exponent} did not round trip");
        }
    }

    #[test]
    fn a_runtime_chosen_format_can_still_reach_format_specific_settings() {
        // The documented escape hatch: a runtime `Format` builder cannot carry a brotli-only
        // setting, so branch on the format, use the concrete builder, and box the result. That
        // works because a boxed compression operation is itself a `Compression`.
        fn compressor_for(format: Format) -> Box<dyn Compression<Mode = Compress>> {
            match format {
                Format::Brotli => Box::new(brotli::Compressor::builder().mode(Mode::Text).build(resources()).built()),
                other => CompressorBuilder::new().build_format(other, resources()).built(),
            }
        }
        let data = b"escape hatch ".repeat(200);

        for &format in Format::ALL {
            let mut tuned = compressor_for(format);
            let compressed = compress(&mut *tuned, &view(&data), usize::MAX).expect("compression succeeds");

            let plain = format.decompress(compressed, resources()).expect("decompression succeeds");
            assert_eq!(plain.to_vec(), data, "{format:?} failed through the escape hatch");
        }
    }

    #[test]
    fn text_mode_does_not_change_the_decompressed_bytes() {
        // The mode is a compressor-side hint only: it must never alter what comes back out.
        let data = b"the quick brown fox jumps over the lazy dog ".repeat(300);

        for mode in [Mode::Generic, Mode::Text, Mode::Font] {
            let mut tuned = brotli::Compressor::builder().mode(mode).build(resources()).built();
            let compressed = compress(&mut tuned, &view(&data), usize::MAX).expect("compression succeeds");

            let plain = brotli::decompress(compressed, resources()).expect("decompression succeeds");
            assert_eq!(plain.to_vec(), data, "{mode:?} changed the decompressed bytes");
        }
    }
}

#[cfg(feature = "zstd")]
mod zstd_specific_settings {
    use super::*;
    use crate::zstd;
    use crate::zstd::{CompressionLevel, WindowLog};

    #[test]
    fn native_level_and_decompressor_window_limit_are_wired() {
        let data = b"zstd format-specific settings ".repeat(400);
        let compressor = zstd::Compressor::builder()
            .compression_level(CompressionLevel::min())
            .build(resources())
            .built();
        let compressed = crate::compress(view(&data), compressor).expect("compression succeeds");

        let decompressor = zstd::Decompressor::builder()
            .max_window_log(WindowLog::DEFAULT)
            .build(resources())
            .built();
        let plain = crate::decompress(compressed, decompressor).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), data);
    }
}

/// Engine reuse must be invisible: a recycled compressor has to behave exactly like a fresh one.
#[cfg(feature = "gzip")]
mod pooling {
    use super::*;
    use crate::gzip;

    /// Resources whose engines are recycled, shared by the tests in this module.
    fn pooled_resources() -> &'static Resources {
        static POOLED: OnceLock<Resources> = OnceLock::new();

        POOLED.get_or_init(|| Resources::new(GlobalPool::new()))
    }

    fn compress_with(resources: &Resources, level: Level, data: &[u8]) -> BytesView {
        let mut compressor = gzip::Compressor::builder().level(level).build(resources).built();
        compress(&mut compressor, &view(data), usize::MAX).expect("compression succeeds")
    }

    #[test]
    fn a_recycled_engine_produces_byte_identical_output() {
        // The whole safety argument for pooling: reset state must leave no trace of the previous
        // stream. Compare many pooled rounds against a fresh-engine baseline.
        let payloads = [
            b"first request body".repeat(50),
            b"a completely different second body, longer".repeat(80),
            b"third".repeat(500),
        ];

        for round in 0..4 {
            for payload in &payloads {
                let pooled = compress_with(pooled_resources(), Level::DEFAULT, payload);
                let fresh = compress_with(&Resources::new(GlobalPool::new()).enable_pooling(0), Level::DEFAULT, payload);

                assert_eq!(
                    pooled.to_vec(),
                    fresh.to_vec(),
                    "round {round}: pooled output diverged from a fresh engine"
                );
                assert_eq!(gzip::decompress(pooled, resources()).expect("decompress").to_vec(), *payload);
            }
        }
    }

    #[test]
    fn a_compressor_abandoned_mid_stream_does_not_poison_the_pool() {
        // A request cancelled part-way through returns a dirty engine. The next user must still
        // get a clean stream.

        {
            let mut abandoned = gzip::Compressor::builder().build(resources()).built();
            abandoned.push(view(&b"half a stream ".repeat(100))).expect("push succeeds");
            let _ = CompressionInternal::pull(&mut abandoned).expect("pull succeeds");
            // Dropped without `end_input`, so its engine is mid-stream.
        }

        let recovered = compress_with(pooled_resources(), Level::DEFAULT, b"a fresh stream");
        let fresh = compress_with(
            &Resources::new(GlobalPool::new()).enable_pooling(0),
            Level::DEFAULT,
            b"a fresh stream",
        );

        assert_eq!(recovered.to_vec(), fresh.to_vec(), "a recycled dirty engine must be reset");
        assert_eq!(
            gzip::decompress(recovered, resources()).expect("decompress").to_vec(),
            b"a fresh stream".to_vec()
        );
    }

    #[test]
    fn levels_do_not_share_engines() {
        // Reset preserves the level, so a level-9 request must never receive a level-1 engine.
        let payload = b"the quick brown fox jumps over the lazy dog ".repeat(200);

        let fast = compress_with(pooled_resources(), Level::FAST, &payload);
        let best = compress_with(pooled_resources(), Level::HIGH, &payload);

        assert_eq!(
            fast.to_vec(),
            compress_with(&Resources::new(GlobalPool::new()).enable_pooling(0), Level::FAST, &payload).to_vec()
        );
        assert_eq!(
            best.to_vec(),
            compress_with(&Resources::new(GlobalPool::new()).enable_pooling(0), Level::HIGH, &payload).to_vec()
        );
        assert!(best.len() <= fast.len(), "level 9 must still out-compress level 1");
    }

    #[test]
    fn a_pool_is_shared_across_threads() {
        // The point of the design: one handle lives in a client and is cloned per request.
        let payload = b"concurrent body ".repeat(200);

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let payload = payload.clone();
                scope.spawn(move || {
                    for _ in 0..10 {
                        let compressed = compress_with(pooled_resources(), Level::DEFAULT, &payload);
                        assert_eq!(gzip::decompress(compressed, resources()).expect("decompress").to_vec(), payload);
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
            for round in 0..4 {
                for payload in &payloads {
                    let compressed = format.compress(view(payload), resources()).expect("compression succeeds");

                    let mut decompressor = DecompressorBuilder::new().build_format(format, resources()).built();
                    let plain = decompress(&mut *decompressor, &compressed, usize::MAX).expect("decompression succeeds");

                    assert_eq!(plain.to_vec(), *payload, "{format:?} round {round} diverged when pooled");
                }
            }
        }
    }

    #[cfg(feature = "zlib")]
    #[test]
    fn a_decompressor_abandoned_mid_stream_does_not_poison_the_pool() {
        use crate::zlib;
        let payload = b"a stream that gets cut short ".repeat(200);
        let compressed = zlib::compress(view(&payload), resources()).expect("compression succeeds");

        {
            let mut abandoned = zlib::Decompressor::builder().build(resources()).built();
            abandoned.push(compressed.range(0..compressed.len() / 2)).expect("push succeeds");
            let _ = CompressionInternal::pull(&mut abandoned).expect("pull succeeds");
            // Dropped mid-stream, so its engine is dirty.
        }

        let mut recovered = zlib::Decompressor::builder().build(resources()).built();
        let plain = decompress(&mut recovered, &compressed, usize::MAX).expect("decompression succeeds");

        assert_eq!(plain.to_vec(), payload, "a recycled dirty decompressor must be reset");
    }

    #[test]
    fn gzip_decompressors_are_not_recycled() {
        // `Decompress::reset` takes a boolean that cannot express gzip framing, so a recycled gzip
        // decompressor would silently decompress as raw deflate. It must therefore never be pooled --
        // and the caller must not be able to tell the difference.
        let payload = b"gzip stays correct ".repeat(200);
        let compressed = gzip::compress(view(&payload), resources()).expect("compression succeeds");

        for round in 0..5 {
            let mut decompressor = gzip::Decompressor::builder().build(resources()).built();
            let plain = decompress(&mut decompressor, &compressed, usize::MAX).expect("decompression succeeds");

            assert_eq!(plain.to_vec(), payload, "gzip round {round} decompressed incorrectly");
        }
    }

    #[test]
    fn resources_without_recycling_still_work() {
        let plain = Resources::new(GlobalPool::new()).enable_pooling(0);
        let payload = b"no recycling here".repeat(20);

        let compressed = compress_with(&plain, Level::DEFAULT, &payload);

        assert_eq!(gzip::decompress(compressed, resources()).expect("decompress").to_vec(), payload);
    }
}

/// The riskiest pooling bug: deflate, zlib and gzip share one engine type, so a mis-keyed pool
/// would hand a zlib compressor to a gzip request and emit a well-formed stream in the wrong
/// format. Nothing else in the suite would catch that.
#[test]
fn formats_never_share_pooled_engines() {
    let data = b"interleaved through one pool ".repeat(200);
    let input = view(&data);

    let baselines: Vec<_> = Format::ALL
        .iter()
        .map(|&format| {
            let mut compressor = CompressorBuilder::new()
                .output_chunk_size(chunk(4096))
                .build_format(format, resources())
                .built();
            let bytes = compress(&mut *compressor, &input, usize::MAX)
                .expect("compression succeeds")
                .to_vec();
            (format, bytes)
        })
        .collect();

    // Interleave, so every format has had a turn before any is asked again.
    for round in 0..6 {
        for (format, baseline) in &baselines {
            let mut compressor = CompressorBuilder::new()
                .output_chunk_size(chunk(4096))
                .build_format(*format, resources())
                .built();
            let pooled = compress(&mut *compressor, &input, usize::MAX).expect("compression succeeds");
            drop(compressor);

            assert_eq!(
                &pooled.to_vec(),
                baseline,
                "{format:?} round {round}: interleaving formats through one pool changed the output"
            );

            // And the bytes really are this format's, not a sibling's that happens to decompress.
            for (other, _) in &baselines {
                let mut reader = DecompressorBuilder::new().build_format(*other, resources()).built();
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
    let data = b"concurrent request body ".repeat(150);

    let baselines: Vec<_> = Format::ALL
        .iter()
        .map(|&format| {
            let input = view(&data);
            let mut compressor = CompressorBuilder::new()
                .output_chunk_size(chunk(4096))
                .build_format(format, resources())
                .built();
            let bytes = compress(&mut *compressor, &input, usize::MAX)
                .expect("compression succeeds")
                .to_vec();
            (format, bytes)
        })
        .collect();

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let data = data.clone();
            let baselines = baselines.clone();

            scope.spawn(move || {
                // Each thread builds its own view, so segmentation is stable within the thread.
                let input = view(&data);

                for round in 0..10 {
                    for (format, baseline) in &baselines {
                        let mut compressor = CompressorBuilder::new()
                            .output_chunk_size(chunk(4096))
                            .build_format(*format, resources())
                            .built();
                        let pooled = compress(&mut *compressor, &input, usize::MAX).expect("compression succeeds");
                        drop(compressor);

                        assert_eq!(&pooled.to_vec(), baseline, "{format:?} round {round}: concurrent pooling diverged");

                        let mut decompressor = DecompressorBuilder::new().build_format(*format, resources()).built();
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
    let data = b"steady state ".repeat(120);

    for &format in Format::ALL {
        let input = view(&data);
        let mut first: Option<Vec<u8>> = None;

        for round in 0..60 {
            let mut compressor = CompressorBuilder::new()
                .output_chunk_size(chunk(4096))
                .build_format(format, resources())
                .built();
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

/// The trait contract itself, exercised through one format and through a runtime-selected one.
///
/// These live here rather than beside the traits because driving them needs a concrete format, and
/// `core` deliberately knows about none.
#[cfg(feature = "gzip")]
mod trait_contract {
    use super::*;
    use crate::gzip;

    #[test]
    fn round_trips_through_the_trait_alone() {
        let mut compressor: Box<dyn Compression<Mode = Compress>> = Box::new(gzip::Compressor::new(resources()));
        CompressionInternal::push(&mut *compressor, view(b"driven through the trait")).expect("push succeeds");
        CompressionInternal::end_input(&mut *compressor);

        let mut collected = BytesBuf::new();
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = CompressionInternal::pull(&mut *compressor).expect("pull succeeds");
            assert!(!output.is_need_input(), "compressor requested input after end");
            let done = output.is_done();
            if let Some(chunk) = output.into_data() {
                collected.put_bytes(chunk);
            }
            if done {
                break;
            }
        }

        let mut decompressor: Box<dyn Compression<Mode = Decompress>> = Box::new(gzip::Decompressor::new(resources()));
        CompressionInternal::push(&mut *decompressor, collected.consume_all()).expect("push succeeds");
        CompressionInternal::end_input(&mut *decompressor);

        let mut plain = BytesBuf::new();
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = CompressionInternal::pull(&mut *decompressor).expect("pull succeeds");
            assert!(!output.is_need_input(), "decompressor requested input after end");
            let done = output.is_done();
            if let Some(chunk) = output.into_data() {
                plain.put_bytes(chunk);
            }
            if done {
                break;
            }
        }

        assert_eq!(plain.consume_all().to_vec(), b"driven through the trait".to_vec());
    }

    #[test]
    fn a_boxed_operation_reports_the_same_counters_as_the_one_it_wraps() {
        // Boxing is how a runtime-selected format reaches the same contract, so the counters must
        // survive the indirection rather than reporting the box's own idea of progress.
        let data = b"counted through the box ".repeat(50);
        let input = view(&data);

        let mut boxed: Box<dyn Compression<Mode = Compress>> = Box::new(gzip::Compressor::new(resources()));
        assert_eq!(boxed.total_in(), 0, "nothing has been consumed yet");
        assert_eq!(boxed.total_out(), 0, "nothing has been produced yet");

        let compressed = compress(&mut *boxed, &input, usize::MAX).expect("compression succeeds");

        assert_eq!(boxed.total_in(), data.len() as u64, "every input byte should be accounted for");
        assert_eq!(
            boxed.total_out(),
            compressed.len() as u64,
            "every output byte should be accounted for"
        );
    }

    #[test]
    fn trait_objects_are_send_sync_and_debug() {
        fn assert_send_sync<T: Send + Sync + ?Sized>(_: &T) {}
        let compressor: Box<dyn Compression<Mode = Compress>> = Box::new(gzip::Compressor::new(resources()));
        let decompressor: Box<dyn Compression<Mode = Decompress>> = Box::new(gzip::Decompressor::new(resources()));

        assert_send_sync(&*compressor);
        assert_send_sync(&*decompressor);
        assert_send_sync(&gzip::Compressor::new(resources()));
        assert_send_sync(&gzip::Decompressor::new(resources()));
        assert!(format!("{compressor:?}").contains("Compressor"));
        assert!(format!("{decompressor:?}").contains("Decompressor"));
    }

    #[test]
    fn direction_specific_traits_work_for_concrete_and_runtime_operations() {
        let input = view(b"direction-specific capabilities");

        let mut concrete = gzip::Compressor::new(resources());
        concrete.push(input.clone()).expect("push succeeds");
        concrete.flush().expect("concrete flush succeeds");
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = concrete.pull().expect("pull succeeds");
            assert!(!output.is_done(), "flush ended the stream");
            if output.is_need_input() {
                break;
            }
        }

        let mut compressor = CompressorBuilder::new()
            .build_format(Format::Gzip, resources())
            .expect("the default settings are accepted");
        compressor.push(input).expect("push succeeds");
        let mut compressed = BytesBuf::new();
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = compressor.pull().expect("pull succeeds");
            assert!(!output.is_done(), "flush ended the stream");
            let need_input = output.is_need_input();
            if let Some(chunk) = output.into_data() {
                compressed.put_bytes(chunk);
            }
            if need_input {
                break;
            }
        }

        // The header alone is already non-empty, so the flush's contribution must be measured
        // against this baseline rather than against emptiness.
        let before_flush = compressed.len();

        compressor.flush().expect("boxed flush succeeds");
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = compressor.pull().expect("pull succeeds");
            assert!(!output.is_done(), "flush ended the stream");
            let need_input = output.is_need_input();
            if let Some(chunk) = output.into_data() {
                compressed.put_bytes(chunk);
            }
            if need_input {
                break;
            }
        }

        assert!(
            compressed.len() > before_flush,
            "boxed flush should have released a sync-flush chunk beyond the header before end_input"
        );

        compressor.end_input();
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = compressor.pull().expect("pull succeeds");
            assert!(!output.is_need_input(), "compressor requested input after end");
            let done = output.is_done();
            if let Some(chunk) = output.into_data() {
                compressed.put_bytes(chunk);
            }
            if done {
                break;
            }
        }

        let joined = BytesView::from_views([compressed.consume_all(), view(b"trailing")]);
        let mut decompressor = DecompressorBuilder::new()
            .multi_stream(false)
            .build_format(Format::Gzip, resources())
            .expect("the default settings are accepted");
        decompressor.push(joined).expect("push succeeds");

        let mut plain = BytesBuf::new();
        let mut guard = StepGuard::new();
        loop {
            guard.step();
            let output = decompressor.pull().expect("pull succeeds");
            assert!(!output.is_need_input(), "complete stream requested more input");
            let done = output.is_done();
            if let Some(chunk) = output.into_data() {
                plain.put_bytes(chunk);
            }
            if done {
                break;
            }
        }

        assert_eq!(plain.consume_all().to_vec(), b"direction-specific capabilities".to_vec());
    }
}
