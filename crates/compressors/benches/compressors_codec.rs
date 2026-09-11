// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Throughput and allocation behaviour of the compression engines.
//!
//! Every benchmark reports both time and allocations, because this crate's central claims are about
//! allocation: input is consumed segment by segment without being flattened, output is written into
//! a caller-supplied memory provider, and [`Resources`] recycles engine state. Timings alone would
//! not show a regression in any of those.
//!
//! [`metabench`] runs the same workloads with Criterion, allocation tracking, and, on Linux,
//! Gungraun. Payload preparation and resource warm-up are outside the measured functions; output
//! disposal remains inside them. Resources are retained across Criterion iterations.
//!
//! Parameter names use Rust identifiers so Criterion and Gungraun report the same cases.
//! Pass `--show-engine-output` to also display the compression-ratio and zstd working-set tables.
//!
//! Read the zstd rows with care. `zstd` allocates its compression and decompression contexts
//! through its own allocator rather than Rust's, so those allocations are invisible here and the
//! zstd rows understate the true cost. Its timings are directly comparable with the other formats;
//! its allocation figures cover only what Rust's global allocator can see.

use std::hint::black_box;
use std::num::NonZeroUsize;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::brotli::{self, WindowSize};
use compressors::format::Format;
use compressors::{CompressorBuilder, DecompressorBuilder, Level, Resources};
use criterion::{BenchmarkId, Criterion, Throughput};

/// Sizes chosen to bracket real traffic: a small API response, a page, and a large document.
const SIZES: [usize; 3] = [1024, 64 * 1024, 1024 * 1024];

/// Builds a payload that compresses like real data rather than like a repeated string.
///
/// A repeated token collapses to a handful of bytes at every level, which hides the differences
/// between formats and between levels.
///
/// The shape is JSON-like records mixing repeated structure with varying values. The field
/// cardinalities -- 100,000 user ids, 1,000 scores, five tags, a boolean -- are chosen to keep that
/// mixture stable across payload sizes rather than calibrated against production data; they are
/// arbitrary in magnitude but deliberate in spread, so no field either collapses to a constant or
/// becomes uniformly random. The seed is fixed, so every run compresses identical bytes and a
/// change in the numbers reflects a change in the code.
fn payload(size: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size + 128);
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;

    let mut id = 0_u64;
    while bytes.len() < size {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;

        bytes.extend_from_slice(
            format!(
                r#"{{"id":{id},"user":"user_{}","score":{},"tag":"{}","ok":{}}},"#,
                seed % 100_000,
                seed % 1_000,
                ["alpha", "beta", "gamma", "delta", "epsilon"][(seed % 5) as usize],
                seed.is_multiple_of(2)
            )
            .as_bytes(),
        );
        id += 1;
    }

    bytes.truncate(size);
    bytes
}

/// The backend the segmentation and chunk-size groups measure against.
///
/// Both groups are about this crate's own buffer handling rather than any engine's compression, so
/// they fix one format instead of sweeping all five. This benchmark declares every format in its
/// `required-features`, so deflate is always compiled in and can be named directly.
const REPRESENTATIVE_FORMAT: Format = Format::Deflate;

/// The native zstd level this crate's portable [`Level`] maps to.
///
/// Mirrors `compression_level` in `src/zstd/codec.rs`, which is crate-private and so cannot be
/// called from a benchmark. Duplicated deliberately rather than measured at the raw scale values:
/// the footprint is only interesting at the levels production actually reaches. If the production
/// mapping changes, this table has to change with it -- the assertion below is what catches that.
fn zstd_native_level(level: Level) -> i32 {
    const MAPPING: [i32; 10] = [1, 1, 2, 2, 3, 3, 3, 6, 9, 12];

    let native = MAPPING[usize::from(level.get().min(9))];

    // Pins the duplicate against the public surface it mirrors: the crate documents `Level::DEFAULT`
    // as zstd's own default of 3, so a mapping edit that broke that would fail here rather than
    // quietly reporting a footprint for the wrong level.
    assert!(
        level != Level::DEFAULT || native == 3,
        "the benchmark's level mapping has drifted from the crate's"
    );

    native
}

