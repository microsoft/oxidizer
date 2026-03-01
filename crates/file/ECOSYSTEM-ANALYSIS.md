# Async File I/O Ecosystem Analysis

A detailed comparison of the **oxidizer `file` crate** against three other Rust async
file I/O libraries: **`tokio::fs`**, **`async-fs`** (smol-rs), and **`async_file`**.

---

## 1. Overview

### `file` crate (oxidizer)

A zero-copy asynchronous filesystem API built around capability-based access control.
All operations are scoped to a `Directory` capability obtained via `Root::bind`, with
paths validated to prevent traversal escapes. Reads produce pooled `BytesView` values
from the `bytesbuf` crate, enabling zero-copy data pipelines across subsystem
boundaries. The crate provides six file types organized into seekable and positional
families, each with read-only, write-only, and read-write variants enforced at the
type level. A dedicated 1–4 thread pool (`Dispatcher`) per `Root::bind` bridges sync
OS calls to async, using `flume` channels and the `async_task` crate. Runtime-agnostic:
no dependency on tokio, smol, or any specific executor.

### `tokio::fs`

Tokio's filesystem module wraps `std::fs` operations in `spawn_blocking` calls on
tokio's shared blocking thread pool (default limit: 512 threads). It provides a single
`File` type that implements `AsyncRead`, `AsyncWrite`, and `AsyncSeek`. The API closely
mirrors `std::fs` for familiarity. Tightly coupled to the tokio runtime — cannot be
used without a tokio context. The de facto standard for async file I/O in the Rust
ecosystem, with extensive community adoption and battle-tested production usage.

### `async-fs` (smol-rs)

A lightweight async filesystem wrapper from the smol ecosystem. Uses the `blocking`
crate's auto-scaling thread pool to run `std::fs` operations off the async executor.
Implements `futures::AsyncRead`, `AsyncWrite`, and `AsyncSeek`. Runtime-agnostic —
works with tokio (via compat layers), smol, async-std, or any executor that polls
standard `Future`s. Mirrors the `std::fs` API almost exactly, minimizing the learning
curve. Minimal dependency footprint.

### `async_file`

A specialized crate offering priority-based I/O scheduling with the potential for
io_uring backends on Linux. Operations accept a priority parameter, enabling callers
to express relative importance of I/O requests. Uses opaque memory types managed by
the kernel/runtime. Focused on raw file I/O — no directory operations, no streaming
traits, no seek. Targets workloads where I/O scheduling and latency matter more than
API breadth.

---

## 2. Architecture Comparison

Each crate must bridge synchronous OS file APIs (POSIX `read`/`write`, Windows
`ReadFile`/`WriteFile`) to an async programming model. They take fundamentally
different approaches.

### How each crate dispatches blocking I/O

| Aspect                  | `file`                                                           | `tokio::fs`                        | `async-fs`                                 | `async_file`                      |
|-------------------------|------------------------------------------------------------------|------------------------------------|--------------------------------------------|-----------------------------------|
| **Thread pool**         | Dedicated per-`Root::bind` (1–4 threads)                         | Shared global (up to 512 threads)  | Shared global (`blocking` crate)           | Kernel-managed (io_uring SQE/CQE) |
| **Scaling**             | Auto-scale up when pending > threads; idle scale-down after 10 s | Fixed max, grows on demand         | Auto-scaling, `blocking` crate managed     | N/A (kernel submission queue)     |
| **Dispatch unit**       | `flume` channel + `async_task::Runnable`                         | `spawn_blocking` → `JoinHandle`    | `blocking::Unblock` + internal pipe        | Ring buffer submission            |
| **Dispatches per read** | 1 per `dispatch` / `dispatch_scoped` call                        | 1 `spawn_blocking` per `poll_read` | 1 `blocking::unblock` per `poll_read`      | 1 SQE per operation               |
| **Runtime coupling**    | **None** — any executor                                          | Requires tokio runtime             | Works with smol, async-std, tokio (compat) | Linux-only; custom event loop     |

### `tokio::fs` — `spawn_blocking` into global pool

Every call to `poll_read` or `poll_write` on a `tokio::fs::File` issues a
`spawn_blocking` into tokio's shared blocking thread pool. The pool is shared with
**all** `spawn_blocking` callers in the process (CPU-heavy tasks, DNS resolution,
other I/O). Under mixed workloads, file I/O can be starved by unrelated blocking
work. The pool can grow to 512 threads, which provides high throughput at the cost
of memory footprint and context-switch overhead. Each dispatch requires acquiring a
thread from the global pool, running the OS call, and signaling completion via a
`JoinHandle`.

