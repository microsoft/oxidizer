// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Throughput and allocation behaviour of the compression engines.
//!
//! Every benchmark reports both time and allocations, because this crate's central claims are about
//! allocation: input is consumed segment by segment without being flattened, output is written into
//! a caller-supplied memory provider, and [`Resources`] recycles engine state. Timings alone would
//! not show a regression in any of those.
//!
//! Allocation figures come from [`alloc_tracker`], which installs a global allocator for this
//! binary and prints a per-iteration table when the session is dropped.
//!
//! Read the zstd rows with care. `zstd` allocates its compression and decompression contexts
//! through its own allocator rather than Rust's, so those allocations are invisible here and the
//! zstd rows understate the true cost. Its timings are directly comparable with the other formats;
//! its allocation figures cover only what Rust's global allocator can see.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Instant;

use alloc_tracker::{Allocator, Operation, Session};
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use compressors::brotli::{self, WindowSize};
use compressors::format::Format;
use compressors::{CompressorBuilder, DecompressorBuilder, Level, Resources};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

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
/// they fix one format instead of sweeping all five. Deflate is preferred when it is compiled in;
/// otherwise the first available format stands in so the benchmark still runs.
fn representative_format() -> Format {
    Format::ALL
        .iter()
        .copied()
        .find(|format| matches!(format!("{format:?}").as_str(), "Deflate"))
        .unwrap_or_else(|| *Format::ALL.first().expect("at least one format is compiled in"))
}

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

/// Runs `body` under Criterion while attributing its allocations to `operation`.
fn measured(bencher: &mut criterion::Bencher<'_>, operation: &Operation, mut body: impl FnMut()) {
    bencher.iter_custom(|iterations| {
        let start = Instant::now();
        let _span = operation.measure_process().iterations(iterations);

        for _ in 0..iterations {
            body();
        }

        start.elapsed()
    });
}

fn compression(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/compress");

    for size in SIZES {
        let bytes = payload(size);
        group.throughput(Throughput::Bytes(size as u64));

        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let resources = Resources::new(memory.clone());
            let input = view(&bytes, &memory);
            let name = format!("{format:?}/{size}");
            let operation = session.operation(format!("compress {name}"));

            group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
                measured(bencher, &operation, || {
                    black_box(compress(format, None, None, &input, &resources));
                });
            });
        }
    }

    group.finish();
}

fn decompression(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/decompress");

    for size in SIZES {
        let bytes = payload(size);
        group.throughput(Throughput::Bytes(size as u64));

        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let resources = Resources::new(memory.clone());
            let compressed = compress(format, None, None, &view(&bytes, &memory), &resources);
            let name = format!("{format:?}/{size}");
            let operation = session.operation(format!("decompress {name}"));

            group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
                measured(bencher, &operation, || {
                    black_box(decompress(format, &compressed, &resources));
                });
            });
        }
    }

    group.finish();
}

/// The headline claim for [`Resources`]: recycling engine state removes per-message setup.
///
/// Also the regression guard for it, but only for the formats the pool actually reuses: the flate
/// family and zstd. Brotli exposes no reset, so it is never pooled, and gzip decompressors are
/// deliberately excluded because the engine's reset cannot restore gzip framing. Those rows are
/// controls -- they should show no material penalty from holding `Resources`, not a speed-up.
/// If a pooled row stops beating its unpooled counterpart, or stops allocating less, something has
/// broken.
fn pooling(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/pooling");
    let bytes = payload(4096);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    for &format in Format::ALL {
        let memory = GlobalPool::new();
        let input = view(&bytes, &memory);
        let fresh = Resources::new(memory.clone()).with_pool_capacity(0);
        let pooled = Resources::new(memory.clone());
        let compressed = compress(format, None, None, &input, &fresh);

        // Warm the pool so the measured iterations all hit it.
        drop(compress(format, None, None, &input, &pooled));
        drop(decompress(format, &compressed, &pooled));

        for (label, resources) in [("fresh", &fresh), ("pooled", &pooled)] {
            let name = format!("{format:?}/compress/{label}");
            let operation = session.operation(format!("pool {name}"));

            group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
                measured(bencher, &operation, || {
                    black_box(compress(format, None, None, &input, resources));
                });
            });

            let name = format!("{format:?}/decompress/{label}");
            let operation = session.operation(format!("pool {name}"));

            group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
                measured(bencher, &operation, || {
                    black_box(decompress(format, &compressed, resources));
                });
            });
        }
    }

    group.finish();
}