fn view(bytes: &[u8], memory: &GlobalPool) -> BytesView {
    BytesView::copied_from_slice(bytes, memory)
}

/// Splits a payload into `segment` sized spans, the shape this crate exists to handle.
fn fragmented(bytes: &[u8], segment: usize, memory: &GlobalPool) -> BytesView {
    BytesView::from_views(bytes.chunks(segment).map(|chunk| BytesView::copied_from_slice(chunk, memory)))
}

fn chunk(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("benchmark chunk sizes are never zero")
}

/// Compresses a view, returning the output so the optimiser cannot discard the work.
fn compress(format: Format, level: Option<Level>, chunk_size: Option<NonZeroUsize>, input: &BytesView, resources: &Resources) -> BytesView {
    let builder = CompressorBuilder::new();
    let builder = match level {
        Some(level) => builder.level(level),
        None => builder,
    };
    let builder = match chunk_size {
        Some(size) => builder.output_chunk_size(size),
        None => builder,
    };

    let compressor = builder.build_format(format, resources).expect("the settings are accepted");

    compressors::compress(input.clone(), compressor).expect("compression succeeds")
}

fn decompress(format: Format, input: &BytesView, resources: &Resources) -> BytesView {
    let decompressor = DecompressorBuilder::new()
        .build_format(format, resources)
        .expect("the settings are accepted");

    compressors::decompress(input.clone(), decompressor).expect("decompression succeeds")
}

/// Compresses with an explicit brotli window, which the runtime `Format` builder cannot express.
fn compress_brotli(window: WindowSize, input: &BytesView, resources: &Resources) -> BytesView {
    let compressor = brotli::Compressor::builder().window_size(window).build(resources);

    compressors::compress(input.clone(), compressor).expect("compression succeeds")
}

struct Input {
    format: Format,
    bytes: BytesView,
    resources: Resources,
}

impl Input {
    fn new(format: Format, size: usize) -> Self {
        let memory = GlobalPool::new();
        Self {
            format,
            bytes: view(&payload(size), &memory),
            resources: Resources::new(memory),
        }
    }

    fn warm_compression(&self, level: Option<Level>, chunk_size: Option<NonZeroUsize>) {
        drop(compress(self.format, level, chunk_size, &self.bytes, &self.resources));
    }

    fn into_compressed(mut self) -> Self {
        self.bytes = compress(self.format, None, None, &self.bytes, &self.resources);
        drop(decompress(self.format, &self.bytes, &self.resources));
        self
    }
}

fn compression_input(format: Format, size: usize) -> Input {
    let input = Input::new(format, size);
    input.warm_compression(None, None);
    input
}

fn decompression_input(format: Format, size: usize) -> Input {
    Input::new(format, size).into_compressed()
}

#[metabench::benchmark(COMPRESS, "compressors_codec", "compress")]
#[bench::brotli_1024(&compression_input(Format::Brotli, 1024))]
#[bench::brotli_65536(&compression_input(Format::Brotli, 64 * 1024))]
#[bench::brotli_1048576(&compression_input(Format::Brotli, 1024 * 1024))]
#[bench::deflate_1024(&compression_input(Format::Deflate, 1024))]
#[bench::deflate_65536(&compression_input(Format::Deflate, 64 * 1024))]
#[bench::deflate_1048576(&compression_input(Format::Deflate, 1024 * 1024))]
#[bench::gzip_1024(&compression_input(Format::Gzip, 1024))]
#[bench::gzip_65536(&compression_input(Format::Gzip, 64 * 1024))]
#[bench::gzip_1048576(&compression_input(Format::Gzip, 1024 * 1024))]
#[bench::zlib_1024(&compression_input(Format::Zlib, 1024))]
#[bench::zlib_65536(&compression_input(Format::Zlib, 64 * 1024))]
#[bench::zlib_1048576(&compression_input(Format::Zlib, 1024 * 1024))]
#[bench::zstd_1024(&compression_input(Format::Zstd, 1024))]
#[bench::zstd_65536(&compression_input(Format::Zstd, 64 * 1024))]
#[bench::zstd_1048576(&compression_input(Format::Zstd, 1024 * 1024))]
fn compress_payload(input: &Input) {
    black_box(compress(input.format, None, None, &input.bytes, &input.resources));
}