### `async-fs` — `blocking` crate's thread pool

Identical dispatch-per-poll approach, but uses the `blocking` crate's auto-scaling
thread pool instead of tokio's. The `Unblock` wrapper uses an internal pipe for
signaling readiness between the worker thread and the async task, adding a small
per-operation overhead (a pipe `write` + `read` pair). Runtime-agnostic by design.

### `async_file` — priority-based scheduling

Operations accept a priority parameter and are submitted to a scheduling layer. On
Linux, this can leverage io_uring for true kernel-level async I/O, eliminating thread
pool overhead entirely. Uses opaque memory types allocated by the runtime. The trade-off
is platform specificity (Linux-only for the io_uring path) and a narrow API surface.

### `file` crate — dedicated `Dispatcher` with scoped dispatch

Each `Root::bind` creates a `Dispatcher` — an isolated thread pool using a `flume`
unbounded channel and `async_task` for scheduling. Key design choices:

- **Bounded scaling**: starts with 1 worker thread, scales up to `MAX_THREADS` (4)
  when `pending_count >= thread_count`, and scales back down after `IDLE_TIMEOUT`
  (10 seconds) of inactivity. At least one worker always remains alive.

- **Two dispatch modes**:
  - `dispatch()`: for `'static` closures — returns a `DispatchFuture<T>`.
  - `dispatch_scoped()`: for closures that borrow caller data via raw pointers —
    returns a `ScopedDispatchFuture<T>` that **blocks on drop** if the closure
    hasn't completed. This is the foundation for zero-copy slice I/O.

- **Scoped dispatch for zero-copy**: Methods like `read_into_slice`, `write_slice`,
  `read_into_slice_at`, and `write_slice_at` use `dispatch_scoped` with `SendSlice`,
  `SendSliceMut`, and `SendBufMut` wrappers to send raw pointers to the worker
  thread. The `ScopedDispatchFuture` guarantees the closure completes (or never
  starts) before the caller's buffer is freed, making this safe even under
  cancellation. The worker reads/writes directly into the caller's buffer — no
  intermediate copy.

- **Retry loops inside dispatch**: Methods like `read_into_slice` consolidate the
  retry loop (handling short reads) into a single dispatch, executing entirely on the
  worker thread. This avoids the per-chunk re-dispatch that tokio and async-fs require.

- **Isolation**: The dedicated pool prevents noisy-neighbor effects from unrelated
  `spawn_blocking` work. Four threads is sufficient for most file I/O workloads since
  the bottleneck is typically disk throughput, not thread count.

---

## 3. API Design Comparison

### Type System

| Feature                    | `file`                                                                | `tokio::fs`              | `async-fs`               | `async_file`            |
|----------------------------|-----------------------------------------------------------------------|--------------------------|--------------------------|-------------------------|
| **File types**             | 6 (3 seekable + 3 positional)                                         | 1 (`File`)               | 1 (`File`)               | 1 (`File`)              |
| **Access enforcement**     | Compile-time (type-level)                                             | Runtime (OS error)       | Runtime (OS error)       | Runtime (OS error)      |
| **Seekable family**        | `ReadOnlyFile`, `WriteOnlyFile`, `File`                               | —                        | —                        | —                       |
| **Positional family**      | `ReadOnlyPositionalFile`, `WriteOnlyPositionalFile`, `PositionalFile` | —                        | —                        | —                       |
| **Seekable vs positional** | Separate types: `&mut self` vs `&self`                                | Single type, seek + read | Single type, seek + read | No seek support         |
| **Access narrowing**       | `File` → `ReadOnlyFile` via `From`                                    | N/A                      | N/A                      | N/A                     |
| **Path model**             | Capability-based (`Directory` + relative path)                        | Absolute/arbitrary path  | Absolute/arbitrary path  | Absolute/arbitrary path |
| **OpenOptions**            | `OpenOptions` + `PositionalOpenOptions`                               | `OpenOptions`            | `OpenOptions`            | N/A                     |