/// Input arrives as a chain of spans, so the cost of that chain is the crate's reason to exist.
///
/// A regression here -- for instance flattening the view before handing it to the engine -- would
/// show up as a jump in allocations for the fragmented cases.
fn segmentation(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/segmentation");
    let bytes = payload(64 * 1024);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    // Deflate is the representative backend for both this group and `chunk_size`: it is the most
    // widely deployed of the five and its engine takes the uninitialized output slice directly, so
    // what these groups measure is this crate's own segment handling rather than a backend quirk.
    // Sweeping every format here would multiply runtime without changing the conclusion.
    let format = representative_format();
    let memory = GlobalPool::new();
    let resources = Resources::new(memory.clone());

    // 64 B is the pathological case -- a view shredded far below any real segment size -- while
    // 1 KiB and 16 KiB bracket what a real chained view looks like. Contiguous is the control.
    for segment in [64_usize, 1024, 16 * 1024] {
        let input = fragmented(&bytes, segment, &memory);
        let name = format!("{segment}B segments");
        let operation = session.operation(format!("segment {name}"));

        group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
            measured(bencher, &operation, || {
                black_box(compress(format, None, None, &input, &resources));
            });
        });
    }

    let contiguous = view(&bytes, &memory);
    let operation = session.operation("segment contiguous");
    group.bench_function(BenchmarkId::from_parameter("contiguous"), |bencher| {
        measured(bencher, &operation, || {
            black_box(compress(format, None, None, &contiguous, &resources));
        });
    });

    group.finish();
}

/// The output chunk size trades per-call overhead against buffer churn.
///
/// Measured on one backend (see [`representative_format`]), so the numbers describe deflate rather
/// than every engine. That is enough to settle a shared default -- the trade-off is a property of
/// how often this crate hands the engine a slice, not of what the engine does with it -- but a
/// claim about brotli or zstd specifically would need its own measurement.
fn chunk_size(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/chunk_size");
    let bytes = payload(256 * 1024);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    let format = representative_format();
    let memory = GlobalPool::new();
    let resources = Resources::new(memory.clone());
    let input = view(&bytes, &memory);

    // 64 KiB is the implementation default; the others bracket the transition either side of it,
    // so the measurements show where the plateau starts rather than only that the default is on it.
    for size in [1024_usize, 8 * 1024, 64 * 1024, 512 * 1024] {
        let name = format!("{size}B chunks");
        let operation = session.operation(format!("chunk {name}"));

        group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
            measured(bencher, &operation, || {
                black_box(compress(format, None, Some(chunk(size)), &input, &resources));
            });
        });
    }

    group.finish();
}

/// Compression levels, so the portable scale's cost across formats is visible rather than assumed.
fn levels(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/levels");
    let bytes = payload(64 * 1024);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    for &format in Format::ALL {
        let memory = GlobalPool::new();
        let resources = Resources::new(memory.clone());
        let input = view(&bytes, &memory);

        for level in [Level::FAST, Level::DEFAULT, Level::HIGH] {
            let name = format!("{format:?}/{}", level.get());
            let operation = session.operation(format!("level {name}"));

            group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
                measured(bencher, &operation, || {
                    black_box(compress(format, Some(level), None, &input, &resources));
                });
            });
        }
    }

    group.finish();
}

/// Guards the counter-intuitive shape of brotli's window setting.
///
/// Brotli is by far the heaviest allocator here, so shrinking its window looks like an obvious way
/// to trim a service that compresses small messages. Measurement says otherwise: allocation and
/// time both behave as a step function of the window, and *both get worse* below the step, so a
/// small window costs memory and speed at once. The exponents below bracket that step so a change
/// in it is visible rather than silent. The cause lies inside the brotli compressor, so treat these
/// figures as the observed shape rather than as a rule about window sizes in general.
fn brotli_window(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/brotli_window");
    let bytes = payload(1024);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    let memory = GlobalPool::new();

    let resources = Resources::new(memory.clone());
    let input = view(&bytes, &memory);

    for exponent in [10_u8, 16, 18, 22] {
        let window = WindowSize::new(exponent).expect("exponents are in range");
        let name = format!("2^{exponent}");
        let operation = session.operation(format!("brotli window {name}"));

        group.bench_function(BenchmarkId::from_parameter(&name), |bencher| {
            measured(bencher, &operation, || {
                black_box(compress_brotli(window, &input, &resources));
            });
        });

        // The decompressor side matters independently: the window is recorded in the stream, so a
        // reader inherits whatever the writer chose.
        let compressed = compress_brotli(window, &input, &resources);
        let operation = session.operation(format!("brotli window {name} decompress"));

        group.bench_function(BenchmarkId::from_parameter(format!("{name}/decompress")), |bencher| {
            measured(bencher, &operation, || {
                black_box(decompress(Format::Brotli, &compressed, &resources));
            });
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
    // Dropping the session prints the per-iteration allocation table.
    let session = Session::new();

    compression(criterion, &session);
    decompression(criterion, &session);
    pooling(criterion, &session);
    segmentation(criterion, &session);
    chunk_size(criterion, &session);
    levels(criterion, &session);
    brotli_window(criterion, &session);

    ratios();
    zstd_footprint();
}

criterion_group!(codec, benches);
criterion_main!(codec);