#[metabench::benchmark(DECOMPRESS, "compressors_codec", "decompress")]
#[bench::brotli_1024(&decompression_input(Format::Brotli, 1024))]
#[bench::brotli_65536(&decompression_input(Format::Brotli, 64 * 1024))]
#[bench::brotli_1048576(&decompression_input(Format::Brotli, 1024 * 1024))]
#[bench::deflate_1024(&decompression_input(Format::Deflate, 1024))]
#[bench::deflate_65536(&decompression_input(Format::Deflate, 64 * 1024))]
#[bench::deflate_1048576(&decompression_input(Format::Deflate, 1024 * 1024))]
#[bench::gzip_1024(&decompression_input(Format::Gzip, 1024))]
#[bench::gzip_65536(&decompression_input(Format::Gzip, 64 * 1024))]
#[bench::gzip_1048576(&decompression_input(Format::Gzip, 1024 * 1024))]
#[bench::zlib_1024(&decompression_input(Format::Zlib, 1024))]
#[bench::zlib_65536(&decompression_input(Format::Zlib, 64 * 1024))]
#[bench::zlib_1048576(&decompression_input(Format::Zlib, 1024 * 1024))]
#[bench::zstd_1024(&decompression_input(Format::Zstd, 1024))]
#[bench::zstd_65536(&decompression_input(Format::Zstd, 64 * 1024))]
#[bench::zstd_1048576(&decompression_input(Format::Zstd, 1024 * 1024))]
fn decompress_payload(input: &Input) {
    black_box(decompress(input.format, &input.bytes, &input.resources));
}

fn compression(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(COMPRESS.to_string());

    for size in SIZES {
        group.throughput(Throughput::Bytes(size as u64));

        for &format in Format::ALL {
            let input = compression_input(format, size);
            let name = format!("{format:?}_{size}").to_lowercase();

            group.bench_function(name, |bencher| {
                bencher.iter(|| compress_payload(&input));
            });
        }
    }

    group.finish();
}

fn decompression(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(DECOMPRESS.to_string());

    for size in SIZES {
        group.throughput(Throughput::Bytes(size as u64));

        for &format in Format::ALL {
            let input = decompression_input(format, size);
            let name = format!("{format:?}_{size}").to_lowercase();

            group.bench_function(name, |bencher| {
                bencher.iter(|| decompress_payload(&input));
            });
        }
    }

    group.finish();
}

fn pooling_input(format: Format, pooled: bool) -> Input {
    let mut input = Input::new(format, 4096);
    if !pooled {
        input.resources = input.resources.with_pool_capacity(0);
    }
    input.warm_compression(None, None);
    input
}

#[metabench::benchmark(POOL_COMPRESS, "compressors_codec/pooling", "compress")]
#[bench::brotli_fresh(&pooling_input(Format::Brotli, false))]
#[bench::brotli_pooled(&pooling_input(Format::Brotli, true))]
#[bench::deflate_fresh(&pooling_input(Format::Deflate, false))]
#[bench::deflate_pooled(&pooling_input(Format::Deflate, true))]
#[bench::gzip_fresh(&pooling_input(Format::Gzip, false))]
#[bench::gzip_pooled(&pooling_input(Format::Gzip, true))]
#[bench::zlib_fresh(&pooling_input(Format::Zlib, false))]
#[bench::zlib_pooled(&pooling_input(Format::Zlib, true))]
#[bench::zstd_fresh(&pooling_input(Format::Zstd, false))]
#[bench::zstd_pooled(&pooling_input(Format::Zstd, true))]
fn compress_pooled(input: &Input) {
    black_box(compress(input.format, None, None, &input.bytes, &input.resources));
}