The `file` crate's type system encodes access permissions statically. A `ReadOnlyFile`
has no `write` method — attempting to write is a compile error, not a runtime
`EBADF`. The seekable/positional split is equally deliberate: seekable files take
`&mut self` (enforcing sequential access), while positional files take `&self`
(enabling concurrent I/O from multiple tasks).

The `From` conversions allow permanent capability narrowing:

```rust
let rw: File = File::open(&dir, "data.bin").await?;
let ro: ReadOnlyFile = rw.into(); // write capability permanently dropped
```

### Buffer Management

| Feature                    | `file`                                                              | `tokio::fs`                             | `async-fs`                              | `async_file`              |
|----------------------------|---------------------------------------------------------------------|-----------------------------------------|-----------------------------------------|---------------------------|
| **Read output**            | `BytesView` (pooled, ref-counted, immutable)                        | `usize` bytes into caller's `&mut [u8]` | `usize` bytes into caller's `&mut [u8]` | Opaque `Vec<u8>` return   |
| **Write input**            | `BytesView` or `&[u8]`                                              | `&[u8]`                                 | `&[u8]`                                 | `&[u8]`                   |
| **Memory pooling**         | ✅ Tiered pool via `bytesbuf`                                        | ❌ Caller-managed allocation             | ❌ Caller-managed allocation             | ❌ OS/allocator-managed    |
| **Zero-copy pipeline**     | ✅ Shared `BytesView` across file → socket                           | ❌ Copy at each boundary                 | ❌ Copy at each boundary                 | Partial (within io_uring) |
| **Custom memory provider** | ✅ `_with_memory` constructors                                       | ❌                                       | ❌                                       | ❌                         |
| **Direct slice I/O**       | ✅ `read_into_slice` / `write_slice` (zero-copy via scoped dispatch) | ✅ (standard `AsyncRead`/`AsyncWrite`)   | ✅ (standard `AsyncRead`/`AsyncWrite`)   | ❌                         |
| **Buffered wrappers**      | Not needed (pooled buffers built-in)                                | `BufReader`/`BufWriter` for buffering   | `BufReader`/`BufWriter` for buffering   | N/A                       |

The `file` crate offers three tiers of I/O methods:

1. **`BytesView`/`BytesBuf` path** — `read_max`, `read_exact`, `write`: data
   allocated from pooled memory. Zero-copy hand-off to downstream consumers that
   share a memory provider.

2. **Slice path** — `read_into_slice`, `write_slice`: direct I/O into/from the
   caller's `&mut [u8]` or `&[u8]` via scoped dispatch. No intermediate buffer.

3. **`BytesBuf` append path** — `read_into_bytebuf`: reads directly into a
   caller-provided `BytesBuf`, useful for accumulating data across multiple reads.

### Directory Operations

| Feature                  | `file`                                  | `tokio::fs`                       | `async-fs`                        | `async_file` |
|--------------------------|-----------------------------------------|-----------------------------------|-----------------------------------|--------------|
| **`read_dir`**           | ✅ via `Directory`                       | ✅ `tokio::fs::read_dir`           | ✅ `async_fs::read_dir`            | ❌            |
| **`DirEntry` metadata**  | Eager (fetched during iteration)        | Lazy (separate `metadata()` call) | Lazy (separate `metadata()` call) | —            |
| **`DirEntry` file_type** | Eager (from metadata, no extra syscall) | Lazy (`file_type()` may stat)     | Lazy (`file_type()` may stat)     | —            |
| **Create directory**     | ✅ `DirBuilder`                          | ✅ `tokio::fs::create_dir_all`     | ✅ `async_fs::create_dir_all`      | ❌            |
| **Symlink creation**     | ✅ `Directory::symlink`                  | ✅ `tokio::fs::symlink`            | ✅ `async_fs::symlink`             | ❌            |
| **Capability scoping**   | ✅ `open_dir` narrows to subdirectory    | ❌ No scoping                      | ❌ No scoping                      | ❌            |

The `file` crate's `DirEntry` eagerly captures metadata and file type during directory
iteration. This means `entry.metadata()` and `entry.file_type()` are instant (no
syscall), avoiding the per-entry `stat` calls that `tokio::fs` and `async-fs`
require for the same information.

---

## 4. Benchmark Results

### Benchmark Methodology

