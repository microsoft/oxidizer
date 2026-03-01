// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "Benchmark harness")]
#![allow(unused_results, reason = "Criterion builder returns are intentionally unused")]
#![allow(clippy::cast_possible_truncation, reason = "Intentional modular byte pattern")]

use std::io::{Read, Seek, SeekFrom, Write};

use async_file::Priority;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use file::{ReadOnlyFile, ReadOnlyPositionalFile, Root, WriteOnlyFile, WriteOnlyPositionalFile};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::runtime::Runtime;

fn make_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

// ---------------------------------------------------------------------------
// A. Sequential Write
// ---------------------------------------------------------------------------

fn bench_sequential_write(c: &mut Criterion) {
    let sizes: &[(usize, &str)] = &[(1024, "1KB"), (64 * 1024, "64KB"), (1024 * 1024, "1MB")];

    let mut group = c.benchmark_group("sequential_write");

    for &(size, label) in sizes {
        let data = make_data(size);
        group.throughput(Throughput::Bytes(size as u64));

        // std::fs
        group.bench_with_input(BenchmarkId::new("std_fs", label), &size, |b, _| {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("out.bin");
            b.iter(|| {
                std::fs::write(&path, &data).expect("write");
            });
        });

        // tokio::fs
        group.bench_with_input(BenchmarkId::new("tokio_fs", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("out.bin");
            let data = data.clone();
            b.iter(|| {
                rt.block_on(async {
                    tokio::fs::write(&path, &data).await.expect("write");
                });
            });
        });

        // file crate
        group.bench_with_input(BenchmarkId::new("file_crate", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
            let data = data.clone();
            b.iter(|| {
                rt.block_on(async {
                    dir.write_slice("out.bin", &data).await.expect("write");
                });
            });
        });

        // async-fs
        group.bench_with_input(BenchmarkId::new("async_fs", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("out.bin");
            let data = data.clone();
            b.iter(|| {
                rt.block_on(async {
                    async_fs::write(&path, &data).await.expect("write");
                });
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// B. Sequential Read
// ---------------------------------------------------------------------------

fn bench_sequential_read(c: &mut Criterion) {
    let sizes: &[(usize, &str)] = &[(1024, "1KB"), (64 * 1024, "64KB"), (1024 * 1024, "1MB")];

    let mut group = c.benchmark_group("sequential_read");

    for &(size, label) in sizes {
        let data = make_data(size);
        group.throughput(Throughput::Bytes(size as u64));

        // std::fs
        group.bench_with_input(BenchmarkId::new("std_fs", label), &size, |b, _| {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("in.bin");
            std::fs::write(&path, &data).expect("setup write");
            b.iter(|| {
                let _ = std::fs::read(&path).expect("read");
            });
        });

        // tokio::fs
        group.bench_with_input(BenchmarkId::new("tokio_fs", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("in.bin");
            std::fs::write(&path, &data).expect("setup write");
            b.iter(|| {
                rt.block_on(async {
                    let _ = tokio::fs::read(&path).await.expect("read");
                });
            });
        });

        // file crate
        group.bench_with_input(BenchmarkId::new("file_crate", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("in.bin");
            std::fs::write(&path, &data).expect("setup write");
            let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
            b.iter(|| {
                rt.block_on(async {
                    let _ = dir.read("in.bin").await.expect("read");
                });
            });
        });

        // async-fs
        group.bench_with_input(BenchmarkId::new("async_fs", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("in.bin");
            std::fs::write(&path, &data).expect("setup write");
            b.iter(|| {
                rt.block_on(async {
                    let _ = async_fs::read(&path).await.expect("read");
                });
            });
        });

        // async_file
        group.bench_with_input(BenchmarkId::new("async_file", label), &size, |b, _| {
            let rt = Runtime::new().expect("runtime");
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("in.bin");
            std::fs::write(&path, &data).expect("setup write");
            b.iter(|| {
                rt.block_on(async {
                    let f = async_file::File::open(&path, Priority::unit_test()).await.expect("open");
                    let _ = f.read_all(Priority::unit_test()).await.expect("read");
                });
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// C. Streaming Read (1 MB file, 8 KB chunks)
// ---------------------------------------------------------------------------

fn bench_streaming_read(c: &mut Criterion) {
    const FILE_SIZE: usize = 1024 * 1024;
    const CHUNK: usize = 8 * 1024;

    let data = make_data(FILE_SIZE);

    let mut group = c.benchmark_group("streaming_read");
    group.throughput(Throughput::Bytes(FILE_SIZE as u64));

    // std::fs
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            let mut f = std::fs::File::open(&path).expect("open");
            let mut buf = [0u8; CHUNK];
            loop {
                let n = f.read(&mut buf).expect("read");
                if n == 0 {
                    break;
                }
            }
        });
    });

    // tokio::fs
    group.bench_function("tokio_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                let mut f = tokio::fs::File::open(&path).await.expect("open");
                let mut buf = [0u8; CHUNK];
                loop {
                    let n = f.read(&mut buf).await.expect("read");
                    if n == 0 {
                        break;
                    }
                }
            });
        });
    });

    // file crate
    group.bench_function("file_crate", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream.bin");
        std::fs::write(&path, &data).expect("setup write");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                let mut f = ReadOnlyFile::open(&dir, "stream.bin").await.expect("open");
                loop {
                    let buf = f.read_max(8192).await.expect("read");
                    if buf.is_empty() {
                        break;
                    }
                }
            });
        });
    });

    // async-fs
    group.bench_function("async_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                let mut f = async_fs::File::open(&path).await.expect("open");
                let mut buf = [0u8; CHUNK];
                loop {
                    let n = futures_lite::io::AsyncReadExt::read(&mut f, &mut buf).await.expect("read");
                    if n == 0 {
                        break;
                    }
                }
            });
        });
    });

    // async_file
    group.bench_function("async_file", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                let f = async_file::File::open(&path, Priority::unit_test()).await.expect("open");
                loop {
                    let buf = f.read(CHUNK, Priority::unit_test()).await.expect("read");
                    if buf.is_empty() {
                        break;
                    }
                }
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// D. Streaming Write (128 chunks of 8 KB)
// ---------------------------------------------------------------------------

fn bench_streaming_write(c: &mut Criterion) {
    const CHUNKS: usize = 128;
    const CHUNK: usize = 8 * 1024;
    const TOTAL: usize = CHUNKS * CHUNK;

    let chunk_data = make_data(CHUNK);

    let mut group = c.benchmark_group("streaming_write");
    group.throughput(Throughput::Bytes(TOTAL as u64));

    // std::fs
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream_w.bin");
        b.iter(|| {
            let mut f = std::fs::File::create(&path).expect("create");
            for _ in 0..CHUNKS {
                f.write_all(&chunk_data).expect("write");
            }
        });
    });

    // tokio::fs
    group.bench_function("tokio_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream_w.bin");
        b.iter(|| {
            rt.block_on(async {
                let mut f = tokio::fs::File::create(&path).await.expect("create");
                for _ in 0..CHUNKS {
                    f.write_all(&chunk_data).await.expect("write");
                }
            });
        });
    });

    // file crate
    group.bench_function("file_crate", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                let mut f = WriteOnlyFile::create(&dir, "stream_w.bin").await.expect("create");
                for _ in 0..CHUNKS {
                    f.write_slice(&chunk_data).await.expect("write");
                }
                f.flush().await.expect("flush");
            });
        });
    });

    // async-fs
    group.bench_function("async_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("stream_w.bin");
        b.iter(|| {
            rt.block_on(async {
                let mut f = async_fs::File::create(&path).await.expect("create");
                for _ in 0..CHUNKS {
                    futures_lite::io::AsyncWriteExt::write_all(&mut f, &chunk_data)
                        .await
                        .expect("write");
                }
                futures_lite::io::AsyncWriteExt::flush(&mut f).await.expect("flush");
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// E. Many Small Files (100 files of 256 bytes: create + write + read + delete)
// ---------------------------------------------------------------------------

fn bench_many_small_files(c: &mut Criterion) {
    const COUNT: usize = 100;
    const SIZE: usize = 256;

    let data = make_data(SIZE);

    let mut group = c.benchmark_group("many_small_files");
    group.throughput(Throughput::Elements(COUNT as u64));

    // std::fs
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        b.iter(|| {
            for i in 0..COUNT {
                let p = base.join(format!("f{i}.bin"));
                std::fs::write(&p, &data).expect("write");
                let _ = std::fs::read(&p).expect("read");
                std::fs::remove_file(&p).expect("remove");
            }
        });
    });

    // tokio::fs
    group.bench_function("tokio_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        b.iter(|| {
            rt.block_on(async {
                for i in 0..COUNT {
                    let p = base.join(format!("f{i}.bin"));
                    tokio::fs::write(&p, &data).await.expect("write");
                    let _ = tokio::fs::read(&p).await.expect("read");
                    tokio::fs::remove_file(&p).await.expect("remove");
                }
            });
        });
    });

    // file crate
    group.bench_function("file_crate", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                for i in 0..COUNT {
                    let name = format!("f{i}.bin");
                    dir.write_slice(&name, &data).await.expect("write");
                    let _ = dir.read(&name).await.expect("read");
                    dir.remove_file(&name).await.expect("remove");
                }
            });
        });
    });

    // async-fs
    group.bench_function("async_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        b.iter(|| {
            rt.block_on(async {
                for i in 0..COUNT {
                    let p = base.join(format!("f{i}.bin"));
                    async_fs::write(&p, &data).await.expect("write");
                    let _ = async_fs::read(&p).await.expect("read");
                    async_fs::remove_file(&p).await.expect("remove");
                }
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// F. Metadata (stat a file 100 times)
// ---------------------------------------------------------------------------

fn bench_metadata(c: &mut Criterion) {
    const ITERS: usize = 100;

    let mut group = c.benchmark_group("metadata");
    group.throughput(Throughput::Elements(ITERS as u64));

    // std::fs
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("meta.bin");
        std::fs::write(&path, b"x").expect("setup write");
        b.iter(|| {
            for _ in 0..ITERS {
                let _ = std::fs::metadata(&path).expect("metadata");
            }
        });
    });

    // tokio::fs
    group.bench_function("tokio_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("meta.bin");
        std::fs::write(&path, b"x").expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                for _ in 0..ITERS {
                    let _ = tokio::fs::metadata(&path).await.expect("metadata");
                }
            });
        });
    });

    // file crate
    group.bench_function("file_crate", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("meta.bin"), b"x").expect("setup write");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                for _ in 0..ITERS {
                    let _ = dir.metadata("meta.bin").await.expect("metadata");
                }
            });
        });
    });

    // async-fs
    group.bench_function("async_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("meta.bin");
        std::fs::write(&path, b"x").expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                for _ in 0..ITERS {
                    let _ = async_fs::metadata(&path).await.expect("metadata");
                }
            });
        });
    });

    // async_file
    group.bench_function("async_file", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("meta.bin");
        std::fs::write(&path, b"x").expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                for _ in 0..ITERS {
                    let f = async_file::File::open(&path, Priority::unit_test()).await.expect("open");
                    let _ = f.metadata(Priority::unit_test()).await.expect("metadata");
                }
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// G. Positional Read (1 MB file, 128 scattered 8 KB reads)
// ---------------------------------------------------------------------------

fn bench_positional_read(c: &mut Criterion) {
    const FILE_SIZE: usize = 1024 * 1024;
    const CHUNKS: usize = 128;
    const CHUNK: usize = 8192;
    const TOTAL: usize = CHUNKS * CHUNK;

    let data = make_data(FILE_SIZE);

    let mut group = c.benchmark_group("positional_read");
    group.throughput(Throughput::Bytes(TOTAL as u64));

    // std::fs
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pos_r.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            let mut f = std::fs::File::open(&path).expect("open");
            let mut buf = [0u8; CHUNK];
            for i in 0..CHUNKS {
                let offset = (i * CHUNK) as u64;
                f.seek(SeekFrom::Start(offset)).expect("seek");
                f.read_exact(&mut buf).expect("read");
            }
        });
    });

    // tokio::fs
    group.bench_function("tokio_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pos_r.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                let mut f = tokio::fs::File::open(&path).await.expect("open");
                let mut buf = [0u8; CHUNK];
                for i in 0..CHUNKS {
                    let offset = (i * CHUNK) as u64;
                    f.seek(SeekFrom::Start(offset)).await.expect("seek");
                    f.read_exact(&mut buf).await.expect("read");
                }
            });
        });
    });

    // file crate
    group.bench_function("file_crate", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("pos_r.bin"), &data).expect("setup write");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                let f = ReadOnlyPositionalFile::open(&dir, "pos_r.bin").await.expect("open");
                for i in 0..CHUNKS {
                    let offset = (i * CHUNK) as u64;
                    let _ = f.read_exact_at(offset, CHUNK).await.expect("read");
                }
            });
        });
    });

    // async-fs
    group.bench_function("async_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pos_r.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            rt.block_on(async {
                let mut f = async_fs::File::open(&path).await.expect("open");
                let mut buf = [0u8; CHUNK];
                for i in 0..CHUNKS {
                    let offset = (i * CHUNK) as u64;
                    futures_lite::io::AsyncSeekExt::seek(&mut f, SeekFrom::Start(offset))
                        .await
                        .expect("seek");
                    futures_lite::io::AsyncReadExt::read(&mut f, &mut buf).await.expect("read");
                }
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// H. Positional Write (128 scattered 8 KB writes)
// ---------------------------------------------------------------------------

fn bench_positional_write(c: &mut Criterion) {
    const CHUNKS: usize = 128;
    const CHUNK: usize = 8192;
    const TOTAL: usize = CHUNKS * CHUNK;

    let chunk_data = make_data(CHUNK);

    let mut group = c.benchmark_group("positional_write");
    group.throughput(Throughput::Bytes(TOTAL as u64));

    // std::fs
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pos_w.bin");
        b.iter(|| {
            let mut f = std::fs::File::create(&path).expect("create");
            for i in 0..CHUNKS {
                let offset = (i * CHUNK) as u64;
                f.seek(SeekFrom::Start(offset)).expect("seek");
                f.write_all(&chunk_data).expect("write");
            }
        });
    });

    // tokio::fs
    group.bench_function("tokio_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pos_w.bin");
        b.iter(|| {
            rt.block_on(async {
                let mut f = tokio::fs::File::create(&path).await.expect("create");
                for i in 0..CHUNKS {
                    let offset = (i * CHUNK) as u64;
                    f.seek(SeekFrom::Start(offset)).await.expect("seek");
                    f.write_all(&chunk_data).await.expect("write");
                }
            });
        });
    });

    // file crate
    group.bench_function("file_crate", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                let f = WriteOnlyPositionalFile::create(&dir, "pos_w.bin").await.expect("create");
                for i in 0..CHUNKS {
                    let offset = (i * CHUNK) as u64;
                    f.write_slice_at(offset, &chunk_data).await.expect("write");
                }
                f.flush().await.expect("flush");
            });
        });
    });

    // async-fs
    group.bench_function("async_fs", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("pos_w.bin");
        b.iter(|| {
            rt.block_on(async {
                let mut f = async_fs::File::create(&path).await.expect("create");
                for i in 0..CHUNKS {
                    let offset = (i * CHUNK) as u64;
                    futures_lite::io::AsyncSeekExt::seek(&mut f, SeekFrom::Start(offset))
                        .await
                        .expect("seek");
                    futures_lite::io::AsyncWriteExt::write_all(&mut f, &chunk_data)
                        .await
                        .expect("write");
                }
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// I. Concurrent Positional Reads (4 concurrent 256 KB reads from a 1 MB file)
// ---------------------------------------------------------------------------

fn bench_concurrent_positional_read(c: &mut Criterion) {
    const FILE_SIZE: usize = 1024 * 1024;
    const CHUNK: usize = 256 * 1024;

    let data = make_data(FILE_SIZE);

    let mut group = c.benchmark_group("concurrent_positional_read");
    group.throughput(Throughput::Bytes(FILE_SIZE as u64));

    // std::fs (sequential baseline)
    group.bench_function("std_fs", |b| {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("conc_r.bin");
        std::fs::write(&path, &data).expect("setup write");
        b.iter(|| {
            let mut f = std::fs::File::open(&path).expect("open");
            let mut buf = vec![0u8; CHUNK];
            for i in 0..4 {
                let offset = (i * CHUNK) as u64;
                f.seek(SeekFrom::Start(offset)).expect("seek");
                f.read_exact(&mut buf).expect("read");
            }
        });
    });

    // file crate (sequential)
    group.bench_function("file_crate_sequential", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("conc_r.bin"), &data).expect("setup write");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                let f = ReadOnlyPositionalFile::open(&dir, "conc_r.bin").await.expect("open");
                for i in 0..4 {
                    let offset = (i * CHUNK) as u64;
                    let _ = f.read_exact_at(offset, CHUNK).await.expect("read");
                }
            });
        });
    });

    // file crate (concurrent — highlights &self positional advantage)
    group.bench_function("file_crate_concurrent", |b| {
        let rt = Runtime::new().expect("runtime");
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("conc_r.bin"), &data).expect("setup write");
        let dir = rt.block_on(Root::bind(tmp.path())).expect("bind");
        b.iter(|| {
            rt.block_on(async {
                let f = ReadOnlyPositionalFile::open(&dir, "conc_r.bin").await.expect("open");
                let (r0, r1, r2, r3) = tokio::join!(
                    f.read_exact_at(0, CHUNK),
                    f.read_exact_at(CHUNK as u64, CHUNK),
                    f.read_exact_at((2 * CHUNK) as u64, CHUNK),
                    f.read_exact_at((3 * CHUNK) as u64, CHUNK),
                );
                r0.expect("read 0");
                r1.expect("read 1");
                r2.expect("read 2");
                r3.expect("read 3");
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_sequential_write,
    bench_sequential_read,
    bench_streaming_read,
    bench_streaming_write,
    bench_many_small_files,
    bench_metadata,
    bench_positional_read,
    bench_positional_write,
    bench_concurrent_positional_read,
);
criterion_main!(benches);