#[metabench::benchmark(POOL_DECOMPRESS, "compressors_codec/pooling", "decompress")]
#[bench::brotli_fresh(&pooling_input(Format::Brotli, false).into_compressed())]
#[bench::brotli_pooled(&pooling_input(Format::Brotli, true).into_compressed())]
#[bench::deflate_fresh(&pooling_input(Format::Deflate, false).into_compressed())]
#[bench::deflate_pooled(&pooling_input(Format::Deflate, true).into_compressed())]
#[bench::gzip_fresh(&pooling_input(Format::Gzip, false).into_compressed())]
#[bench::gzip_pooled(&pooling_input(Format::Gzip, true).into_compressed())]
#[bench::zlib_fresh(&pooling_input(Format::Zlib, false).into_compressed())]
#[bench::zlib_pooled(&pooling_input(Format::Zlib, true).into_compressed())]
#[bench::zstd_fresh(&pooling_input(Format::Zstd, false).into_compressed())]
#[bench::zstd_pooled(&pooling_input(Format::Zstd, true).into_compressed())]
fn decompress_pooled(input: &Input) {
    black_box(decompress(input.format, &input.bytes, &input.resources));
}

/// The headline claim for [`Resources`]: recycling engine state removes per-message setup.
///
/// Also the regression guard for it, but only for the formats the pool actually reuses: the flate
/// family and zstd. Brotli exposes no reset, so it is never pooled, and gzip decompressors are
/// deliberately excluded because the engine's reset cannot restore gzip framing. Those rows are
/// controls -- they should show no material penalty from holding `Resources`, not a speed-up.
/// If a pooled row stops beating its unpooled counterpart, or stops allocating less, something has
/// broken.
fn pooling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(POOL_COMPRESS.group_name());
    group.throughput(Throughput::Bytes(4096));

    for &format in Format::ALL {
        for (label, pooled) in [("fresh", false), ("pooled", true)] {
            let input = pooling_input(format, pooled);
            let name = format!("{format:?}_{label}").to_lowercase();

            group.bench_function(BenchmarkId::new(POOL_COMPRESS.benchmark_name(), &name), |bencher| {
                bencher.iter(|| compress_pooled(&input));
            });

            let compressed = input.into_compressed();

            group.bench_function(BenchmarkId::new(POOL_DECOMPRESS.benchmark_name(), &name), |bencher| {
                bencher.iter(|| decompress_pooled(&compressed));
            });
        }
    }

    group.finish();
}

fn segmentation_input(segment: Option<usize>) -> Input {
    let memory = GlobalPool::new();
    let bytes = payload(64 * 1024);
    let input = Input {
        format: REPRESENTATIVE_FORMAT,
        bytes: match segment {
            Some(segment) => fragmented(&bytes, segment, &memory),
            None => view(&bytes, &memory),
        },
        resources: Resources::new(memory),
    };
    input.warm_compression(None, None);
    input
}

#[metabench::benchmark(SEGMENTATION, "compressors_codec", "segmentation")]
#[bench::segments_64(&segmentation_input(Some(64)))]
#[bench::segments_1024(&segmentation_input(Some(1024)))]
#[bench::segments_16384(&segmentation_input(Some(16 * 1024)))]
#[bench::contiguous(&segmentation_input(None))]
fn compress_segmented(input: &Input) {
    black_box(compress(input.format, None, None, &input.bytes, &input.resources));
}