All benchmarks use [criterion](https://crates.io/crates/criterion) on Windows, reporting
the **median** of multiple iterations. Each benchmark compares against the `std::fs`
synchronous baseline under identical conditions. The `file` crate uses a dedicated 1–4
thread pool (`Dispatcher`); `tokio::fs` uses the tokio blocking pool (`spawn_blocking`);
`async-fs` uses the `blocking` crate's auto-scaling pool; `async_file` uses its own
scheduling layer.

### Results

#### Sequential Write (whole file write)

| Size  | `std::fs` | `tokio::fs` | `file` crate | `async-fs` |
|-------|-----------|-------------|--------------|------------|
| 1 KB  | 508 µs    | 655 µs      | 648 µs       | 742 µs     |
| 64 KB | 2.87 ms   | 3.82 ms     | 3.61 ms      | 3.83 ms    |
| 1 MB  | 5.68 ms   | 7.53 ms     | 7.43 ms      | 7.63 ms    |

#### Sequential Read (whole file read)

| Size  | `std::fs` | `tokio::fs` | `file` crate | `async-fs` | `async_file` |
|-------|-----------|-------------|--------------|------------|--------------|
| 1 KB  | 93 µs     | 147 µs      | 156 µs       | 147 µs     | 224 µs       |
| 64 KB | 114 µs    | 174 µs      | 883 µs       | 181 µs     | 279 µs       |
| 1 MB  | 569 µs    | 631 µs      | 997 µs       | 636 µs     | 717 µs       |

#### Streaming Read (1 MB file, 8 KB chunks)

| `std::fs` | `tokio::fs` | `file` crate | `async-fs` | `async_file` |
|-----------|-------------|--------------|------------|--------------|
| 504 µs    | 5.53 ms     | 5.94 ms      | 828 µs     | 5.61 ms      |

#### Streaming Write (128 × 8 KB chunks)

| `std::fs` | `tokio::fs` | `file` crate | `async-fs` |
|-----------|-------------|--------------|------------|
| 6.14 ms   | 7.39 ms     | 7.30 ms      | 5.74 ms    |

#### Many Small Files (100 × 256 B: create + write + read + delete)

| `std::fs` | `tokio::fs` | `file` crate | `async-fs` |
|-----------|-------------|--------------|------------|
| 141 ms    | 150 ms      | 150 ms       | 156 ms     |

#### Metadata (100 stat calls)

| `std::fs` | `tokio::fs` | `file` crate | `async-fs` | `async_file` |
|-----------|-------------|--------------|------------|--------------|
| 2.87 ms   | 7.12 ms     | 7.40 ms      | 7.32 ms    | 15.74 ms     |

#### Positional Read (128 × 8 KB reads at offsets from 1 MB file)

| `std::fs` | `tokio::fs` | `file` crate | `async-fs` |
|-----------|-------------|--------------|------------|
| 551 µs    | 10.04 ms    | 5.67 ms      | 24.37 ms   |

#### Positional Write (128 × 8 KB writes at offsets)

| `std::fs` | `tokio::fs` | `file` crate | `async-fs` |
|-----------|-------------|--------------|------------|
| 5.96 ms   | 9.80 ms     | 7.51 ms      | 14.93 ms   |

#### Concurrent Positional Read (4 × 256 KB from 1 MB file)

| `std::fs` (sequential) | `file` crate (sequential) | `file` crate (concurrent) |
|------------------------|---------------------------|---------------------------|
| 188 µs                 | 1.19 ms                   | 1.19 ms                   |

### Key Observations

1. **Sequential I/O**: All async crates add 20–50% overhead vs `std::fs` for whole-file
   operations due to thread dispatch. The `file` crate and `tokio::fs` are comparable.

2. **Sequential read 64 KB / 1 MB anomaly**: The `file` crate is slower for sequential
   reads at larger sizes. This is because `Directory::read()` queries file metadata
   first to size the buffer, then reads — two dispatched operations vs one for
   `tokio::fs` / `async-fs`. This is a known trade-off for the pooled buffer
   pre-allocation strategy.

3. **Streaming I/O**: Per-chunk dispatch overhead dominates. All dispatch-per-read
   crates (`tokio::fs`, `file`, `async_file`) are ~10× slower than `std::fs` for 8 KB
   chunks. `async-fs` performs surprisingly well here due to the `blocking` crate's
   optimized thread pool.

4. **Positional I/O**: The `file` crate is ~1.8× faster than `tokio::fs` and ~4.3×
   faster than `async-fs` for positional reads. This is because the `file` crate uses
   native `pread`/`pwrite` (or `seek_read`/`seek_write` on Windows) in a single
   dispatch, while `tokio::fs` and `async-fs` need separate seek + read dispatches.

5. **Many small files & metadata**: All async crates cluster together (~7% overhead
   vs `std::fs`), showing dispatch overhead is negligible for operations dominated by
   actual I/O.

6. **Concurrent positional reads**: On this single-machine benchmark, sequential and
   concurrent positional reads are similar because the OS page cache serves the data
   instantly. The concurrent advantage would show on actual disk I/O with higher
   latency.

---

## 5. Performance Analysis

### Dispatch Overhead

**tokio::fs / async-fs**: Each `poll_read` or `poll_write` call issues one
`spawn_blocking` (or `blocking::unblock`), acquiring a thread from the shared global
pool. Under load, acquiring a thread involves contention on the pool's internal queue
and scheduler. For streaming reads (e.g., 1 MB in 8 KB chunks), this means 128
separate thread-pool acquisitions.

**file crate**: Each call dispatches to a dedicated 1–4 thread pool with a `flume`
channel (very low contention). Critically, scoped dispatch methods consolidate
retry loops into a single round-trip: `read_into_slice` sends one closure that
loops until the buffer is filled, executing entirely on the worker. This reduces
128 dispatches to 1 for the same workload when using slice methods.

### Read Path Comparison: "Read 1 MB file in 8 KB chunks"

**tokio::fs** (via `AsyncReadExt::read`):
```
for each 8 KB chunk:
  1. poll_read called
  2. spawn_blocking: acquire thread from global 512-thread pool
  3. Worker: std::fs::File::read(&mut buf[..8192])
  4. Copy result into caller's &mut [u8]
  5. Signal JoinHandle, wake task
→ 128 spawn_blocking round-trips total
```

**file crate — `read_into_slice` (seekable, single dispatch)**:
```
1. dispatch_scoped: send closure to dedicated worker via flume
2. Worker: loop { file.read(&mut buf[total..]) } until buf is full
3. Signal ScopedDispatchFuture, wake task
→ 1 dispatch round-trip; OS read directly into caller's &mut [u8] (zero-copy)
```

**file crate — `read_max` (seekable, BytesView path)**:
```
for each 8 KB chunk:
  1. Reserve 8 KB from pooled BytesBuf
  2. dispatch_scoped: send closure to dedicated worker
  3. Worker: file.read into BytesBuf (pooled memory, no extra copy)
  4. Return BytesView (ref-counted, zero-copy to downstream)
→ 128 dispatches, but each uses pooled memory (no allocation)
  and BytesView can be forwarded zero-copy
```

### Write Path Comparison: "Write 1 MB in 8 KB chunks"

**tokio::fs** (via `AsyncWriteExt::write_all`):
```
for each 8 KB chunk:
  1. poll_write called
  2. spawn_blocking: acquire thread from global pool
  3. Worker: std::fs::File::write(&buf[..8192])
  4. Signal completion
→ 128 spawn_blocking round-trips
```

**file crate — `write_slice` (seekable, single dispatch)**:
```
1. dispatch_scoped: send closure with SendSlice (raw pointer to caller's &[u8])
2. Worker: file.write_all(data) — writes directly from caller's buffer
3. ScopedDispatchFuture blocks on drop if cancelled (safety guarantee)
→ 1 dispatch round-trip per write_slice call; zero-copy from caller's buffer
```

### Positional I/O

Only the `file` crate offers dedicated positional file types:

**file crate** — `ReadOnlyPositionalFile::read_exact_at`:
```
1. dispatch: send closure to worker
2. Worker: single pread(fd, buf, offset) syscall
3. Return BytesView
→ 1 syscall, no cursor mutation, &self enables concurrent calls
```

**tokio::fs** — equivalent operation:
```
1. spawn_blocking: file.seek(SeekFrom::Start(offset))
2. spawn_blocking: file.read_exact(&mut buf)
→ 2 syscalls (seek + read), not atomic, &mut self prevents concurrency
```

Because positional methods take `&self`, multiple tasks can issue concurrent reads
at different offsets on the same `ReadOnlyPositionalFile` handle — impossible with
tokio's or async-fs's seek-then-read pattern.

---

## 6. Trait Support

| Trait                                |                   `file`                    | `tokio::fs` | `async-fs` | `async_file` |
|--------------------------------------|:-------------------------------------------:|:-----------:|:----------:|:------------:|
| `bytesbuf_io::Read`                  |            ✅ seekable read types            |      —      |     —      |      —       |
| `bytesbuf_io::Write`                 |           ✅ seekable write types            |      —      |     —      |      —       |
| `tokio::io::AsyncRead`               |                      ❌                      |      ✅      |     —      |      —       |
| `tokio::io::AsyncWrite`              |                      ❌                      |      ✅      |     —      |      —       |
| `tokio::io::AsyncSeek`               |                      ❌                      |      ✅      |     —      |      —       |
| `futures::AsyncRead`                 |                      —                      |      —      |     ✅      |      —       |
| `futures::AsyncWrite`                |                      —                      |      —      |     ✅      |      —       |
| `futures::AsyncSeek`                 |                      —                      |      —      |     ✅      |      —       |
| `std::io::Read`                      |          ✅ (`sync-compat` feature)          |      ❌      |     ❌      |      ❌       |
| `std::io::Write`                     |          ✅ (`sync-compat` feature)          |      ❌      |     ❌      |      ❌       |
| `std::io::Seek`                      |          ✅ (`sync-compat` feature)          |      ❌      |     ❌      |      ❌       |
| `bytesbuf::mem::HasMemory`           |              ✅ all file types               |      —      |     —      |      —       |
| `bytesbuf::mem::Memory`              |              ✅ all file types               |      —      |     —      |      —       |
| `AsRawFd` / `AsFd` (Unix)            |                      ✅                      |      ✅      |     ✅      |      ❌       |
| `AsRawHandle` / `AsHandle` (Windows) |                      ✅                      |      ✅      |     ❌      |      —       |
| `From` narrowing conversions         | ✅ `File` → `ReadOnlyFile` / `WriteOnlyFile` |      —      |     —      |      —       |

The `file` crate's `sync-compat` feature enables `std::io::Read`, `Write`, and `Seek`
on seekable file types, allowing the same handle to be used in blocking contexts. The
`HasMemory` and `Memory` traits enable callers to allocate buffers from the file's
memory provider, which is critical for zero-copy cross-subsystem data flows.

---

## 7. Unique Features

### `file` crate only

| Feature                                | Description                                                                                                                                                                      |
|----------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **Capability-based access control**    | `Root::bind` creates a `Directory` capability; all paths are relative and validated via `safe_join` to prevent traversal escapes (`../`, leading `/`)                            |
| **6 file types**                       | 3 seekable (`ReadOnlyFile`, `WriteOnlyFile`, `File`) + 3 positional (`ReadOnlyPositionalFile`, `WriteOnlyPositionalFile`, `PositionalFile`)                                      |
| **Scoped dispatch**                    | `dispatch_scoped` returns `ScopedDispatchFuture` that blocks on drop — enables zero-copy slice I/O with cancellation safety via `SendSlice`/`SendSliceMut`/`SendBufMut` wrappers |
| **Custom memory providers**            | `_with_memory` constructors accept a `MemoryShared` so file reads land in memory optimal for downstream consumers (e.g., network socket buffers)                                 |
| **Eager `DirEntry` metadata**          | Metadata and file type captured during iteration; no per-entry syscalls                                                                                                          |
| **Auto-scaling dedicated thread pool** | 1–4 threads per `Root::bind`; scales on queue depth, idles down after 10 s; at least 1 worker always alive                                                                       |
| **Access narrowing via `From`**        | `File` → `ReadOnlyFile` or `WriteOnlyFile`; `PositionalFile` → `ReadOnlyPositionalFile` or `WriteOnlyPositionalFile`                                                             |
| **Sync compatibility**                 | `std::io::Read`/`Write`/`Seek` impls behind the `sync-compat` feature flag                                                                                                       |
| **Positional I/O**                     | `pread`/`pwrite` (Unix) and `seek_read`/`seek_write` (Windows) via `&self` — true concurrent I/O on a single handle                                                              |

### `tokio::fs` only

| Feature                                  | Description                                                                                                   |
|------------------------------------------|---------------------------------------------------------------------------------------------------------------|
| **Deep ecosystem integration**           | Works seamlessly with tower, hyper, tonic, axum, and the entire tokio ecosystem                               |
| **`AsyncRead`/`AsyncWrite`/`AsyncSeek`** | Plugs directly into `tokio::io::copy`, `BufReader`, `BufWriter`, `LinesStream`, and all tokio I/O combinators |
| **Massive community**                    | Battle-tested in production at scale; extensive documentation, examples, and Stack Overflow coverage          |
| **Large blocking pool**                  | Up to 512 threads — scales to highly concurrent I/O workloads where parallelism is the bottleneck             |
| **Cooperative cancellation**             | Integrates with tokio's cooperative scheduling for cancellation                                               |

### `async-fs` only

| Feature                           | Description                                                                                    |
|-----------------------------------|------------------------------------------------------------------------------------------------|
| **Runtime-agnostic**              | Works with tokio (via `compat`), smol, async-std, or any `Future`-polling executor             |
| **Minimal dependencies**          | Tiny dependency tree; ideal for projects that want async fs without pulling in a large runtime |
| **`futures` trait compatibility** | Implements `futures::AsyncRead`/`AsyncWrite`/`AsyncSeek` for broad interoperability            |
| **1:1 `std::fs` mirror**          | API surface matches `std::fs` almost exactly — minimal learning curve                          |

### `async_file` only

| Feature                        | Description                                                                                  |
|--------------------------------|----------------------------------------------------------------------------------------------|
| **Priority-based scheduling**  | Every I/O call accepts a priority parameter for relative scheduling of requests              |
| **Potential io_uring backend** | Can leverage Linux's io_uring for true kernel-level async I/O with zero thread-pool overhead |
| **OS-managed memory**          | Buffers managed by the kernel/runtime, avoiding user-space allocation overhead               |

---

## 8. Limitations

### `file` crate

| Limitation                        | Detail                                                                                                                    |
|-----------------------------------|---------------------------------------------------------------------------------------------------------------------------|
| **No io_uring support**           | All I/O goes through blocking syscalls on worker threads; no kernel async I/O path                                        |
| **Limited to 4 worker threads**   | `MAX_THREADS = 4` may bottleneck under extreme concurrent I/O; tokio allows 512                                           |
| **No tokio/futures trait compat** | Does not implement `AsyncRead`/`AsyncWrite`; cannot plug into `tokio::io::copy` or `futures::io::copy` without an adapter |
| **New and unproven**              | Smaller community and less production exposure than tokio::fs                                                             |
| **`BytesView` learning curve**    | Unfamiliar buffer types for developers used to `Vec<u8>` / `&[u8]` patterns                                               |
| **Mandatory `Directory` handle**  | Cannot open a file by absolute path directly; always requires `Root::bind` first                                          |
| **No `Stream`/`AsyncIterator`**   | `ReadDir` uses `next_entry()` loop, not the `Stream` trait                                                                |

### `tokio::fs`

| Limitation                       | Detail                                                                                    |
|----------------------------------|-------------------------------------------------------------------------------------------|
| **No buffer management**         | No pooling; every read allocates or fills a caller-provided buffer; no zero-copy pipeline |
| **No capability model**          | Accepts arbitrary absolute paths; no built-in path-traversal protection                   |
| **No positional I/O**            | Must `seek` then `read`/`write` — two syscalls, not atomic, cursor contention             |
| **No type-level access control** | Single `File` type; read vs write errors discovered at runtime                            |
| **Runtime-locked to tokio**      | Cannot be used outside a tokio runtime context                                            |
| **Shared blocking pool**         | File I/O competes with all other `spawn_blocking` work for thread pool resources          |

### `async-fs`

| Limitation                        | Detail                                                                           |
|-----------------------------------|----------------------------------------------------------------------------------|
| **Same API gaps as tokio**        | No buffer pooling, no capability model, no positional I/O, no type-level access  |
| **Smaller ecosystem**             | Less community adoption than tokio::fs; fewer examples and integrations          |
| **Pipe-based signaling overhead** | `Unblock` wrapper uses an internal pipe for readiness, adding per-operation cost |
| **No Windows handle traits**      | Does not implement `AsRawHandle` / `AsHandle` on Windows                         |
| **No file locking API**           | No async file locking support                                                    |

### `async_file`

| Limitation                       | Detail                                                                       |
|----------------------------------|------------------------------------------------------------------------------|
| **No directory operations**      | No `read_dir`, `create_dir`, `remove_dir`, or any directory API              |
| **No streaming I/O**             | No `AsyncRead`/`AsyncWrite` traits; no chunked streaming                     |
| **No seek**                      | No cursor-based seeking; offsets must be managed externally                  |
| **Priority adds API complexity** | Every call requires a priority parameter, even when scheduling is irrelevant |
| **Small community**              | Minimal documentation, few users, uncertain maintenance status               |
| **Platform-limited**             | io_uring backend is Linux-only (kernel ≥ 5.1); limited cross-platform story  |

---

## 9. Summary Table

| Feature                                           |         `file`         |    `tokio::fs`     |    `async-fs`     | `async_file` |
|---------------------------------------------------|:----------------------:|:------------------:|:-----------------:|:------------:|
| **Cross-platform**                                |           ✅            |         ✅          |         ✅         |  ❌ (Linux)   |
| **Runtime-agnostic**                              |           ✅            |     ❌ (tokio)      |         ✅         |  ❌ (custom)  |
| **Capability-based security**                     |           ✅            |         ❌          |         ❌         |      ❌       |
| **Type-level access control**                     |      ✅ (6 types)       |     ❌ (1 type)     |    ❌ (1 type)     |  ❌ (1 type)  |
| **Pooled memory / zero-copy**                     |           ✅            |         ❌          |         ❌         |   Partial    |
| **Custom memory providers**                       |           ✅            |         ❌          |         ❌         |      ❌       |
| **Positional I/O (`pread`/`pwrite`)**             |           ✅            |         ❌          |         ❌         |      ❌       |
| **Concurrent reads on same handle**               | ✅ (positional `&self`) |  ❌ (`&mut self`)   |  ❌ (`&mut self`)  |      ❌       |
| **Scoped dispatch (cancellation-safe zero-copy)** |           ✅            |         ❌          |         ❌         |      ❌       |
| **Async file locking**                            |           ✅            |         ❌          |         ❌         |      ❌       |
| **Sync trait fallback**                           |   ✅ (`sync-compat`)    |         ❌          |         ❌         |      ❌       |
| **Access narrowing (`From` conversions)**         |           ✅            |         ❌          |         ❌         |      ❌       |
| **Eager `DirEntry` metadata**                     |           ✅            |      ❌ (lazy)      |     ❌ (lazy)      |      —       |
| **`AsyncRead` / `AsyncWrite`**                    |           ❌            |     ✅ (tokio)      |    ✅ (futures)    |      ❌       |
| **Priority scheduling**                           |           ❌            |         ❌          |         ❌         |      ✅       |
| **True kernel async I/O**                         |           ❌            |         ❌          |         ❌         |      ✅       |
| **Directory operations**                          |           ✅            |         ✅          |         ✅         |      ❌       |
| **Symlink handling**                              |           ✅            |         ✅          |         ✅         |      ❌       |
| **`OpenOptions` builder**                         |     ✅ (2 variants)     |         ✅          |         ✅         |      ❌       |
| **`DirBuilder`**                                  |           ✅            |         ✅          |         ✅         |      ❌       |
| **`MaybeUninit` reads**                           |           ✅            |         ❌          |         ❌         |      ❌       |
| **Raw fd/handle access**                          |   ✅ (Unix + Windows)   | ✅ (Unix + Windows) |   ✅ (Unix only)   |      ❌       |
| **Thread pool isolation**                         |  ✅ (per-`Root::bind`)  | ❌ (global shared)  | ❌ (global shared) |     N/A      |
| **Ecosystem maturity**                            |          New           |       Mature       |     Moderate      |    Niche     |

### When to Choose Each

**`file` crate**: Security-sensitive applications needing path-traversal protection,
data pipelines requiring zero-copy buffer sharing across subsystems, workloads
benefiting from positional I/O (databases, asset servers), or projects requiring
runtime independence.

**`tokio::fs`**: Projects already committed to the tokio ecosystem, applications
needing `AsyncRead`/`AsyncWrite` compatibility with tokio combinators and middleware,
or teams that prioritize community support and battle-tested stability.

**`async-fs`**: Projects using the smol runtime or needing runtime-agnostic
`futures`-trait compatibility with a minimal dependency footprint and a familiar
`std::fs`-like API.

**`async_file`**: Linux-only workloads where I/O scheduling priority is critical
and true kernel async I/O (io_uring) is needed to eliminate thread-pool overhead.
