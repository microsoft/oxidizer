// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Throughput and allocation behaviour of the codecs.
//!
//! Every benchmark reports both time and allocations, because this crate's central claims are about
//! allocation: input is consumed segment by segment without being flattened, output is written into
//! a caller-supplied memory provider, and [`Pool`] recycles engine state. Timings alone would not
//! show a regression in any of those.
//!
//! Allocation figures come from [`alloc_tracker`], which installs a global allocator for this
//! binary and prints a per-iteration table when the session is dropped.
//!
//! Read the zstd rows with care. `zstd` allocates its compression and decompression contexts
//! through its own allocator rather than Rust's, so those allocations are invisible here and the
//! zstd rows understate the true cost. Its timings are unaffected, so compare zstd against itself
//! on time and against the other formats only on the figures the global allocator can see.

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
    let compressor = brotli::Compressor::builder()
        .window_size(window)
        .build(resources)
        .expect("the window size is accepted");

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
/// Also the regression guard for it. If pooled stops beating unpooled, or stops allocating less,
/// something has broken.
fn pooling(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/pooling");
    let bytes = payload(4096);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    for &format in Format::ALL {
        let memory = GlobalPool::new();
        let input = view(&bytes, &memory);
        let fresh = Resources::new(memory.clone()).enable_pooling(0);
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

    let format = *Format::ALL.first().expect("at least one format is compiled in");
    let memory = GlobalPool::new();
    let resources = Resources::new(memory.clone());

    for segment in [64_usize, 1024, 16 * 1024] {
        let input = fragmented(&bytes, segment, &memory);
        let name = format!("{segment}B spans");
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
/// The engines zero-fill the uninitialized output slice they are handed, so a larger chunk is not
/// automatically better; this is what settles the default.
fn chunk_size(criterion: &mut Criterion, session: &Session) {
    let mut group = criterion.benchmark_group("compressors_codec/chunk_size");
    let bytes = payload(256 * 1024);
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    let format = *Format::ALL.first().expect("at least one format is compiled in");
    let memory = GlobalPool::new();
    let resources = Resources::new(memory.clone());
    let input = view(&bytes, &memory);

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
    let mut buffer = vec![0_u8; 128 * 1024];
    let bytes = payload(64 * 1024);

    println!("\nzstd working set, reported by zstd itself:\n");
    println!("| Level | CCtx bytes | DCtx bytes |");
    println!("|-------|------------|------------|");

    for level in [Level::FAST, Level::DEFAULT, Level::HIGH] {
        // The contexts allocate lazily, so measure only after real work has sized them.
        let mut context = zstd_safe::CCtx::create();
        let written = context
            .compress(&mut *buffer, &bytes, i32::from(level.get()))
            .expect("compression succeeds");

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