/// Input arrives as a chain of spans, so the cost of that chain is the crate's reason to exist.
///
/// A regression here -- for instance flattening the view before handing it to the engine -- would
/// show up as a jump in allocations for the fragmented cases.
fn segmentation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(SEGMENTATION.to_string());
    group.throughput(Throughput::Bytes(64 * 1024));

    // Deflate is the representative backend for both this group and `chunk_size`: it is the most
    // widely deployed of the five and its engine takes the uninitialized output slice directly, so
    // what these groups measure is this crate's own segment handling rather than a backend quirk.
    // Sweeping every format here would multiply runtime without changing the conclusion.
    //
    // 64 B is the pathological case -- a view shredded far below any real segment size -- while
    // 1 KiB and 16 KiB bracket what a real chained view looks like. Contiguous is the control.
    for segment in [Some(64), Some(1024), Some(16 * 1024), None] {
        let input = segmentation_input(segment);
        let name = segment.map_or_else(|| "contiguous".to_owned(), |segment| format!("segments_{segment}"));

        group.bench_function(name, |bencher| {
            bencher.iter(|| compress_segmented(&input));
        });
    }

    group.finish();
}

fn chunk_input(size: usize) -> Input {
    let input = Input::new(REPRESENTATIVE_FORMAT, 256 * 1024);
    input.warm_compression(None, Some(chunk(size)));
    input
}

#[metabench::benchmark(CHUNK_SIZE, "compressors_codec", "chunk_size")]
#[bench::chunks_1024(&chunk_input(1024), 1024)]
#[bench::chunks_8192(&chunk_input(8 * 1024), 8 * 1024)]
#[bench::chunks_65536(&chunk_input(64 * 1024), 64 * 1024)]
#[bench::chunks_524288(&chunk_input(512 * 1024), 512 * 1024)]
fn compress_chunked(input: &Input, size: usize) {
    black_box(compress(input.format, None, Some(chunk(size)), &input.bytes, &input.resources));
}

/// The output chunk size trades per-call overhead against buffer churn.
///
/// Measured on one backend (see [`REPRESENTATIVE_FORMAT`]), so the numbers describe deflate rather
/// than every engine. That is enough to settle a shared default -- the trade-off is a property of
/// how often this crate hands the engine a slice, not of what the engine does with it -- but a
/// claim about brotli or zstd specifically would need its own measurement.
fn chunk_size(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(CHUNK_SIZE.to_string());
    group.throughput(Throughput::Bytes(256 * 1024));

    // 64 KiB is the implementation default; the others bracket the transition either side of it,
    // so the measurements show where the plateau starts rather than only that the default is on it.
    for size in [1024_usize, 8 * 1024, 64 * 1024, 512 * 1024] {
        let input = chunk_input(size);

        group.bench_function(format!("chunks_{size}"), |bencher| {
            bencher.iter(|| compress_chunked(&input, size));
        });
    }

    group.finish();
}

fn level_input(format: Format, level: Level) -> Input {
    let input = Input::new(format, 64 * 1024);
    input.warm_compression(Some(level), None);
    input
}

#[metabench::benchmark(LEVELS, "compressors_codec", "levels")]
#[bench::brotli_1(&level_input(Format::Brotli, Level::FAST), Level::FAST)]
#[bench::brotli_6(&level_input(Format::Brotli, Level::DEFAULT), Level::DEFAULT)]
#[bench::brotli_9(&level_input(Format::Brotli, Level::HIGH), Level::HIGH)]
#[bench::deflate_1(&level_input(Format::Deflate, Level::FAST), Level::FAST)]
#[bench::deflate_6(&level_input(Format::Deflate, Level::DEFAULT), Level::DEFAULT)]
#[bench::deflate_9(&level_input(Format::Deflate, Level::HIGH), Level::HIGH)]
#[bench::gzip_1(&level_input(Format::Gzip, Level::FAST), Level::FAST)]
#[bench::gzip_6(&level_input(Format::Gzip, Level::DEFAULT), Level::DEFAULT)]
#[bench::gzip_9(&level_input(Format::Gzip, Level::HIGH), Level::HIGH)]
#[bench::zlib_1(&level_input(Format::Zlib, Level::FAST), Level::FAST)]
#[bench::zlib_6(&level_input(Format::Zlib, Level::DEFAULT), Level::DEFAULT)]
#[bench::zlib_9(&level_input(Format::Zlib, Level::HIGH), Level::HIGH)]
#[bench::zstd_1(&level_input(Format::Zstd, Level::FAST), Level::FAST)]
#[bench::zstd_6(&level_input(Format::Zstd, Level::DEFAULT), Level::DEFAULT)]
#[bench::zstd_9(&level_input(Format::Zstd, Level::HIGH), Level::HIGH)]
fn compress_at_level(input: &Input, level: Level) {
    black_box(compress(input.format, Some(level), None, &input.bytes, &input.resources));
}

/// Compression levels, so the portable scale's cost across formats is visible rather than assumed.
fn levels(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(LEVELS.to_string());
    group.throughput(Throughput::Bytes(64 * 1024));

    for &format in Format::ALL {
        for level in [Level::FAST, Level::DEFAULT, Level::HIGH] {
            let input = level_input(format, level);
            let name = format!("{format:?}_{}", level.get()).to_lowercase();

            group.bench_function(name, |bencher| {
                bencher.iter(|| compress_at_level(&input, level));
            });
        }
    }

    group.finish();
}

fn window_input(exponent: u8) -> (Input, WindowSize) {
    let input = Input::new(Format::Brotli, 1024);
    let window = WindowSize::new(exponent).expect("benchmark window exponents are in the supported range");
    drop(compress_brotli(window, &input.bytes, &input.resources));
    (input, window)
}

fn window_compressed_input(exponent: u8) -> Input {
    let (mut input, window) = window_input(exponent);
    input.bytes = compress_brotli(window, &input.bytes, &input.resources);
    drop(decompress(input.format, &input.bytes, &input.resources));
    input
}

#[metabench::benchmark(WINDOW_COMPRESS, "compressors_codec/brotli_window", "compress")]
#[bench::window_10(&window_input(10))]
#[bench::window_16(&window_input(16))]
#[bench::window_18(&window_input(18))]
#[bench::window_22(&window_input(22))]
fn compress_at_window((input, window): &(Input, WindowSize)) {
    black_box(compress_brotli(*window, &input.bytes, &input.resources));
}

#[metabench::benchmark(WINDOW_DECOMPRESS, "compressors_codec/brotli_window", "decompress")]
#[bench::window_10(&window_compressed_input(10))]
#[bench::window_16(&window_compressed_input(16))]
#[bench::window_18(&window_compressed_input(18))]
#[bench::window_22(&window_compressed_input(22))]
fn decompress_at_window(input: &Input) {
    black_box(decompress(input.format, &input.bytes, &input.resources));
}

/// Guards the counter-intuitive shape of brotli's window setting.
///
/// Brotli is by far the heaviest allocator here, so shrinking its window looks like an obvious way
/// to trim a service that compresses small messages. Measurement says otherwise: allocation and
/// time both behave as a step function of the window, and *both get worse* below the step, so a
/// small window costs memory and speed at once. The exponents below bracket that step so a change
/// in it is visible rather than silent. The cause lies inside the brotli compressor, so treat these
/// figures as the observed shape rather than as a rule about window sizes in general.
fn brotli_window(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(WINDOW_COMPRESS.group_name());
    group.throughput(Throughput::Bytes(1024));

    for exponent in [10_u8, 16, 18, 22] {
        let input = window_input(exponent);
        let name = format!("window_{exponent}");

        group.bench_function(BenchmarkId::new(WINDOW_COMPRESS.benchmark_name(), &name), |bencher| {
            bencher.iter(|| compress_at_window(&input));
        });

        // The decompressor side matters independently: the window is recorded in the stream, so a
        // reader inherits whatever the writer chose.
        let compressed = window_compressed_input(exponent);

        group.bench_function(BenchmarkId::new(WINDOW_DECOMPRESS.benchmark_name(), &name), |bencher| {
            bencher.iter(|| decompress_at_window(&compressed));
        });
    }

    group.finish();
}

/// Prints the ratio each format and level achieves.
///
/// The timing and allocation groups measure what a setting costs but not what it buys, which
/// leaves the level groups undecidable on their own. Ratio is deterministic, so it is computed
/// once rather than benchmarked.
fn ratios() {
    let memory = GlobalPool::new();
    let resources = Resources::new(memory.clone());
    let bytes = payload(64 * 1024);
    let input = view(&bytes, &memory);

    println!("\nCompression ratio (64 KiB of JSON-like input):\n");
    println!("| Format  | Level | Ratio |");
    println!("|---------|-------|-------|");

    for &format in Format::ALL {
        for level in [Level::FAST, Level::DEFAULT, Level::HIGH] {
            let compressed = compress(format, Some(level), None, &input, &resources);

            #[expect(clippy::cast_precision_loss, reason = "a ratio needs no more precision than this")]
            let ratio = bytes.len() as f64 / compressed.len() as f64;

            println!("| {:<7} | {:<5} | {ratio:>5.2} |", format!("{format:?}"), level.get());
        }
    }

    println!("\nCompression ratio by brotli window (64 KiB):\n");
    println!("| Window | Ratio |");
    println!("|--------|-------|");

    for exponent in [10_u8, 16, 18, 22] {
        let window = WindowSize::new(exponent).expect("exponents are in range");
        let compressed = compress_brotli(window, &input, &resources);

        #[expect(clippy::cast_precision_loss, reason = "a ratio needs no more precision than this")]
        let ratio = bytes.len() as f64 / compressed.len() as f64;

        println!("| 2^{exponent:<4} | {ratio:>5.2} |");
    }
}

/// Reports zstd's real working-set size, which the global allocator cannot see.
///
/// `zstd` allocates its contexts through its own allocator, so every zstd row in the allocation
/// table understates the cost. Asking zstd itself restores the comparison.
fn zstd_footprint() {
    let bytes = payload(64 * 1024);
    // Sized from the payload rather than a round number, so the destination is a guaranteed upper
    // bound for whatever zstd produces at any level.
    let mut buffer = vec![0_u8; zstd_safe::compress_bound(bytes.len())];

    println!("\nzstd working set, reported by zstd itself:\n");
    println!("| Level | Compressor context bytes | Decompressor context bytes |");
    println!("|-------|--------------------------|----------------------------|");

    for level in [Level::FAST, Level::DEFAULT, Level::HIGH] {
        // Through the same portable-to-native mapping the codec uses, so the footprint is measured
        // at the levels production actually reaches rather than at the raw scale values.
        let native = zstd_native_level(level);

        // The contexts allocate lazily, so measure only after real work has sized them.
        let mut context = zstd_safe::CCtx::create();
        let written = context.compress(&mut *buffer, &bytes, native).expect("compression succeeds");

        let mut decompressor = zstd_safe::DCtx::create();
        let mut plain = vec![0_u8; bytes.len()];
        decompressor
            .decompress(&mut *plain, &buffer[..written])
            .expect("decompression succeeds");

        println!("| {:<5} | {:>10} | {:>10} |", level.get(), context.sizeof(), decompressor.sizeof());
    }
}

fn benches(criterion: &mut Criterion) {
    compression(criterion);
    decompression(criterion);
    pooling(criterion);
    segmentation(criterion);
    chunk_size(criterion);
    levels(criterion);
    brotli_window(criterion);

    ratios();
    zstd_footprint();
}

metabench::main!(
    criterion = benches,
    benchmarks = [
        COMPRESS,
        DECOMPRESS,
        POOL_COMPRESS,
        POOL_DECOMPRESS,
        SEGMENTATION,
        CHUNK_SIZE,
        LEVELS,
        WINDOW_COMPRESS,
        WINDOW_DECOMPRESS,
    ],
);
